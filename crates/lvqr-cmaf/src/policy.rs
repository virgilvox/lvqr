//! Segmenter policy: decides which [`CmafChunkKind`] a newly-arriving
//! fragment triggers.
//!
//! The policy is a pure state machine. It takes the fragment's
//! keyframe flag and monotonically-increasing DTS and returns the
//! chunk boundary classification. No I/O, no allocations, no async.
//! This makes the policy trivial to proptest: every HLS/DASH alignment
//! invariant is expressible as a property on the decision sequence.
//!
//! ## Target boundaries
//!
//! * **Partial duration** defaults to 200 ms (5 per second at 30 fps)
//!   which is the LL-HLS sweet spot.
//! * **Segment duration** defaults to 2 s, a common DASH / HLS target.
//!
//! Both are expressed in the track's own timescale when
//! [`CmafPolicy`] is constructed so the policy does not need to know
//! about the timescale at decision time.

use crate::chunk::CmafChunkKind;

/// Tuning parameters for the segmenter.
#[derive(Debug, Clone, Copy)]
pub struct CmafPolicy {
    /// Target chunk (HLS partial) duration in the track's timescale.
    pub partial_duration: u64,
    /// Target segment duration in the track's timescale. Must be an
    /// integer multiple of `partial_duration` for HLS partial alignment
    /// to make sense; the policy does not enforce this because a
    /// non-multiple is still legal DASH.
    pub segment_duration: u64,
}

impl CmafPolicy {
    /// Typical 90-kHz video defaults: 200 ms partials, 2 s segments.
    pub const VIDEO_90KHZ_DEFAULT: Self = Self {
        partial_duration: 18_000,  // 0.2 s at 90 kHz
        segment_duration: 180_000, // 2.0 s at 90 kHz
    };

    /// Typical 48-kHz audio defaults: 200 ms partials, 2 s segments.
    pub const AUDIO_48KHZ_DEFAULT: Self = Self {
        partial_duration: 9_600,
        segment_duration: 96_000,
    };

    /// Build a policy for an arbitrary track timescale. 200 ms partial
    /// targets and 2 s segment targets scaled to the given timescale.
    /// `VIDEO_90KHZ_DEFAULT` and `AUDIO_48KHZ_DEFAULT` are the
    /// specialized shapes this constructor returns for 90_000 Hz and
    /// 48_000 Hz respectively.
    ///
    /// The HLS bridge uses this so LL-HLS `#EXT-X-PART:DURATION`
    /// reporting matches the track's actual sample rate -- e.g. AAC at
    /// 44_100 Hz reports `1024 / 44100 ≈ 0.02322 s` per frame, not
    /// `1024 / 48000 ≈ 0.02133 s` which was off by ~8.8% when the
    /// bridge hard-wired the 48 kHz constant regardless of the real
    /// sample rate.
    pub const fn for_timescale(timescale: u32) -> Self {
        let ts = timescale as u64;
        Self {
            partial_duration: ts / 5, // 200 ms
            segment_duration: ts * 2, // 2 s
        }
    }

    /// Build a policy for an arbitrary timescale with explicit
    /// durations. `segment_duration_ms` and `part_duration_ms` are
    /// in milliseconds; the constructor converts them to the
    /// track's native timescale ticks.
    pub const fn with_durations(timescale: u32, segment_duration_ms: u32, part_duration_ms: u32) -> Self {
        let ts = timescale as u64;
        Self {
            partial_duration: ts * part_duration_ms as u64 / 1000,
            segment_duration: ts * segment_duration_ms as u64 / 1000,
        }
    }
}

/// Result of a policy step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyDecision {
    /// Classification of the chunk this fragment begins.
    pub kind: CmafChunkKind,
}

/// Mutable per-track state the policy walks across as fragments arrive.
///
/// Construct one per logical track. The state tracks the DTS of the
/// last partial boundary and the last segment boundary so the policy
/// can emit `Segment` / `PartialIndependent` / `Partial` in the right
/// order without having to scan prior fragments.
#[derive(Debug, Clone)]
pub struct CmafPolicyState {
    policy: CmafPolicy,
    last_segment_start_dts: Option<u64>,
    last_partial_start_dts: Option<u64>,
}

impl CmafPolicyState {
    pub fn new(policy: CmafPolicy) -> Self {
        Self {
            policy,
            last_segment_start_dts: None,
            last_partial_start_dts: None,
        }
    }

    pub fn policy(&self) -> CmafPolicy {
        self.policy
    }

    /// Advance the state machine with the next fragment's keyframe flag
    /// and DTS. Returns the classification the resulting chunk should
    /// carry.
    ///
    /// Rules, in priority order:
    ///
    /// 1. If the fragment is a keyframe AND the DTS is at or past the
    ///    next segment boundary, it is a [`CmafChunkKind::Segment`] and
    ///    both the segment and partial timers reset.
    /// 2. If the fragment is a keyframe (but not yet at a segment
    ///    boundary), it is [`CmafChunkKind::PartialIndependent`]. The
    ///    partial timer resets, the segment timer does not.
    /// 3. Otherwise, if the DTS is at or past the next partial
    ///    boundary, it is still [`CmafChunkKind::PartialIndependent`]
    ///    only when the first fragment of the partial happens to be a
    ///    keyframe; non-keyframes on partial boundaries are
    ///    [`CmafChunkKind::Partial`] per LL-HLS semantics.
    /// 4. Default: [`CmafChunkKind::Partial`].
    ///
    /// Note that rule 3 intentionally drops partial-boundary alignment
    /// for non-keyframe fragments: LL-HLS `EXT-X-PART` requires
    /// `INDEPENDENT=YES` only when the partial *actually* starts with
    /// a keyframe, and the HLS spec allows partials to start at
    /// arbitrary samples.
    pub fn step(&mut self, keyframe: bool, dts: u64) -> PolicyDecision {
        // The very first fragment always starts both a segment and a
        // partial regardless of the keyframe flag. Real-world producers
        // always emit a keyframe first, but the segmenter should not
        // crash if one does not.
        if self.last_segment_start_dts.is_none() {
            self.last_segment_start_dts = Some(dts);
            self.last_partial_start_dts = Some(dts);
            return PolicyDecision {
                kind: CmafChunkKind::Segment,
            };
        }

        // Rule 1: keyframe at or past the next segment boundary -> Segment.
        let seg_start = self.last_segment_start_dts.unwrap();
        if keyframe && dts.saturating_sub(seg_start) >= self.policy.segment_duration {
            self.last_segment_start_dts = Some(dts);
            self.last_partial_start_dts = Some(dts);
            return PolicyDecision {
                kind: CmafChunkKind::Segment,
            };
        }

        // Rule 2: keyframe within the current segment -> PartialIndependent.
        if keyframe {
            self.last_partial_start_dts = Some(dts);
            return PolicyDecision {
                kind: CmafChunkKind::PartialIndependent,
            };
        }

        // Rule 3/4: non-keyframe. If we've crossed a partial boundary
        // we still record the new partial start, but it is not
        // independent.
        let par_start = self.last_partial_start_dts.unwrap();
        if dts.saturating_sub(par_start) >= self.policy.partial_duration {
            self.last_partial_start_dts = Some(dts);
        }
        PolicyDecision {
            kind: CmafChunkKind::Partial,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fragment_is_segment_start() {
        let mut s = CmafPolicyState::new(CmafPolicy::VIDEO_90KHZ_DEFAULT);
        let d = s.step(true, 0);
        assert_eq!(d.kind, CmafChunkKind::Segment);
    }

    #[test]
    fn second_keyframe_inside_segment_is_partial_independent() {
        let mut s = CmafPolicyState::new(CmafPolicy::VIDEO_90KHZ_DEFAULT);
        let _ = s.step(true, 0);
        // 1 s later, still inside the 2 s segment window, another keyframe.
        let d = s.step(true, 90_000);
        assert_eq!(d.kind, CmafChunkKind::PartialIndependent);
    }

    #[test]
    fn keyframe_past_segment_boundary_starts_new_segment() {
        let mut s = CmafPolicyState::new(CmafPolicy::VIDEO_90KHZ_DEFAULT);
        let _ = s.step(true, 0);
        // 2.5 s later (past the 2 s segment boundary) with a keyframe.
        let d = s.step(true, 225_000);
        assert_eq!(d.kind, CmafChunkKind::Segment);
    }

    #[test]
    fn non_keyframes_between_segments_are_partials() {
        let mut s = CmafPolicyState::new(CmafPolicy::VIDEO_90KHZ_DEFAULT);
        let _ = s.step(true, 0);
        for i in 1..10 {
            let d = s.step(false, i * 3000);
            assert_eq!(d.kind, CmafChunkKind::Partial, "step {i}");
        }
    }

    #[test]
    fn segment_starts_always_independent() {
        let mut s = CmafPolicyState::new(CmafPolicy::VIDEO_90KHZ_DEFAULT);
        let d0 = s.step(true, 0);
        let d1 = s.step(false, 90_000);
        let d2 = s.step(true, 180_000);
        assert!(d0.kind.is_independent());
        assert!(!d1.kind.is_independent());
        assert!(d2.kind.is_independent());
    }
}

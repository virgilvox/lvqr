//! Raw-sample coalescer.
//!
//! [`TrackCoalescer`] consumes [`RawSample`] values from one track
//! and emits [`CmafChunk`] values aligned for HLS partials, DASH
//! segments, and MoQ groups. Unlike the pass-through
//! [`crate::CmafSegmenter`], which wraps pre-muxed `moof + mdat`
//! fragments, the coalescer builds its own `moof + mdat` via
//! `mp4-atom` every time it flushes a pending batch.
//!
//! This is the Tier 2.3 load-bearing piece the session-6 HANDOFF
//! design note scoped. Everything the producer side of LVQR needs
//! to drive real HLS / DASH / MoQ egress from raw samples (rather
//! than from the current RTMP-bridge pre-muxed path) flows through
//! this type.
//!
//! ## State machine
//!
//! On each [`push`] call:
//!
//! 1. If there is a pending batch AND the new sample would cross a
//!    partial boundary (`sample.dts - partial_start >= partial_duration`)
//!    OR a segment boundary (`sample.keyframe && sample.dts -
//!    segment_start >= segment_duration`), flush the pending batch
//!    as a [`CmafChunk`] and return it.
//! 2. If the pending batch is empty (either because this is the
//!    very first sample or because the previous step flushed),
//!    classify the incoming sample:
//!    * Very-first sample or keyframe past the segment window:
//!      [`CmafChunkKind::Segment`].
//!    * Keyframe inside the current segment:
//!      [`CmafChunkKind::PartialIndependent`].
//!    * Non-keyframe: [`CmafChunkKind::Partial`].
//! 3. Append the sample to the pending batch.
//!
//! A trailing [`flush`] at end-of-stream drains whatever is in
//! pending as one last chunk so no samples are lost.
//!
//! ## Construction of `moof + mdat`
//!
//! `flush_pending` builds an `mp4_atom::Moof` populated with one
//! `Traf` that carries:
//!
//! * `Tfhd` with the track id. Flags default to zero, so
//!   `default_base_is_moof` is implicit per the 14496-12:2015
//!   amendment (omitting both `base_data_offset_present` and
//!   `default_base_is_moof` makes the base the start of the
//!   enclosing `moof`).
//! * `Tfdt` with the DTS of the first sample in the batch, encoded
//!   as version 1 (64-bit) so a 24-hour broadcast does not wrap
//!   around.
//! * `Trun` with per-sample duration, size, flags, and cts offset.
//!   The flags use the `0x02000000` sync / `0x01010000` non-sync
//!   layout the lvqr-ingest hand-rolled writer ships today, so
//!   byte comparisons between the two writers see identical sample
//!   flag fields.
//!
//! Computing the `trun::data_offset` requires knowing the moof
//! size, which is only available after the first encode. The
//! coalescer handles this by encoding the moof twice: once with
//! `data_offset = 0` to measure, once with the correct value. The
//! total moof size is stable across the two encodes because every
//! field is fixed-width.

use bytes::{BufMut, Bytes, BytesMut};
use mp4_atom::{Encode, Mfhd, Moof, Tfdt, Tfhd, Traf, Trun, TrunEntry};

use crate::chunk::{CmafChunk, CmafChunkKind};
use crate::policy::CmafPolicy;
use crate::sample::RawSample;

/// Per-track sample coalescer. Construct one per track.
#[derive(Debug)]
pub struct TrackCoalescer {
    track_id: u32,
    policy: CmafPolicy,
    /// Samples accumulated since the last flush.
    pending: Vec<RawSample>,
    /// Running sum of pending sample durations in track ticks.
    pending_duration_ticks: u64,
    /// DTS of the first sample in the pending batch. `None` while
    /// pending is empty.
    pending_dts: Option<u64>,
    /// DTS of the first sample in the current partial window. This
    /// is what the partial-boundary test compares against.
    partial_start_dts: Option<u64>,
    /// DTS of the first sample in the current segment window. A
    /// fresh coalescer has no segment yet, so this starts `None`.
    segment_start_dts: Option<u64>,
    /// Classification the pending batch will carry when it
    /// flushes. Pinned at the moment pending becomes non-empty so
    /// later samples inside the same partial window cannot change
    /// the chunk's kind.
    pending_kind: CmafChunkKind,
    /// Monotonically increasing `mfhd.sequence_number`. Incremented
    /// on every flush.
    next_sequence: u32,
}

impl TrackCoalescer {
    /// Build a coalescer for `track_id` with the given policy.
    pub fn new(track_id: u32, policy: CmafPolicy) -> Self {
        Self {
            track_id,
            policy,
            pending: Vec::new(),
            pending_duration_ticks: 0,
            pending_dts: None,
            partial_start_dts: None,
            segment_start_dts: None,
            pending_kind: CmafChunkKind::Segment,
            next_sequence: 1,
        }
    }

    /// Push one sample. Returns `Some(CmafChunk)` when the state
    /// machine closes a partial or segment boundary as a side
    /// effect of this push.
    pub fn push(&mut self, sample: RawSample) -> Option<CmafChunk> {
        // Step 1: does this sample close the current pending batch?
        let mut emitted = None;
        if !self.pending.is_empty() {
            let partial_start = self.partial_start_dts.expect("pending implies partial_start");
            let segment_start = self.segment_start_dts.expect("pending implies segment_start");
            let crosses_partial = sample.dts.saturating_sub(partial_start) >= self.policy.partial_duration;
            let forces_segment =
                sample.keyframe && sample.dts.saturating_sub(segment_start) >= self.policy.segment_duration;
            if crosses_partial || forces_segment {
                emitted = Some(self.flush_pending());
            }
        }

        // Step 2: classify this sample if pending is empty (either
        // because we just flushed or because this is the very
        // first sample on this coalescer).
        if self.pending.is_empty() {
            let kind = match self.segment_start_dts {
                None => {
                    // Very first sample on the track. Always
                    // starts a segment regardless of keyframe flag
                    // so a malformed producer that skips its first
                    // IDR does not crash the coalescer.
                    self.segment_start_dts = Some(sample.dts);
                    CmafChunkKind::Segment
                }
                Some(segment_start) => {
                    if sample.keyframe && sample.dts.saturating_sub(segment_start) >= self.policy.segment_duration {
                        self.segment_start_dts = Some(sample.dts);
                        CmafChunkKind::Segment
                    } else if sample.keyframe {
                        CmafChunkKind::PartialIndependent
                    } else {
                        CmafChunkKind::Partial
                    }
                }
            };
            self.pending_kind = kind;
            self.partial_start_dts = Some(sample.dts);
            self.pending_dts = Some(sample.dts);
        }

        // Step 3: append to pending.
        self.pending_duration_ticks += sample.duration as u64;
        self.pending.push(sample);

        emitted
    }

    /// Drain whatever is in pending as one final chunk. Call at
    /// end-of-stream so no samples are lost.
    pub fn flush(&mut self) -> Option<CmafChunk> {
        if self.pending.is_empty() {
            return None;
        }
        Some(self.flush_pending())
    }

    /// Build a chunk from the current pending batch and reset.
    fn flush_pending(&mut self) -> CmafChunk {
        debug_assert!(!self.pending.is_empty(), "flush_pending called on empty batch");
        let samples = std::mem::take(&mut self.pending);
        let dts = self.pending_dts.expect("non-empty pending has a DTS");
        let duration = self.pending_duration_ticks;
        let kind = self.pending_kind;
        let sequence = self.next_sequence;
        self.next_sequence += 1;

        let payload = build_moof_mdat(sequence, self.track_id, dts, &samples);

        // Reset pending state. The partial window advances to the
        // DTS the next sample will land at; we defer computing
        // that to `push` because the next sample carries the
        // authoritative DTS.
        self.pending_duration_ticks = 0;
        self.pending_dts = None;
        self.partial_start_dts = None;

        CmafChunk {
            track_id: format!("{}.mp4", self.track_id),
            payload,
            dts,
            duration,
            kind,
        }
    }
}

/// Build a wire-ready `moof + mdat` pair for one batch of samples.
///
/// Exposed as a free function so tests can drive it directly
/// without standing up a full `TrackCoalescer` state machine.
pub fn build_moof_mdat(sequence: u32, track_id: u32, base_dts: u64, samples: &[RawSample]) -> Bytes {
    let entries: Vec<TrunEntry> = samples
        .iter()
        .map(|s| TrunEntry {
            duration: Some(s.duration),
            size: Some(s.payload.len() as u32),
            flags: Some(sample_flags(s.keyframe)),
            cts: Some(s.cts_offset),
        })
        .collect();

    let mut moof = Moof {
        mfhd: Mfhd {
            sequence_number: sequence,
        },
        traf: vec![Traf {
            tfhd: Tfhd {
                track_id,
                ..Default::default()
            },
            tfdt: Some(Tfdt {
                base_media_decode_time: base_dts,
            }),
            trun: vec![Trun {
                // Placeholder; we rewrite this after the first
                // encode pass.
                data_offset: Some(0),
                entries,
            }],
            ..Default::default()
        }],
    };

    // First pass: encode to measure the moof size. Every field in
    // the moof is fixed-width relative to the sample count, so the
    // size from this pass is stable across the second pass even
    // though the data_offset field changes value.
    let mut buf = BytesMut::with_capacity(256);
    moof.encode(&mut buf).expect("first moof encode");
    let moof_size = buf.len();
    let data_offset = (moof_size + 8) as i32; // +8 for mdat header

    // Second pass: re-encode with the real data_offset.
    moof.traf[0].trun[0].data_offset = Some(data_offset);
    buf.clear();
    moof.encode(&mut buf).expect("second moof encode");
    debug_assert_eq!(buf.len(), moof_size, "moof size must be stable across encodes");

    // Append mdat. We write the header by hand (4 bytes size, 4
    // bytes "mdat") rather than going through mp4-atom's `Mdat`
    // struct because `Mdat` takes ownership of a `Vec<u8>` and
    // would force a copy of every sample payload; writing the
    // header by hand and extending `buf` with each `Bytes`
    // payload avoids the intermediate allocation.
    let mdat_body_len: usize = samples.iter().map(|s| s.payload.len()).sum();
    let mdat_size = 8 + mdat_body_len;
    buf.put_u32(mdat_size as u32);
    buf.put_slice(b"mdat");
    for s in samples {
        buf.put_slice(&s.payload);
    }

    buf.freeze()
}

/// ISO/IEC 14496-12 `sample_flags` for a video sample.
///
/// The bit layout matches what the hand-rolled
/// `lvqr-ingest::remux::fmp4::video_segment` writer ships today:
/// keyframes set `depends_on = 2` (does NOT depend) and leave
/// `is_non_sync_sample = 0`; non-keyframes set `depends_on = 1`
/// (does depend) and `is_non_sync_sample = 1`.
const fn sample_flags(keyframe: bool) -> u32 {
    if keyframe { 0x02000000 } else { 0x01010000 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mp4_atom::Decode;

    fn mk_sample(dts: u64, duration: u32, keyframe: bool) -> RawSample {
        RawSample {
            track_id: 1,
            dts,
            cts_offset: 0,
            duration,
            payload: Bytes::from_static(&[0u8; 16]),
            keyframe,
        }
    }

    #[test]
    fn first_sample_does_not_flush() {
        let mut c = TrackCoalescer::new(1, CmafPolicy::VIDEO_90KHZ_DEFAULT);
        assert!(c.push(mk_sample(0, 3000, true)).is_none());
    }

    #[test]
    fn partial_boundary_flushes_pending() {
        let mut c = TrackCoalescer::new(1, CmafPolicy::VIDEO_90KHZ_DEFAULT);
        // Push samples that fit inside one 200 ms partial window
        // (partial_duration = 18_000 at 90 kHz).
        assert!(c.push(mk_sample(0, 3000, true)).is_none());
        assert!(c.push(mk_sample(3000, 3000, false)).is_none());
        assert!(c.push(mk_sample(6000, 3000, false)).is_none());
        assert!(c.push(mk_sample(9000, 3000, false)).is_none());
        assert!(c.push(mk_sample(12_000, 3000, false)).is_none());
        assert!(c.push(mk_sample(15_000, 3000, false)).is_none());
        // This push crosses the 18_000-tick partial boundary.
        let chunk = c
            .push(mk_sample(18_000, 3000, false))
            .expect("partial boundary emits a chunk");
        assert_eq!(chunk.kind, CmafChunkKind::Segment, "first chunk is a segment start");
        // The chunk's duration covers the six samples inside the
        // partial window (6 * 3000 = 18_000).
        assert_eq!(chunk.duration, 18_000);
    }

    #[test]
    fn segment_boundary_fires_on_keyframe_past_window() {
        let mut c = TrackCoalescer::new(1, CmafPolicy::VIDEO_90KHZ_DEFAULT);
        // First sample (keyframe) starts segment 0.
        c.push(mk_sample(0, 3000, true));
        // Walk through 60 delta samples at 3000 ticks each. The
        // first partial boundary (18_000 ticks) closes the very
        // first pending batch, which is the head of segment 0 and
        // therefore carries kind = Segment. Subsequent flushes
        // inside segment 0 are Partial because the batches no
        // longer include the original keyframe at their head.
        let mut chunk_count = 0;
        let mut saw_partial = false;
        for i in 1..60 {
            if let Some(chunk) = c.push(mk_sample(i * 3000, 3000, false)) {
                chunk_count += 1;
                if chunk_count == 1 {
                    // First emission is the head of segment 0.
                    assert_eq!(chunk.kind, CmafChunkKind::Segment);
                } else {
                    // Subsequent emissions are non-keyframe
                    // partials inside segment 0. They must NOT
                    // carry Segment kind because the policy only
                    // reopens a segment on a keyframe at or past
                    // the segment window.
                    assert_ne!(chunk.kind, CmafChunkKind::Segment);
                    if chunk.kind == CmafChunkKind::Partial {
                        saw_partial = true;
                    }
                }
            }
        }
        assert!(saw_partial, "expected at least one Partial chunk inside segment 0");
        // Now push a keyframe well past the 2 s (180_000 tick)
        // segment window. It must trigger a segment-kind flush of
        // whatever pending was left over, AND start a new
        // Segment-kind pending. The `push` return value here is
        // the flushed chunk from BEFORE the new sample landed, so
        // it may be a Partial (the last open non-keyframe batch);
        // a subsequent flush call drains the new Segment-kind
        // pending.
        let emit_at_jump = c.push(mk_sample(200_000, 3000, true));
        if let Some(ch) = emit_at_jump {
            // The push flushed the previous pending; that batch
            // was non-keyframe so its kind is Partial, not Segment.
            assert_ne!(ch.kind, CmafChunkKind::Segment);
        }
        // Draining now must yield a Segment-kind chunk containing
        // the keyframe we just pushed.
        let final_chunk = c.flush().expect("flush yields the new segment head");
        assert_eq!(final_chunk.kind, CmafChunkKind::Segment);
    }

    #[test]
    fn flush_drains_pending_at_end_of_stream() {
        let mut c = TrackCoalescer::new(1, CmafPolicy::VIDEO_90KHZ_DEFAULT);
        c.push(mk_sample(0, 3000, true));
        c.push(mk_sample(3000, 3000, false));
        let last = c.flush().expect("flush returns pending");
        assert_eq!(last.duration, 6000);
        assert!(c.flush().is_none(), "idempotent after drain");
    }

    #[test]
    fn moof_mdat_round_trips_through_mp4_atom_decoder() {
        let samples = vec![
            RawSample::keyframe(1, 0, 3000, Bytes::from_static(&[0u8; 32])),
            RawSample::delta(1, 3000, 3000, Bytes::from_static(&[0u8; 16])),
        ];
        let bytes = build_moof_mdat(1, 1, 0, &samples);
        let mut cursor = std::io::Cursor::new(bytes.as_ref());
        let moof = mp4_atom::Moof::decode(&mut cursor).expect("decode moof");
        assert_eq!(moof.mfhd.sequence_number, 1);
        assert_eq!(moof.traf.len(), 1);
        assert_eq!(moof.traf[0].tfhd.track_id, 1);
        assert_eq!(moof.traf[0].tfdt.as_ref().map(|t| t.base_media_decode_time), Some(0));
        let trun = &moof.traf[0].trun[0];
        assert_eq!(trun.entries.len(), 2);
        assert_eq!(trun.entries[0].size, Some(32));
        assert_eq!(trun.entries[1].size, Some(16));
        // data_offset must point just past the moof header into
        // the mdat payload. Our writer sets it to
        // `moof_size + 8` (mdat header).
        let data_offset = trun.data_offset.unwrap();
        assert!(data_offset > 0);
    }
}

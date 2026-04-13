//! [`CmafSegmenter`]: drives a [`FragmentStream`] into [`CmafChunk`] values.
//!
//! The segmenter is deliberately thin. It owns a [`CmafPolicyState`]
//! per track and a reference to the producer's [`FragmentMeta`]. For
//! every [`Fragment`] the producer emits, the segmenter:
//!
//! 1. Asks the policy state machine for the chunk kind.
//! 2. Wraps the fragment payload and timing data in a [`CmafChunk`].
//! 3. Returns the chunk to the caller.
//!
//! Today every `Fragment::payload` is already a wire-ready `moof +
//! mdat` pair emitted by `lvqr-ingest::remux::fmp4`, so the segmenter
//! is a 1:1 pass-through that annotates the chunk with HLS/DASH/MoQ
//! boundary info. When the full Tier 2.3 migration lands (new ingest
//! path that emits raw samples rather than pre-muxed fragments), the
//! segmenter will grow a sample coalescer that builds its own `moof +
//! mdat` via `mp4-atom`. Keeping the scaffold thin today means that
//! migration is additive: new sample-coalescer code, zero changes to
//! the public surface.
//!
//! ## Raw-sample coalescer design note (session 7)
//!
//! Before the sample coalescer lands, we need to agree on how raw
//! samples enter the pipeline, how per-track timing is tracked, and
//! how the HLS partial / DASH segment / MoQ group boundaries interact
//! with the keyframe cadence. This section is the scope document.
//! Treat it as a living spec that the first implementation PR is
//! allowed to rewrite.
//!
//! ### Input shape
//!
//! The coalescer consumes a stream of `RawSample` values. A
//! `RawSample` is a minimal value type carrying:
//!
//! ```text
//! RawSample {
//!     track_id: u32,
//!     dts: u64,          // per-track timescale ticks
//!     cts_offset: i32,   // composition offset, usually zero for audio
//!     duration: u32,     // per-sample duration in timescale ticks
//!     payload: Bytes,    // AVCC length-prefixed for video,
//!                        // raw AU for AAC (no ADTS header)
//!     flags: SampleFlags,// { keyframe, depends_on, is_depended_on }
//! }
//! ```
//!
//! The producer is authoritative for timing. The coalescer never
//! re-derives DTS from PTS or vice versa and never re-parses the
//! payload to infer keyframe status; every bit of metadata the
//! coalescer needs is in the struct. This is the opposite of the SRS
//! "parse the RTMP tag inside the core type" design and the same as
//! the OvenMediaEngine `Provider/Publisher/Stream` split.
//!
//! ### State per track
//!
//! One `TrackCoalescer` per track. Each owns:
//!
//! * A `CmafPolicyState` (already implemented).
//! * A `pending_samples: Vec<RawSample>` buffer holding the samples
//!   that have not yet been flushed into a chunk.
//! * A `next_sequence_number: u32` counter for the `mfhd` box.
//! * An `init_segment: Bytes` produced once by the HEVC / AVC / AAC
//!   init writers from `crate::init` the moment the coalescer sees
//!   the first sample and knows enough to build an init.
//!
//! The multi-track surface is a `HashMap<u32, TrackCoalescer>` keyed
//! by track id. The segmenter emits one `CmafChunk` per track per
//! flush; the HLS / DASH egress crates are responsible for aligning
//! the per-track outputs into a single playlist.
//!
//! ### Boundary decision
//!
//! For each incoming sample the coalescer runs the policy state
//! machine. The decision is:
//!
//! 1. **Append**: the sample stays in the pending buffer, no chunk
//!    emitted.
//! 2. **Flush as partial**: emit a chunk built from the pending
//!    samples, kind = `Partial` or `PartialIndependent`.
//! 3. **Flush as segment**: emit a chunk built from the pending
//!    samples, kind = `Segment`. This is always the start of a new
//!    HLS segment, a new DASH segment, and a new MoQ group.
//!
//! The "flush as partial" decision fires when the pending duration
//! hits `CmafPolicy::partial_duration`. The "flush as segment"
//! decision fires when a keyframe arrives after the pending duration
//! has already passed `CmafPolicy::segment_duration`. Edge case: a
//! keyframe that arrives before the segment duration closes ends the
//! current segment early iff the policy sets `honor_keyframe_cadence`
//! (not yet in the policy, add with the coalescer).
//!
//! ### `moof + mdat` construction
//!
//! Build the `moof` via `mp4_atom::Moof` + `mp4_atom::Traf` +
//! `mp4_atom::Tfhd` + `mp4_atom::Tfdt` + `mp4_atom::Trun`, then append
//! an `mp4_atom::Mdat` carrying the concatenated sample payloads.
//! `Trun` must set the `data_offset_present` flag and carry a
//! placeholder offset that is patched after the `moof` is encoded but
//! before the `mdat` is written (mp4-atom does not offer a
//! `patch_data_offset` helper today; the coalescer will either pass a
//! post-encode offset patcher or precompute the moof size). The
//! hand-rolled `lvqr-ingest` writer at `remux/fmp4.rs:528` is the
//! reference implementation; the coalescer should produce the same
//! byte layout modulo the harmless differences already cataloged in
//! `crates/lvqr-cmaf/tests/parity_avc_init.rs`.
//!
//! Per-sample fields inside the `Trun`:
//!
//! * `sample_duration`: from `RawSample::duration`.
//! * `sample_size`: `payload.len()`.
//! * `sample_flags`: built from `RawSample::flags` per the ISO BMFF
//!   `sample_flags` layout (6 bits of `is_leading` + `depends_on` +
//!   `is_depended_on` + `has_redundancy`, plus the 16-bit degradation
//!   priority and the 1-bit `is_non_sync_sample`).
//! * `sample_composition_time_offset`: `cts_offset`, present only
//!   when any sample in the chunk has a non-zero offset (saves bytes
//!   on audio tracks that never need CTS).
//!
//! ### Init segment lifecycle
//!
//! The first sample on a given track triggers init segment
//! construction. For video, the producer supplies SPS / PPS / VPS /
//! dimensions + parsed HEVC or AVC sample entries as part of the
//! `RawSample` sidecar (or a separate `TrackInit` message that
//! precedes the first `RawSample`). The coalescer passes those into
//! `write_avc_init_segment` / `write_hevc_init_segment` and caches
//! the result as `TrackCoalescer::init_segment`.
//!
//! The `CmafSegmenter` public surface grows one method:
//!
//! ```text
//! pub fn init_segment(&self, track_id: u32) -> Option<&Bytes>;
//! ```
//!
//! Egress crates call it the moment they need to prepend an init
//! segment to a new subscriber's stream. Returns `None` until the
//! first sample on that track has been seen.
//!
//! ### Interaction with the existing passthrough path
//!
//! The sample coalescer is strictly additive. The existing
//! `FragmentStream` -> pre-muxed `moof + mdat` passthrough stays in
//! place for the `rtmp_ws_e2e` path because retiring it requires a
//! full rewrite of the RTMP bridge to stop emitting pre-muxed
//! fragments. The coalescer enters via a new constructor
//! `CmafSegmenter::from_sample_stream(sample_stream, ...)` and lives
//! alongside the existing `CmafSegmenter::new(fragment_stream, ...)`
//! during the transition.
//!
//! ### Session 7 deliverable
//!
//! Session 7 should land:
//!
//! 1. A `RawSample` + `SampleStream` trait pair in `lvqr-fragment` or
//!    a new `lvqr-cmaf::sample` module. Pick the location based on
//!    which crate ends up owning the producer side (ingest will
//!    eventually emit `RawSample` directly, so `lvqr-fragment` is the
//!    better long-term home).
//! 2. A `TrackCoalescer` struct with a `push(sample) -> Option<CmafChunk>`
//!    method. Pure state machine, no I/O, trivially proptest-able.
//! 3. A thin `CmafSegmenter::from_sample_stream` constructor wrapping
//!    a `HashMap<TrackId, TrackCoalescer>` and pulling samples via
//!    the new trait.
//! 4. A round-trip test that drives a scripted sample stream through
//!    the coalescer and asserts the output chunks are structurally
//!    equivalent to what `lvqr-ingest::remux::fmp4::video_segment`
//!    produces for the same input.
//!
//! Items 1 and 2 are load-bearing. Items 3 and 4 can be split into
//! session 7.5 if the trait design turns out to require more
//! negotiation with the lvqr-ingest side than anticipated.

use crate::chunk::CmafChunk;
use crate::policy::{CmafPolicy, CmafPolicyState};
use lvqr_fragment::{Fragment, FragmentMeta, FragmentStream};

/// Errors the segmenter can surface.
#[derive(Debug, thiserror::Error)]
pub enum SegmenterError {
    /// The upstream [`FragmentStream`] ended before emitting any
    /// fragments. Callers typically treat this as "source disconnected"
    /// and propagate a session-level error.
    #[error("fragment stream ended before emitting any fragments")]
    EmptyStream,
}

/// Pull-based CMAF segmenter.
///
/// Constructed with an owned [`FragmentStream`] and a
/// [`CmafPolicy`]; exposes `next_chunk` as the counterpart to
/// `FragmentStream::next_fragment`. Multi-track support is not
/// implemented at this scaffold stage: the segmenter keeps one
/// policy state machine per instance, so callers that need video and
/// audio should construct two segmenters and zip the outputs.
pub struct CmafSegmenter<S: FragmentStream> {
    stream: S,
    policy: CmafPolicyState,
}

impl<S: FragmentStream> CmafSegmenter<S> {
    pub fn new(stream: S, policy: CmafPolicy) -> Self {
        Self {
            stream,
            policy: CmafPolicyState::new(policy),
        }
    }

    /// Borrow the underlying [`FragmentMeta`] for callers that need the
    /// codec string or init segment before the first chunk arrives.
    pub fn meta(&self) -> &FragmentMeta {
        self.stream.meta()
    }

    /// Pull the next chunk. Returns `None` when the upstream stream is
    /// exhausted.
    pub async fn next_chunk(&mut self) -> Option<CmafChunk> {
        let frag: Fragment = self.stream.next_fragment().await?;
        Some(self.fragment_to_chunk(frag))
    }

    fn fragment_to_chunk(&mut self, frag: Fragment) -> CmafChunk {
        let decision = self.policy.step(frag.flags.keyframe, frag.dts);
        CmafChunk {
            track_id: frag.track_id,
            payload: frag.payload,
            dts: frag.dts,
            duration: frag.duration,
            kind: decision.kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::CmafChunkKind;
    use bytes::Bytes;
    use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta, FragmentStream};
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;

    /// Deterministic in-memory fragment stream used by every
    /// segmenter test. Keeps the test code free of real codec
    /// parsing: we feed pre-built fragments and assert on the
    /// segmenter's policy output.
    struct VecStream {
        meta: FragmentMeta,
        remaining: VecDeque<Fragment>,
    }

    impl FragmentStream for VecStream {
        fn meta(&self) -> &FragmentMeta {
            &self.meta
        }
        fn next_fragment<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Option<Fragment>> + Send + 'a>> {
            Box::pin(async move { self.remaining.pop_front() })
        }
    }

    fn make_fragment(group_id: u64, object_id: u64, dts: u64, duration: u64, keyframe: bool) -> Fragment {
        Fragment::new(
            "0.mp4",
            group_id,
            object_id,
            0,
            dts,
            dts,
            duration,
            if keyframe {
                FragmentFlags::KEYFRAME
            } else {
                FragmentFlags::DELTA
            },
            Bytes::from_static(b"moof+mdat placeholder"),
        )
    }

    #[tokio::test]
    async fn segmenter_emits_segment_then_partials_then_new_segment() {
        // Feed: keyframe at 0, delta at 30k, delta at 60k, delta at 90k,
        // delta at 120k, delta at 150k, keyframe at 180k (exactly one
        // segment duration later). With the default 90kHz / 2s policy
        // this should yield: Segment, Partial, Partial, Partial,
        // Partial, Partial, Segment.
        let meta = FragmentMeta::new("avc1.640028", 90_000);
        let stream = VecStream {
            meta,
            remaining: [
                (0, 0, true),
                (0, 1, false),
                (0, 2, false),
                (0, 3, false),
                (0, 4, false),
                (0, 5, false),
                (1, 0, true),
            ]
            .into_iter()
            .map(|(g, o, kf)| make_fragment(g, o, o * 30_000, 30_000, kf))
            .collect::<VecDeque<_>>(),
        };
        // The last keyframe is at DTS 180000, which matches exactly
        // the 2 s segment boundary. Fix up by passing its real dts:
        let mut stream = stream;
        stream.remaining[6] = make_fragment(1, 6, 180_000, 30_000, true);

        let mut seg = CmafSegmenter::new(stream, CmafPolicy::VIDEO_90KHZ_DEFAULT);
        let kinds: Vec<_> = {
            let mut out = Vec::new();
            while let Some(chunk) = seg.next_chunk().await {
                out.push(chunk.kind);
            }
            out
        };
        assert_eq!(
            kinds,
            vec![
                CmafChunkKind::Segment,
                CmafChunkKind::Partial,
                CmafChunkKind::Partial,
                CmafChunkKind::Partial,
                CmafChunkKind::Partial,
                CmafChunkKind::Partial,
                CmafChunkKind::Segment,
            ]
        );
    }

    #[tokio::test]
    async fn segmenter_passthrough_payload_and_timing() {
        let meta = FragmentMeta::new("avc1.640028", 90_000);
        let stream = VecStream {
            meta,
            remaining: [make_fragment(0, 0, 0, 3000, true)].into_iter().collect(),
        };
        let mut seg = CmafSegmenter::new(stream, CmafPolicy::VIDEO_90KHZ_DEFAULT);
        let chunk = seg.next_chunk().await.expect("one chunk");
        assert_eq!(chunk.track_id, "0.mp4");
        assert_eq!(chunk.dts, 0);
        assert_eq!(chunk.duration, 3000);
        assert_eq!(chunk.kind, CmafChunkKind::Segment);
        assert_eq!(chunk.payload.as_ref(), b"moof+mdat placeholder");
    }
}

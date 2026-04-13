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

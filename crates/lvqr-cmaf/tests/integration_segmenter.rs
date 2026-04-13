//! Integration test for [`lvqr_cmaf::CmafSegmenter`].
//!
//! Drives a hand-built [`lvqr_fragment::FragmentStream`] through the
//! segmenter and asserts on the observable sequence of
//! [`lvqr_cmaf::CmafChunk`] values. This is the "integration" slot of
//! the 5-artifact contract for `lvqr-cmaf`: no real ingest, no real
//! network, but every moving part of the segmenter (policy state,
//! chunk construction, FragmentStream plumbing) is exercised together.

use bytes::Bytes;
use lvqr_cmaf::{CmafChunkKind, CmafPolicy, CmafSegmenter};
use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta, FragmentStream};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;

struct ScriptedStream {
    meta: FragmentMeta,
    frames: VecDeque<Fragment>,
}

impl FragmentStream for ScriptedStream {
    fn meta(&self) -> &FragmentMeta {
        &self.meta
    }
    fn next_fragment<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Option<Fragment>> + Send + 'a>> {
        Box::pin(async move { self.frames.pop_front() })
    }
}

fn frag(group: u64, object: u64, dts: u64, keyframe: bool) -> Fragment {
    Fragment::new(
        "0.mp4",
        group,
        object,
        0,
        dts,
        dts,
        3_000, // 30 fps at 90 kHz
        if keyframe {
            FragmentFlags::KEYFRAME
        } else {
            FragmentFlags::DELTA
        },
        Bytes::from_static(b"synthetic-moof-mdat"),
    )
}

#[tokio::test]
async fn segments_every_two_seconds_at_30fps() {
    // 30 fps at 90 kHz => 3000 ticks per frame. Two keyframes at DTS 0
    // and DTS 180000 (exactly the 2 s boundary). Every frame in
    // between is a delta. The segmenter should emit Segment, 59
    // Partials, Segment.
    let meta = FragmentMeta::new("avc1.640028", 90_000);
    let mut frames = VecDeque::new();
    for i in 0..=60u64 {
        let dts = i * 3_000;
        let keyframe = i == 0 || i == 60;
        frames.push_back(frag(i / 60, i, dts, keyframe));
    }

    let mut seg = CmafSegmenter::new(ScriptedStream { meta, frames }, CmafPolicy::VIDEO_90KHZ_DEFAULT);

    let mut kinds = Vec::new();
    while let Some(chunk) = seg.next_chunk().await {
        kinds.push(chunk.kind);
    }

    assert_eq!(kinds.len(), 61);
    assert_eq!(kinds[0], CmafChunkKind::Segment);
    assert_eq!(kinds[60], CmafChunkKind::Segment);
    for (i, k) in kinds.iter().enumerate().skip(1).take(59) {
        assert_eq!(*k, CmafChunkKind::Partial, "frame {i} should be Partial");
    }
}

#[tokio::test]
async fn idr_refresh_mid_segment_emits_partial_independent() {
    // Two keyframes at DTS 0 and DTS 90_000 (1 s in). The second
    // keyframe should not close a segment (1 s < 2 s policy) but
    // should be PartialIndependent so LL-HLS can flag it with
    // INDEPENDENT=YES.
    let meta = FragmentMeta::new("avc1.640028", 90_000);
    let frames: VecDeque<Fragment> = [
        frag(0, 0, 0, true),
        frag(0, 1, 30_000, false),
        frag(1, 0, 90_000, true),
        frag(1, 1, 120_000, false),
    ]
    .into_iter()
    .collect();

    let mut seg = CmafSegmenter::new(ScriptedStream { meta, frames }, CmafPolicy::VIDEO_90KHZ_DEFAULT);
    let kinds: Vec<_> = {
        let mut out = Vec::new();
        while let Some(c) = seg.next_chunk().await {
            out.push(c.kind);
        }
        out
    };
    assert_eq!(
        kinds,
        vec![
            CmafChunkKind::Segment,
            CmafChunkKind::Partial,
            CmafChunkKind::PartialIndependent,
            CmafChunkKind::Partial,
        ]
    );
}

#[tokio::test]
async fn empty_stream_returns_none() {
    let meta = FragmentMeta::new("avc1.640028", 90_000);
    let mut seg = CmafSegmenter::new(
        ScriptedStream {
            meta,
            frames: VecDeque::new(),
        },
        CmafPolicy::VIDEO_90KHZ_DEFAULT,
    );
    assert!(seg.next_chunk().await.is_none());
}

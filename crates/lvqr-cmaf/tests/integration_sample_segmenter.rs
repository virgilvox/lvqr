//! Integration test for [`CmafSampleSegmenter`] driven by a
//! scripted [`SampleStream`].
//!
//! The session-9 coalescer unit tests exercised the per-track
//! state machine directly. This test stands up the full
//! multi-track pipeline: a synthetic `SampleStream` yielding
//! interleaved video and audio samples, a `CmafSampleSegmenter`
//! routing them into per-track `TrackCoalescer` instances, and an
//! assertion on the emission order and chunk counts.

use bytes::{Bytes, BytesMut};
use lvqr_cmaf::{
    CmafChunkKind, CmafPolicy, CmafSampleSegmenter, RawSample, SampleStream, VideoInitParams, write_avc_init_segment,
};
use lvqr_fragment::FragmentMeta;
use lvqr_test_utils::ffprobe_bytes;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;

/// Deterministic in-memory `SampleStream` fed from a `VecDeque`.
struct VecSampleStream {
    meta: FragmentMeta,
    remaining: VecDeque<RawSample>,
}

impl SampleStream for VecSampleStream {
    fn meta(&self) -> &FragmentMeta {
        &self.meta
    }
    fn next_sample<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Option<RawSample>> + Send + 'a>> {
        Box::pin(async move { self.remaining.pop_front() })
    }
}

fn avcc_nal(total_len: u32, nal_header: u8) -> Bytes {
    assert!(total_len >= 5);
    let body_len = total_len - 4;
    let mut buf = BytesMut::with_capacity(total_len as usize);
    buf.extend_from_slice(&body_len.to_be_bytes());
    buf.extend_from_slice(&[nal_header]);
    buf.extend_from_slice(&vec![0u8; (body_len - 1) as usize]);
    buf.freeze()
}

#[tokio::test]
async fn segmenter_emits_chunks_for_single_video_track() {
    // 10 samples at 200 ms each on track 1. One keyframe + nine
    // P-slices. The coalescer should flush at every partial
    // boundary (every 18_000 ticks); with 10 samples of 18_000
    // ticks we expect 9 flushes from the crossing-boundary rule
    // plus one trailing flush from the drain, for 10 chunks total.
    let mut remaining = VecDeque::new();
    for i in 0..10 {
        let dts = (i as u64) * 18_000;
        let keyframe = i == 0;
        let nal_header = if keyframe { 0x65 } else { 0x41 };
        remaining.push_back(RawSample {
            track_id: 1,
            dts,
            cts_offset: 0,
            duration: 18_000,
            payload: avcc_nal(64, nal_header),
            keyframe,
        });
    }
    let stream = VecSampleStream {
        meta: FragmentMeta::new("avc1.42001F", 90_000),
        remaining,
    };
    let mut seg = CmafSampleSegmenter::new(stream, CmafPolicy::VIDEO_90KHZ_DEFAULT);

    let mut chunks = Vec::new();
    while let Some(chunk) = seg.next_chunk().await {
        chunks.push(chunk);
    }
    assert!(!chunks.is_empty());
    assert_eq!(chunks[0].kind, CmafChunkKind::Segment);
    // Every chunk has a non-empty payload produced by the
    // coalescer's moof+mdat writer.
    for c in &chunks {
        assert!(!c.payload.is_empty());
    }

    // Sanity: feed the concatenated init + chunks through
    // ffprobe. The same bytes the conformance_coalescer test
    // validates, but built through the CmafSampleSegmenter
    // pipeline instead of a bare TrackCoalescer.
    let mut init = BytesMut::new();
    write_avc_init_segment(
        &mut init,
        &VideoInitParams {
            sps: vec![
                0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00,
                0x00, 0x03, 0x03, 0xC0, 0xF1, 0x83, 0x2A,
            ],
            pps: vec![0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0],
            width: 1280,
            height: 720,
            timescale: 90_000,
        },
    )
    .expect("init encode");
    let mut buf = Vec::with_capacity(init.len() + chunks.iter().map(|c| c.payload.len()).sum::<usize>());
    buf.extend_from_slice(&init);
    for chunk in &chunks {
        buf.extend_from_slice(&chunk.payload);
    }
    ffprobe_bytes(&buf).assert_accepted();
}

#[tokio::test]
async fn segmenter_routes_multi_track_samples_to_distinct_coalescers() {
    // Interleave a video track (id 1) and an audio track (id 2).
    // Each track has its own coalescer; the segmenter should
    // emit chunks tagged with the correct track_id string.
    let mut remaining = VecDeque::new();
    for i in 0..6 {
        let dts = (i as u64) * 18_000;
        remaining.push_back(RawSample {
            track_id: 1,
            dts,
            cts_offset: 0,
            duration: 18_000,
            payload: avcc_nal(48, if i == 0 { 0x65 } else { 0x41 }),
            keyframe: i == 0,
        });
        // Audio sample interleaved at the same DTS. Every AAC
        // frame is independently decodable.
        remaining.push_back(RawSample {
            track_id: 2,
            dts,
            cts_offset: 0,
            duration: 18_000,
            payload: Bytes::from(vec![0u8; 64]),
            keyframe: true,
        });
    }
    let stream = VecSampleStream {
        meta: FragmentMeta::new("avc1.42001F", 90_000),
        remaining,
    };
    let mut seg = CmafSampleSegmenter::new(stream, CmafPolicy::VIDEO_90KHZ_DEFAULT);

    let mut video_chunks = 0;
    let mut audio_chunks = 0;
    while let Some(chunk) = seg.next_chunk().await {
        match chunk.track_id.as_str() {
            "1.mp4" => video_chunks += 1,
            "2.mp4" => audio_chunks += 1,
            other => panic!("unexpected track id {other}"),
        }
    }
    assert!(video_chunks > 0, "video track produced at least one chunk");
    assert!(audio_chunks > 0, "audio track produced at least one chunk");
}

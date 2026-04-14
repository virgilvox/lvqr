//! End-to-end conformance check for the `mp4-atom`-backed
//! [`TrackCoalescer`].
//!
//! Builds a real AVC init segment via `lvqr_cmaf::write_avc_init_segment`,
//! pushes a scripted burst of `RawSample` values through a
//! `TrackCoalescer`, concatenates the init segment with the
//! coalescer-produced media segments, and runs the whole thing
//! through `ffprobe 8.1` via the soft-skip helper in
//! `lvqr-test-utils`.
//!
//! This is the first real-encoder-validated proof that the
//! coalescer's `moof + mdat` output is structurally sound. When the
//! Tier 2.3 migration retires the hand-rolled
//! `lvqr-ingest::remux::fmp4::video_segment` writer, this test
//! becomes the gate that catches drift between the replacement and
//! the reference.

use bytes::{Bytes, BytesMut};
use lvqr_cmaf::{CmafChunkKind, CmafPolicy, RawSample, TrackCoalescer, VideoInitParams, write_avc_init_segment};
use lvqr_test_utils::ffprobe_bytes;

/// Deterministic SPS + PPS from the AVC init segment's existing
/// unit test fixture. Same bytes that the parity test and the
/// conformance test already pin.
const SPS: &[u8] = &[
    0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03,
    0x03, 0xC0, 0xF1, 0x83, 0x2A,
];
const PPS: &[u8] = &[0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];

/// Build a synthetic AVCC-length-prefixed NAL unit blob of the
/// given total size. The first 4 bytes are the big-endian length
/// of the NAL body; the next byte is the NAL header; the rest is
/// zero padding. `nal_header = 0x65` produces an IDR slice
/// (keyframe); `0x41` produces a non-IDR P-slice (delta).
fn avcc_nal(total_len: u32, nal_header: u8) -> Bytes {
    assert!(total_len >= 5);
    let body_len = total_len - 4;
    let mut buf = BytesMut::with_capacity(total_len as usize);
    buf.extend_from_slice(&body_len.to_be_bytes());
    buf.extend_from_slice(&[nal_header]);
    buf.extend_from_slice(&vec![0u8; (body_len - 1) as usize]);
    buf.freeze()
}

fn init_segment_bytes() -> Vec<u8> {
    let mut buf = BytesMut::new();
    write_avc_init_segment(
        &mut buf,
        &VideoInitParams {
            sps: SPS.to_vec(),
            pps: PPS.to_vec(),
            width: 1280,
            height: 720,
            timescale: 90_000,
        },
    )
    .expect("init encode");
    buf.to_vec()
}

#[test]
fn ffprobe_accepts_init_plus_coalescer_segment() {
    // Coalescer configured to cut one partial every 200 ms (18_000
    // ticks at 90 kHz) and one segment every 2 s (180_000 ticks).
    let mut c = TrackCoalescer::new(1, CmafPolicy::VIDEO_90KHZ_DEFAULT);

    // Push 10 samples at 200 ms each so the coalescer has a full
    // 2 s segment in hand. First sample is the IDR keyframe; the
    // rest are P-slices. Every sample is 64 bytes of AVCC-wrapped
    // synthetic NAL content so ffprobe can walk the container
    // without tripping on a malformed length field.
    let part_ticks = 18_000u32; // 200 ms at 90 kHz
    let mut chunks = Vec::new();
    for i in 0..10 {
        let dts = (i as u64) * part_ticks as u64;
        let (nal_header, keyframe) = if i == 0 { (0x65, true) } else { (0x41, false) };
        let payload = avcc_nal(64, nal_header);
        if let Some(chunk) = c.push(RawSample {
            track_id: 1,
            dts,
            cts_offset: 0,
            duration: part_ticks,
            payload,
            keyframe,
        }) {
            chunks.push(chunk);
        }
    }
    // Drain whatever is still pending at end-of-stream.
    if let Some(final_chunk) = c.flush() {
        chunks.push(final_chunk);
    }
    assert!(
        !chunks.is_empty(),
        "coalescer should have emitted at least one chunk for a 2 s burst"
    );
    // First chunk must be the head of segment 0.
    assert_eq!(chunks[0].kind, CmafChunkKind::Segment);

    // Concatenate init + every chunk's payload and feed it to
    // ffprobe. Every chunk payload is already a `moof + mdat`
    // pair; stacking them back-to-back is how fMP4 live streams
    // are served.
    let init = init_segment_bytes();
    let mut buf = Vec::with_capacity(init.len() + chunks.iter().map(|c| c.payload.len()).sum::<usize>());
    buf.extend_from_slice(&init);
    for chunk in &chunks {
        buf.extend_from_slice(&chunk.payload);
    }

    ffprobe_bytes(&buf).assert_accepted();
}

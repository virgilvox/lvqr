//! Criterion benches for the RTP packetizers on the PLAY egress
//! hot path.
//!
//! These cover the per-packet work the drain tasks do for every
//! NAL / access unit / Opus frame they emit. Measuring the
//! hot path is the first step toward the M4 performance claims in
//! the roadmap -- the data we collect here feeds the "LiveKit
//! alternative" positioning without having to hand-wave about
//! ballpark throughput.
//!
//! Bench layout:
//!
//! * `h264_single_nal_500B`: single-NAL RTP packet, common case.
//! * `h264_fu_a_4500B`: FU-A fragmentation over the default
//!   1400-byte MTU (4 packets).
//! * `h264_fu_a_64KB`: long FU-A chain so the per-fragment
//!   overhead is separable from the first-packet fixed cost.
//! * `hevc_single_nal_500B`: single-NAL HEVC packet.
//! * `hevc_fu_4500B`: HEVC FU fragmentation.
//! * `aac_256B_au`: RFC 3640 AAC-hbr packetization.
//! * `opus_80B_frame`: RFC 7587 Opus packetization.
//!
//! The packetizer functions take `&mut self` and bump sequence
//! numbers per emission; Criterion re-runs the closure many times
//! so sequence wrap is not a concern (every run happens against a
//! fresh packetizer with `u16::wrapping_add` on the sequence field).

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use lvqr_rtsp::rtp::{AacPacketizer, H264Packetizer, HevcPacketizer, OpusPacketizer};

/// Build a synthetic H.264 NAL of the requested byte length. Byte 0
/// is a realistic IDR slice header (0x65); the remainder is filler.
fn make_h264_nal(size: usize) -> Vec<u8> {
    assert!(size >= 1);
    let mut nal = Vec::with_capacity(size);
    nal.push(0x65u8);
    nal.resize(size, 0xAB);
    nal
}

/// Build a synthetic HEVC NAL of the requested byte length. Bytes
/// 0-1 form an IDR_W_RADL header (type 19); the remainder is filler.
fn make_hevc_nal(size: usize) -> Vec<u8> {
    assert!(size >= 2);
    let mut nal = Vec::with_capacity(size);
    nal.push(0x26u8);
    nal.push(0x01u8);
    nal.resize(size, 0xAB);
    nal
}

fn bench_h264(c: &mut Criterion) {
    let nal_500 = make_h264_nal(500);
    c.bench_function("h264_single_nal_500B", |b| {
        let mut p = H264Packetizer::new(0xDEAD_BEEF, 96, 0);
        b.iter(|| {
            let out = p.packetize(black_box(&nal_500), 0, true);
            black_box(out);
        });
    });

    let nal_4500 = make_h264_nal(4500);
    c.bench_function("h264_fu_a_4500B", |b| {
        let mut p = H264Packetizer::new(0xDEAD_BEEF, 96, 0);
        b.iter(|| {
            let out = p.packetize(black_box(&nal_4500), 0, true);
            black_box(out);
        });
    });

    let nal_64k = make_h264_nal(64 * 1024);
    c.bench_function("h264_fu_a_64KB", |b| {
        let mut p = H264Packetizer::new(0xDEAD_BEEF, 96, 0);
        b.iter(|| {
            let out = p.packetize(black_box(&nal_64k), 0, true);
            black_box(out);
        });
    });
}

fn bench_hevc(c: &mut Criterion) {
    let nal_500 = make_hevc_nal(500);
    c.bench_function("hevc_single_nal_500B", |b| {
        let mut p = HevcPacketizer::new(0xDEAD_BEEF, 96, 0);
        b.iter(|| {
            let out = p.packetize(black_box(&nal_500), 0, true);
            black_box(out);
        });
    });

    let nal_4500 = make_hevc_nal(4500);
    c.bench_function("hevc_fu_4500B", |b| {
        let mut p = HevcPacketizer::new(0xDEAD_BEEF, 96, 0);
        b.iter(|| {
            let out = p.packetize(black_box(&nal_4500), 0, true);
            black_box(out);
        });
    });
}

fn bench_aac(c: &mut Criterion) {
    // Typical AAC-LC AU is under 1 KB; 256 bytes mirrors a mid-
    // bitrate music stream at 128 kbps / 1024 samples.
    let au = vec![0x42u8; 256];
    c.bench_function("aac_256B_au", |b| {
        let mut p = AacPacketizer::new(0xCAFE, 97, 0);
        b.iter(|| {
            let out = p.packetize(black_box(&au), 0);
            black_box(out);
        });
    });
}

fn bench_opus(c: &mut Criterion) {
    // 80 bytes mirrors a WebRTC Opus 20 ms frame at ~32 kbps stereo.
    let frame = vec![0xFCu8; 80];
    c.bench_function("opus_80B_frame", |b| {
        let mut p = OpusPacketizer::new(0xCAFE, 98, 0);
        b.iter(|| {
            let out = p.packetize(black_box(&frame), 0);
            black_box(out);
        });
    });
}

criterion_group!(benches, bench_h264, bench_hevc, bench_aac, bench_opus);
criterion_main!(benches);

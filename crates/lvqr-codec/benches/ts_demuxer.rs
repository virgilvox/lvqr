//! Criterion microbenches for `lvqr_codec::ts::TsDemuxer`.
//!
//! Measures the throughput of MPEG-TS demuxing: PAT/PMT discovery,
//! PES reassembly, and PTS extraction. The hot path in SRT ingest
//! is `TsDemuxer::feed` called per UDP datagram; a linear slowdown
//! here shows up as ingest backpressure before any downstream
//! processing bottleneck.
//!
//! Run with:
//!
//!     cargo bench -p lvqr-codec --bench ts_demuxer

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use lvqr_codec::ts::TsDemuxer;

const TS_PACKET_SIZE: usize = 188;

fn make_ts_packet(pid: u16, pusi: bool, cc: u8, payload: &[u8]) -> [u8; TS_PACKET_SIZE] {
    let mut pkt = [0xFFu8; TS_PACKET_SIZE];
    pkt[0] = 0x47;
    pkt[1] = if pusi { 0x40 } else { 0x00 } | ((pid >> 8) as u8 & 0x1F);
    pkt[2] = pid as u8;
    pkt[3] = 0x10 | (cc & 0x0F);
    let copy_len = payload.len().min(184);
    pkt[4..4 + copy_len].copy_from_slice(&payload[..copy_len]);
    pkt
}

fn minimal_pat(pmt_pid: u16) -> Vec<u8> {
    let mut data = vec![0x00, 0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01];
    data.push(0xE0 | ((pmt_pid >> 8) as u8 & 0x1F));
    data.push(pmt_pid as u8);
    data.extend_from_slice(&[0x00; 4]);
    data
}

fn minimal_pmt(video_pid: u16) -> Vec<u8> {
    vec![
        0x00,
        0x02,
        0xB0,
        0x12,
        0x00,
        0x01,
        0xC1,
        0x00,
        0x00,
        0xE1,
        0x00,
        0xF0,
        0x00,
        0x1B,
        0xE0 | ((video_pid >> 8) as u8 & 0x1F),
        video_pid as u8,
        0xF0,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
    ]
}

fn encode_pts(pts_90k: u64) -> [u8; 5] {
    let pts = pts_90k & 0x1_FFFF_FFFF;
    [
        0x21 | ((pts >> 29) as u8 & 0x0E),
        (pts >> 22) as u8,
        0x01 | ((pts >> 14) as u8 & 0xFE),
        (pts >> 7) as u8,
        0x01 | ((pts << 1) as u8 & 0xFE),
    ]
}

fn video_pes(pts_90k: u64, es_len: usize) -> Vec<u8> {
    let es_payload = vec![0xAA; es_len];
    let pes_payload_len = (3 + 5 + es_payload.len()) as u16;
    let mut data = vec![
        0x00,
        0x00,
        0x01,
        0xE0,
        (pes_payload_len >> 8) as u8,
        pes_payload_len as u8,
        0x80,
        0x80,
        0x05,
    ];
    data.extend_from_slice(&encode_pts(pts_90k));
    data.extend_from_slice(&es_payload);
    data
}

/// Build a complete TS stream: PAT + PMT + N video PES packets,
/// each carrying `es_bytes` of elementary stream payload. Returns
/// the raw byte buffer and the number of 188-byte TS packets.
fn build_ts_stream(pes_count: usize, es_bytes: usize) -> (Vec<u8>, usize) {
    let video_pid = 0x100u16;
    let pmt_pid = 0x1000u16;
    let mut buf = Vec::new();
    let mut pkt_count = 0usize;

    buf.extend_from_slice(&make_ts_packet(0, true, 0, &minimal_pat(pmt_pid)));
    pkt_count += 1;
    buf.extend_from_slice(&make_ts_packet(pmt_pid, true, 0, &minimal_pmt(video_pid)));
    pkt_count += 1;

    for i in 0..pes_count {
        let pts = (i as u64) * 3000;
        let pes = video_pes(pts, es_bytes);
        // Split PES across TS packets (184 bytes payload per packet).
        let mut offset = 0;
        let mut cc = (i & 0x0F) as u8;
        let mut first = true;
        while offset < pes.len() {
            let chunk_end = (offset + 184).min(pes.len());
            buf.extend_from_slice(&make_ts_packet(video_pid, first, cc, &pes[offset..chunk_end]));
            pkt_count += 1;
            offset = chunk_end;
            cc = cc.wrapping_add(1) & 0x0F;
            first = false;
        }
    }

    (buf, pkt_count)
}

/// Primed demuxer with PAT/PMT already parsed.
fn primed_demuxer() -> TsDemuxer {
    let video_pid = 0x100u16;
    let pmt_pid = 0x1000u16;
    let mut demux = TsDemuxer::new();
    demux.feed(&make_ts_packet(0, true, 0, &minimal_pat(pmt_pid)));
    demux.feed(&make_ts_packet(pmt_pid, true, 0, &minimal_pmt(video_pid)));
    demux
}

/// Measures throughput of feeding a complete TS stream in one call.
/// This is the typical SRT path: one UDP datagram contains multiple
/// TS packets that are fed as a single slice.
fn bench_feed_bulk(c: &mut Criterion) {
    let mut group = c.benchmark_group("feed_bulk");
    for &(pes_count, es_bytes) in &[(10, 100), (100, 100), (100, 1000), (100, 4000)] {
        let (stream, pkt_count) = build_ts_stream(pes_count, es_bytes);
        group.throughput(Throughput::Bytes(stream.len() as u64));
        group.bench_with_input(
            BenchmarkId::new(format!("{pes_count}pes_{es_bytes}es"), pkt_count),
            &stream,
            |b, stream| {
                b.iter_batched(
                    TsDemuxer::new,
                    |mut demux| {
                        let _ = demux.feed(stream);
                        demux
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

/// Measures per-packet feed cost on a primed demuxer. Simulates
/// the incremental path where packets arrive one at a time.
fn bench_feed_per_packet(c: &mut Criterion) {
    let mut group = c.benchmark_group("feed_per_packet");
    group.throughput(Throughput::Bytes(TS_PACKET_SIZE as u64));
    let video_pid = 0x100u16;

    // PES that fits in one TS packet.
    let pes = video_pes(0, 100);
    let pkt = make_ts_packet(video_pid, true, 0, &pes);

    group.bench_function("single_pes_packet", |b| {
        b.iter_batched(
            primed_demuxer,
            |mut demux| {
                let _ = demux.feed(&pkt);
                demux
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

/// Measures PES reassembly cost when a single PES spans multiple
/// TS packets (large video frames).
fn bench_reassembly(c: &mut Criterion) {
    let mut group = c.benchmark_group("pes_reassembly");
    let video_pid = 0x100u16;

    for &es_bytes in &[1000, 4000, 16000] {
        let pes = video_pes(0, es_bytes);
        let ts_packets_needed = pes.len().div_ceil(184);
        group.throughput(Throughput::Bytes(pes.len() as u64));

        let mut packets = Vec::new();
        let mut offset = 0;
        let mut cc = 0u8;
        let mut first = true;
        while offset < pes.len() {
            let end = (offset + 184).min(pes.len());
            packets.push(make_ts_packet(video_pid, first, cc, &pes[offset..end]));
            offset = end;
            cc = cc.wrapping_add(1) & 0x0F;
            first = false;
        }
        // Add a second PUSI to flush the first PES.
        let pes2 = video_pes(3000, 10);
        packets.push(make_ts_packet(video_pid, true, cc, &pes2));

        group.bench_with_input(BenchmarkId::new("es_bytes", es_bytes), &packets, |b, packets| {
            b.iter_batched(
                primed_demuxer,
                |mut demux| {
                    for pkt in packets {
                        let _ = demux.feed(pkt);
                    }
                    demux
                },
                BatchSize::SmallInput,
            );
        });

        // Verify the bench setup actually produces a PES.
        let mut verify = primed_demuxer();
        let mut total = Vec::new();
        for pkt in &packets {
            total.extend_from_slice(pkt);
        }
        let result = verify.feed(&total);
        assert!(
            !result.is_empty(),
            "reassembly bench must produce PES ({ts_packets_needed} TS packets, {es_bytes} ES bytes)"
        );
    }
    group.finish();
}

criterion_group!(benches, bench_feed_bulk, bench_feed_per_packet, bench_reassembly);
criterion_main!(benches);

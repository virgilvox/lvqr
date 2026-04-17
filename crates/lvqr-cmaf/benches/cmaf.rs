//! Criterion benches for the `lvqr-cmaf` writer + extractor hot
//! paths.
//!
//! Two distinct paths land on the data plane:
//!
//! * `build_moof_mdat` runs per emitted fragment (RTMP / SRT / RTSP
//!   ingest all call it through `lvqr-ingest`). Worth knowing the
//!   per-sample cost so the ingest path CPU budget stays tractable
//!   at high fragment rates.
//! * `write_*_init_segment` + `extract_*_config` /
//!   `extract_*_parameter_sets` run once per PLAY session (DESCRIBE
//!   on the RTSP server, `/playlist.m3u8` prep on LL-HLS). These are
//!   latency-shaped, not throughput-shaped -- a long setup delays
//!   every new subscriber's first frame.

use std::hint::black_box;

use bytes::{Bytes, BytesMut};
use criterion::{Criterion, criterion_group, criterion_main};
use lvqr_cmaf::{
    AudioInitParams, HevcInitParams, OpusInitParams, RawSample, VideoInitParams, build_moof_mdat, extract_aac_config,
    extract_avc_parameter_sets, extract_hevc_parameter_sets, extract_opus_config, write_aac_init_segment,
    write_avc_init_segment, write_hevc_init_segment, write_opus_init_segment,
};
use lvqr_codec::hevc::HevcSps;

// Parameter sets reused across every bench in this file. Bytes match
// the corpus the unit tests already pin so the numbers here apply
// directly to production-shaped inputs.

const AVC_SPS: &[u8] = &[0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00];
const AVC_PPS: &[u8] = &[0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];

const HEVC_VPS: &[u8] = &[
    0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03,
    0x00, 0x3c, 0x95, 0x94, 0x09,
];
const HEVC_SPS_NAL: &[u8] = &[
    0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c, 0xa0,
    0x0a, 0x08, 0x0f, 0x16, 0x59, 0x59, 0x52, 0x93, 0x0b, 0xc0, 0x5a, 0x02, 0x00, 0x00, 0x03, 0x00, 0x02, 0x00, 0x00,
    0x03, 0x00, 0x3c, 0x10,
];
const HEVC_PPS_NAL: &[u8] = &[0x44, 0x01, 0xc0, 0x73, 0xc1, 0x89];

fn hevc_sps_info() -> HevcSps {
    HevcSps {
        general_profile_space: 0,
        general_tier_flag: false,
        general_profile_idc: 1,
        general_profile_compatibility_flags: 0x60000000,
        general_level_idc: 60,
        chroma_format_idc: 1,
        pic_width_in_luma_samples: 320,
        pic_height_in_luma_samples: 240,
    }
}

fn avc_init_bytes() -> Bytes {
    let mut buf = BytesMut::new();
    write_avc_init_segment(
        &mut buf,
        &VideoInitParams {
            sps: AVC_SPS.to_vec(),
            pps: AVC_PPS.to_vec(),
            width: 1280,
            height: 720,
            timescale: 90_000,
        },
    )
    .expect("write avc init");
    buf.freeze()
}

fn hevc_init_bytes() -> Bytes {
    let mut buf = BytesMut::new();
    write_hevc_init_segment(
        &mut buf,
        &HevcInitParams {
            vps: HEVC_VPS.to_vec(),
            sps: HEVC_SPS_NAL.to_vec(),
            pps: HEVC_PPS_NAL.to_vec(),
            sps_info: hevc_sps_info(),
            timescale: 90_000,
        },
    )
    .expect("write hevc init");
    buf.freeze()
}

fn aac_init_bytes() -> Bytes {
    let mut buf = BytesMut::new();
    write_aac_init_segment(
        &mut buf,
        &AudioInitParams {
            asc: vec![0x12, 0x10],
            timescale: 44_100,
        },
    )
    .expect("write aac init");
    buf.freeze()
}

fn opus_init_bytes() -> Bytes {
    let mut buf = BytesMut::new();
    write_opus_init_segment(
        &mut buf,
        &OpusInitParams {
            channel_count: 2,
            pre_skip: 312,
            input_sample_rate: 48_000,
            timescale: 48_000,
        },
    )
    .expect("write opus init");
    buf.freeze()
}

fn make_sample(dts: u64, payload_size: usize, keyframe: bool) -> RawSample {
    // 4-byte AVCC length prefix + synthetic payload. Matches the
    // shape the RTMP ingest path feeds into build_moof_mdat.
    let mut payload = Vec::with_capacity(payload_size + 4);
    payload.extend_from_slice(&(payload_size as u32).to_be_bytes());
    payload.extend(std::iter::repeat_n(0xABu8, payload_size));
    RawSample {
        track_id: 1,
        dts,
        cts_offset: 0,
        duration: 3000,
        payload: Bytes::from(payload),
        keyframe,
    }
}

fn bench_build_moof_mdat(c: &mut Criterion) {
    // 1 sample = typical H.264 IDR fragment, ~10 KB payload.
    let samples_1 = vec![make_sample(0, 10_000, true)];
    c.bench_function("build_moof_mdat_1_sample_10KB", |b| {
        b.iter(|| {
            let out = build_moof_mdat(1, 1, 0, black_box(&samples_1));
            black_box(out);
        });
    });

    // 10 samples = multi-AU audio batch or a GOP-worth of AAC frames.
    // Each sample 256 B (mid-bitrate AAC-LC AU).
    let samples_10: Vec<RawSample> = (0..10u64).map(|i| make_sample(i * 1024, 256, i == 0)).collect();
    c.bench_function("build_moof_mdat_10_samples_256B", |b| {
        b.iter(|| {
            let out = build_moof_mdat(1, 1, 0, black_box(&samples_10));
            black_box(out);
        });
    });
}

fn bench_write_init(c: &mut Criterion) {
    c.bench_function("write_avc_init_segment", |b| {
        let params = VideoInitParams {
            sps: AVC_SPS.to_vec(),
            pps: AVC_PPS.to_vec(),
            width: 1280,
            height: 720,
            timescale: 90_000,
        };
        b.iter(|| {
            let mut buf = BytesMut::new();
            write_avc_init_segment(&mut buf, black_box(&params)).expect("encode");
            black_box(buf);
        });
    });

    c.bench_function("write_hevc_init_segment", |b| {
        let params = HevcInitParams {
            vps: HEVC_VPS.to_vec(),
            sps: HEVC_SPS_NAL.to_vec(),
            pps: HEVC_PPS_NAL.to_vec(),
            sps_info: hevc_sps_info(),
            timescale: 90_000,
        };
        b.iter(|| {
            let mut buf = BytesMut::new();
            write_hevc_init_segment(&mut buf, black_box(&params)).expect("encode");
            black_box(buf);
        });
    });
}

fn bench_extract(c: &mut Criterion) {
    let avc = avc_init_bytes();
    c.bench_function("extract_avc_parameter_sets", |b| {
        b.iter(|| {
            let out = extract_avc_parameter_sets(black_box(&avc));
            black_box(out);
        });
    });

    let hevc = hevc_init_bytes();
    c.bench_function("extract_hevc_parameter_sets", |b| {
        b.iter(|| {
            let out = extract_hevc_parameter_sets(black_box(&hevc));
            black_box(out);
        });
    });

    let aac = aac_init_bytes();
    c.bench_function("extract_aac_config", |b| {
        b.iter(|| {
            let out = extract_aac_config(black_box(&aac));
            black_box(out);
        });
    });

    let opus = opus_init_bytes();
    c.bench_function("extract_opus_config", |b| {
        b.iter(|| {
            let out = extract_opus_config(black_box(&opus));
            black_box(out);
        });
    });
}

criterion_group!(benches, bench_build_moof_mdat, bench_write_init, bench_extract);
criterion_main!(benches);

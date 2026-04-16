//! Conformance check for fMP4 segments emitted by the RTSP fragment
//! emission path.
//!
//! Builds AVC and AAC init segments + moof/mdat media segments using
//! the same functions the RTSP server calls (`write_avc_init_segment`,
//! `build_moof_mdat`) and pipes them into ffprobe for structural
//! validation. Passes if ffprobe accepts the output; soft-skips if
//! ffprobe is not installed.
//!
//! This is the "conformance" slot of the 5-artifact contract for
//! `lvqr-rtsp`.

use bytes::{Bytes, BytesMut};
use lvqr_cmaf::{
    AudioInitParams, RawSample, VideoInitParams, build_moof_mdat, write_aac_init_segment, write_avc_init_segment,
};
use lvqr_test_utils::ffprobe_bytes;

/// Real SPS/PPS from an x264 Baseline 3.1 1280x720 encode.
const SPS: &[u8] = &[
    0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03,
    0x03, 0xC0, 0xF1, 0x83, 0x2A,
];
const PPS: &[u8] = &[0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];

#[test]
fn ffprobe_accepts_rtsp_avc_init_plus_media() {
    let params = VideoInitParams {
        sps: SPS.to_vec(),
        pps: PPS.to_vec(),
        width: 1280,
        height: 720,
        timescale: 90_000,
    };
    let mut init_buf = BytesMut::with_capacity(512);
    write_avc_init_segment(&mut init_buf, &params).expect("encode init");

    // Build one IDR media segment.
    let idr_nalu = vec![0x65, 0x88, 0x84, 0x00, 0xDE, 0xAD, 0xBE, 0xEF];
    let avcc_payload = {
        let len = idr_nalu.len() as u32;
        let mut p = len.to_be_bytes().to_vec();
        p.extend_from_slice(&idr_nalu);
        p
    };
    let sample = RawSample {
        track_id: 1,
        dts: 0,
        cts_offset: 0,
        duration: 3000,
        payload: Bytes::from(avcc_payload),
        keyframe: true,
    };
    let moof_mdat = build_moof_mdat(1, 1, 0, &[sample]);

    // Concatenate init + media and validate.
    let mut combined = init_buf.freeze().to_vec();
    combined.extend_from_slice(&moof_mdat);
    ffprobe_bytes(&combined).assert_accepted();
}

#[test]
fn ffprobe_accepts_rtsp_aac_init_plus_media() {
    // AAC-LC 44100 Hz stereo: object_type=2, freq_idx=4, channels=2
    let asc = vec![0x12, 0x10];
    let params = AudioInitParams {
        asc: asc.clone(),
        timescale: 44100,
    };
    let mut init_buf = BytesMut::with_capacity(512);
    write_aac_init_segment(&mut init_buf, &params).expect("encode init");

    // Build one AAC media segment (synthetic silence frame).
    // track_id must be 1 to match the init segment (single-track mp4).
    let aac_frame = vec![0xFF, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    let sample = RawSample {
        track_id: 1,
        dts: 0,
        cts_offset: 0,
        duration: 1024,
        payload: Bytes::from(aac_frame),
        keyframe: true,
    };
    let moof_mdat = build_moof_mdat(1, 1, 0, &[sample]);

    let mut combined = init_buf.freeze().to_vec();
    combined.extend_from_slice(&moof_mdat);
    ffprobe_bytes(&combined).assert_accepted();
}

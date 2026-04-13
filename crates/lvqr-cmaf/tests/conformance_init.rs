//! Conformance check for the `mp4-atom`-backed init-segment writer.
//!
//! Feeds the output of [`lvqr_cmaf::write_avc_init_segment`] to
//! ffprobe via `lvqr_test_utils::ffprobe_bytes`. The test passes if
//! ffprobe exits zero (accepted) or if ffprobe is unavailable on
//! PATH (soft-skip). A "parsed-but-rejected" outcome fails loudly.
//!
//! This is the "conformance" slot of the 5-artifact contract for
//! `lvqr-cmaf`. The AVC init segment is the first real handshake
//! between the `mp4-atom` box writer and an external validator; a
//! regression here means the library integration is broken before
//! any downstream egress crate starts consuming chunks.
//!
//! Note: ffprobe wants to see at least one media segment before it
//! will decode anything, but an init segment alone produces
//! structural warnings on stderr without failing. The helper already
//! treats stderr warnings as diagnostics after the session-4 fix and
//! trusts the exit code as authoritative.

use bytes::BytesMut;
use lvqr_cmaf::{
    AudioInitParams, HevcInitParams, VideoInitParams, write_aac_init_segment, write_avc_init_segment,
    write_hevc_init_segment,
};
use lvqr_codec::hevc::HevcSps;
use lvqr_conformance::codec as codec_fixtures;
use lvqr_test_utils::ffprobe_bytes;

#[test]
fn ffprobe_accepts_avc_init_segment() {
    let params = VideoInitParams {
        sps: vec![
            0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00,
            0x03, 0x03, 0xC0, 0xF1, 0x83, 0x2A,
        ],
        pps: vec![0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0],
        width: 1280,
        height: 720,
        timescale: 90_000,
    };
    let mut buf = BytesMut::new();
    write_avc_init_segment(&mut buf, &params).expect("encode");
    ffprobe_bytes(&buf).assert_accepted();
}

/// Real x265 HEVC Main 3.0 VPS / SPS / PPS captured from a session-6
/// ffmpeg 8.1 run: `ffmpeg -f lavfi -i testsrc2=320x240:rate=30 -t 1
/// -c:v libx265 -preset ultrafast -movflags +frag_keyframe+empty_moov
/// -f mp4 /tmp/hevc.mp4`. The hvcC arrays in the resulting fMP4 were
/// parsed and their NAL unit bytes pinned here so the conformance
/// test does not depend on a live ffmpeg encode at test time. If x265
/// drifts (unlikely for the Main 3.0 path on testsrc2) the byte
/// blobs can be refreshed with the same command plus a 30-line
/// Python hvcC walker.
const HEVC_VPS_X265: &[u8] = &[
    0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03,
    0x00, 0x3c, 0x95, 0x94, 0x09,
];
const HEVC_SPS_X265_NAL: &[u8] = &[
    0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c, 0xa0,
    0x0a, 0x08, 0x0f, 0x16, 0x59, 0x59, 0x52, 0x93, 0x0b, 0xc0, 0x5a, 0x02, 0x00, 0x00, 0x03, 0x00, 0x02, 0x00, 0x00,
    0x03, 0x00, 0x3c, 0x10,
];
const HEVC_PPS_X265: &[u8] = &[0x44, 0x01, 0xc0, 0x73, 0xc1, 0x89];

#[test]
fn ffprobe_accepts_hevc_init_segment() {
    // The HevcSps values come from the lvqr-conformance codec fixture
    // corpus so a parser regression on the existing x265 SPS corpus
    // also fails this test. The SPS NAL byte blob above is a
    // different capture (newer ffmpeg build) from the sidecar's
    // fixture, but the decoded general-profile / level / chroma
    // fields are identical because the target resolution and
    // profile are the same.
    let fixture = codec_fixtures::load("hevc-sps-x265-main-320x240").expect("load hevc sps fixture");
    let expected = fixture.meta.expected.hevc_sps.expect("hevc_sps sidecar");
    let sps_info = HevcSps {
        general_profile_space: expected.general_profile_space,
        general_tier_flag: expected.general_tier_flag,
        general_profile_idc: expected.general_profile_idc,
        general_profile_compatibility_flags: expected.general_profile_compatibility_flags,
        general_level_idc: expected.general_level_idc,
        chroma_format_idc: expected.chroma_format_idc,
        pic_width_in_luma_samples: expected.pic_width_in_luma_samples,
        pic_height_in_luma_samples: expected.pic_height_in_luma_samples,
    };
    let params = HevcInitParams {
        vps: HEVC_VPS_X265.to_vec(),
        sps: HEVC_SPS_X265_NAL.to_vec(),
        pps: HEVC_PPS_X265.to_vec(),
        sps_info,
        timescale: 90_000,
    };
    let mut buf = BytesMut::new();
    write_hevc_init_segment(&mut buf, &params).expect("encode");
    ffprobe_bytes(&buf).assert_accepted();
}

#[test]
fn ffprobe_accepts_aac_init_segment() {
    // Feed the same AAC-LC 44.1 kHz stereo ASC the lvqr-conformance
    // codec corpus pins. The writer parses the ASC through
    // lvqr_codec::aac::parse_asc and builds the `esds` descriptor
    // from the result, so a parser regression on this fixture fails
    // this test as well. The 48 kHz variant gets exercised in unit
    // tests; the 44.1 kHz variant is the common RTMP ingest case
    // that the lvqr-ingest hand-rolled esds path already validates
    // end-to-end.
    let fixture = codec_fixtures::load("aac-asc-aaclc-44100hz-stereo").expect("load aac asc fixture");
    let params = AudioInitParams {
        asc: fixture.bytes.to_vec(),
        timescale: 44_100,
    };
    let mut buf = BytesMut::new();
    write_aac_init_segment(&mut buf, &params).expect("encode");
    ffprobe_bytes(&buf).assert_accepted();
}

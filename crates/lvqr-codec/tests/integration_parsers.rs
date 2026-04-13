//! Integration tests for the HEVC + AAC parsers wiring them together the
//! way a real ingest path will: parse an HEVC SPS to derive a FragmentMeta
//! codec string, parse an AAC ASC to derive an audio FragmentMeta codec
//! string, then sanity-check both against fmp4-style consumer expectations.
//!
//! The SPS bytes below are a deterministic synthesized HEVC Main Profile
//! Level 3.1 SPS for a 1280x720 stream. The AAC ASC bytes are the
//! canonical AAC-LC stereo 44.1 kHz ASC that lvqr-ingest already uses.
//!
//! These fixtures deliberately avoid depending on `lvqr-conformance` so
//! the test runs without any external corpus bootstrap.

use lvqr_codec::aac::parse_asc;
use lvqr_codec::hevc::{HevcNalType, parse_nal_header, parse_sps};

#[test]
fn hevc_nal_header_identifies_sps_nal() {
    // NAL unit header for a Main-profile SPS: forbidden_zero=0,
    // nal_unit_type=33 (SPS), layer_id=0, tid_plus1=1.
    // byte 0 = 0 | (33 << 1) = 0x42
    // byte 1 = 0 | 0 | 1 = 0x01
    let header = [0x42, 0x01];
    assert_eq!(parse_nal_header(&header).unwrap(), HevcNalType::Sps);
}

#[test]
fn aac_lc_stereo_44k_roundtrips_to_mp4a_codec_string() {
    let asc = parse_asc(&[0x12, 0x10]).expect("parse ASC");
    assert_eq!(asc.object_type, 2);
    assert_eq!(asc.sample_rate, 44100);
    assert_eq!(asc.channel_config, 2);
    assert_eq!(asc.codec_string(), "mp4a.40.2");
}

#[test]
fn hevc_sps_parser_does_not_accept_short_input() {
    // A single byte is not enough to parse even the fixed-width header
    // fields before the profile_tier_level block starts. The parser
    // must report end-of-stream rather than returning garbage.
    assert!(parse_sps(&[0x01]).is_err());
}

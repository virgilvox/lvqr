//! Codec conformance slot for `lvqr-codec`.
//!
//! Iterates the `lvqr-conformance` codec fixture corpus (captured
//! from real encoders and pinned into `fixtures/codec/` with a TOML
//! sidecar per fixture) and asserts that `lvqr_codec::hevc::parse_sps`
//! and `lvqr_codec::aac::parse_asc` decode each blob to the expected
//! values. This is the conformance slot of the 5-artifact contract
//! for `lvqr-codec`: any drift between the parser and a real encoder
//! output shows up as a test failure, not a silent silent drift.
//!
//! Adding a new fixture is a zero-code-change operation: drop the
//! byte blob and its `.toml` sidecar under
//! `crates/lvqr-conformance/fixtures/codec/` and this test picks it
//! up on the next run. The goal is for the corpus to grow
//! monotonically as more encoders are captured.

use lvqr_codec::aac::parse_asc;
use lvqr_codec::hevc::parse_sps;
use lvqr_conformance::codec::{CodecFixture, list};

#[test]
fn codec_fixtures_decode_to_expected_values() {
    let fixtures = list().expect("load codec fixture corpus");
    assert!(
        !fixtures.is_empty(),
        "codec fixture corpus is empty; expected at least the session-5 bootstrap fixtures"
    );

    let mut hevc_count = 0;
    let mut aac_count = 0;
    for fixture in fixtures {
        if fixture.meta.expected.hevc_sps.is_some() {
            assert_hevc(&fixture);
            hevc_count += 1;
        } else if fixture.meta.expected.aac_asc.is_some() {
            assert_aac(&fixture);
            aac_count += 1;
        } else {
            panic!(
                "fixture {} has no expected parser output (loader would have already caught this)",
                fixture.name
            );
        }
    }
    // Guard the corpus against accidental deletion: the session-5
    // bootstrap added at least one HEVC and two AAC fixtures. If a
    // future session drops below these floors on purpose, bump the
    // floor alongside the deletion.
    assert!(hevc_count >= 1, "expected at least one HEVC SPS fixture");
    assert!(aac_count >= 2, "expected at least two AAC ASC fixtures");
}

fn assert_hevc(fixture: &CodecFixture) {
    let expected = fixture
        .meta
        .expected
        .hevc_sps
        .as_ref()
        .expect("hevc_sps expectation present");
    let sps = parse_sps(&fixture.bytes).unwrap_or_else(|e| {
        panic!(
            "parse_sps failed on fixture {}: {e}\nbytes: {}",
            fixture.name,
            hex(&fixture.bytes)
        )
    });
    assert_eq!(
        sps.general_profile_space, expected.general_profile_space,
        "{} general_profile_space",
        fixture.name
    );
    assert_eq!(
        sps.general_tier_flag, expected.general_tier_flag,
        "{} tier_flag",
        fixture.name
    );
    assert_eq!(
        sps.general_profile_idc, expected.general_profile_idc,
        "{} profile_idc",
        fixture.name
    );
    assert_eq!(
        sps.general_profile_compatibility_flags, expected.general_profile_compatibility_flags,
        "{} profile_compat",
        fixture.name
    );
    assert_eq!(
        sps.general_level_idc, expected.general_level_idc,
        "{} level_idc",
        fixture.name
    );
    assert_eq!(
        sps.chroma_format_idc, expected.chroma_format_idc,
        "{} chroma_format_idc",
        fixture.name
    );
    assert_eq!(
        sps.pic_width_in_luma_samples, expected.pic_width_in_luma_samples,
        "{} width",
        fixture.name
    );
    assert_eq!(
        sps.pic_height_in_luma_samples, expected.pic_height_in_luma_samples,
        "{} height",
        fixture.name
    );
    // Codec string invariant: must start with `hev1.` and match the
    // sidecar's declared codec field exactly.
    assert_eq!(sps.codec_string(), fixture.meta.codec, "{} codec string", fixture.name);
}

fn assert_aac(fixture: &CodecFixture) {
    let expected = fixture
        .meta
        .expected
        .aac_asc
        .as_ref()
        .expect("aac_asc expectation present");
    let asc = parse_asc(&fixture.bytes).unwrap_or_else(|e| {
        panic!(
            "parse_asc failed on fixture {}: {e}\nbytes: {}",
            fixture.name,
            hex(&fixture.bytes)
        )
    });
    assert_eq!(asc.object_type, expected.object_type, "{} object_type", fixture.name);
    assert_eq!(asc.sample_rate, expected.sample_rate, "{} sample_rate", fixture.name);
    assert_eq!(
        asc.channel_config, expected.channel_config,
        "{} channel_config",
        fixture.name
    );
    assert_eq!(asc.sbr_present, expected.sbr_present, "{} sbr_present", fixture.name);
    assert_eq!(asc.ps_present, expected.ps_present, "{} ps_present", fixture.name);
    assert_eq!(asc.codec_string(), fixture.meta.codec, "{} codec string", fixture.name);
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

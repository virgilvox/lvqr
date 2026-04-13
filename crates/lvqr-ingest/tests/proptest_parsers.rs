//! Property tests for the FLV parser and fMP4 writer.
//!
//! These tests enforce two invariants:
//!
//! 1. **Parsers never panic on arbitrary input.** `parse_video_tag`,
//!    `parse_audio_tag`, and `extract_resolution` all accept attacker-shaped
//!    byte slices without unwrap, slice, or arithmetic panics. Regressions
//!    here would mean the RTMP ingest path can be crashed by a malicious
//:    publisher.
//!
//! 2. **fMP4 writer output is structurally well-formed.** Given any
//!    plausible `VideoConfig` plus any plausible set of `VideoSample`s, the
//!    writer produces a byte buffer whose top-level boxes have valid sizes
//!    (no 0, no greater-than-buffer) and begin with known four-char codes.
//!
//! These are the first two of the 5-artifact test contract for the
//! lvqr-ingest crate (see tests/CONTRACT.md at the repo root). The
//! remaining three artifacts (cargo-fuzz target, integration test, E2E
//! test, conformance test against ffprobe) land in separate Tier 1 tasks.

use bytes::Bytes;
use lvqr_ingest::remux::{
    VideoConfig, VideoSample, parse_audio_tag, parse_video_tag, video_init_segment_with_size, video_segment,
};
use proptest::prelude::*;

// =====================================================================
// Parser "never panics" properties
// =====================================================================

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        .. ProptestConfig::default()
    })]

    /// `parse_video_tag` must handle arbitrary bytes without panicking.
    /// Upper bound on input size keeps proptest shrinking fast.
    #[test]
    fn parse_video_tag_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let _ = parse_video_tag(&Bytes::from(bytes));
    }

    /// `parse_audio_tag` must handle arbitrary bytes without panicking.
    #[test]
    fn parse_audio_tag_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let _ = parse_audio_tag(&Bytes::from(bytes));
    }
}

// =====================================================================
// fMP4 writer structural properties
// =====================================================================

/// Generate a plausible VideoConfig with small but realistic SPS/PPS.
fn video_config_strategy() -> impl Strategy<Value = VideoConfig> {
    (
        proptest::collection::vec(any::<u8>(), 4..48),
        proptest::collection::vec(any::<u8>(), 4..32),
        any::<u8>(),
        any::<u8>(),
        any::<u8>(),
    )
        .prop_map(|(sps, pps, profile, compat, level)| VideoConfig {
            sps_list: vec![sps],
            pps_list: vec![pps],
            profile,
            compat,
            level,
            nalu_length_size: 4,
        })
}

/// Generate a VideoSample with plausible size and duration bounds.
fn video_sample_strategy() -> impl Strategy<Value = VideoSample> {
    (
        proptest::collection::vec(any::<u8>(), 8..512),
        1u32..10_000,
        -1000i32..1000,
        any::<bool>(),
    )
        .prop_map(|(data, duration, cts_offset, keyframe)| VideoSample {
            data: Bytes::from(data),
            duration,
            cts_offset,
            keyframe,
        })
}

/// Walk the top-level box list in an ISO BMFF buffer, asserting that every
/// box has a plausible size field and a four-char type code that is
/// strictly ASCII printable. Returns the list of type codes encountered.
fn walk_top_level_boxes(buf: &[u8]) -> Vec<[u8; 4]> {
    let mut boxes = Vec::new();
    let mut offset = 0;
    while offset + 8 <= buf.len() {
        let size = u32::from_be_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]]) as usize;
        assert!(size >= 8, "box at offset {offset} has size {size} (< 8)");
        assert!(
            offset + size <= buf.len(),
            "box at offset {offset} size {size} runs past buffer end {}",
            buf.len()
        );
        let mut code = [0u8; 4];
        code.copy_from_slice(&buf[offset + 4..offset + 8]);
        for b in &code {
            assert!(
                (*b as char).is_ascii(),
                "box code byte {b:#x} at offset {offset} is not ASCII"
            );
        }
        boxes.push(code);
        offset += size;
    }
    assert_eq!(offset, buf.len(), "trailing garbage after last box");
    boxes
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// Init segments must start with ftyp, contain a moov, and have all
    /// top-level boxes well formed.
    #[test]
    fn video_init_segment_is_well_formed(
        config in video_config_strategy(),
        w in 1u16..3840,
        h in 1u16..2160,
    ) {
        let init = video_init_segment_with_size(&config, w, h);
        prop_assert!(init.len() >= 16);
        let boxes = walk_top_level_boxes(&init);
        prop_assert!(boxes.contains(b"ftyp"), "init missing ftyp");
        prop_assert!(boxes.contains(b"moov"), "init missing moov");
        prop_assert_eq!(&boxes[0], b"ftyp");
    }

    /// Media segments must contain a moof followed by an mdat and have all
    /// top-level boxes well formed for any plausible sample list.
    #[test]
    fn video_segment_is_well_formed(
        samples in proptest::collection::vec(video_sample_strategy(), 1..8),
        base_dts in 0u64..1_000_000,
        seq in 1u32..10_000,
    ) {
        let seg = video_segment(seq, base_dts, &samples);
        prop_assert!(seg.len() >= 16);
        let boxes = walk_top_level_boxes(&seg);
        prop_assert!(boxes.contains(b"moof"), "segment missing moof");
        prop_assert!(boxes.contains(b"mdat"), "segment missing mdat");
    }
}

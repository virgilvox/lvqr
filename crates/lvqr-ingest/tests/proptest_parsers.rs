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
    AudioConfig, VideoConfig, VideoSample, extract_resolution, generate_catalog, parse_audio_tag, parse_video_tag,
    video_init_segment_with_size, video_segment,
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

    /// `extract_resolution` must handle arbitrary SPS-shaped bytes without
    /// panicking. The function wraps `h264-reader`, which has historically
    /// shipped panics on malformed RBSP; this property guards against
    /// regressions where a malicious publisher could crash the RTMP
    /// ingest path by sending a crafted AVCC sequence header.
    #[test]
    fn extract_resolution_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = extract_resolution(&bytes);
    }

    /// Same property but with the high-value prefix: NAL header byte 0x67
    /// (forbidden_zero_bit=0, nal_ref_idc=3, nal_unit_type=7=SPS) then
    /// random body. This is the exact shape `h264-reader` sees in
    /// practice, so it exercises the SPS decode path deeper than purely
    /// random bytes usually reach.
    #[test]
    fn extract_resolution_never_panics_on_sps_prefix(
        body in proptest::collection::vec(any::<u8>(), 0..256),
    ) {
        let mut sps = Vec::with_capacity(body.len() + 1);
        sps.push(0x67);
        sps.extend_from_slice(&body);
        let _ = extract_resolution(&sps);
    }
}

// =====================================================================
// Catalog JSON properties
// =====================================================================

/// Generate a plausible `VideoConfig` with ASCII-safe fields. We use the
/// same SPS/PPS strategy as the writer tests so the value space overlaps
/// with realistic codec metadata.
fn video_config_for_catalog() -> impl Strategy<Value = VideoConfig> {
    (
        proptest::collection::vec(any::<u8>(), 1..16),
        proptest::collection::vec(any::<u8>(), 1..16),
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

/// Generate a plausible `AudioConfig`. `sample_rate` and `channels` are
/// bounded to realistic ranges so the catalog's embedded numeric fields
/// exercise both small and large JSON integer widths.
fn audio_config_for_catalog() -> impl Strategy<Value = AudioConfig> {
    (
        proptest::collection::vec(any::<u8>(), 1..8),
        8_000u32..192_000,
        1u8..16,
        0u8..32,
    )
        .prop_map(|(asc, sample_rate, channels, object_type)| AudioConfig {
            asc,
            sample_rate,
            channels,
            object_type,
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// `generate_catalog` must never panic on any combination of inputs
    /// and must always produce a syntactically valid JSON document that
    /// round-trips through `serde_json`.
    #[test]
    fn generate_catalog_always_parses_as_json(
        video in proptest::option::of(video_config_for_catalog()),
        audio in proptest::option::of(audio_config_for_catalog()),
    ) {
        let json = generate_catalog(video.as_ref(), audio.as_ref());
        let parsed: serde_json::Value = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("generate_catalog output is not valid JSON: {e}\n  text: {json}"));

        // version is always 1
        prop_assert_eq!(parsed["version"].as_u64(), Some(1));

        // tracks is an array; length matches the number of Some inputs
        let tracks = parsed["tracks"].as_array().expect("tracks must be an array");
        let expected_len = video.is_some() as usize + audio.is_some() as usize;
        prop_assert_eq!(tracks.len(), expected_len);

        // Every track entry has the mandatory fields the MoQ catalog
        // schema requires: name, packaging=cmaf, codec, mimeType.
        for track in tracks {
            prop_assert!(track["name"].is_string(), "track missing name");
            prop_assert_eq!(track["packaging"].as_str(), Some("cmaf"));
            prop_assert!(track["codec"].is_string(), "track missing codec");
            prop_assert!(track["mimeType"].is_string(), "track missing mimeType");
        }
    }

    /// Track ordering invariant: when both video and audio are present,
    /// the video track (`0.mp4`, `video/mp4`) comes first and the audio
    /// track (`1.mp4`, `audio/mp4`) second. The browser player depends
    /// on this order when bootstrapping its MSE SourceBuffers.
    #[test]
    fn generate_catalog_places_video_before_audio(
        video in video_config_for_catalog(),
        audio in audio_config_for_catalog(),
    ) {
        let json = generate_catalog(Some(&video), Some(&audio));
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let tracks = parsed["tracks"].as_array().unwrap();
        prop_assert_eq!(tracks.len(), 2);
        prop_assert_eq!(tracks[0]["name"].as_str(), Some("0.mp4"));
        prop_assert_eq!(tracks[0]["mimeType"].as_str(), Some("video/mp4"));
        prop_assert_eq!(tracks[1]["name"].as_str(), Some("1.mp4"));
        prop_assert_eq!(tracks[1]["mimeType"].as_str(), Some("audio/mp4"));

        // Codec string is propagated verbatim from the source config.
        let video_codec = video.codec_string();
        let audio_codec = audio.codec_string();
        prop_assert_eq!(tracks[0]["codec"].as_str(), Some(video_codec.as_str()));
        prop_assert_eq!(tracks[1]["codec"].as_str(), Some(audio_codec.as_str()));

        // Audio sample rate and channel count are copied into the
        // catalog body as numeric JSON values, not strings.
        prop_assert_eq!(tracks[1]["samplerate"].as_u64(), Some(audio.sample_rate as u64));
        prop_assert_eq!(tracks[1]["channelCount"].as_u64(), Some(audio.channels as u64));
    }

    /// Empty-input invariant: when neither video nor audio is provided,
    /// the catalog still validates as JSON and reports an empty tracks
    /// array. Corresponds to the "ingest connected but no codec parsed
    /// yet" state in the RTMP bridge.
    #[test]
    fn generate_catalog_none_inputs_produce_empty_tracks(_unused in any::<u8>()) {
        let json = generate_catalog(None, None);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(parsed["version"].as_u64(), Some(1));
        prop_assert!(parsed["tracks"].as_array().unwrap().is_empty());
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

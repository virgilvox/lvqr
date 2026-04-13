//! Golden-file regression test for the fMP4 writer.
//!
//! Captures a canonical video init segment produced by the current
//! `video_init_segment_with_size` implementation and compares every future
//! build against it byte for byte. A byte-level diff against a known-good
//! reference catches silent regressions in the writer (field layout
//! changes, box ordering, padding drift) that integration tests and
//! proptests would miss.
//!
//! To regenerate the golden file after an intentional format change, run:
//!
//! ```text
//! BLESS=1 cargo test -p lvqr-ingest --test golden_fmp4
//! ```
//!
//! This is part of the Tier 1 "5-artifact test contract" (see
//! tests/CONTRACT.md at the repo root): proptest, fuzz target, integration
//! test, E2E test, and conformance check. Golden files count as the
//! first slot; the `ffprobe_accepts_concatenated_cmaf` test below adds a
//! real external-validator conformance check on top of the byte-exact
//! golden, upgrading this file to cover two of the five artifacts for
//! the fMP4 writer.

use bytes::Bytes;
use lvqr_ingest::remux::{
    AudioConfig, VideoConfig, VideoSample, audio_init_segment, audio_segment, video_init_segment_with_size,
    video_segment,
};
use lvqr_test_utils::ffprobe_bytes;
use std::path::{Path, PathBuf};

/// Build a deterministic VideoConfig for golden tests. The SPS and PPS
/// bytes below are from a synthesized H.264 Baseline 3.1 stream; they are
/// fixed so the golden output is bit-stable across runs.
fn golden_video_config() -> VideoConfig {
    VideoConfig {
        sps_list: vec![vec![
            0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00,
            0x03, 0x03, 0xC0, 0xF1, 0x83, 0x2A,
        ]],
        pps_list: vec![vec![0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0]],
        profile: 0x42,
        compat: 0x00,
        level: 0x1F,
        nalu_length_size: 4,
    }
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden")
}

fn golden_path(name: &str) -> PathBuf {
    fixtures_dir().join(name)
}

/// Either assert that `actual` matches the golden file on disk, or if
/// `BLESS=1` is set in the environment, rewrite the golden file with the
/// new bytes. Creates parent directories as needed when blessing.
fn assert_golden(name: &str, actual: &[u8]) {
    let path = golden_path(name);
    if std::env::var("BLESS").ok().as_deref() == Some("1") {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create golden parent dir");
        }
        std::fs::write(&path, actual).expect("write golden file");
        eprintln!("blessed golden file {}", path.display());
        return;
    }
    let expected = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => panic!(
            "golden file {} missing ({e}); regenerate with BLESS=1 cargo test",
            path.display()
        ),
    };
    if expected != actual {
        let expected_len = expected.len();
        let actual_len = actual.len();
        let first_diff = expected
            .iter()
            .zip(actual.iter())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| expected_len.min(actual_len));
        panic!(
            "golden mismatch for {}\n  expected_len = {expected_len}\n  actual_len   = {actual_len}\n  first_diff   = {first_diff}\n  regenerate with BLESS=1 cargo test -p lvqr-ingest --test golden_fmp4",
            path.display()
        );
    }
}

#[test]
fn video_init_segment_matches_golden() {
    let config = golden_video_config();
    let init = video_init_segment_with_size(&config, 1280, 720);
    assert_golden("video_init_h264_baseline_720p.mp4", &init);
}

#[test]
fn video_keyframe_segment_matches_golden() {
    // Deterministic one-sample keyframe segment at a fixed DTS.
    let sample = VideoSample {
        data: Bytes::from(vec![
            0x00, 0x00, 0x00, 0x10, 0x65, 0x88, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ]),
        duration: 3000,
        cts_offset: 0,
        keyframe: true,
    };
    let seg = video_segment(1, 0, std::slice::from_ref(&sample));
    assert_golden("video_segment_keyframe.mp4", &seg);
}

/// Concatenate the golden init segment and a deterministic keyframe
/// media segment into a single fMP4 byte buffer and feed it to ffprobe.
/// ffprobe must either accept the buffer (confirming the writer produces
/// spec-compliant ISO BMFF) or be unavailable (soft-skip). Any "parsed
/// but rejected" outcome fails the test.
///
/// This is the conformance slot of the 5-artifact contract for the fMP4
/// writer. If ffprobe rejects our output in CI, we broke the writer.
#[test]
fn ffprobe_accepts_concatenated_cmaf() {
    let config = golden_video_config();
    let init = video_init_segment_with_size(&config, 1280, 720);
    let sample = VideoSample {
        data: Bytes::from(vec![
            0x00, 0x00, 0x00, 0x10, 0x65, 0x88, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ]),
        duration: 3000,
        cts_offset: 0,
        keyframe: true,
    };
    let seg = video_segment(1, 0, std::slice::from_ref(&sample));

    let mut buf = Vec::with_capacity(init.len() + seg.len());
    buf.extend_from_slice(&init);
    buf.extend_from_slice(&seg);

    ffprobe_bytes(&buf).assert_accepted();
}

/// Conformance slot for the audio branch of the fMP4 writer. Builds an
/// AAC-LC init segment plus a one-frame audio media segment and feeds
/// the concatenation to ffprobe. This is the real validation for the
/// `esds` MPEG-4 descriptor length encoding introduced when
/// `parse_audio_specific_config` migrated to the hardened
/// `lvqr_codec::aac::parse_asc`: if the descriptor size fields are off
/// by a byte, ffprobe rejects the stream.
#[test]
fn ffprobe_accepts_audio_init_and_frame() {
    let config = AudioConfig {
        asc: vec![0x12, 0x10], // AAC-LC, 44100 Hz, stereo (same ASC as the unit tests)
        sample_rate: 44100,
        channels: 2,
        object_type: 2,
    };
    let init = audio_init_segment(&config);
    // A single zeroed AAC frame is enough payload for ffprobe to walk
    // the container structure; we are validating the esds descriptor
    // lengths, not the codec payload.
    let frame = Bytes::from(vec![0u8; 64]);
    let seg = audio_segment(1, 0, 1024, &frame);

    let mut buf = Vec::with_capacity(init.len() + seg.len());
    buf.extend_from_slice(&init);
    buf.extend_from_slice(&seg);

    ffprobe_bytes(&buf).assert_accepted();
}

#[test]
fn golden_dir_exists() {
    // Smoke check that the fixtures directory layout is present. This is
    // the lightest-possible proof that the BLESS workflow works end-to-end.
    let dir: &Path = &fixtures_dir();
    assert!(
        dir.exists() || std::env::var("BLESS").ok().as_deref() == Some("1"),
        "tests/fixtures/golden missing; run BLESS=1 cargo test -p lvqr-ingest --test golden_fmp4 to create it"
    );
}

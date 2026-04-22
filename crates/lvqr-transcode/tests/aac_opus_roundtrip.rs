//! AAC-to-Opus encoder round-trip test (session 113).
//!
//! Generates 400 ms of real AAC-LC audio via an in-test GStreamer
//! pipeline, pushes each access unit through
//! [`lvqr_transcode::AacToOpusEncoder`], and asserts that the
//! encoder produces at least a handful of non-empty Opus frames on
//! the wire. Skips gracefully when the host lacks the GStreamer
//! plugin set (`aacparse`, `avdec_aac`, `audioconvert`,
//! `audioresample`, `opusenc`, or the AAC encoder `avenc_aac` used
//! by the sample generator).
//!
//! The test is gated on the `transcode` feature so the default CI
//! gate (GStreamer-absent hosts) continues to skip the build of
//! this target entirely.

#![cfg(feature = "transcode")]

use std::time::Duration;

use bytes::Bytes;
use lvqr_transcode::test_support::generate_aac_access_units;
use lvqr_transcode::{AacAudioConfig, AacToOpusEncoderFactory, OpusFrame};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aac_to_opus_encoder_emits_opus_frames_for_real_aac_input() {
    let factory = AacToOpusEncoderFactory::new();
    if !factory.is_available() {
        eprintln!(
            "skipping aac_opus_roundtrip: AacToOpusEncoderFactory unavailable, missing {:?}",
            factory.missing_elements()
        );
        return;
    }

    let Some(aac_access_units) = generate_aac_access_units(400) else {
        eprintln!("skipping aac_opus_roundtrip: failed to generate AAC source samples via GStreamer");
        return;
    };
    assert!(!aac_access_units.is_empty(), "aac generator produced no access units");

    let (opus_tx, mut opus_rx) = tokio::sync::mpsc::unbounded_channel::<OpusFrame>();
    // AAC-LC, 48 kHz, stereo -> AudioSpecificConfig: object_type=2,
    // freq_idx=3 (48 kHz), channel_config=2. That packs into two
    // bytes: 0x11, 0x90.
    let asc = Bytes::from_static(&[0x11, 0x90]);
    let config = AacAudioConfig {
        asc: asc.clone(),
        sample_rate: 48_000,
        channels: 2,
        object_type: 2,
    };
    let encoder = factory
        .build(config, opus_tx)
        .expect("factory.build() must succeed when is_available() is true");

    // 1024 AAC samples per frame at 48 kHz -> ~21.3 ms per frame.
    // Stamp each sample's dts at the 48 kHz tick cadence so opusenc's
    // PTS chain stays monotonic across the input frames.
    let mut dts: u64 = 0;
    for au in &aac_access_units {
        encoder.push(au, dts);
        dts = dts.saturating_add(1024);
    }

    // Push a bit of tail silence so opusenc flushes its last frame.
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(encoder);

    let mut opus_frames: Vec<OpusFrame> = Vec::new();
    let gather_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < gather_deadline {
        match tokio::time::timeout(Duration::from_millis(200), opus_rx.recv()).await {
            Ok(Some(frame)) => opus_frames.push(frame),
            Ok(None) => break,
            Err(_) => break,
        }
    }

    assert!(
        !opus_frames.is_empty(),
        "AAC-to-Opus encoder produced zero Opus frames; expected several 20 ms frames over 400 ms input",
    );
    for (i, frame) in opus_frames.iter().enumerate() {
        assert!(!frame.payload.is_empty(), "Opus frame {i} empty");
        assert_eq!(
            frame.duration_ticks, 960,
            "Opus frame {i} duration should be 20 ms at 48 kHz"
        );
    }
    eprintln!(
        "aac_opus_roundtrip: {} AAC access units in -> {} Opus frames out",
        aac_access_units.len(),
        opus_frames.len()
    );
}

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
use lvqr_transcode::{AacAudioConfig, AacToOpusEncoderFactory, OpusFrame};

use glib::object::Cast;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

/// Build an ADTS-framed AAC stream from a silence-ish 48 kHz test
/// source. Returns `Some(bytes)` on success, `None` on any plugin
/// shortfall (the encoder side also probes independently, so the
/// parent test prints the skip reason and exits clean).
fn generate_aac_bytes_for_test(duration_ms: u64) -> Option<Vec<Vec<u8>>> {
    gst::init().ok()?;
    for elem in [
        "audiotestsrc",
        "audioconvert",
        "audioresample",
        "avenc_aac",
        "aacparse",
        "appsink",
    ] {
        if gst::ElementFactory::find(elem).is_none() {
            eprintln!("skipping aac_opus_roundtrip: generator missing element {elem}");
            return None;
        }
    }

    let pipeline_str = format!(
        "audiotestsrc num-buffers={nb} wave=ticks samplesperbuffer=1024 \
         ! audio/x-raw,rate=48000,channels=2 \
         ! audioconvert \
         ! audioresample \
         ! avenc_aac \
         ! aacparse \
         ! audio/mpeg,mpegversion=4,stream-format=adts \
         ! appsink name=sink emit-signals=true sync=false",
        nb = (duration_ms as i32 * 48 / 1024).max(8),
    );
    let element = gst::parse::launch(&pipeline_str).ok()?;
    let pipeline = element.downcast::<gst::Pipeline>().ok()?;
    let sink_elem = pipeline.by_name("sink")?;
    let sink = sink_elem.downcast::<gst_app::AppSink>().ok()?;

    let mut buffers: Vec<Vec<u8>> = Vec::new();
    pipeline.set_state(gst::State::Playing).ok()?;

    // Pull samples until the pipeline reaches EOS.
    let bus = pipeline.bus()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(sample) = sink.pull_sample() {
            if let Some(buf) = sample.buffer() {
                let map = buf.map_readable().ok()?;
                // Strip the ADTS header so the transcoder's own
                // wrapper drives the wire framing; keep only the
                // AAC raw access unit body.
                let slice = map.as_slice();
                if slice.len() > 7 {
                    buffers.push(slice[7..].to_vec());
                }
            }
        }
        let msg = bus.timed_pop(gst::ClockTime::from_mseconds(20));
        if let Some(msg) = msg {
            match msg.view() {
                gst::MessageView::Eos(_) | gst::MessageView::Error(_) => break,
                _ => {}
            }
        }
        if std::time::Instant::now() > deadline {
            break;
        }
    }
    pipeline.set_state(gst::State::Null).ok()?;
    if buffers.is_empty() { None } else { Some(buffers) }
}

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

    let Some(aac_access_units) = generate_aac_bytes_for_test(400) else {
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

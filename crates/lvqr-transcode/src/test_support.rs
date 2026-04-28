//! Shared test-scaffolding helpers for the `transcode` feature.
//!
//! The in-test AAC source pipeline used by
//! `crates/lvqr-transcode/tests/aac_opus_roundtrip.rs` to exercise
//! the encoder round-trip is also the natural AAC source for
//! downstream cross-crate tests (e.g. the session 115
//! `rtmp_whep_audio_e2e.rs` in `lvqr-cli`). Lifting the helper out
//! of the in-test file lets downstream callers depend on this
//! crate's public API under the same `transcode` feature gate
//! instead of pulling `gstreamer` / `gstreamer-app` / `glib` in as
//! direct dev-deps of their own.
//!
//! Only visible when the `transcode` feature is active; on hosts
//! without GStreamer this module does not compile and the helpers
//! are unreachable.

use std::time::{Duration, Instant};

use glib::object::Cast;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

/// Generate `duration_ms` of 48 kHz stereo AAC-LC access units via
/// an in-process `audiotestsrc ! avenc_aac ! aacparse` pipeline.
/// Each returned `Vec<u8>` is a single raw AAC access unit with the
/// 7-byte ADTS header stripped, so callers that wrap in their own
/// framing (FLV audio tag for RTMP, ADTS for the encoder's own
/// input side) can do so cleanly.
///
/// Returns `None` on any plugin shortfall: missing `audiotestsrc`,
/// `audioconvert`, `audioresample`, `avenc_aac`, `aacparse`, or
/// `appsink`. The test-side caller should print its own skip
/// message so the test output names the skip reason with its own
/// test context.
pub fn generate_aac_access_units(duration_ms: u64) -> Option<Vec<Vec<u8>>> {
    gst::init().ok()?;
    for elem in [
        "audiotestsrc",
        "audioconvert",
        "audioresample",
        "avenc_aac",
        "aacparse",
        "appsink",
    ] {
        gst::ElementFactory::find(elem)?;
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

    let mut access_units: Vec<Vec<u8>> = Vec::new();
    pipeline.set_state(gst::State::Playing).ok()?;

    let bus = pipeline.bus()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(sample) = sink.pull_sample()
            && let Some(buf) = sample.buffer()
            && let Ok(map) = buf.map_readable()
        {
            let slice = map.as_slice();
            if slice.len() > 7 {
                access_units.push(slice[7..].to_vec());
            }
        }
        if let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(20))
            && matches!(msg.view(), gst::MessageView::Eos(_) | gst::MessageView::Error(_))
        {
            break;
        }
        if Instant::now() > deadline {
            break;
        }
    }
    pipeline.set_state(gst::State::Null).ok()?;
    if access_units.is_empty() {
        None
    } else {
        Some(access_units)
    }
}

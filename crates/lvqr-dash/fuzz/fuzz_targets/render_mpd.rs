#![no_main]
//! libfuzzer target for the `lvqr-dash` MPD renderer.
//!
//! The renderer is a pure function over structured input, so there
//! is no attacker-controlled byte buffer in the usual sense. The
//! externally-reachable surface that *does* see arbitrary strings
//! is the `codecs` attribute: it flows directly from
//! `lvqr_cmaf::detect_video_codec_string` /
//! `detect_audio_codec_string`, which parse publisher-provided init
//! segments. A malicious publisher that crafts an init segment the
//! codec detector decodes into a codec string containing `"` or
//! `<` or `&` could in principle break the XML output. The
//! proptest harness at `tests/proptest_mpd.rs` deliberately pins
//! codecs to safe values; this libfuzzer target takes the
//! unstructured-mutation side of the same surface.
//!
//! Invariant asserted: `Mpd::render` never panics on any utf8
//! codecs string. Stronger invariants (exactly one `<MPD ...>` root
//! tag, no attribute-boundary injection) are deliberately NOT
//! asserted here because the current hand-rolled renderer does
//! not XML-escape attribute values yet; adding those invariants
//! requires shipping an escape-on-write path first, tracked as a
//! future-session item in `tracking/HANDOFF.md`.

use libfuzzer_sys::fuzz_target;
use lvqr_dash::{AdaptationSet, Mpd, MpdType, Period, Representation, SegmentTemplate};

fuzz_target!(|data: &[u8]| {
    // Interpret the fuzzer input as a utf8 codecs string. Reject
    // non-utf8 inputs so we stay focused on the attribute-escape
    // path rather than the codec detector itself.
    let Ok(codecs) = std::str::from_utf8(data) else {
        return;
    };

    let mpd = Mpd {
        mpd_type: MpdType::Dynamic,
        profiles: "urn:mpeg:dash:profile:isoff-live:2011".into(),
        min_buffer_time: "PT2.0S".into(),
        minimum_update_period: "PT2.0S".into(),
        periods: vec![Period {
            id: "0".into(),
            start: "PT0S".into(),
            adaptation_sets: vec![AdaptationSet {
                id: 0,
                mime_type: "video/mp4".into(),
                content_type: "video".into(),
                lang: None,
                representations: vec![Representation {
                    id: "video".into(),
                    codecs: codecs.to_string(),
                    bandwidth_bps: 2_500_000,
                    width: Some(1280),
                    height: Some(720),
                    audio_sampling_rate: None,
                }],
                segment_template: SegmentTemplate {
                    initialization: "init-video.m4s".into(),
                    media: "seg-video-$Number$.m4s".into(),
                    start_number: 1,
                    duration: 180_000,
                    timescale: 90_000,
                },
            }],
        }],
    };

    // The only invariant the current renderer strictly guarantees
    // is panic-freedom. Any crash here is a real bug.
    let _ = mpd.render();
});

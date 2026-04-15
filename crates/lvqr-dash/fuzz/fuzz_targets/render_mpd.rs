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
//! Invariants asserted:
//!
//! 1. `Mpd::render` never panics on any utf8 codecs string.
//! 2. When `render` returns `Ok`, the output contains exactly one
//!    `<MPD ` opening tag and one `</MPD>` closing tag regardless
//!    of what the fuzzer stuffs into the codecs attribute. Before
//!    the session-33 escape path this invariant was not safe to
//!    assert because a crafted codecs string like `"/><foo` would
//!    have broken the MPD root; the escape helper in
//!    `mpd::esc` now makes it hold unconditionally.
//! 3. The adaptation-set count in the output matches the one in
//!    the input (this MPD has exactly one), so any fuzzer-chosen
//!    content that accidentally produced a second `<AdaptationSet`
//!    tag would trip the assertion.

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

    let Ok(xml) = mpd.render() else { return };
    // Stronger invariants: the fuzzed codecs string must not be
    // able to tear the MPD root apart.
    assert_eq!(xml.matches("<MPD ").count(), 1);
    assert_eq!(xml.matches("</MPD>").count(), 1);
    assert_eq!(xml.matches("<AdaptationSet ").count(), 1);
});

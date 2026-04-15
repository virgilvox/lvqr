//! Property tests for the `lvqr-dash` typed MPD renderer.
//!
//! Session 33 landed these to close the proptest slot of the
//! 5-artifact contract for `lvqr-dash`. The renderer is a pure
//! function over structured input, so the natural invariants are
//! absence-of-panic under well-formed input and shape properties
//! of the rendered XML.
//!
//! Invariants we enforce:
//!
//! 1. `Mpd::render` never panics on any well-formed `Mpd` value
//!    (at least one period with at least one adaptation set with
//!    at least one representation). It may return `Ok` or
//!    `Err(DashError)`, but must not panic.
//! 2. When the call returns `Ok`, the body starts with the XML
//!    prologue, contains exactly one `<MPD ...>` root, ends with
//!    `</MPD>\n`, and contains as many `<AdaptationSet` occurrences
//!    as the input structure has adaptation sets summed across
//!    every period.
//! 3. `render_mpd(&mpd)` and `mpd.render()` are byte-equal for
//!    every input (the free function is a trivial re-export and
//!    must never drift).
//! 4. The `type="dynamic"` / `type="static"` attribute matches
//!    `MpdType` in every rendered body.

use lvqr_dash::{AdaptationSet, Mpd, MpdType, Period, Representation, SegmentTemplate, render_mpd};
use proptest::prelude::*;

fn arb_mpd_type() -> impl Strategy<Value = MpdType> {
    prop_oneof![Just(MpdType::Dynamic), Just(MpdType::Static)]
}

fn arb_representation() -> impl Strategy<Value = Representation> {
    // The `codecs` generator is deliberately unrestricted ("\\PC*"
    // = any printable unicode) so the proptest exercises the XML
    // attribute-escape path the session-33 `esc` helper landed
    // for. Pre-escape, a generator containing `"` or `<` would
    // have broken the `<AdaptationSet ` count invariant below.
    (
        1u32..100,
        "\\PC*",
        1u32..10_000_000,
        prop::option::of(240u32..3840),
        prop::option::of(240u32..2160),
        prop::option::of(8_000u32..192_000),
    )
        .prop_map(|(id_n, codecs, bw, w, h, sr)| Representation {
            id: format!("rep-{id_n}"),
            codecs,
            bandwidth_bps: bw,
            width: w,
            height: h,
            audio_sampling_rate: sr,
        })
}

fn arb_segment_template() -> impl Strategy<Value = SegmentTemplate> {
    (1u64..1_000, 1u64..1_000_000, 1u32..200_000).prop_map(|(sn, dur, ts)| SegmentTemplate {
        initialization: "init.m4s".into(),
        media: "seg-$Number$.m4s".into(),
        start_number: sn,
        duration: dur,
        timescale: ts,
    })
}

fn arb_adaptation_set() -> impl Strategy<Value = AdaptationSet> {
    (
        0u32..100,
        prop::collection::vec(arb_representation(), 1..4),
        arb_segment_template(),
    )
        .prop_map(|(id, reps, st)| AdaptationSet {
            id,
            mime_type: "video/mp4".into(),
            content_type: "video".into(),
            lang: None,
            representations: reps,
            segment_template: st,
        })
}

fn arb_period() -> impl Strategy<Value = Period> {
    prop::collection::vec(arb_adaptation_set(), 1..4).prop_map(|sets| Period {
        id: "0".into(),
        start: "PT0S".into(),
        adaptation_sets: sets,
    })
}

fn arb_mpd() -> impl Strategy<Value = Mpd> {
    (arb_mpd_type(), prop::collection::vec(arb_period(), 1..3)).prop_map(|(ty, periods)| Mpd {
        mpd_type: ty,
        profiles: "urn:mpeg:dash:profile:isoff-live:2011".into(),
        min_buffer_time: "PT2.0S".into(),
        minimum_update_period: "PT2.0S".into(),
        periods,
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn render_never_panics_on_well_formed_mpd(mpd in arb_mpd()) {
        // Panic-freedom: render may return Ok or Err, but must not
        // unwind. Any panic inside `Mpd::render` bubbles up as a
        // proptest failure the harness reports with the shrinking
        // counterexample.
        let _ = mpd.render();
    }

    #[test]
    fn render_emits_xml_prologue_and_root_on_ok(mpd in arb_mpd()) {
        let Ok(xml) = mpd.render() else { return Ok(()); };
        prop_assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
        prop_assert!(xml.contains("<MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\""));
        prop_assert!(xml.ends_with("</MPD>\n"));
    }

    #[test]
    fn render_adaptation_set_count_matches_input(mpd in arb_mpd()) {
        let Ok(xml) = mpd.render() else { return Ok(()); };
        let expected: usize = mpd.periods.iter().map(|p| p.adaptation_sets.len()).sum();
        let actual = xml.matches("<AdaptationSet ").count();
        prop_assert_eq!(expected, actual);
    }

    #[test]
    fn render_mpd_free_function_matches_method(mpd in arb_mpd()) {
        let a = mpd.render();
        let b = render_mpd(&mpd);
        match (a, b) {
            (Ok(s1), Ok(s2)) => prop_assert_eq!(s1, s2),
            (Err(_), Err(_)) => {}
            _ => prop_assert!(false, "render and render_mpd disagreed on success"),
        }
    }

    #[test]
    fn render_type_attribute_matches_mpd_type(mpd in arb_mpd()) {
        let Ok(xml) = mpd.render() else { return Ok(()); };
        match mpd.mpd_type {
            MpdType::Dynamic => {
                prop_assert!(xml.contains("type=\"dynamic\""));
                prop_assert!(!xml.contains("type=\"static\""));
            }
            MpdType::Static => {
                prop_assert!(xml.contains("type=\"static\""));
                prop_assert!(!xml.contains("type=\"dynamic\""));
            }
        }
    }
}

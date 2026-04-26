//! Golden-file conformance test for the `lvqr-dash` MPD renderer.
//!
//! Closes the conformance slot of the 5-artifact contract for
//! `lvqr-dash`. The renderer is a hand-written XML builder and the
//! byte-for-byte output is part of the public interface: any
//! deliberate change (whitespace, attribute order, tag grouping)
//! must flow through this test as a visible diff before it ships.
//!
//! There is no external DASH-IF conformance tool in this test;
//! that is still an open slot and will land alongside the
//! self-hosted macOS runner that hosts Apple HTTP Live Streaming
//! Tools. Golden files are the accepted substitute for
//! hand-rolled writers under `tests/CONTRACT.md` rationale until
//! an external validator is wired in.

use lvqr_dash::{AdaptationSet, Mpd, MpdType, Period, Representation, SegmentTemplate};

/// Canonical single-video live-profile MPD. 90 kHz video timescale,
/// 2 s segment (180_000 ticks), 1280x720 at 2.5 Mbps, avc1.640028.
fn video_only_live() -> Mpd {
    Mpd {
        mpd_type: MpdType::Dynamic,
        profiles: "urn:mpeg:dash:profile:isoff-live:2011".into(),
        min_buffer_time: "PT2.0S".into(),
        minimum_update_period: "PT2.0S".into(),
        periods: vec![Period {
            id: "0".into(),
            start: "PT0S".into(),
            event_streams: Vec::new(),
            adaptation_sets: vec![AdaptationSet {
                id: 0,
                mime_type: "video/mp4".into(),
                content_type: "video".into(),
                lang: None,
                representations: vec![Representation {
                    id: "video".into(),
                    codecs: "avc1.640028".into(),
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
    }
}

const VIDEO_ONLY_LIVE_GOLDEN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic" profiles="urn:mpeg:dash:profile:isoff-live:2011" minBufferTime="PT2.0S" minimumUpdatePeriod="PT2.0S">
  <Period id="0" start="PT0S">
    <AdaptationSet id="0" mimeType="video/mp4" contentType="video" segmentAlignment="true">
      <SegmentTemplate initialization="init-video.m4s" media="seg-video-$Number$.m4s" startNumber="1" duration="180000" timescale="90000"/>
      <Representation id="video" codecs="avc1.640028" bandwidth="2500000" width="1280" height="720">
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>
"#;

#[test]
fn video_only_live_profile_matches_golden() {
    let mpd = video_only_live();
    let xml = mpd.render().expect("golden video mpd renders");
    assert_eq!(
        xml, VIDEO_ONLY_LIVE_GOLDEN,
        "video-only golden drifted; either a real MPD change or a regression"
    );
}

/// Canonical audio+video live-profile MPD. 90 kHz video + 48 kHz
/// Opus audio, lang="en".
fn av_live() -> Mpd {
    let mut mpd = video_only_live();
    mpd.periods[0].adaptation_sets.push(AdaptationSet {
        id: 1,
        mime_type: "audio/mp4".into(),
        content_type: "audio".into(),
        lang: Some("en".into()),
        representations: vec![Representation {
            id: "audio".into(),
            codecs: "opus".into(),
            bandwidth_bps: 128_000,
            width: None,
            height: None,
            audio_sampling_rate: Some(48_000),
        }],
        segment_template: SegmentTemplate {
            initialization: "init-audio.m4s".into(),
            media: "seg-audio-$Number$.m4s".into(),
            start_number: 1,
            duration: 96_000,
            timescale: 48_000,
        },
    });
    mpd
}

const AV_LIVE_GOLDEN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic" profiles="urn:mpeg:dash:profile:isoff-live:2011" minBufferTime="PT2.0S" minimumUpdatePeriod="PT2.0S">
  <Period id="0" start="PT0S">
    <AdaptationSet id="0" mimeType="video/mp4" contentType="video" segmentAlignment="true">
      <SegmentTemplate initialization="init-video.m4s" media="seg-video-$Number$.m4s" startNumber="1" duration="180000" timescale="90000"/>
      <Representation id="video" codecs="avc1.640028" bandwidth="2500000" width="1280" height="720">
      </Representation>
    </AdaptationSet>
    <AdaptationSet id="1" mimeType="audio/mp4" contentType="audio" segmentAlignment="true" lang="en">
      <SegmentTemplate initialization="init-audio.m4s" media="seg-audio-$Number$.m4s" startNumber="1" duration="96000" timescale="48000"/>
      <Representation id="audio" codecs="opus" bandwidth="128000" audioSamplingRate="48000">
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>
"#;

#[test]
fn av_live_profile_matches_golden() {
    let mpd = av_live();
    let xml = mpd.render().expect("golden av mpd renders");
    assert_eq!(
        xml, AV_LIVE_GOLDEN,
        "av golden drifted; either a real MPD change or a regression"
    );
}

/// VOD variant: same shape, `type="static"`.
#[test]
fn vod_variant_matches_golden() {
    let mut mpd = video_only_live();
    mpd.mpd_type = MpdType::Static;
    let xml = mpd.render().expect("golden vod mpd renders");
    let expected = VIDEO_ONLY_LIVE_GOLDEN.replace("type=\"dynamic\"", "type=\"static\"");
    assert_eq!(xml, expected, "vod golden drifted");
}

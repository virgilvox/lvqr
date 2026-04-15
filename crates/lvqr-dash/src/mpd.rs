//! Typed Media Presentation Description (MPD) renderer.
//!
//! The MPD is the root document of a DASH stream: a hierarchical
//! XML description of Periods, AdaptationSets, and Representations
//! that tells the client how to find media segments. For a live
//! stream with LVQR's fixed-duration CMAF segments, the MPD is
//! extremely compact:
//!
//! ```xml
//! <?xml version="1.0" encoding="UTF-8"?>
//! <MPD xmlns="urn:mpeg:dash:schema:mpd:2011"
//!      type="dynamic"
//!      profiles="urn:mpeg:dash:profile:isoff-live:2011"
//!      minBufferTime="PT2.0S"
//!      minimumUpdatePeriod="PT2.0S">
//!   <Period id="0" start="PT0S">
//!     <AdaptationSet id="0" mimeType="video/mp4"
//!                    contentType="video" segmentAlignment="true">
//!       <Representation id="video" codecs="avc1.64001f" bandwidth="2500000"
//!                       width="1280" height="720">
//!         <SegmentTemplate initialization="init-video.m4s"
//!                          media="seg-video-$Number$.m4s"
//!                          startNumber="1"
//!                          duration="180000"
//!                          timescale="90000"/>
//!       </Representation>
//!     </AdaptationSet>
//!     <AdaptationSet ...> <!-- audio --> </AdaptationSet>
//!   </Period>
//! </MPD>
//! ```
//!
//! Each LVQR track (one video, optional one audio) becomes an
//! AdaptationSet with a single Representation. The `SegmentTemplate`
//! `$Number$` placeholder is resolved client-side against the fixed
//! `duration` / `timescale` pair; a client that has observed the
//! stream for `N` seconds pulls segment number
//! `startNumber + floor(N / (duration / timescale))`.
//!
//! The renderer is a pure function over the [`Mpd`] struct. Callers
//! build the struct from observed init + segment state (future
//! `DashServer`) and call [`Mpd::render`] per manifest request.

use std::borrow::Cow;
use std::fmt::Write as _;
use thiserror::Error;

/// Escape a string for inclusion as an XML attribute value.
///
/// Replaces the five XML special characters with their entity
/// references: `&` -> `&amp;`, `<` -> `&lt;`, `>` -> `&gt;`,
/// `"` -> `&quot;`, `'` -> `&apos;`. Strings that contain none
/// of these are returned as a zero-cost `Cow::Borrowed`, so the
/// common-case hit (all ASCII-safe ISO BMFF codec strings,
/// fixed URIs, etc.) allocates nothing.
///
/// This is deliberately applied at the single serialization
/// boundary rather than sanitized at construction time because
/// `lvqr-dash` public structs are still typed with `String` and
/// a future session may want to accept unicode names or lang
/// tags that contain characters the upstream producer cannot
/// strip. Keeping the escape inline means the public API cannot
/// drift out of sync with the renderer.
fn esc(s: &str) -> Cow<'_, str> {
    if !s.bytes().any(|b| matches!(b, b'<' | b'>' | b'&' | b'"' | b'\'')) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Result type for MPD construction. Render itself is infallible;
/// the error type exists so future growing constraints (e.g. a
/// required attribute missing on a Representation) have a home.
#[derive(Debug, Error)]
pub enum DashError {
    #[error("MPD period has no adaptation sets")]
    EmptyPeriod,
    #[error("AdaptationSet has no representations")]
    EmptyAdaptationSet,
}

/// `type` attribute on the MPD root element. DASH distinguishes a
/// live stream (`dynamic`) from a VOD stream (`static`). Live is
/// the only mode LVQR ships today; VOD lands alongside the LL-HLS
/// DVR scrub story.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MpdType {
    /// `type="dynamic"`. Live stream; the client polls the MPD for
    /// updates and follows the SegmentTemplate to the live edge.
    Dynamic,
    /// `type="static"`. VOD stream; the MPD is immutable and the
    /// client sees the full duration up front.
    Static,
}

impl MpdType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::Static => "static",
        }
    }
}

/// One AdaptationSet's `SegmentTemplate` element. LVQR emits a
/// single template per AdaptationSet because all segments are
/// constant-duration and sit under a predictable URI; the more
/// complex `SegmentTimeline` element is a future-session addition
/// when the segmenter starts producing variable-duration segments.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentTemplate {
    /// `initialization` attribute: URI of the init segment that
    /// the client fetches once per Representation before any media
    /// segments. Relative to the Period's `BaseURL` (which LVQR
    /// leaves empty so URIs resolve against the MPD's HTTP path).
    pub initialization: String,
    /// `media` attribute: URI template with `$Number$` placeholder
    /// for each media segment.
    pub media: String,
    /// `startNumber` attribute: the number of the first segment
    /// available. LVQR starts at 1 to match most DASH client
    /// expectations and the `SegmentTemplate` live profile default.
    pub start_number: u64,
    /// `duration` attribute: nominal segment duration in the
    /// template's `timescale`. All segments are assumed to share
    /// this duration in the live profile; real drift is handled by
    /// the client re-polling the MPD.
    pub duration: u64,
    /// `timescale` attribute: tick rate of the `duration` field.
    /// Matches the track's native timescale (90 000 for 90 kHz
    /// video, 48 000 for Opus audio, 44 100 for 44.1 kHz AAC).
    pub timescale: u32,
}

impl SegmentTemplate {
    fn write(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent);
        let _ = writeln!(
            out,
            r#"{pad}<SegmentTemplate initialization="{init}" media="{media}" startNumber="{sn}" duration="{dur}" timescale="{ts}"/>"#,
            init = esc(&self.initialization),
            media = esc(&self.media),
            sn = self.start_number,
            dur = self.duration,
            ts = self.timescale,
        );
    }
}

/// One Representation inside an AdaptationSet. In LVQR's default
/// single-bitrate live profile there is exactly one Representation
/// per AdaptationSet (one for video, optionally one for audio).
/// ABR ladder Representations land when the server-side transcoding
/// Tier 4 moat ships.
#[derive(Debug, Clone, PartialEq)]
pub struct Representation {
    /// `id` attribute. Must be unique within the Period.
    pub id: String,
    /// `codecs` attribute: ISO BMFF codec string. Pulled from
    /// `lvqr_cmaf::detect_video_codec_string` /
    /// `detect_audio_codec_string` so H.264 / HEVC / AAC / Opus
    /// publishers automatically populate the right value.
    pub codecs: String,
    /// `bandwidth` attribute: estimated max bitrate in bits per
    /// second. DASH clients use it to pick between
    /// Representations in an ABR ladder. LVQR's current single-
    /// bitrate profile hardcodes a conservative 2.5 Mbps for video
    /// and 128 kbps for audio; a future bandwidth-discovery
    /// producer will fill in real numbers.
    pub bandwidth_bps: u32,
    /// `width` attribute for video Representations. `None` for
    /// audio.
    pub width: Option<u32>,
    /// `height` attribute for video Representations. `None` for
    /// audio.
    pub height: Option<u32>,
    /// `audioSamplingRate` attribute for audio Representations.
    /// `None` for video.
    pub audio_sampling_rate: Option<u32>,
}

impl Representation {
    fn write(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent);
        let _ = write!(
            out,
            r#"{pad}<Representation id="{id}" codecs="{codecs}" bandwidth="{bw}""#,
            id = esc(&self.id),
            codecs = esc(&self.codecs),
            bw = self.bandwidth_bps,
        );
        if let Some(w) = self.width {
            let _ = write!(out, r#" width="{w}""#);
        }
        if let Some(h) = self.height {
            let _ = write!(out, r#" height="{h}""#);
        }
        if let Some(sr) = self.audio_sampling_rate {
            let _ = write!(out, r#" audioSamplingRate="{sr}""#);
        }
        out.push_str(">\n");
    }

    fn write_close(out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent);
        let _ = writeln!(out, "{pad}</Representation>");
    }
}

/// One AdaptationSet inside a Period. An AdaptationSet groups
/// Representations that are interchangeable at the codec level: a
/// client may switch among them without retriggering a full
/// decoder reset. LVQR ships one AdaptationSet per track (video,
/// audio) with a single Representation inside.
#[derive(Debug, Clone, PartialEq)]
pub struct AdaptationSet {
    /// `id` attribute. Unique within the Period.
    pub id: u32,
    /// `mimeType` attribute. `"video/mp4"` for H.264 / HEVC,
    /// `"audio/mp4"` for AAC / Opus.
    pub mime_type: String,
    /// `contentType` attribute: `"video"` or `"audio"`. DASH uses
    /// this for ISO BMFF box selection and accessibility labelling.
    pub content_type: String,
    /// `lang` attribute for audio AdaptationSets. `None` for video.
    pub lang: Option<String>,
    /// Representations to emit inside this set.
    pub representations: Vec<Representation>,
    /// Shared SegmentTemplate for every Representation in the set.
    /// In the MPD spec the template may live on the AdaptationSet
    /// to avoid duplicating it per Representation; LVQR always
    /// puts it on the set because every Representation in a given
    /// set shares the same segment duration + timescale.
    pub segment_template: SegmentTemplate,
}

impl AdaptationSet {
    fn write(&self, out: &mut String, indent: usize) -> Result<(), DashError> {
        if self.representations.is_empty() {
            return Err(DashError::EmptyAdaptationSet);
        }
        let pad = "  ".repeat(indent);
        let _ = write!(
            out,
            r#"{pad}<AdaptationSet id="{id}" mimeType="{mt}" contentType="{ct}" segmentAlignment="true""#,
            id = self.id,
            mt = esc(&self.mime_type),
            ct = esc(&self.content_type),
        );
        if let Some(lang) = &self.lang {
            let _ = write!(out, r#" lang="{lang}""#, lang = esc(lang));
        }
        out.push_str(">\n");
        self.segment_template.write(out, indent + 1);
        for rep in &self.representations {
            rep.write(out, indent + 1);
            Representation::write_close(out, indent + 1);
        }
        let _ = writeln!(out, "{pad}</AdaptationSet>");
        Ok(())
    }
}

/// One Period inside the MPD. LVQR uses a single Period for the
/// entire live stream; multi-Period support is a future-session
/// addition when SCTE-35 insertion points or mid-stream codec
/// changes land.
#[derive(Debug, Clone, PartialEq)]
pub struct Period {
    /// `id` attribute. Unique within the MPD.
    pub id: String,
    /// `start` attribute as an ISO 8601 duration (e.g. `"PT0S"`).
    /// Live streams almost always start at `PT0S`.
    pub start: String,
    /// AdaptationSets inside this Period.
    pub adaptation_sets: Vec<AdaptationSet>,
}

impl Period {
    fn write(&self, out: &mut String, indent: usize) -> Result<(), DashError> {
        if self.adaptation_sets.is_empty() {
            return Err(DashError::EmptyPeriod);
        }
        let pad = "  ".repeat(indent);
        let _ = writeln!(
            out,
            r#"{pad}<Period id="{id}" start="{start}">"#,
            id = esc(&self.id),
            start = esc(&self.start)
        );
        for set in &self.adaptation_sets {
            set.write(out, indent + 1)?;
        }
        let _ = writeln!(out, "{pad}</Period>");
        Ok(())
    }
}

/// The complete Media Presentation Description. Owns one or more
/// [`Period`] values plus the top-level timing attributes DASH
/// clients read to decide how aggressively to poll.
#[derive(Debug, Clone, PartialEq)]
pub struct Mpd {
    /// `type` attribute: live (`dynamic`) or VOD (`static`).
    pub mpd_type: MpdType,
    /// `profiles` attribute. LVQR ships the ISO BMFF live profile
    /// (`urn:mpeg:dash:profile:isoff-live:2011`).
    pub profiles: String,
    /// `minBufferTime` attribute as an ISO 8601 duration (e.g.
    /// `"PT2.0S"`). Matches LVQR's default 2 s segment.
    pub min_buffer_time: String,
    /// `minimumUpdatePeriod` attribute: the shortest interval a
    /// client should wait before re-fetching the MPD. LVQR
    /// defaults to 2 s to match the segment cadence; the
    /// LL-DASH profile can be ratcheted down once chunked-transfer
    /// segment writing lands.
    pub minimum_update_period: String,
    /// One or more Periods.
    pub periods: Vec<Period>,
}

impl Mpd {
    /// Render the MPD as UTF-8 XML. Returns an owned `String`
    /// ready to serve as an `application/dash+xml` response body.
    /// Fails if the MPD has no Periods, a Period has no
    /// AdaptationSets, or an AdaptationSet has no Representations;
    /// the individual check gives a typed error value instead of
    /// panicking.
    pub fn render(&self) -> Result<String, DashError> {
        if self.periods.is_empty() {
            return Err(DashError::EmptyPeriod);
        }
        let mut out = String::with_capacity(1024);
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let _ = writeln!(
            out,
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="{ty}" profiles="{profiles}" minBufferTime="{mbt}" minimumUpdatePeriod="{mup}">"#,
            ty = self.mpd_type.as_str(),
            profiles = esc(&self.profiles),
            mbt = esc(&self.min_buffer_time),
            mup = esc(&self.minimum_update_period),
        );
        for period in &self.periods {
            period.write(&mut out, 1)?;
        }
        out.push_str("</MPD>\n");
        Ok(out)
    }
}

/// Free-standing version of [`Mpd::render`] mirroring the pattern
/// `lvqr-hls::render_manifest` uses. Convenient for callers that
/// want to compose the MPD and render it in one expression.
pub fn render_mpd(mpd: &Mpd) -> Result<String, DashError> {
    mpd.render()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_mpd_with_video() -> Mpd {
        Mpd {
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
                        codecs: "avc1.42001F".into(),
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

    #[test]
    fn render_emits_well_formed_live_mpd_skeleton() {
        let mpd = live_mpd_with_video();
        let xml = mpd.render().expect("render");
        // XML declaration + root element.
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
        assert!(xml.contains(r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011""#));
        assert!(xml.contains(r#"type="dynamic""#));
        assert!(xml.contains(r#"profiles="urn:mpeg:dash:profile:isoff-live:2011""#));
        assert!(xml.contains(r#"minBufferTime="PT2.0S""#));
        assert!(xml.contains(r#"minimumUpdatePeriod="PT2.0S""#));
        // Period + AdaptationSet + Representation + SegmentTemplate.
        assert!(xml.contains(r#"<Period id="0" start="PT0S">"#));
        assert!(
            xml.contains(r#"<AdaptationSet id="0" mimeType="video/mp4" contentType="video" segmentAlignment="true">"#)
        );
        assert!(xml.contains(
            r#"<Representation id="video" codecs="avc1.42001F" bandwidth="2500000" width="1280" height="720">"#
        ));
        assert!(xml.contains(
            r#"<SegmentTemplate initialization="init-video.m4s" media="seg-video-$Number$.m4s" startNumber="1" duration="180000" timescale="90000"/>"#
        ));
        // Closing tags.
        assert!(xml.contains("</Representation>"));
        assert!(xml.contains("</AdaptationSet>"));
        assert!(xml.contains("</Period>"));
        assert!(xml.ends_with("</MPD>\n"));
    }

    #[test]
    fn render_emits_audio_adaptation_set_when_present() {
        let mut mpd = live_mpd_with_video();
        // Append an audio AdaptationSet. 48 kHz Opus representation.
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
        let xml = mpd.render().expect("render");
        // Audio AdaptationSet carries lang="en" and the Opus codec
        // string from the shared lvqr-cmaf codec detector family.
        assert!(xml.contains(
            r#"<AdaptationSet id="1" mimeType="audio/mp4" contentType="audio" segmentAlignment="true" lang="en">"#
        ));
        assert!(
            xml.contains(r#"<Representation id="audio" codecs="opus" bandwidth="128000" audioSamplingRate="48000">"#)
        );
        assert!(xml.contains(
            r#"<SegmentTemplate initialization="init-audio.m4s" media="seg-audio-$Number$.m4s" startNumber="1" duration="96000" timescale="48000"/>"#
        ));
        // Video rep still present and emitted BEFORE the audio one.
        let video_pos = xml.find(r#"id="video""#).expect("video rep present");
        let audio_pos = xml.find(r#"id="audio""#).expect("audio rep present");
        assert!(video_pos < audio_pos, "video rep must render before audio rep");
    }

    #[test]
    fn render_rejects_empty_period() {
        let mpd = Mpd {
            mpd_type: MpdType::Dynamic,
            profiles: "urn:mpeg:dash:profile:isoff-live:2011".into(),
            min_buffer_time: "PT2.0S".into(),
            minimum_update_period: "PT2.0S".into(),
            periods: vec![Period {
                id: "0".into(),
                start: "PT0S".into(),
                adaptation_sets: Vec::new(),
            }],
        };
        assert!(matches!(mpd.render(), Err(DashError::EmptyPeriod)));
    }

    #[test]
    fn render_rejects_empty_mpd() {
        let mpd = Mpd {
            mpd_type: MpdType::Dynamic,
            profiles: "urn:mpeg:dash:profile:isoff-live:2011".into(),
            min_buffer_time: "PT2.0S".into(),
            minimum_update_period: "PT2.0S".into(),
            periods: Vec::new(),
        };
        assert!(matches!(mpd.render(), Err(DashError::EmptyPeriod)));
    }

    #[test]
    fn render_rejects_empty_adaptation_set() {
        let mut mpd = live_mpd_with_video();
        mpd.periods[0].adaptation_sets[0].representations.clear();
        assert!(matches!(mpd.render(), Err(DashError::EmptyAdaptationSet)));
    }

    #[test]
    fn esc_is_zero_copy_on_safe_input() {
        let s = "avc1.640028";
        match esc(s) {
            Cow::Borrowed(b) => assert_eq!(b, s),
            Cow::Owned(_) => panic!("esc should not allocate for safe input"),
        }
    }

    #[test]
    fn esc_escapes_all_five_xml_specials() {
        let out = esc("a\"b<c>d&e'f");
        assert_eq!(&*out, "a&quot;b&lt;c&gt;d&amp;e&apos;f");
    }

    #[test]
    fn hostile_codecs_string_does_not_break_xml_root() {
        // Codec strings come from lvqr_cmaf::detect_*_codec_string
        // parsing publisher-provided init segments. A crafted init
        // that decoded into `"/><foo` would have torn the MPD root
        // element apart before the session-33 escape path landed.
        let mut mpd = live_mpd_with_video();
        mpd.periods[0].adaptation_sets[0].representations[0].codecs = "\"/><injection\"".into();
        let xml = mpd.render().expect("render should succeed with hostile codecs");
        // Exactly one <MPD ...> root and one </MPD>.
        assert_eq!(xml.matches("<MPD ").count(), 1);
        assert_eq!(xml.matches("</MPD>").count(), 1);
        // The injection payload is escaped, not parsed as markup.
        assert!(
            xml.contains(r#"codecs="&quot;/&gt;&lt;injection&quot;""#),
            "codecs attribute not escaped correctly:\n{xml}"
        );
    }

    #[test]
    fn static_type_renders_static_attribute() {
        let mut mpd = live_mpd_with_video();
        mpd.mpd_type = MpdType::Static;
        let xml = mpd.render().expect("render");
        assert!(xml.contains(r#"type="static""#));
        assert!(!xml.contains(r#"type="dynamic""#));
    }
}

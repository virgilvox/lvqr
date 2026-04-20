//! HLS master (multivariant) playlist types and renderer.
//!
//! Session 13 scope: the minimum viable master-playlist generator
//! needed to declare an audio rendition group alongside a video
//! variant, so a player that fetches `/hls/{broadcast}/master.m3u8`
//! discovers both tracks and picks them up from their per-rendition
//! media playlists. The renderer is intentionally tiny -- no
//! subtitles, no closed captions, no iframe playlists, no resolution
//! / bandwidth estimation. Those land when a real browser player
//! exercises the path.
//!
//! The output targets `#EXT-X-VERSION:9` because every LVQR media
//! playlist already declares 9; keeping the master playlist on the
//! same version avoids a compat footgun for clients that look at
//! the master tag and pick a parser.
//!
//! ## Emitted tags (in order)
//!
//! * `#EXTM3U`
//! * `#EXT-X-VERSION:9`
//! * `#EXT-X-INDEPENDENT-SEGMENTS` -- LL-HLS requires this so the
//!   client can join an open segment without waiting for the next
//!   IDR.
//! * one `#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=...,NAME=...,URI=...`
//!   per audio rendition
//! * one `#EXT-X-STREAM-INF:BANDWIDTH=...,CODECS=...[,RESOLUTION=W x H][,AUDIO="..."]`
//!   followed by the variant URI on the next line

use std::fmt::Write;

/// `TYPE` values accepted by the `#EXT-X-MEDIA` tag. Only the audio
/// slot is exercised today; the others exist so later sessions can
/// extend the struct without a breaking rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaRenditionType {
    /// `TYPE=AUDIO`. A standalone audio rendition referenced from a
    /// variant stream's `AUDIO="<group-id>"` attribute.
    Audio,
    /// `TYPE=VIDEO`. Reserved for future use.
    Video,
    /// `TYPE=SUBTITLES`. Reserved for future use.
    Subtitles,
    /// `TYPE=CLOSED-CAPTIONS`. Reserved for future use.
    ClosedCaptions,
}

impl MediaRenditionType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "AUDIO",
            Self::Video => "VIDEO",
            Self::Subtitles => "SUBTITLES",
            Self::ClosedCaptions => "CLOSED-CAPTIONS",
        }
    }
}

/// A single `#EXT-X-MEDIA` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRendition {
    /// `TYPE=...`.
    pub rendition_type: MediaRenditionType,
    /// `GROUP-ID="..."`. Variant streams reference this group by
    /// name via their `AUDIO="<group-id>"` attribute.
    pub group_id: String,
    /// `NAME="..."`. Human-readable label shown in the player's
    /// rendition picker.
    pub name: String,
    /// `URI="..."`. Path to the media playlist for this rendition,
    /// relative to the master playlist.
    pub uri: String,
    /// `DEFAULT=YES|NO`.
    pub default: bool,
    /// `AUTOSELECT=YES|NO`.
    pub autoselect: bool,
    /// Optional `LANGUAGE="..."` attribute. BCP-47 tag. Omitted
    /// when `None`.
    pub language: Option<String>,
}

/// A single `#EXT-X-STREAM-INF` variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantStream {
    /// `BANDWIDTH=...` in bits per second. Required by the HLS spec.
    pub bandwidth_bps: u64,
    /// `CODECS="..."`, e.g. `avc1.640020,mp4a.40.2`. Required in
    /// practice for modern HLS players to know whether they can
    /// decode the variant without a round-trip through the media
    /// playlist.
    pub codecs: String,
    /// Optional `RESOLUTION=WxH` attribute.
    pub resolution: Option<(u32, u32)>,
    /// Optional `AUDIO="..."` attribute referencing a
    /// [`MediaRendition`] group. When set, the variant's video
    /// track is combined with the referenced audio rendition at
    /// playback time.
    pub audio_group: Option<String>,
    /// Optional `SUBTITLES="..."` attribute referencing a
    /// [`MediaRendition`] group of `TYPE=SUBTITLES`. Browser
    /// players (hls.js, native Safari) pick the rendition's
    /// playlist URI off the master playlist's matching
    /// `EXT-X-MEDIA` entry and request the per-segment .vtt
    /// files in time-aligned with the variant's video / audio.
    /// Tier 4 item 4.5 session C wires this for the
    /// WhisperCaptionsAgent's English captions output.
    pub subtitles_group: Option<String>,
    /// URI to the variant's media playlist, relative to the master
    /// playlist. Rendered on the line immediately after the
    /// `#EXT-X-STREAM-INF` tag.
    pub uri: String,
}

/// A full master / multivariant playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterPlaylist {
    /// `#EXT-X-VERSION`. Defaults to 9 to match the media playlists
    /// that `PlaylistBuilder` emits.
    pub version: u8,
    /// `#EXT-X-MEDIA` entries.
    pub renditions: Vec<MediaRendition>,
    /// `#EXT-X-STREAM-INF` entries. At least one variant is
    /// required for the playlist to be useful, though the renderer
    /// will happily emit an empty master playlist if asked.
    pub variants: Vec<VariantStream>,
}

impl Default for MasterPlaylist {
    fn default() -> Self {
        Self {
            version: 9,
            renditions: Vec::new(),
            variants: Vec::new(),
        }
    }
}

impl MasterPlaylist {
    /// Render the master playlist as UTF-8 text, ready to serve as
    /// an `application/vnd.apple.mpegurl` response body.
    pub fn render(&self) -> String {
        let mut out = String::with_capacity(256 + self.variants.len() * 128 + self.renditions.len() * 128);
        let _ = writeln!(out, "#EXTM3U");
        let _ = writeln!(out, "#EXT-X-VERSION:{}", self.version);
        let _ = writeln!(out, "#EXT-X-INDEPENDENT-SEGMENTS");
        for media in &self.renditions {
            render_media(&mut out, media);
        }
        for variant in &self.variants {
            render_variant(&mut out, variant);
        }
        out
    }
}

fn render_media(out: &mut String, media: &MediaRendition) {
    let _ = write!(
        out,
        "#EXT-X-MEDIA:TYPE={},GROUP-ID=\"{}\",NAME=\"{}\",DEFAULT={},AUTOSELECT={}",
        media.rendition_type.as_str(),
        media.group_id,
        media.name,
        if media.default { "YES" } else { "NO" },
        if media.autoselect { "YES" } else { "NO" },
    );
    if let Some(lang) = &media.language {
        let _ = write!(out, ",LANGUAGE=\"{lang}\"");
    }
    let _ = write!(out, ",URI=\"{}\"", media.uri);
    out.push('\n');
}

fn render_variant(out: &mut String, variant: &VariantStream) {
    let _ = write!(
        out,
        "#EXT-X-STREAM-INF:BANDWIDTH={},CODECS=\"{}\"",
        variant.bandwidth_bps, variant.codecs
    );
    if let Some((w, h)) = variant.resolution {
        let _ = write!(out, ",RESOLUTION={w}x{h}");
    }
    if let Some(audio_group) = &variant.audio_group {
        let _ = write!(out, ",AUDIO=\"{audio_group}\"");
    }
    if let Some(subtitles_group) = &variant.subtitles_group {
        let _ = write!(out, ",SUBTITLES=\"{subtitles_group}\"");
    }
    out.push('\n');
    let _ = writeln!(out, "{}", variant.uri);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_audio_rendition() -> MediaRendition {
        MediaRendition {
            rendition_type: MediaRenditionType::Audio,
            group_id: "audio".into(),
            name: "default".into(),
            uri: "audio.m3u8".into(),
            default: true,
            autoselect: true,
            language: None,
        }
    }

    fn sample_variant_with_audio() -> VariantStream {
        VariantStream {
            bandwidth_bps: 2_500_000,
            codecs: "avc1.640020,mp4a.40.2".into(),
            resolution: Some((1280, 720)),
            audio_group: Some("audio".into()),
            subtitles_group: None,
            uri: "playlist.m3u8".into(),
        }
    }

    #[test]
    fn empty_master_still_renders_header() {
        let m = MasterPlaylist::default();
        let body = m.render();
        assert!(body.starts_with("#EXTM3U\n#EXT-X-VERSION:9"));
        assert!(body.contains("#EXT-X-INDEPENDENT-SEGMENTS"));
        assert!(!body.contains("#EXT-X-MEDIA"));
        assert!(!body.contains("#EXT-X-STREAM-INF"));
    }

    #[test]
    fn single_rendition_audio_group_renders_expected_lines() {
        let m = MasterPlaylist {
            version: 9,
            renditions: vec![sample_audio_rendition()],
            variants: vec![sample_variant_with_audio()],
        };
        let body = m.render();
        // Header.
        assert!(body.starts_with("#EXTM3U\n"), "body: {body}");
        assert!(body.contains("#EXT-X-VERSION:9"));
        assert!(body.contains("#EXT-X-INDEPENDENT-SEGMENTS"));
        // Audio rendition.
        assert!(
            body.contains("#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"default\",DEFAULT=YES,AUTOSELECT=YES,URI=\"audio.m3u8\""),
            "body missing audio media line: {body}"
        );
        // Variant with audio group reference.
        assert!(
            body.contains("#EXT-X-STREAM-INF:BANDWIDTH=2500000,CODECS=\"avc1.640020,mp4a.40.2\",RESOLUTION=1280x720,AUDIO=\"audio\""),
            "body missing variant stream line: {body}"
        );
        // Variant URI on the line immediately after the STREAM-INF tag.
        let stream_inf_pos = body.find("#EXT-X-STREAM-INF").unwrap();
        let rest = &body[stream_inf_pos..];
        let newline = rest.find('\n').unwrap();
        let next_line_start = stream_inf_pos + newline + 1;
        let next_line_end = body[next_line_start..].find('\n').unwrap() + next_line_start;
        assert_eq!(&body[next_line_start..next_line_end], "playlist.m3u8");
    }

    #[test]
    fn language_attribute_is_omitted_when_none() {
        let mut media = sample_audio_rendition();
        media.language = None;
        let m = MasterPlaylist {
            version: 9,
            renditions: vec![media],
            variants: vec![sample_variant_with_audio()],
        };
        let body = m.render();
        assert!(!body.contains("LANGUAGE="), "LANGUAGE should be absent: {body}");
    }

    #[test]
    fn language_attribute_is_present_when_set() {
        let mut media = sample_audio_rendition();
        media.language = Some("en".into());
        let m = MasterPlaylist {
            version: 9,
            renditions: vec![media],
            variants: vec![sample_variant_with_audio()],
        };
        let body = m.render();
        assert!(body.contains("LANGUAGE=\"en\""), "LANGUAGE should be present: {body}");
    }

    #[test]
    fn variant_without_audio_group_omits_audio_attribute() {
        let mut variant = sample_variant_with_audio();
        variant.audio_group = None;
        let m = MasterPlaylist {
            version: 9,
            renditions: vec![],
            variants: vec![variant],
        };
        let body = m.render();
        assert!(
            body.contains("#EXT-X-STREAM-INF:BANDWIDTH=2500000,CODECS=\"avc1.640020,mp4a.40.2\",RESOLUTION=1280x720\n"),
            "unexpected variant line: {body}"
        );
        assert!(!body.contains("AUDIO="));
    }

    #[test]
    fn variant_without_resolution_omits_resolution_attribute() {
        let mut variant = sample_variant_with_audio();
        variant.resolution = None;
        let m = MasterPlaylist {
            version: 9,
            renditions: vec![],
            variants: vec![variant],
        };
        let body = m.render();
        assert!(!body.contains("RESOLUTION="));
    }

    #[test]
    fn subtitles_rendition_renders_with_language_and_subtitles_group_on_variant() {
        let subs = MediaRendition {
            rendition_type: MediaRenditionType::Subtitles,
            group_id: "subs".into(),
            name: "English".into(),
            uri: "captions/playlist.m3u8".into(),
            default: true,
            autoselect: true,
            language: Some("en".into()),
        };
        let mut variant = sample_variant_with_audio();
        variant.subtitles_group = Some("subs".into());
        let m = MasterPlaylist {
            version: 9,
            renditions: vec![sample_audio_rendition(), subs],
            variants: vec![variant],
        };
        let body = m.render();
        assert!(
            body.contains(
                "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"English\",DEFAULT=YES,\
                 AUTOSELECT=YES,LANGUAGE=\"en\",URI=\"captions/playlist.m3u8\""
            ),
            "subtitles media line missing: {body}"
        );
        assert!(
            body.contains("AUDIO=\"audio\",SUBTITLES=\"subs\""),
            "variant should reference both audio + subtitles groups: {body}"
        );
    }
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MpdType {
    /// `type="dynamic"`. Live stream; the client polls the MPD for
    /// updates and follows the SegmentTemplate to the live edge.
    #[default]
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

/// `Default` exists so external embedders can absorb future
/// optional-field additions via the `..Default::default()` spread
/// pattern. The default values render syntactically-valid XML but
/// are placeholders: callers that want a usable template MUST set
/// `initialization`, `media`, `duration`, and `timescale`.
impl Default for SegmentTemplate {
    fn default() -> Self {
        Self {
            initialization: String::new(),
            media: String::new(),
            start_number: 1,
            duration: 0,
            timescale: 0,
        }
    }
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

/// Same `Default`-spread rationale as [`SegmentTemplate`]. Callers
/// that intend to render the result must set `id`, `codecs`, and
/// `bandwidth_bps` plus the dimensional / sampling-rate fields
/// appropriate for their track type.
impl Default for Representation {
    fn default() -> Self {
        Self {
            id: String::new(),
            codecs: String::new(),
            bandwidth_bps: 0,
            width: None,
            height: None,
            audio_sampling_rate: None,
        }
    }
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

/// Same `Default`-spread rationale. The defaults shape a video
/// `AdaptationSet` because that is the dominant LVQR case;
/// embedders adding an audio set should override `mime_type`,
/// `content_type`, and supply a `lang`.
impl Default for AdaptationSet {
    fn default() -> Self {
        Self {
            id: 0,
            mime_type: "video/mp4".into(),
            content_type: "video".into(),
            lang: None,
            representations: Vec::new(),
            segment_template: SegmentTemplate::default(),
        }
    }
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

/// SCTE-35 ad-marker scheme identifier per ANSI/SCTE 35-2024
/// section 12.2 / SCTE 214-1, the "XML+bin" variant used by
/// `<EventStream>` carriages that wrap base64-encoded
/// `splice_info_section` bytes inside a `<Signal><Binary>` body.
pub const SCTE35_SCHEME_ID: &str = "urn:scte:scte35:2014:xml+bin";

/// SCTE-35 Signal/Binary XML namespace per SCTE 35-2016.
pub const SCTE35_SIGNAL_NS: &str = "http://www.scte.org/schemas/35/2016";

/// One `<Event>` element inside an `<EventStream>` per ISO/IEC
/// 23009-1 section 5.10.4. LVQR uses Events to surface SCTE-35
/// splice events to DASH clients; the `body` field carries the
/// base64-encoded splice_info_section bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct DashEvent {
    /// `id` attribute. Carries the SCTE-35 splice_event_id when
    /// available, otherwise zero. Must be unique within the
    /// containing EventStream.
    pub id: u64,
    /// `presentationTime` attribute in the EventStream's timescale
    /// (90 kHz for SCTE-35). Absolute splice PTS from the
    /// publisher's section.
    pub presentation_time: u64,
    /// `duration` attribute in the EventStream's timescale.
    /// `None` (zero on the wire) when the publisher did not set
    /// `break_duration`.
    pub duration: Option<u64>,
    /// Element body. For `urn:scte:scte35:2014:xml+bin` LVQR
    /// renders `<Signal xmlns=".../35/2016"><Binary>BASE64</Binary>
    /// </Signal>` with the splice_info_section base64-encoded.
    pub binary_base64: String,
}

impl DashEvent {
    fn write(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent);
        let _ = write!(out, r#"{pad}<Event presentationTime="{}""#, self.presentation_time);
        if let Some(d) = self.duration {
            let _ = write!(out, r#" duration="{d}""#);
        }
        let _ = write!(out, r#" id="{}">"#, self.id);
        out.push('\n');
        let inner_pad = "  ".repeat(indent + 1);
        let _ = writeln!(out, r#"{inner_pad}<Signal xmlns="{ns}">"#, ns = SCTE35_SIGNAL_NS);
        let _ = writeln!(
            out,
            r#"{p}  <Binary>{b}</Binary>"#,
            p = inner_pad,
            b = esc(&self.binary_base64)
        );
        let _ = writeln!(out, r#"{inner_pad}</Signal>"#);
        let _ = writeln!(out, r#"{pad}</Event>"#);
    }
}

/// One `<EventStream>` element inside a Period per ISO/IEC
/// 23009-1 section 5.10.2. LVQR uses one EventStream per
/// SCTE-35 carriage; `scheme_id_uri` is `SCTE35_SCHEME_ID` for
/// the standard "xml+bin" variant.
#[derive(Debug, Clone, PartialEq)]
pub struct EventStream {
    /// `schemeIdUri` attribute. `SCTE35_SCHEME_ID` for SCTE-35.
    pub scheme_id_uri: String,
    /// `value` attribute. Optional per spec; LVQR omits when None.
    pub value: Option<String>,
    /// `timescale` attribute. 90000 for SCTE-35 (matches the PTS
    /// units in the wire payload).
    pub timescale: u32,
    /// Events to render inside this stream, in presentationTime
    /// order.
    pub events: Vec<DashEvent>,
}

impl EventStream {
    fn write(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent);
        let _ = write!(
            out,
            r#"{pad}<EventStream schemeIdUri="{s}" timescale="{ts}""#,
            s = esc(&self.scheme_id_uri),
            ts = self.timescale,
        );
        if let Some(v) = &self.value {
            let _ = write!(out, r#" value="{}""#, esc(v));
        }
        out.push_str(">\n");
        for event in &self.events {
            event.write(out, indent + 1);
        }
        let _ = writeln!(out, "{pad}</EventStream>");
    }
}

/// One Period inside the MPD. LVQR uses a single Period for the
/// entire live stream; multi-Period support is a future-session
/// addition when mid-stream codec changes land. Session 152 added
/// the `event_streams` field for SCTE-35 ad-marker passthrough at
/// Period level per ISO/IEC 23009-1 G.7.
#[derive(Debug, Clone, PartialEq)]
pub struct Period {
    /// `id` attribute. Unique within the MPD.
    pub id: String,
    /// `start` attribute as an ISO 8601 duration (e.g. `"PT0S"`).
    /// Live streams almost always start at `PT0S`.
    pub start: String,
    /// `<EventStream>` elements. Rendered BEFORE AdaptationSets
    /// per ISO/IEC 23009-1 ordering. Empty for streams that carry
    /// no ad markers.
    pub event_streams: Vec<EventStream>,
    /// AdaptationSets inside this Period.
    pub adaptation_sets: Vec<AdaptationSet>,
}

/// Same `Default`-spread rationale. `start="PT0S"` is the
/// almost-universal first-Period offset for live LVQR streams.
impl Default for Period {
    fn default() -> Self {
        Self {
            id: "0".into(),
            start: "PT0S".into(),
            event_streams: Vec::new(),
            adaptation_sets: Vec::new(),
        }
    }
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
        // Per ISO/IEC 23009-1 section 5.3.2.1 EventStream elements
        // come before AdaptationSet siblings inside a Period.
        for es in &self.event_streams {
            es.write(out, indent + 1);
        }
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
    /// `availabilityStartTime` attribute as milliseconds since the
    /// UNIX epoch. ISO/IEC 23009-1 §5.3.1.2 makes this REQUIRED on
    /// dynamic MPDs because the live segment timeline is anchored
    /// to it: a client computes "segment N is available at
    /// `availabilityStartTime + N * (duration/timescale)`". A
    /// dynamic MPD without this attribute is rejected by dash.js
    /// and Shaka with a clock-sync error. Captured ONCE at the
    /// moment the per-broadcast state first observed media (so the
    /// anchor stays constant across MPD re-renders within a
    /// session); `None` skips the attribute, which is appropriate
    /// for static (VOD) MPDs.
    pub availability_start_time_millis: Option<u64>,
    /// `publishTime` attribute as milliseconds since the UNIX
    /// epoch. ISO/IEC 23009-1 §5.3.1.2 says this SHOULD be set on
    /// dynamic MPDs to the wall-clock moment the MPD doc was
    /// generated. Updated on every render so a client can detect
    /// when the manifest has been re-published.
    pub publish_time_millis: Option<u64>,
    /// `timeShiftBufferDepth` attribute as DVR seconds. Matches
    /// the LL-HLS DVR window depth so a DASH client knows how far
    /// back into the live stream it is allowed to seek. `None`
    /// omits the attribute entirely, which the spec treats as
    /// "the publisher does not advertise a DVR window".
    pub time_shift_buffer_depth_secs: Option<u32>,
    /// `<UTCTiming>` descriptor. ISO/IEC 23009-1 §5.3.1.5 lets
    /// the server inline its current wall-clock via the
    /// `urn:mpeg:dash:utc:direct:2014` scheme so dash.js / Shaka
    /// can clock-sync without an external HTTP/NTP probe. The
    /// value is the same milliseconds-since-epoch the server
    /// stamps on `publishTime` (effectively "trust the server's
    /// clock"). `None` skips the descriptor.
    pub utc_timing_value_millis: Option<u64>,
    /// One or more Periods.
    pub periods: Vec<Period>,
}

/// `Default` lets external embedders absorb the four optional
/// timing fields (`availability_start_time_millis`,
/// `publish_time_millis`, `time_shift_buffer_depth_secs`,
/// `utc_timing_value_millis`) added in the C-3 fix via the
/// `..Default::default()` spread pattern, instead of every
/// downstream struct-literal site needing to be updated each time
/// a new optional field appears. The non-optional fields default
/// to LVQR's in-tree live-profile values.
impl Default for Mpd {
    fn default() -> Self {
        Self {
            mpd_type: MpdType::Dynamic,
            profiles: "urn:mpeg:dash:profile:isoff-live:2011".into(),
            min_buffer_time: "PT2.0S".into(),
            minimum_update_period: "PT2.0S".into(),
            availability_start_time_millis: None,
            publish_time_millis: None,
            time_shift_buffer_depth_secs: None,
            utc_timing_value_millis: None,
            periods: Vec::new(),
        }
    }
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
        let mup_attr = if self.minimum_update_period.is_empty() {
            String::new()
        } else {
            format!(r#" minimumUpdatePeriod="{}""#, esc(&self.minimum_update_period))
        };
        let ast_attr = self
            .availability_start_time_millis
            .map(|m| format!(r#" availabilityStartTime="{}""#, format_iso8601_utc(m)))
            .unwrap_or_default();
        let pt_attr = self
            .publish_time_millis
            .map(|m| format!(r#" publishTime="{}""#, format_iso8601_utc(m)))
            .unwrap_or_default();
        let tsbd_attr = self
            .time_shift_buffer_depth_secs
            .map(|s| format!(r#" timeShiftBufferDepth="PT{s}.000S""#))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="{ty}" profiles="{profiles}" minBufferTime="{mbt}"{mup}{ast}{pt}{tsbd}>"#,
            ty = self.mpd_type.as_str(),
            profiles = esc(&self.profiles),
            mbt = esc(&self.min_buffer_time),
            mup = mup_attr,
            ast = ast_attr,
            pt = pt_attr,
            tsbd = tsbd_attr,
        );
        for period in &self.periods {
            period.write(&mut out, 1)?;
        }
        // ISO/IEC 23009-1 §5.3.1.2 child-element ordering puts
        // `<UTCTiming>` AFTER the Period(s). Use the
        // `urn:mpeg:dash:utc:direct:2014` scheme to inline the
        // server's clock so a freshly-fetched MPD is
        // self-clocking; players that prefer an external HTTP/NTP
        // probe simply ignore the `direct` scheme and keep their
        // own time source.
        if let Some(value_ms) = self.utc_timing_value_millis {
            let _ = writeln!(
                out,
                r#"  <UTCTiming schemeIdUri="urn:mpeg:dash:utc:direct:2014" value="{}"/>"#,
                format_iso8601_utc(value_ms),
            );
        }
        out.push_str("</MPD>\n");
        Ok(out)
    }
}

/// Format milliseconds since the UNIX epoch as an ISO 8601 UTC
/// datetime (e.g. `"2026-04-30T12:34:56.789Z"`). DASH requires this
/// exact shape on `availabilityStartTime`, `publishTime`, and the
/// `urn:mpeg:dash:utc:direct:2014` UTCTiming value. Implemented
/// against Howard Hinnant's civil_from_days algorithm so lvqr-dash
/// stays free of a chrono / time crate dependency. Mirrors the
/// `format_program_date_time` helper inside `lvqr-hls::manifest`;
/// kept as a private helper here rather than factored into a shared
/// crate because the two callsites are self-contained and the
/// algorithm is small enough that a shared module would not pull
/// its weight.
fn format_iso8601_utc(epoch_millis: u64) -> String {
    let total_secs = (epoch_millis / 1000) as i64;
    let millis = epoch_millis % 1000;
    let day_secs = total_secs.rem_euclid(86400) as u32;
    let h = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    let z = total_secs.div_euclid(86400) + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}.{millis:03}Z")
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
            availability_start_time_millis: None,
            publish_time_millis: None,
            time_shift_buffer_depth_secs: None,
            utc_timing_value_millis: None,
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
            availability_start_time_millis: None,
            publish_time_millis: None,
            time_shift_buffer_depth_secs: None,
            utc_timing_value_millis: None,
            periods: vec![Period {
                id: "0".into(),
                start: "PT0S".into(),
                event_streams: Vec::new(),
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
            availability_start_time_millis: None,
            publish_time_millis: None,
            time_shift_buffer_depth_secs: None,
            utc_timing_value_millis: None,
            periods: Vec::new(),
        };
        assert!(matches!(mpd.render(), Err(DashError::EmptyPeriod)));
    }

    #[test]
    fn render_emits_availability_start_time_publish_time_and_utc_timing() {
        // ISO/IEC 23009-1 §5.3.1.2 + §5.3.1.5: a dynamic MPD with
        // availabilityStartTime + publishTime + a UTCTiming(direct)
        // descriptor renders all three in the canonical
        // ISO 8601 UTC-millis-Z form. dash.js + Shaka rely on these
        // for clock sync; the timeShiftBufferDepth attribute is the
        // DVR-window equivalent of LL-HLS `EXT-X-TARGETDURATION *
        // max_segments` and tells the player how far back into the
        // live stream a seek is allowed to go.
        //
        // The expected ISO 8601 strings are derived by re-running
        // the same `format_iso8601_utc` helper the renderer uses,
        // so the test stays decoupled from any specific calendar
        // arithmetic and survives a future leap-day fix without a
        // brittle hand-computed assertion.
        let mut mpd = live_mpd_with_video();
        let ast = 1_777_983_296_789u64;
        let pub_t = ast + 1_500;
        mpd.availability_start_time_millis = Some(ast);
        mpd.publish_time_millis = Some(pub_t);
        mpd.time_shift_buffer_depth_secs = Some(60);
        mpd.utc_timing_value_millis = Some(pub_t);
        let xml = mpd.render().expect("render dynamic mpd with timing");
        let ast_iso = format_iso8601_utc(ast);
        let pub_iso = format_iso8601_utc(pub_t);
        assert!(
            xml.contains(&format!(r#"availabilityStartTime="{ast_iso}""#)),
            "expected ISO 8601 availabilityStartTime; got:\n{xml}"
        );
        assert!(
            xml.contains(&format!(r#"publishTime="{pub_iso}""#)),
            "expected ISO 8601 publishTime; got:\n{xml}"
        );
        assert!(
            xml.contains(r#"timeShiftBufferDepth="PT60.000S""#),
            "expected DVR window attribute; got:\n{xml}"
        );
        assert!(
            xml.contains(&format!(
                r#"<UTCTiming schemeIdUri="urn:mpeg:dash:utc:direct:2014" value="{pub_iso}"/>"#
            )),
            "expected UTCTiming(direct) descriptor; got:\n{xml}"
        );
        // §5.3.1 child-element ordering: UTCTiming sits AFTER the
        // Period element.
        let period_close = xml.find("</Period>").expect("period close");
        let utc_timing = xml.find("<UTCTiming").expect("utc timing present");
        assert!(period_close < utc_timing, "UTCTiming must appear after Period");
    }

    #[test]
    fn render_omits_timing_attrs_when_unset() {
        // Backwards-compat: the four new fields default to None and
        // produce no attributes / descriptors so existing tests +
        // any embedder that builds an Mpd by hand against the
        // pre-C-3 shape keeps the same output (no spurious
        // attributes that break literal-XML diff tests).
        let mpd = live_mpd_with_video();
        let xml = mpd.render().expect("render");
        assert!(!xml.contains("availabilityStartTime"));
        assert!(!xml.contains("publishTime"));
        assert!(!xml.contains("timeShiftBufferDepth"));
        assert!(!xml.contains("UTCTiming"));
    }

    #[test]
    fn format_iso8601_utc_known_epoch() {
        // UNIX epoch + millisecond precision baseline. A regression
        // in the civil_from_days arithmetic would silently shift
        // every live MPD's anchor by a day or month, which dash.js
        // would surface as "segment timeline drifted" warnings; the
        // baseline assertions below pin the algorithm against
        // Hinnant's original test vectors.
        assert_eq!(format_iso8601_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_iso8601_utc(1_234), "1970-01-01T00:00:01.234Z");
        // 2024-01-01T00:00:00.000Z = 1704067200000 ms (the most
        // recent leap-year boundary at the time of writing). The
        // civil-from-days implementation handles 2024 as a leap
        // year correctly: Jan 1 has YOY day-of-year 0, so the result
        // is 2024-01-01 not 2024-01-02.
        assert_eq!(format_iso8601_utc(1_704_067_200_000), "2024-01-01T00:00:00.000Z");
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
    fn event_stream_renders_with_period_level_signal_xml_bin() {
        let mut mpd = live_mpd_with_video();
        mpd.periods[0].event_streams.push(EventStream {
            scheme_id_uri: SCTE35_SCHEME_ID.into(),
            value: None,
            timescale: 90_000,
            events: vec![DashEvent {
                id: 1234567,
                presentation_time: 8_100_000,
                duration: Some(2_700_000),
                binary_base64: "/DAvAAAAAAAAAA/wBQb+ABAA".into(),
            }],
        });
        let xml = mpd.render().expect("render");
        assert!(
            xml.contains(r#"<EventStream schemeIdUri="urn:scte:scte35:2014:xml+bin" timescale="90000">"#),
            "{xml}"
        );
        assert!(
            xml.contains(r#"<Event presentationTime="8100000" duration="2700000" id="1234567">"#),
            "{xml}"
        );
        assert!(
            xml.contains(r#"<Signal xmlns="http://www.scte.org/schemas/35/2016">"#),
            "{xml}"
        );
        assert!(xml.contains("<Binary>/DAvAAAAAAAAAA/wBQb+ABAA</Binary>"), "{xml}");
    }

    #[test]
    fn event_stream_renders_before_adaptation_set_per_iso_23009_1() {
        // Per ISO/IEC 23009-1 section 5.3.2.1 EventStream must appear
        // before AdaptationSet siblings inside a Period; Shaka and
        // dash.js both rely on the documented order.
        let mut mpd = live_mpd_with_video();
        mpd.periods[0].event_streams.push(EventStream {
            scheme_id_uri: SCTE35_SCHEME_ID.into(),
            value: None,
            timescale: 90_000,
            events: vec![DashEvent {
                id: 1,
                presentation_time: 0,
                duration: None,
                binary_base64: "ABCD".into(),
            }],
        });
        let xml = mpd.render().expect("render");
        let es_pos = xml.find("<EventStream ").expect("EventStream present");
        let as_pos = xml.find("<AdaptationSet ").expect("AdaptationSet present");
        assert!(es_pos < as_pos, "EventStream must precede AdaptationSet:\n{xml}");
    }

    #[test]
    fn event_stream_omits_duration_when_none() {
        let mut mpd = live_mpd_with_video();
        mpd.periods[0].event_streams.push(EventStream {
            scheme_id_uri: SCTE35_SCHEME_ID.into(),
            value: None,
            timescale: 90_000,
            events: vec![DashEvent {
                id: 7,
                presentation_time: 100,
                duration: None,
                binary_base64: "AA==".into(),
            }],
        });
        let xml = mpd.render().expect("render");
        assert!(
            xml.contains(r#"<Event presentationTime="100" id="7">"#),
            "duration must be omitted from the wire when None:\n{xml}"
        );
        assert!(!xml.contains("duration=\"\""), "no empty duration attribute");
    }

    #[test]
    fn static_type_renders_static_attribute() {
        let mut mpd = live_mpd_with_video();
        mpd.mpd_type = MpdType::Static;
        let xml = mpd.render().expect("render");
        assert!(xml.contains(r#"type="static""#));
        assert!(!xml.contains(r#"type="dynamic""#));
    }

    /// Locks the `..Default::default()` spread pattern external
    /// embedders use after the C-3 fix added four optional timing
    /// fields. A 1.0.0 consumer can express the pre-C-3 shape as
    /// `Mpd { periods, ..Default::default() }` and stay forwards-
    /// compatible with future optional-field additions without
    /// every struct-literal site having to be updated each time.
    #[test]
    fn default_spread_lets_embedder_supply_only_meaningful_fields() {
        let segment_template = SegmentTemplate {
            initialization: "init-video.m4s".into(),
            media: "seg-video-$Number$.m4s".into(),
            duration: 180_000,
            timescale: 90_000,
            ..Default::default()
        };
        let representation = Representation {
            id: "video".into(),
            codecs: "avc1.42001F".into(),
            bandwidth_bps: 2_500_000,
            width: Some(1280),
            height: Some(720),
            ..Default::default()
        };
        let adaptation_set = AdaptationSet {
            representations: vec![representation],
            segment_template,
            ..Default::default()
        };
        let period = Period {
            adaptation_sets: vec![adaptation_set],
            ..Default::default()
        };
        let mpd = Mpd {
            periods: vec![period],
            ..Default::default()
        };
        let xml = mpd.render().expect("default-spread mpd should render");
        assert!(xml.contains(r#"type="dynamic""#));
        assert!(xml.contains(r#"profiles="urn:mpeg:dash:profile:isoff-live:2011""#));
        assert!(xml.contains(r#"minBufferTime="PT2.0S""#));
        assert!(xml.contains(r#"<Period id="0" start="PT0S">"#));
        assert!(xml.contains(r#"<AdaptationSet id="0" mimeType="video/mp4""#));
        assert!(xml.contains(r#"<Representation id="video" codecs="avc1.42001F""#));
        // The four optional timing fields default to None and stay
        // off the wire so a Default-spread MPD matches the pre-C-3
        // shape byte-for-byte.
        assert!(!xml.contains("availabilityStartTime"));
        assert!(!xml.contains("publishTime"));
        assert!(!xml.contains("timeShiftBufferDepth"));
        assert!(!xml.contains("UTCTiming"));
    }
}

//! HLS WebVTT subtitle rendition surface.
//!
//! **Tier 4 item 4.5 session C.** This module owns the
//! per-broadcast subtitles state that
//! [`crate::MultiHlsServer`] mounts under
//! `/hls/{broadcast}/captions/...`. It is the consumer side
//! of the captions track that
//! [`lvqr_agent_whisper::WhisperCaptionsAgent`] produces
//! into the shared `lvqr_fragment::FragmentBroadcasterRegistry`
//! under track id `"captions"`.
//!
//! ## Wire shape (session 99 C, intentionally minimal)
//!
//! * **One cue = one HLS segment.** Each
//!   `lvqr_agent_whisper::TranscribedCaption` becomes a
//!   single `.vtt` segment file. The captions playlist gains
//!   one `#EXTINF:duration,\nseg-NN.vtt` entry per cue. The
//!   target-duration is the maximum cue duration ever seen,
//!   bumped on demand.
//! * **PROGRAM-DATE-TIME alignment.** The captions playlist
//!   carries an `#EXT-X-PROGRAM-DATE-TIME` tag stamped at
//!   the same wall-clock anchor as the audio + video
//!   renditions, so a hls.js / Safari player aligns subtitle
//!   cues against the live PDT axis without needing
//!   `X-TIMESTAMP-MAP` inside the .vtt body.
//! * **No LL-HLS partials.** Subtitles are text-only and
//!   small; partials add complexity without latency benefit.
//!   The renditions emit standard HLS playlists
//!   (`#EXT-X-VERSION:9` to match the rest of the surface).
//! * **No fMP4 wrapper.** WebVTT subtitle renditions in HLS
//!   are served as plain `.vtt` files. The captions playlist
//!   does NOT reference an `EXT-X-MAP`.
//!
//! ## Late subscriber semantics
//!
//! A viewer who joins the broadcast mid-stream sees only
//! cues emitted from the moment of join onwards, mirroring
//! the audio + video LL-HLS sliding-window behaviour. The
//! captions playlist drops cues older than
//! [`SubtitlesServer::max_cues`] (default 50) so the
//! playlist stays bounded.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;

/// Default maximum number of cues retained in the captions
/// playlist's sliding window.
pub const DEFAULT_MAX_CUES: usize = 50;

/// Default minimum target-duration emitted on the captions
/// playlist when no cue has been seen yet (or all cues are
/// shorter). Keeps short cues from advertising a microscopic
/// target-duration that some clients reject.
pub const DEFAULT_MIN_TARGET_DURATION_SECS: u64 = 6;

/// One WebVTT cue with absolute wall-clock timing.
///
/// `start_ms` / `end_ms` are wall-clock milliseconds since
/// the epoch. The producer (the captions adapter in
/// `lvqr-agent-whisper`) computes them by adding a per-segment
/// offset against the broadcast's start PDT; this module
/// renders them straight into `HH:MM:SS.mmm` cue timestamps
/// relative to the segment start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionCue {
    /// Cue start (wall-clock ms since epoch).
    pub start_ms: u64,
    /// Cue end (wall-clock ms since epoch).
    pub end_ms: u64,
    /// Caption text. UTF-8, single line. Multi-line cues are
    /// supported by inserting `\n` characters; the renderer
    /// passes them through verbatim.
    pub text: String,
}

impl CaptionCue {
    /// Cue duration in milliseconds, saturating at zero.
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }

    /// Cue duration in seconds (rounded up to the next whole
    /// second, minimum 1). Used for the playlist's
    /// `#EXT-X-TARGETDURATION` and per-segment `#EXTINF`.
    pub fn duration_secs_ceil(&self) -> u64 {
        let ms = self.duration_ms();
        if ms == 0 { 1 } else { ms.div_ceil(1000) }
    }
}

/// Internal segment record: one cue + the URI fragment used
/// to address it inside the captions playlist.
#[derive(Debug, Clone)]
struct CaptionSegment {
    media_sequence: u64,
    cue: CaptionCue,
}

impl CaptionSegment {
    fn uri(&self) -> String {
        format!("seg-{}.vtt", self.media_sequence)
    }
}

/// Per-broadcast captions state: the rolling cue window plus
/// the captions playlist + per-segment .vtt body renderer.
///
/// Cheap to clone (internal state behind an `Arc<RwLock<..>>`).
/// `MultiHlsServer` constructs one per broadcast on the first
/// `ensure_subtitles` call.
#[derive(Clone)]
pub struct SubtitlesServer {
    inner: Arc<RwLock<SubtitlesState>>,
}

impl std::fmt::Debug for SubtitlesServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.inner.read().expect("subtitles state lock poisoned");
        f.debug_struct("SubtitlesServer")
            .field("cue_count", &s.window.len())
            .field("media_sequence_base", &s.media_sequence_base)
            .field("max_cues", &s.max_cues)
            .field("target_duration_secs", &s.target_duration_secs)
            .finish()
    }
}

#[derive(Debug)]
struct SubtitlesState {
    /// Rolling window of cues; oldest at the front.
    window: VecDeque<CaptionSegment>,
    /// Maximum cues retained in the window.
    max_cues: usize,
    /// Sequence number of the oldest cue currently in the
    /// window. Bumped each time the window evicts a cue.
    media_sequence_base: u64,
    /// Sequence number of the next cue to be ingested. Always
    /// `>= media_sequence_base + window.len()`; equal under
    /// normal operation.
    next_media_sequence: u64,
    /// Largest cue duration (seconds, ceil) ever seen, clamped
    /// to >= [`DEFAULT_MIN_TARGET_DURATION_SECS`].
    target_duration_secs: u64,
    /// Whether `finalize` has been called. After finalize the
    /// playlist gains `#EXT-X-ENDLIST` and the cue ingest path
    /// is closed.
    ended: bool,
}

impl SubtitlesServer {
    /// Build a new server with the default [`DEFAULT_MAX_CUES`]
    /// retention.
    pub fn new() -> Self {
        Self::with_max_cues(DEFAULT_MAX_CUES)
    }

    /// Build a new server retaining at most `max_cues` cues in
    /// its sliding window.
    pub fn with_max_cues(max_cues: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(SubtitlesState {
                window: VecDeque::with_capacity(max_cues.max(1)),
                max_cues: max_cues.max(1),
                media_sequence_base: 0,
                next_media_sequence: 0,
                target_duration_secs: DEFAULT_MIN_TARGET_DURATION_SECS,
                ended: false,
            })),
        }
    }

    /// Append a cue. Bumps the target-duration if the cue is
    /// longer than any prior cue. Drops the oldest cue when
    /// the window is full.
    pub fn push_cue(&self, cue: CaptionCue) {
        let mut state = self.inner.write().expect("subtitles state lock poisoned");
        if state.ended {
            tracing::debug!(text = %cue.text, "SubtitlesServer: push after finalize ignored");
            return;
        }
        let dur_secs = cue.duration_secs_ceil();
        if dur_secs > state.target_duration_secs {
            state.target_duration_secs = dur_secs;
        }
        let media_sequence = state.next_media_sequence;
        state.next_media_sequence += 1;
        let segment = CaptionSegment { media_sequence, cue };
        state.window.push_back(segment);
        let max_cues = state.max_cues;
        while state.window.len() > max_cues {
            state.window.pop_front();
            state.media_sequence_base += 1;
        }
    }

    /// Mark the captions stream as ended. Subsequent
    /// [`SubtitlesServer::push_cue`] calls are silently
    /// ignored, and the next [`SubtitlesServer::render_playlist`]
    /// emits `#EXT-X-ENDLIST`.
    pub fn finalize(&self) {
        let mut state = self.inner.write().expect("subtitles state lock poisoned");
        state.ended = true;
    }

    /// Number of cues currently in the sliding window.
    pub fn cue_count(&self) -> usize {
        self.inner.read().expect("subtitles state lock poisoned").window.len()
    }

    /// Render the captions playlist as UTF-8 text. Returns an
    /// empty playlist (header + zero EXTINF entries) when no
    /// cues have been seen yet.
    pub fn render_playlist(&self) -> String {
        use std::fmt::Write as _;
        let state = self.inner.read().expect("subtitles state lock poisoned");
        let mut out = String::with_capacity(256 + state.window.len() * 64);
        let _ = writeln!(out, "#EXTM3U");
        let _ = writeln!(out, "#EXT-X-VERSION:9");
        let _ = writeln!(out, "#EXT-X-TARGETDURATION:{}", state.target_duration_secs);
        let _ = writeln!(out, "#EXT-X-MEDIA-SEQUENCE:{}", state.media_sequence_base);
        for segment in &state.window {
            let dur = segment.cue.duration_secs_ceil();
            let _ = writeln!(
                out,
                "#EXT-X-PROGRAM-DATE-TIME:{}",
                format_iso8601_utc(segment.cue.start_ms)
            );
            let _ = writeln!(out, "#EXTINF:{:.3},", dur as f64);
            let _ = writeln!(out, "{}", segment.uri());
        }
        if state.ended {
            let _ = writeln!(out, "#EXT-X-ENDLIST");
        }
        out
    }

    /// Render a single .vtt segment by its media sequence
    /// number. Returns `None` when the requested cue has been
    /// evicted from the window.
    pub fn render_segment(&self, media_sequence: u64) -> Option<Bytes> {
        let state = self.inner.read().expect("subtitles state lock poisoned");
        let segment = state.window.iter().find(|s| s.media_sequence == media_sequence)?;
        Some(render_webvtt_body(&segment.cue))
    }

    /// Look up a segment URI -> media sequence number.
    /// `seg-NN.vtt` -> `Some(NN)`.
    pub fn parse_segment_uri(uri: &str) -> Option<u64> {
        let stem = uri.strip_prefix("seg-")?.strip_suffix(".vtt")?;
        stem.parse().ok()
    }
}

impl Default for SubtitlesServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Render a single cue as a WebVTT segment body.
///
/// Format:
///
/// ```text
/// WEBVTT
///
/// 00:00:00.000 --> 00:00:05.000
/// hello world
/// ```
///
/// Cue timestamps are zero-anchored within the segment so
/// hls.js can place the cue at its segment's start time
/// (which the captions playlist's `#EXT-X-PROGRAM-DATE-TIME`
/// stamps onto wall-clock).
fn render_webvtt_body(cue: &CaptionCue) -> Bytes {
    let dur_ms = cue.duration_ms();
    let mut body = String::with_capacity(64 + cue.text.len());
    body.push_str("WEBVTT\n\n");
    body.push_str(&format_webvtt_timestamp(0));
    body.push_str(" --> ");
    body.push_str(&format_webvtt_timestamp(dur_ms));
    body.push('\n');
    body.push_str(&cue.text);
    body.push('\n');
    Bytes::from(body)
}

/// Render `ms` as `HH:MM:SS.mmm` per the WebVTT cue-timestamp
/// format. Saturates to `99:59:59.999` for inputs that would
/// overflow the 100-hour cap (well past anything a live
/// captions stream produces).
fn format_webvtt_timestamp(ms: u64) -> String {
    let total_seconds = ms / 1000;
    let millis = (ms % 1000) as u32;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours >= 100 {
        return "99:59:59.999".to_string();
    }
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

/// Render `ms` (since UNIX epoch) as an RFC 3339 / ISO 8601
/// UTC timestamp, e.g. `"2026-04-21T03:30:15.250Z"`. Used for
/// the `#EXT-X-PROGRAM-DATE-TIME` tag on each segment.
///
/// Hand-rolled because `chrono` and `time` are not workspace
/// deps; the audio + video renditions also format PDT
/// timestamps by hand inside `lvqr-hls::manifest`. Keeping
/// the same approach here avoids a new transitive dep.
fn format_iso8601_utc(ms_since_epoch: u64) -> String {
    let dur = std::time::Duration::from_millis(ms_since_epoch);
    let total_secs = dur.as_secs();
    let millis = (dur.subsec_millis()) as u64;

    // Days since 1970-01-01.
    let days = total_secs / 86_400;
    let secs_of_day = total_secs % 86_400;
    let hours = secs_of_day / 3600;
    let minutes = (secs_of_day % 3600) / 60;
    let seconds = secs_of_day % 60;

    let (year, month, day) = days_since_epoch_to_ymd(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z")
}

/// Convert "days since 1970-01-01" to `(year, month, day)`
/// using the proleptic Gregorian calendar. Algorithm adapted
/// from Howard Hinnant's "date" library (public domain).
fn days_since_epoch_to_ymd(days: i64) -> (i64, u32, u32) {
    // Shift epoch to 0000-03-01 so leap-year math is uniform.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m, d)
}

/// Snapshot the current wall-clock UTC time as milliseconds
/// since the UNIX epoch. Used by callers that want to seed a
/// cue's `start_ms` from "now". Cheap; no monotonic-clock
/// trickery because the captions playlist is wall-clock
/// aligned anyway.
pub fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start_ms: u64, dur_ms: u64, text: &str) -> CaptionCue {
        CaptionCue {
            start_ms,
            end_ms: start_ms + dur_ms,
            text: text.into(),
        }
    }

    #[test]
    fn cue_duration_handles_inverted_bounds_safely() {
        let inverted = CaptionCue {
            start_ms: 1000,
            end_ms: 500,
            text: "oops".into(),
        };
        assert_eq!(inverted.duration_ms(), 0);
        assert_eq!(inverted.duration_secs_ceil(), 1, "duration_secs_ceil floors at 1");
    }

    #[test]
    fn webvtt_timestamp_zero() {
        assert_eq!(format_webvtt_timestamp(0), "00:00:00.000");
    }

    #[test]
    fn webvtt_timestamp_milliseconds_only() {
        assert_eq!(format_webvtt_timestamp(250), "00:00:00.250");
    }

    #[test]
    fn webvtt_timestamp_hours_minutes_seconds() {
        // 1h 23m 45.678s = (3600 + 1380 + 45) * 1000 + 678 ms.
        let ms = (3_600 + 23 * 60 + 45) * 1000 + 678;
        assert_eq!(format_webvtt_timestamp(ms), "01:23:45.678");
    }

    #[test]
    fn webvtt_timestamp_caps_at_99_hours() {
        let huge = 200 * 3600 * 1000;
        assert_eq!(format_webvtt_timestamp(huge), "99:59:59.999");
    }

    #[test]
    fn render_webvtt_body_emits_header_and_cue() {
        let body = render_webvtt_body(&cue(0, 5_000, "hello world"));
        let text = std::str::from_utf8(&body).unwrap();
        assert!(text.starts_with("WEBVTT\n\n"), "body: {text}");
        assert!(text.contains("00:00:00.000 --> 00:00:05.000"), "body: {text}");
        assert!(text.contains("hello world"), "body: {text}");
    }

    #[test]
    fn iso8601_renders_known_epoch_anchor() {
        // 1970-01-01T00:00:00.000Z
        assert_eq!(format_iso8601_utc(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn iso8601_renders_a_known_recent_date() {
        // 2026-04-20 = 56 years (14 leap days: 1972, 1976, ..., 2024)
        // = 20440 + 14 = 20454 days from 1970-01-01 to 2026-01-01,
        // plus 90 days (jan 31 + feb 28 + mar 31) + 19 = 109 days
        // into 2026 to land on April 20. Total: 20563 days.
        let days = 20_563u64;
        let ms = days * 86_400_000;
        assert_eq!(format_iso8601_utc(ms), "2026-04-20T00:00:00.000Z");
    }

    #[test]
    fn iso8601_includes_milliseconds() {
        let days = 20_563u64;
        let ms = days * 86_400_000 + 1500;
        assert_eq!(format_iso8601_utc(ms), "2026-04-20T00:00:01.500Z");
    }

    #[test]
    fn empty_server_renders_header_only_playlist() {
        let s = SubtitlesServer::new();
        let pl = s.render_playlist();
        assert!(pl.contains("#EXTM3U"));
        assert!(pl.contains("#EXT-X-VERSION:9"));
        assert!(
            pl.contains(&format!("#EXT-X-TARGETDURATION:{DEFAULT_MIN_TARGET_DURATION_SECS}")),
            "playlist: {pl}"
        );
        assert!(pl.contains("#EXT-X-MEDIA-SEQUENCE:0"));
        assert!(!pl.contains("#EXTINF"), "no cues yet: {pl}");
        assert!(!pl.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn push_cue_appears_in_playlist_and_segment() {
        let s = SubtitlesServer::new();
        s.push_cue(cue(1_000_000, 4_500, "first cue"));
        let pl = s.render_playlist();
        assert!(pl.contains("#EXTINF:5.000,"), "playlist: {pl}");
        assert!(pl.contains("seg-0.vtt"), "playlist: {pl}");

        let body = s.render_segment(0).expect("segment 0 present");
        let text = std::str::from_utf8(&body).unwrap();
        assert!(text.contains("first cue"), "body: {text}");
    }

    #[test]
    fn target_duration_grows_with_largest_cue() {
        let s = SubtitlesServer::new();
        s.push_cue(cue(0, 12_500, "long cue")); // 13s ceil
        let pl = s.render_playlist();
        assert!(pl.contains("#EXT-X-TARGETDURATION:13"), "playlist: {pl}");
    }

    #[test]
    fn sliding_window_evicts_oldest_cue_and_bumps_media_sequence() {
        let s = SubtitlesServer::with_max_cues(2);
        s.push_cue(cue(0, 1_000, "a"));
        s.push_cue(cue(1_000, 1_000, "b"));
        s.push_cue(cue(2_000, 1_000, "c"));
        assert_eq!(s.cue_count(), 2);
        let pl = s.render_playlist();
        assert!(pl.contains("#EXT-X-MEDIA-SEQUENCE:1"), "playlist: {pl}");
        assert!(!pl.contains("seg-0.vtt"), "evicted cue must be gone: {pl}");
        assert!(pl.contains("seg-1.vtt"));
        assert!(pl.contains("seg-2.vtt"));
    }

    #[test]
    fn finalize_appends_endlist_and_blocks_further_pushes() {
        let s = SubtitlesServer::new();
        s.push_cue(cue(0, 1_000, "a"));
        s.finalize();
        let pl = s.render_playlist();
        assert!(pl.contains("#EXT-X-ENDLIST"), "playlist: {pl}");
        s.push_cue(cue(2_000, 1_000, "ignored"));
        assert_eq!(s.cue_count(), 1, "post-finalize push silently dropped");
    }

    #[test]
    fn render_segment_returns_none_for_unknown_sequence() {
        let s = SubtitlesServer::new();
        s.push_cue(cue(0, 1_000, "a"));
        assert!(s.render_segment(999).is_none());
    }

    #[test]
    fn parse_segment_uri_round_trip() {
        assert_eq!(SubtitlesServer::parse_segment_uri("seg-0.vtt"), Some(0));
        assert_eq!(SubtitlesServer::parse_segment_uri("seg-42.vtt"), Some(42));
        assert!(SubtitlesServer::parse_segment_uri("seg-0.txt").is_none());
        assert!(SubtitlesServer::parse_segment_uri("foo.vtt").is_none());
        assert!(SubtitlesServer::parse_segment_uri("").is_none());
    }
}

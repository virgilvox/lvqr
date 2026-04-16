//! HTTP server surface for the MPEG-DASH egress.
//!
//! Companion to [`crate::mpd`]: the MPD renderer is a pure function
//! over typed input, and this module is the state machine that holds
//! the per-broadcast init + segment cache and projects it into an
//! `axum::Router`. The shape deliberately mirrors
//! `lvqr-hls::server`:
//!
//! * [`DashServer`] owns one broadcast's video + audio state,
//!   exposes `push_*` producer methods for the bridge, and serves
//!   four routes: `/manifest.mpd`, `/init-video.m4s`,
//!   `/init-audio.m4s`, and `/seg-<track>-<n>.m4s`.
//! * [`MultiDashServer`] fans per-broadcast [`DashServer`] instances
//!   behind a single `/dash/{broadcast}/...` catch-all router so
//!   `lvqr-cli` can mount one axum server for every live publisher.
//!
//! Segment numbering follows the DASH live profile: each track has a
//! monotonic counter the bridge stamps onto every pushed fragment
//! (see `crate::bridge::DashFragmentBridge`). The MPD's
//! `SegmentTemplate` uses `$Number$` addressing with `startNumber=1`,
//! so a client resolves `seg-video-1.m4s`, `seg-video-2.m4s`, ...
//! in order from the first produced fragment.
//!
//! Codec strings come from `lvqr_cmaf::detect_video_codec_string` /
//! `detect_audio_codec_string` at init-segment push time so H.264,
//! HEVC, AAC, and Opus publishers all populate a correct `codecs`
//! attribute without any DASH-specific detection. Before an init
//! segment arrives the MPD falls back to a conservative
//! `avc1.640020` / `mp4a.40.2` pair so a client polling the manifest
//! early never sees an empty `codecs=""` attribute.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use bytes::Bytes;
use lvqr_cmaf::{detect_audio_codec_string, detect_video_codec_string};

use crate::mpd::{AdaptationSet, Mpd, MpdType, Period, Representation, SegmentTemplate};

/// Default base path used when mounting a [`MultiDashServer`] router.
const MULTI_DASH_PREFIX: &str = "/dash";

/// Fallback video codec string used in the rendered MPD before an
/// init segment has arrived. Mirrors the LL-HLS master-playlist
/// handler's `avc1.640020` default so the two egresses never
/// diverge on the pre-init fallback.
const FALLBACK_VIDEO_CODEC: &str = "avc1.640020";

/// Fallback audio codec string used in the rendered MPD before an
/// audio init segment has arrived.
const FALLBACK_AUDIO_CODEC: &str = "mp4a.40.2";

/// Static configuration for a single [`DashServer`].
///
/// The fields mirror the attributes the MPD renderer needs on every
/// `SegmentTemplate` / `Representation`. Every value has a
/// conservative default so tests and the CLI wiring can accept
/// [`DashConfig::default`] and still render a syntactically valid
/// manifest against any publisher.
#[derive(Debug, Clone)]
pub struct DashConfig {
    /// `minBufferTime` attribute on the MPD root element.
    pub min_buffer_time: String,
    /// `minimumUpdatePeriod` attribute. Matches the segment cadence.
    pub minimum_update_period: String,
    /// Estimated video bitrate in bits per second.
    pub video_bandwidth_bps: u32,
    /// Estimated audio bitrate in bits per second.
    pub audio_bandwidth_bps: u32,
    /// Video track timescale in Hz (90_000 for LVQR's video path).
    pub video_timescale: u32,
    /// Nominal video segment duration in `video_timescale` ticks.
    pub video_segment_duration: u64,
    /// Audio track timescale in Hz (44_100 / 48_000 depending on
    /// codec). The default matches the Opus path; the bridge
    /// overrides this on the fly when AAC 44.1 kHz publishes.
    pub audio_timescale: u32,
    /// Nominal audio segment duration in `audio_timescale` ticks.
    pub audio_segment_duration: u64,
    /// Video Representation `width` attribute.
    pub video_width: u32,
    /// Video Representation `height` attribute.
    pub video_height: u32,
}

impl Default for DashConfig {
    fn default() -> Self {
        Self {
            min_buffer_time: "PT2.0S".into(),
            minimum_update_period: "PT2.0S".into(),
            video_bandwidth_bps: 2_500_000,
            audio_bandwidth_bps: 128_000,
            video_timescale: 90_000,
            video_segment_duration: 180_000,
            audio_timescale: 48_000,
            audio_segment_duration: 96_000,
            video_width: 1280,
            video_height: 720,
        }
    }
}

/// Per-track cache: init segment, latest codec string, and a map
/// of sequence number to segment bytes.
#[derive(Debug)]
struct TrackState {
    init: Option<Bytes>,
    codec: Option<String>,
    segments: HashMap<u64, Bytes>,
    latest_seq: u64,
    any_segment: bool,
}

impl TrackState {
    fn new() -> Self {
        Self {
            init: None,
            codec: None,
            segments: HashMap::new(),
            latest_seq: 0,
            any_segment: false,
        }
    }
}

/// Shared inner state of a [`DashServer`].
#[derive(Debug)]
struct DashState {
    config: DashConfig,
    video: Mutex<TrackState>,
    audio: Mutex<TrackState>,
    /// Set by [`DashServer::finalize`] when the broadcast ends.
    /// The renderer reads this to switch `MpdType::Dynamic` to
    /// `MpdType::Static` and omit `minimumUpdatePeriod` so DASH
    /// clients stop polling for new segments.
    finalized: std::sync::atomic::AtomicBool,
}

/// Per-broadcast DASH server. Cheap to clone; internally one `Arc`.
///
/// Producers call [`push_video_init`](Self::push_video_init),
/// [`push_audio_init`](Self::push_audio_init),
/// [`push_video_segment`](Self::push_video_segment), and
/// [`push_audio_segment`](Self::push_audio_segment) to feed the
/// state. Consumers hit the axum router returned by
/// [`router`](Self::router).
#[derive(Debug, Clone)]
pub struct DashServer {
    state: Arc<DashState>,
}

impl DashServer {
    /// Build a new per-broadcast server with the given configuration.
    pub fn new(config: DashConfig) -> Self {
        Self {
            state: Arc::new(DashState {
                config,
                video: Mutex::new(TrackState::new()),
                audio: Mutex::new(TrackState::new()),
                finalized: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    /// Publish the video init segment. Also re-parses the bytes
    /// through [`detect_video_codec_string`] so the rendered MPD
    /// picks up a real codec attribute for H.264 / HEVC publishers.
    pub fn push_video_init(&self, bytes: Bytes) {
        let codec = detect_video_codec_string(&bytes);
        let mut v = self.state.video.lock().expect("dash video lock poisoned");
        v.codec = codec;
        v.init = Some(bytes);
    }

    /// Publish the audio init segment. Re-parses through
    /// [`detect_audio_codec_string`] to pick up `mp4a.40.2` for AAC
    /// or `opus` for Opus publishers.
    pub fn push_audio_init(&self, bytes: Bytes) {
        let codec = detect_audio_codec_string(&bytes);
        let mut a = self.state.audio.lock().expect("dash audio lock poisoned");
        a.codec = codec;
        a.init = Some(bytes);
    }

    /// Store one video segment under the given `$Number$` key.
    pub fn push_video_segment(&self, seq: u64, bytes: Bytes) {
        let mut v = self.state.video.lock().expect("dash video lock poisoned");
        if !v.any_segment || seq > v.latest_seq {
            v.latest_seq = seq;
        }
        v.any_segment = true;
        v.segments.insert(seq, bytes);
    }

    /// Store one audio segment under the given `$Number$` key.
    pub fn push_audio_segment(&self, seq: u64, bytes: Bytes) {
        let mut a = self.state.audio.lock().expect("dash audio lock poisoned");
        if !a.any_segment || seq > a.latest_seq {
            a.latest_seq = seq;
        }
        a.any_segment = true;
        a.segments.insert(seq, bytes);
    }

    pub(crate) fn video_init(&self) -> Option<Bytes> {
        self.state.video.lock().expect("dash video lock poisoned").init.clone()
    }

    pub(crate) fn audio_init(&self) -> Option<Bytes> {
        self.state.audio.lock().expect("dash audio lock poisoned").init.clone()
    }

    pub(crate) fn video_segment(&self, seq: u64) -> Option<Bytes> {
        self.state
            .video
            .lock()
            .expect("dash video lock poisoned")
            .segments
            .get(&seq)
            .cloned()
    }

    pub(crate) fn audio_segment(&self, seq: u64) -> Option<Bytes> {
        self.state
            .audio
            .lock()
            .expect("dash audio lock poisoned")
            .segments
            .get(&seq)
            .cloned()
    }

    /// Build an MPD snapshot from the current observed state.
    ///
    /// Returns `None` when the broadcast has no video state yet
    /// (neither init nor segment); DASH requires at least one video
    /// AdaptationSet for a well-formed live manifest. The audio
    /// AdaptationSet is appended conditionally when audio state
    /// exists. Codec strings fall back to the conservative
    /// [`FALLBACK_VIDEO_CODEC`] / [`FALLBACK_AUDIO_CODEC`] pair when
    /// the init segment has not arrived yet so a client polling
    /// early never sees an empty `codecs=""` attribute.
    pub fn render_manifest(&self) -> Option<String> {
        let cfg = &self.state.config;
        let (video_codec, has_video_state) = {
            let v = self.state.video.lock().expect("dash video lock poisoned");
            let has = v.init.is_some() || v.any_segment;
            (v.codec.clone().unwrap_or_else(|| FALLBACK_VIDEO_CODEC.to_string()), has)
        };
        if !has_video_state {
            return None;
        }
        let (audio_codec, has_audio_state) = {
            let a = self.state.audio.lock().expect("dash audio lock poisoned");
            let has = a.init.is_some() || a.any_segment;
            (a.codec.clone().unwrap_or_else(|| FALLBACK_AUDIO_CODEC.to_string()), has)
        };

        let mut adaptation_sets = Vec::with_capacity(2);
        adaptation_sets.push(AdaptationSet {
            id: 0,
            mime_type: "video/mp4".into(),
            content_type: "video".into(),
            lang: None,
            representations: vec![Representation {
                id: "video".into(),
                codecs: video_codec,
                bandwidth_bps: cfg.video_bandwidth_bps,
                width: Some(cfg.video_width),
                height: Some(cfg.video_height),
                audio_sampling_rate: None,
            }],
            segment_template: SegmentTemplate {
                initialization: "init-video.m4s".into(),
                media: "seg-video-$Number$.m4s".into(),
                start_number: 1,
                duration: cfg.video_segment_duration,
                timescale: cfg.video_timescale,
            },
        });
        if has_audio_state {
            adaptation_sets.push(AdaptationSet {
                id: 1,
                mime_type: "audio/mp4".into(),
                content_type: "audio".into(),
                lang: None,
                representations: vec![Representation {
                    id: "audio".into(),
                    codecs: audio_codec,
                    bandwidth_bps: cfg.audio_bandwidth_bps,
                    width: None,
                    height: None,
                    audio_sampling_rate: Some(cfg.audio_timescale),
                }],
                segment_template: SegmentTemplate {
                    initialization: "init-audio.m4s".into(),
                    media: "seg-audio-$Number$.m4s".into(),
                    start_number: 1,
                    duration: cfg.audio_segment_duration,
                    timescale: cfg.audio_timescale,
                },
            });
        }

        let finalized = self.state.finalized.load(std::sync::atomic::Ordering::Relaxed);
        let mpd = Mpd {
            mpd_type: if finalized { MpdType::Static } else { MpdType::Dynamic },
            profiles: if finalized {
                "urn:mpeg:dash:profile:isoff-on-demand:2011".into()
            } else {
                "urn:mpeg:dash:profile:isoff-live:2011".into()
            },
            min_buffer_time: cfg.min_buffer_time.clone(),
            minimum_update_period: if finalized {
                String::new()
            } else {
                cfg.minimum_update_period.clone()
            },
            periods: vec![Period {
                id: "0".into(),
                start: "PT0S".into(),
                adaptation_sets,
            }],
        };
        mpd.render().ok()
    }

    /// Build an `axum::Router` that serves this single broadcast's
    /// MPD, init segments, and numbered media segments. Mount it on
    /// a dedicated listener via `axum::serve` when a single-broadcast
    /// surface is enough; [`MultiDashServer::router`] is the
    /// multi-broadcast counterpart.
    /// Mark this broadcast as ended. Subsequent `render_manifest`
    /// calls will produce an MPD with `type="static"` and omit
    /// `minimumUpdatePeriod` so DASH clients stop polling for new
    /// segments. Calling `finalize()` twice is harmless.
    pub fn finalize(&self) {
        self.state.finalized.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/manifest.mpd", get(handle_manifest))
            .route("/init-video.m4s", get(handle_init_video))
            .route("/init-audio.m4s", get(handle_init_audio))
            .route("/{*uri}", get(handle_segment_uri))
            .with_state(self.clone())
    }
}

async fn handle_manifest(State(server): State<DashServer>) -> Response {
    match server.render_manifest() {
        Some(body) => ([(header::CONTENT_TYPE, "application/dash+xml")], body).into_response(),
        None => (StatusCode::NOT_FOUND, "no DASH state yet").into_response(),
    }
}

async fn handle_init_video(State(server): State<DashServer>) -> Response {
    match server.video_init() {
        Some(b) => ([(header::CONTENT_TYPE, "video/mp4")], b).into_response(),
        None => (StatusCode::NOT_FOUND, "no video init yet").into_response(),
    }
}

async fn handle_init_audio(State(server): State<DashServer>) -> Response {
    match server.audio_init() {
        Some(b) => ([(header::CONTENT_TYPE, "audio/mp4")], b).into_response(),
        None => (StatusCode::NOT_FOUND, "no audio init yet").into_response(),
    }
}

async fn handle_segment_uri(State(server): State<DashServer>, Path(uri): Path<String>) -> Response {
    serve_segment(&server, &uri)
}

fn serve_segment(server: &DashServer, uri: &str) -> Response {
    if let Some(seq) = parse_seq(uri, "seg-video-") {
        return match server.video_segment(seq) {
            Some(b) => ([(header::CONTENT_TYPE, "video/iso.segment")], b).into_response(),
            None => (StatusCode::NOT_FOUND, format!("unknown video segment {seq}")).into_response(),
        };
    }
    if let Some(seq) = parse_seq(uri, "seg-audio-") {
        return match server.audio_segment(seq) {
            Some(b) => ([(header::CONTENT_TYPE, "video/iso.segment")], b).into_response(),
            None => (StatusCode::NOT_FOUND, format!("unknown audio segment {seq}")).into_response(),
        };
    }
    (StatusCode::NOT_FOUND, format!("unknown dash uri {uri}")).into_response()
}

fn parse_seq(uri: &str, prefix: &str) -> Option<u64> {
    uri.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(".m4s"))
        .and_then(|n| n.parse::<u64>().ok())
}

// =====================================================================
// MultiDashServer: per-broadcast fan-out
// =====================================================================

/// Multi-broadcast DASH server. Holds one [`DashServer`] per
/// broadcast name and demultiplexes requests under
/// `/dash/{broadcast}/...`.
///
/// Broadcast entries are created lazily by
/// [`MultiDashServer::ensure`] on the first `push_*` call the bridge
/// issues for that broadcast. Consumer lookups via
/// [`MultiDashServer::get`] return `None` for broadcasts that have
/// never published, which the router turns into a 404.
#[derive(Debug, Clone)]
pub struct MultiDashServer {
    inner: Arc<MultiDashState>,
}

#[derive(Debug)]
struct MultiDashState {
    config: DashConfig,
    broadcasts: Mutex<HashMap<String, DashServer>>,
}

impl MultiDashServer {
    /// Build a new multi-broadcast server. The supplied
    /// [`DashConfig`] is cloned per-broadcast on the fly when a new
    /// broadcast first publishes.
    pub fn new(config: DashConfig) -> Self {
        Self {
            inner: Arc::new(MultiDashState {
                config,
                broadcasts: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Producer-side entry point. Returns a cheap clone of the
    /// per-broadcast [`DashServer`], constructing a fresh entry if
    /// the broadcast has not been seen yet.
    pub fn ensure(&self, broadcast: &str) -> DashServer {
        let mut map = self.inner.broadcasts.lock().expect("dash broadcasts lock poisoned");
        if let Some(existing) = map.get(broadcast) {
            return existing.clone();
        }
        let server = DashServer::new(self.inner.config.clone());
        map.insert(broadcast.to_string(), server.clone());
        server
    }

    /// Consumer-side lookup. Returns `None` for broadcasts that have
    /// not yet published anything.
    pub fn get(&self, broadcast: &str) -> Option<DashServer> {
        self.inner
            .broadcasts
            .lock()
            .expect("dash broadcasts lock poisoned")
            .get(broadcast)
            .cloned()
    }

    /// Mark a broadcast as ended. Calls [`DashServer::finalize`] on
    /// the per-broadcast server so the rendered MPD switches from
    /// `type="dynamic"` to `type="static"` and DASH clients stop
    /// polling. No-op if the broadcast is unknown.
    pub fn finalize_broadcast(&self, broadcast: &str) {
        let map = self.inner.broadcasts.lock().expect("dash broadcasts lock poisoned");
        if let Some(server) = map.get(broadcast) {
            server.finalize();
        }
    }

    /// Number of tracked broadcasts. Test-oriented.
    pub fn broadcast_count(&self) -> usize {
        self.inner
            .broadcasts
            .lock()
            .expect("dash broadcasts lock poisoned")
            .len()
    }

    /// Build an `axum::Router` that serves every tracked broadcast
    /// under `/dash/{broadcast}/...`. A single catch-all is used
    /// because broadcast names legitimately contain slashes
    /// (`live/test`), same pattern `MultiHlsServer::router` uses.
    pub fn router(&self) -> Router {
        Router::new()
            .route(&format!("{MULTI_DASH_PREFIX}/{{*path}}"), get(handle_multi_get))
            .with_state(self.clone())
    }
}

fn split_broadcast_path(path: &str) -> Option<(&str, &str)> {
    let idx = path.rfind('/')?;
    if idx == 0 {
        return None;
    }
    let broadcast = &path[..idx];
    let tail = &path[idx + 1..];
    if broadcast.is_empty() || tail.is_empty() {
        return None;
    }
    Some((broadcast, tail))
}

async fn handle_multi_get(State(multi): State<MultiDashServer>, Path(path): Path<String>) -> Response {
    let Some((broadcast, tail)) = split_broadcast_path(&path) else {
        return (StatusCode::NOT_FOUND, "malformed dash path").into_response();
    };
    let Some(server) = multi.get(broadcast) else {
        return (StatusCode::NOT_FOUND, format!("unknown broadcast {broadcast}")).into_response();
    };
    match tail {
        "manifest.mpd" => handle_manifest(State(server)).await,
        "init-video.m4s" => handle_init_video(State(server)).await,
        "init-audio.m4s" => handle_init_audio(State(server)).await,
        other => serve_segment(&server, other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_manifest_is_none_before_any_push() {
        let server = DashServer::new(DashConfig::default());
        assert!(server.render_manifest().is_none());
    }

    #[test]
    fn render_manifest_after_video_init_emits_video_only() {
        let server = DashServer::new(DashConfig::default());
        server.push_video_init(Bytes::from_static(b"\x00init-bytes"));
        let xml = server.render_manifest().expect("video manifest renders");
        assert!(xml.contains("<AdaptationSet id=\"0\""));
        assert!(!xml.contains("<AdaptationSet id=\"1\""));
        assert!(xml.contains("seg-video-$Number$.m4s"));
    }

    #[test]
    fn render_manifest_includes_audio_when_audio_published() {
        let server = DashServer::new(DashConfig::default());
        server.push_video_init(Bytes::from_static(b"\x00video-init"));
        server.push_audio_init(Bytes::from_static(b"\x00audio-init"));
        let xml = server.render_manifest().expect("av manifest renders");
        assert!(xml.contains("<AdaptationSet id=\"0\""));
        assert!(xml.contains("<AdaptationSet id=\"1\""));
        assert!(xml.contains("seg-audio-$Number$.m4s"));
    }

    #[test]
    fn push_and_read_back_video_segment_bytes() {
        let server = DashServer::new(DashConfig::default());
        server.push_video_init(Bytes::from_static(b"\x00init"));
        server.push_video_segment(1, Bytes::from_static(b"seg1-body"));
        server.push_video_segment(2, Bytes::from_static(b"seg2-body"));
        assert_eq!(server.video_segment(1).unwrap(), Bytes::from_static(b"seg1-body"));
        assert_eq!(server.video_segment(2).unwrap(), Bytes::from_static(b"seg2-body"));
        assert!(server.video_segment(3).is_none());
    }

    #[test]
    fn multi_dash_server_creates_entry_per_broadcast() {
        let multi = MultiDashServer::new(DashConfig::default());
        let a = multi.ensure("live/one");
        let b = multi.ensure("live/two");
        a.push_video_init(Bytes::from_static(b"\x00a-init"));
        b.push_video_init(Bytes::from_static(b"\x00b-init"));
        assert_eq!(multi.broadcast_count(), 2);
        assert!(multi.get("live/one").is_some());
        assert!(multi.get("live/two").is_some());
        assert!(multi.get("live/ghost").is_none());
    }

    #[test]
    fn split_broadcast_path_handles_nested_paths() {
        assert_eq!(
            split_broadcast_path("live/test/manifest.mpd"),
            Some(("live/test", "manifest.mpd"))
        );
        assert_eq!(
            split_broadcast_path("live/test/seg-video-1.m4s"),
            Some(("live/test", "seg-video-1.m4s"))
        );
        assert!(split_broadcast_path("no-slash").is_none());
    }

    #[test]
    fn parse_seq_rejects_malformed_uris() {
        assert_eq!(parse_seq("seg-video-7.m4s", "seg-video-"), Some(7));
        assert_eq!(parse_seq("seg-audio-42.m4s", "seg-audio-"), Some(42));
        assert_eq!(parse_seq("seg-video-xx.m4s", "seg-video-"), None);
        assert_eq!(parse_seq("other.m4s", "seg-video-"), None);
    }

    #[test]
    fn finalize_switches_mpd_to_static() {
        let server = DashServer::new(DashConfig::default());
        server.push_video_init(Bytes::from_static(b"\x00\x00\x00\x08ftypiso5"));
        server.push_video_segment(1, Bytes::from_static(b"seg1"));

        let live = server.render_manifest().unwrap();
        assert!(
            live.contains(r#"type="dynamic""#),
            "before finalize, MPD must be dynamic; got:\n{live}"
        );
        assert!(
            live.contains("minimumUpdatePeriod="),
            "before finalize, MPD must have minimumUpdatePeriod; got:\n{live}"
        );

        server.finalize();
        let vod = server.render_manifest().unwrap();
        assert!(
            vod.contains(r#"type="static""#),
            "after finalize, MPD must be static; got:\n{vod}"
        );
        assert!(
            !vod.contains("minimumUpdatePeriod="),
            "after finalize, MPD must omit minimumUpdatePeriod; got:\n{vod}"
        );
        assert!(
            vod.contains("isoff-on-demand"),
            "after finalize, profile must be on-demand; got:\n{vod}"
        );
    }

    #[test]
    fn finalize_twice_is_harmless() {
        let server = DashServer::new(DashConfig::default());
        server.push_video_init(Bytes::from_static(b"\x00\x00\x00\x08ftypiso5"));
        server.push_video_segment(1, Bytes::from_static(b"seg1"));

        server.finalize();
        let first = server.render_manifest().unwrap();
        server.finalize();
        let second = server.render_manifest().unwrap();
        assert_eq!(first, second, "second finalize must not change the MPD");
    }
}

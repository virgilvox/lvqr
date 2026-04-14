//! LL-HLS HTTP server built on top of [`PlaylistBuilder`].
//!
//! This module is the second half of the `lvqr-hls` day-one scope:
//! the session-7 scaffold shipped the manifest library, and this
//! session wires it behind an `axum` router that real HLS clients
//! can hit. The router is deliberately tiny -- four routes, no
//! middleware, no CORS (the deployment adds that at the reverse
//! proxy), no auth (auth lives in `lvqr-auth` and slots in via the
//! `lvqr-cli` serve path when HLS joins the composition root).
//!
//! ## Routes
//!
//! * `GET /playlist.m3u8` -- the LL-HLS media playlist. If the
//!   request carries an `_HLS_msn=N` query parameter, the handler
//!   blocks until a segment with media sequence `>= N` has been
//!   closed (or a short timeout elapses). If `_HLS_part=M` is also
//!   present, the handler waits until at least `M+1` partials are
//!   available inside the open segment with media sequence `N`.
//! * `GET /init.mp4` -- the fMP4 init segment, served as
//!   `video/mp4`. Returns 404 until the producer calls
//!   [`HlsServer::push_init`].
//! * `GET /{uri}` -- catch-all for closed segments
//!   (`seg-<msn>.m4s`) and partials (`part-<msn>-<idx>.m4s`). Any
//!   URI that has been published via [`HlsServer::push_chunk_bytes`]
//!   is served as `video/iso.segment`; everything else is a 404.
//!
//! ## Push API
//!
//! A producer (session 8: the `lvqr-cli` serve path driven by
//! `rtmp_ws_e2e` or a real RTMP ingest) calls the following once per
//! chunk, in DTS order:
//!
//! ```text
//! server.push_init(init_bytes)?;            // once, at stream start
//! server.push_chunk_bytes(chunk, body)?;    // per chunk forever
//! ```
//!
//! `push_chunk_bytes` wraps two operations: it pushes the chunk into
//! the `PlaylistBuilder` state machine (so the manifest view gains
//! the new partial / segment boundary) and it stores the payload
//! bytes under the URI the builder will generate for that chunk.
//! Callers that want to publish the init segment alone (e.g. for a
//! producer that emits init + media separately) call `push_init`
//! and then `push_chunk_bytes` for every chunk that follows.
//!
//! ## Blocking reload
//!
//! The LL-HLS blocking-reload semantic is implemented via
//! `tokio::sync::Notify::notify_waiters()` on every push. Clients
//! that specify `_HLS_msn=N` park on a `Notify::notified()` future
//! in a loop: on every wake, they re-check the playlist, and return
//! when the condition is satisfied or the `BLOCK_TIMEOUT` elapses.
//!
//! The timeout is a safety net for the pathological case where the
//! producer stalls; without it a disconnected producer would hang
//! every subscriber indefinitely. The timeout chooses a conservative
//! default of three target durations, per the Apple LL-HLS draft
//! recommendation that clients give up and re-request after the
//! server hold-back window passes.
//!
//! Caveat: this is a simple non-prefetching implementation. A
//! production-grade server would use `_HLS_skip=YES` to send a
//! delta playlist and would prefetch the next part via a
//! `#EXT-X-PRELOAD-HINT` tag. Both land when a real consumer
//! (hls.js, Safari, mediastreamvalidator) exercises the path and
//! surfaces the gaps.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use bytes::Bytes;
use lvqr_cmaf::CmafChunk;
use tokio::sync::{Notify, RwLock};

use crate::manifest::{HlsError, PlaylistBuilder, PlaylistBuilderConfig};

/// Default base path used when mounting a [`MultiHlsServer`] router.
const MULTI_HLS_PREFIX: &str = "/hls";

/// Maximum time a blocking-reload request will park on the
/// `Notify` waker before giving up and returning the current
/// playlist anyway. Three target durations is the Apple-recommended
/// server-side hold-back ceiling.
const BLOCK_TIMEOUT_MULTIPLIER: u32 = 3;

/// Shared state the axum handlers read.
///
/// Wrapped in an `Arc` at construction time so the router can pass a
/// cheap clone into every handler. Producers push into the same
/// state through [`HlsServer`] directly, which holds an
/// `Arc<HlsState>` and exposes an ergonomic API over it.
#[derive(Debug)]
struct HlsState {
    builder: RwLock<PlaylistBuilder>,
    init_segment: RwLock<Option<Bytes>>,
    /// URI -> bytes. Keys are emitted by `PlaylistBuilder` so they
    /// match what the rendered playlist points at. We keep a single
    /// flat map for both segments and partials; the router does not
    /// distinguish them.
    cache: RwLock<HashMap<String, Bytes>>,
    /// Wake target for blocking-reload waiters. `notify_waiters()`
    /// on every push is enough because the waiter checks the
    /// manifest state each time it wakes.
    notify: Notify,
    /// Target segment duration in seconds, cached from the builder
    /// config so the block timeout can be computed without taking
    /// the builder lock.
    target_duration_secs: u32,
}

/// Ergonomic handle over the shared state.
///
/// Cheap to clone (it is one `Arc` internally). Hand it to the code
/// that produces chunks as well as the code that owns the axum
/// router; both sides see the same underlying playlist.
#[derive(Debug, Clone)]
pub struct HlsServer {
    state: Arc<HlsState>,
}

impl HlsServer {
    /// Build a new server with the given playlist configuration.
    pub fn new(config: PlaylistBuilderConfig) -> Self {
        let target_duration_secs = config.target_duration_secs;
        let builder = PlaylistBuilder::new(config);
        Self {
            state: Arc::new(HlsState {
                builder: RwLock::new(builder),
                init_segment: RwLock::new(None),
                cache: RwLock::new(HashMap::new()),
                notify: Notify::new(),
                target_duration_secs,
            }),
        }
    }

    /// Publish the init segment. Idempotent: subsequent calls
    /// overwrite, which is the right thing to do when the producer
    /// resets (e.g. after an RTMP reconnect on the same broadcast).
    pub async fn push_init(&self, bytes: Bytes) {
        *self.state.init_segment.write().await = Some(bytes);
        self.state.notify.notify_waiters();
    }

    /// Push a chunk plus its wire-ready bytes. Returns an error iff
    /// the underlying [`PlaylistBuilder`] refuses the chunk (zero
    /// duration, non-monotonic DTS, etc.).
    ///
    /// The chunk's URI is derived by the builder and used as the
    /// key in the segment / partial byte cache, so the rendered
    /// playlist and the cached bytes always agree.
    pub async fn push_chunk_bytes(&self, chunk: &CmafChunk, body: Bytes) -> Result<(), HlsError> {
        let mut builder = self.state.builder.write().await;
        builder.push(chunk)?;
        // The URI for the chunk just pushed is the last entry in
        // preliminary_parts (because `push` always appends the new
        // part to the open segment before the next Segment-kind
        // chunk closes it).
        let uri = builder
            .manifest()
            .preliminary_parts
            .last()
            .map(|p| p.uri.clone())
            .unwrap_or_default();
        drop(builder);
        if !uri.is_empty() {
            self.state.cache.write().await.insert(uri, body);
        }
        self.state.notify.notify_waiters();
        Ok(())
    }

    /// Force-close the currently open segment. Mirrors
    /// [`PlaylistBuilder::close_pending_segment`] and is exposed
    /// here so end-of-stream paths do not need to reach through the
    /// server into the underlying builder.
    pub async fn close_pending_segment(&self) {
        self.state.builder.write().await.close_pending_segment();
        self.state.notify.notify_waiters();
    }

    /// Build an `axum::Router` that serves the LL-HLS surface.
    ///
    /// The returned router is state-less in the axum type sense:
    /// the handlers carry their state via a cloned `Arc<HlsState>`
    /// rather than the axum extension system, so callers can merge
    /// the router into a larger composition without having to
    /// reason about layer ordering.
    pub fn router(&self) -> Router {
        let state = self.state.clone();
        Router::new()
            .route("/playlist.m3u8", get(handle_playlist))
            .route("/init.mp4", get(handle_init))
            .route("/{*uri}", get(handle_uri))
            .with_state(state)
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct BlockingReloadQuery {
    #[serde(rename = "_HLS_msn")]
    hls_msn: Option<u64>,
    #[serde(rename = "_HLS_part")]
    hls_part: Option<u32>,
}

/// Render the playlist response for an [`HlsState`], honouring the
/// LL-HLS blocking reload semantic. Shared between the single-broadcast
/// router and the [`MultiHlsServer`] router so the blocking behaviour
/// lives in exactly one place.
async fn render_playlist(state: &Arc<HlsState>, hls_msn: Option<u64>, hls_part: Option<u32>) -> Response {
    let timeout = Duration::from_secs((state.target_duration_secs * BLOCK_TIMEOUT_MULTIPLIER).max(1) as u64);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let ready = {
            let builder = state.builder.read().await;
            let m = builder.manifest();
            match (hls_msn, hls_part) {
                (None, _) => true,
                (Some(target_msn), None) => m.segments.iter().any(|s| s.sequence >= target_msn),
                (Some(target_msn), Some(target_part)) => {
                    let closed = m.segments.iter().any(|s| s.sequence >= target_msn);
                    if closed {
                        true
                    } else {
                        let open_seq = m.segments.last().map(|s| s.sequence + 1).unwrap_or(0);
                        open_seq == target_msn && (m.preliminary_parts.len() as u32) > target_part
                    }
                }
            }
        };
        if ready {
            let builder = state.builder.read().await;
            let body = builder.manifest().render();
            return ([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")], body).into_response();
        }
        let notified = state.notify.notified();
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let builder = state.builder.read().await;
            let body = builder.manifest().render();
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
                body,
            )
                .into_response();
        }
        if tokio::time::timeout(remaining, notified).await.is_err() {
            continue;
        }
    }
}

async fn render_init(state: &Arc<HlsState>) -> Response {
    match state.init_segment.read().await.clone() {
        Some(bytes) => ([(header::CONTENT_TYPE, "video/mp4")], bytes).into_response(),
        None => (StatusCode::NOT_FOUND, "init segment not yet published").into_response(),
    }
}

async fn render_uri(state: &Arc<HlsState>, uri: &str) -> Response {
    match state.cache.read().await.get(uri).cloned() {
        Some(bytes) => ([(header::CONTENT_TYPE, "video/iso.segment")], bytes).into_response(),
        None => (StatusCode::NOT_FOUND, format!("unknown chunk {uri}")).into_response(),
    }
}

async fn handle_playlist(State(state): State<Arc<HlsState>>, Query(q): Query<BlockingReloadQuery>) -> Response {
    render_playlist(&state, q.hls_msn, q.hls_part).await
}

async fn handle_init(State(state): State<Arc<HlsState>>) -> Response {
    render_init(&state).await
}

async fn handle_uri(
    State(state): State<Arc<HlsState>>,
    axum::extract::Path(uri): axum::extract::Path<String>,
) -> Response {
    render_uri(&state, &uri).await
}

// =====================================================================
// MultiHlsServer: per-broadcast, per-track LL-HLS fan-out.
// =====================================================================

/// Multi-broadcast LL-HLS server with per-broadcast video + audio
/// renditions.
///
/// Wraps a map of broadcast name -> [`BroadcastEntry`] so that a single
/// axum router can serve `/hls/{broadcast}/playlist.m3u8` (video),
/// `/hls/{broadcast}/audio.m3u8` (audio, when present), and a
/// synthesized `/hls/{broadcast}/master.m3u8` for many parallel
/// broadcasts. The producer side creates per-broadcast state on demand
/// via [`MultiHlsServer::ensure_video`] and [`MultiHlsServer::ensure_audio`];
/// the consumer side looks up existing state via
/// [`MultiHlsServer::video`] / [`MultiHlsServer::audio`] and returns
/// `404` for broadcasts or renditions that have not published anything
/// yet.
///
/// Session 13 scope: one video and (optionally) one audio rendition
/// per broadcast. Multi-variant video ladders land when a real
/// transcoder is wired in; for now the master playlist declares one
/// video variant with an optional audio rendition group reference.
///
/// The routed path is a single catch-all (`/hls/{*path}`) because
/// broadcast names legitimately contain slashes today -- the RTMP
/// bridge names broadcasts `{app}/{key}` (for example `live/test`) --
/// so a simple `/hls/{broadcast}/...` pattern would not capture them.
/// The handler splits the tail off and matches on the filename to
/// dispatch between video and audio renditions.
#[derive(Debug, Clone)]
pub struct MultiHlsServer {
    inner: Arc<MultiHlsState>,
}

#[derive(Debug)]
struct MultiHlsState {
    config: PlaylistBuilderConfig,
    broadcasts: std::sync::Mutex<HashMap<String, BroadcastEntry>>,
}

/// Per-broadcast state tracked by [`MultiHlsServer`].
///
/// Video is always present (broadcasts are created on the first
/// `ensure_video` call); audio is optional and appears once
/// `ensure_audio` is called for the same broadcast name.
#[derive(Debug, Clone)]
struct BroadcastEntry {
    video: HlsServer,
    audio: Option<HlsServer>,
}

impl MultiHlsServer {
    /// Build a new multi-broadcast server. `config` is used as the
    /// template `PlaylistBuilderConfig` for every video rendition
    /// created on the fly by [`Self::ensure_video`]. The audio
    /// renditions use a derived config with a different `map_uri`
    /// and `uri_prefix` so video and audio chunks never collide in
    /// the cache.
    pub fn new(config: PlaylistBuilderConfig) -> Self {
        Self {
            inner: Arc::new(MultiHlsState {
                config,
                broadcasts: std::sync::Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Producer-side entry point for the video rendition of
    /// `broadcast`. Returns a cheap clone of the per-broadcast
    /// video [`HlsServer`], creating the broadcast entry if this is
    /// the first time it has been seen.
    pub fn ensure_video(&self, broadcast: &str) -> HlsServer {
        let mut map = self
            .inner
            .broadcasts
            .lock()
            .expect("multi hls broadcasts mutex poisoned");
        if let Some(existing) = map.get(broadcast) {
            return existing.video.clone();
        }
        let entry = BroadcastEntry {
            video: HlsServer::new(self.inner.config.clone()),
            audio: None,
        };
        let video = entry.video.clone();
        map.insert(broadcast.to_string(), entry);
        video
    }

    /// Producer-side entry point for the audio rendition of
    /// `broadcast`. Returns a cheap clone of the per-broadcast
    /// audio [`HlsServer`], creating both the broadcast entry and
    /// the audio rendition if either does not yet exist. Audio
    /// renditions are configured with a distinct init-segment URI
    /// (`audio-init.mp4`) and chunk URI prefix (`audio-`) so they
    /// never collide with the video rendition's cache.
    pub fn ensure_audio(&self, broadcast: &str) -> HlsServer {
        let mut map = self
            .inner
            .broadcasts
            .lock()
            .expect("multi hls broadcasts mutex poisoned");
        let entry = map.entry(broadcast.to_string()).or_insert_with(|| BroadcastEntry {
            video: HlsServer::new(self.inner.config.clone()),
            audio: None,
        });
        if entry.audio.is_none() {
            entry.audio = Some(HlsServer::new(audio_config_from(&self.inner.config)));
        }
        entry.audio.clone().expect("audio just assigned")
    }

    /// Consumer-side lookup for the video rendition of `broadcast`.
    /// Returns `None` when the broadcast has never been announced.
    pub fn video(&self, broadcast: &str) -> Option<HlsServer> {
        self.inner
            .broadcasts
            .lock()
            .expect("multi hls broadcasts mutex poisoned")
            .get(broadcast)
            .map(|e| e.video.clone())
    }

    /// Consumer-side lookup for the audio rendition of `broadcast`.
    /// Returns `None` when the broadcast has no audio rendition
    /// (either the broadcast is unknown or only video has been
    /// published so far).
    pub fn audio(&self, broadcast: &str) -> Option<HlsServer> {
        self.inner
            .broadcasts
            .lock()
            .expect("multi hls broadcasts mutex poisoned")
            .get(broadcast)
            .and_then(|e| e.audio.clone())
    }

    /// Number of broadcasts currently tracked (regardless of how
    /// many renditions each broadcast has). Test-oriented.
    pub fn broadcast_count(&self) -> usize {
        self.inner
            .broadcasts
            .lock()
            .expect("multi hls broadcasts mutex poisoned")
            .len()
    }

    /// Build an `axum::Router` that serves every tracked broadcast
    /// under `/hls/{broadcast}/...`. Routes:
    ///
    /// * `/hls/{broadcast}/master.m3u8` -- synthesized master
    ///   playlist; 404 if the broadcast has no video yet, audio
    ///   rendition is included only when the broadcast has called
    ///   `ensure_audio`.
    /// * `/hls/{broadcast}/playlist.m3u8` -- video media playlist.
    /// * `/hls/{broadcast}/init.mp4` -- video init segment.
    /// * `/hls/{broadcast}/audio.m3u8` -- audio media playlist.
    /// * `/hls/{broadcast}/audio-init.mp4` -- audio init segment.
    /// * `/hls/{broadcast}/audio-<uri>` -- audio chunk (matched by
    ///   the `audio-` prefix that `audio_config_from` installs on
    ///   the audio [`PlaylistBuilderConfig`]).
    /// * `/hls/{broadcast}/<uri>` -- video chunk (everything else).
    pub fn router(&self) -> Router {
        Router::new()
            .route(&format!("{MULTI_HLS_PREFIX}/{{*path}}"), get(handle_multi_get))
            .with_state(self.clone())
    }
}

/// Derive an audio-rendition [`PlaylistBuilderConfig`] from the
/// video template. Uses the video template's timing parameters but
/// swaps the `map_uri`, `uri_prefix`, and timescale to values
/// appropriate for a 48 kHz AAC track. Session 13 hardcodes 48 kHz;
/// later sessions will read the real sample rate from the
/// producer-supplied [`lvqr_cmaf::RawSample`] metadata.
fn audio_config_from(video: &PlaylistBuilderConfig) -> PlaylistBuilderConfig {
    PlaylistBuilderConfig {
        timescale: 48_000,
        starting_sequence: video.starting_sequence,
        map_uri: "audio-init.mp4".into(),
        uri_prefix: "audio-".into(),
        target_duration_secs: video.target_duration_secs,
        part_target_secs: video.part_target_secs,
    }
}

/// Split an `/hls/{broadcast}/<tail>` catch-all capture into
/// `(broadcast, tail)` where `tail` is one of `master.m3u8`,
/// `playlist.m3u8`, `init.mp4`, `audio.m3u8`, `audio-init.mp4`, or
/// a chunk URI. The broadcast is everything before the final `/`.
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

async fn handle_multi_get(
    State(multi): State<MultiHlsServer>,
    axum::extract::Path(path): axum::extract::Path<String>,
    Query(q): Query<BlockingReloadQuery>,
) -> Response {
    let Some((broadcast, tail)) = split_broadcast_path(&path) else {
        return (StatusCode::NOT_FOUND, "malformed hls path").into_response();
    };
    if tail == "master.m3u8" {
        return handle_master_playlist(&multi, broadcast).await;
    }
    let video = multi.video(broadcast);
    let audio = multi.audio(broadcast);
    let (server_opt, video_uri): (Option<HlsServer>, bool) = match tail {
        "playlist.m3u8" | "init.mp4" => (video, true),
        "audio.m3u8" | "audio-init.mp4" => (audio, false),
        other if other.starts_with("audio-") => (audio, false),
        _ => (video, true),
    };
    let Some(server) = server_opt else {
        let which = if video_uri { "video" } else { "audio" };
        return (
            StatusCode::NOT_FOUND,
            format!("unknown {which} rendition for broadcast {broadcast}"),
        )
            .into_response();
    };
    let state = server.state.clone();
    match tail {
        "playlist.m3u8" | "audio.m3u8" => render_playlist(&state, q.hls_msn, q.hls_part).await,
        "init.mp4" | "audio-init.mp4" => render_init(&state).await,
        other => render_uri(&state, other).await,
    }
}

async fn handle_master_playlist(multi: &MultiHlsServer, broadcast: &str) -> Response {
    if multi.video(broadcast).is_none() {
        return (StatusCode::NOT_FOUND, format!("unknown broadcast {broadcast}")).into_response();
    }
    let mut master = crate::master::MasterPlaylist::default();
    let audio_group_id = "audio";
    if multi.audio(broadcast).is_some() {
        master.renditions.push(crate::master::MediaRendition {
            rendition_type: crate::master::MediaRenditionType::Audio,
            group_id: audio_group_id.into(),
            name: "default".into(),
            uri: "audio.m3u8".into(),
            default: true,
            autoselect: true,
            language: None,
        });
    }
    master.variants.push(crate::master::VariantStream {
        // Session 13 ships no real bandwidth / codecs estimation;
        // emit a plausible H.264 baseline + AAC-LC string plus a
        // conservative bitrate so a player can pick the variant up
        // without complaining. Real values will come from the
        // producer-side catalog once the codec parsers land.
        bandwidth_bps: 2_500_000,
        codecs: "avc1.640020,mp4a.40.2".into(),
        resolution: None,
        audio_group: multi.audio(broadcast).is_some().then(|| audio_group_id.to_string()),
        uri: "playlist.m3u8".into(),
    });
    let body = master.render();
    ([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")], body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lvqr_cmaf::CmafChunkKind;

    fn mk_chunk(dts: u64, duration: u64, kind: CmafChunkKind) -> CmafChunk {
        CmafChunk {
            track_id: "0.mp4".into(),
            payload: Bytes::from_static(b""),
            dts,
            duration,
            kind,
        }
    }

    #[tokio::test]
    async fn push_init_and_chunk_are_visible_in_cache() {
        let server = HlsServer::new(PlaylistBuilderConfig::default());
        server.push_init(Bytes::from_static(b"init")).await;
        let chunk = mk_chunk(0, 180_000, CmafChunkKind::Segment);
        server
            .push_chunk_bytes(&chunk, Bytes::from_static(b"seg0part0"))
            .await
            .unwrap();

        assert_eq!(
            server.state.init_segment.read().await.as_deref(),
            Some(b"init".as_ref())
        );
        let cache = server.state.cache.read().await;
        assert!(
            cache.values().any(|v| v.as_ref() == b"seg0part0"),
            "cache: {:?}",
            cache.keys().collect::<Vec<_>>()
        );
    }
}

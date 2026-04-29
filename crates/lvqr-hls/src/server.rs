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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use bytes::Bytes;
use lvqr_cmaf::{CmafChunk, detect_audio_codec_string, detect_video_codec_string};
use tokio::sync::{Notify, RwLock};

use crate::manifest::{HlsError, PlaylistBuilder, PlaylistBuilderConfig};
use crate::subtitles::SubtitlesServer;

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
    /// ISO BMFF codec string parsed out of the init segment at
    /// [`HlsServer::push_init`] time (e.g. `"avc1.42001F"` or
    /// `"hvc1.1.6.L60.B0"`). `None` until an init segment has
    /// been pushed or when the init bytes cannot be parsed. The
    /// master-playlist handler reads this to populate the
    /// `CODECS="..."` attribute so HEVC publishers no longer
    /// advertise an `avc1` codec and vice versa.
    video_codec_string: RwLock<Option<String>>,
    /// Audio codec string parsed out of the audio init segment
    /// the same way (e.g. `"mp4a.40.2"` for AAC-LC or `"opus"`
    /// for Opus). Populated alongside `video_codec_string` in
    /// [`HlsServer::push_init`]; for a video-only `HlsServer`
    /// this stays `None` and the master-playlist handler uses
    /// it only when a parallel audio server exists on the same
    /// broadcast.
    audio_codec_string: RwLock<Option<String>>,
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
                video_codec_string: RwLock::new(None),
                audio_codec_string: RwLock::new(None),
                cache: RwLock::new(HashMap::new()),
                notify: Notify::new(),
                target_duration_secs,
            }),
        }
    }

    /// Publish the init segment. Idempotent: subsequent calls
    /// overwrite, which is the right thing to do when the producer
    /// resets (e.g. after an RTMP reconnect on the same broadcast).
    ///
    /// Also re-parses the init bytes through
    /// [`detect_video_codec_string`] and
    /// [`detect_audio_codec_string`] so the master-playlist
    /// handler has a correct `CODECS="..."` attribute for AVC,
    /// HEVC, AAC, **and** Opus publishers. A video-only
    /// `HlsServer` populates the video slot; the audio sibling
    /// inside a [`MultiHlsServer`] populates the audio slot; a
    /// single init segment carrying both tracks would populate
    /// both, which is harmless.
    pub async fn push_init(&self, bytes: Bytes) {
        let video = detect_video_codec_string(&bytes);
        let audio = detect_audio_codec_string(&bytes);
        // RFC 8216bis §4.4.4.4: a publisher reconnect (or any
        // mid-stream codec / encoder change) is exactly what
        // EXT-X-DISCONTINUITY signals. The CLI's HLS bridge calls
        // `push_init` once at stream start AND every time it observes
        // new init bytes from the broadcaster (e.g. the publisher
        // dropped + re-published on the same broadcast slot). The
        // first call must NOT mark a discontinuity (it is the start
        // of the stream); every subsequent call MUST.
        let was_initialized = self.state.init_segment.read().await.is_some();
        *self.state.video_codec_string.write().await = video;
        *self.state.audio_codec_string.write().await = audio;
        *self.state.init_segment.write().await = Some(bytes);
        if was_initialized {
            self.state.builder.write().await.mark_discontinuity_pending();
        }
        self.state.notify.notify_waiters();
    }

    /// Read the cached video codec string parsed out of the
    /// init segment at [`Self::push_init`] time. `None` when no
    /// init segment has arrived yet or the sample entry is not
    /// one we stringify (e.g. AAC-only init).
    pub async fn video_codec_string(&self) -> Option<String> {
        self.state.video_codec_string.read().await.clone()
    }

    /// Read the cached audio codec string parsed out of the
    /// init segment. Returns `None` for a video-only server.
    pub async fn audio_codec_string(&self) -> Option<String> {
        self.state.audio_codec_string.read().await.clone()
    }

    /// Return the current `(last_msn, last_part)` position of this
    /// rendition, matching the LL-HLS `EXT-X-RENDITION-REPORT`
    /// definition: "the Media Sequence Number of the Media Segment
    /// containing the last Partial Segment" and its index.
    ///
    /// * When the builder has pending partials in an open segment:
    ///   `last_msn` is the sequence the open segment will carry
    ///   once it closes, and `last_part` is the zero-based index of
    ///   the trailing partial.
    /// * When only closed segments exist: `last_msn` is the
    ///   sequence of the most recent closed segment, and
    ///   `last_part` is the index of its trailing partial.
    /// * Before any chunk has been pushed: `None`.
    ///
    /// Used by [`MultiHlsServer`]'s router to populate the
    /// sibling-rendition `EXT-X-RENDITION-REPORT` tag in each
    /// rendition's media playlist so a client that polls one
    /// rendition can discover the others' live position without an
    /// extra round trip.
    pub async fn current_rendition_position(&self) -> Option<(u64, Option<u32>)> {
        let builder = self.state.builder.read().await;
        let m = builder.manifest();
        if !m.preliminary_parts.is_empty() {
            let open_seq = m.segments.last().map(|s| s.sequence + 1).unwrap_or(0);
            Some((open_seq, Some((m.preliminary_parts.len() - 1) as u32)))
        } else if let Some(last) = m.segments.last() {
            if last.parts.is_empty() {
                Some((last.sequence, None))
            } else {
                Some((last.sequence, Some((last.parts.len() - 1) as u32)))
            }
        } else {
            None
        }
    }

    /// Push a chunk plus its wire-ready bytes. Returns an error iff
    /// the underlying [`PlaylistBuilder`] refuses the chunk (zero
    /// duration, non-monotonic DTS, etc.).
    ///
    /// The chunk's URI is derived by the builder and used as the
    /// key in the segment / partial byte cache, so the rendered
    /// playlist and the cached bytes always agree. When this push
    /// closes one or more segments (i.e. the builder's
    /// `manifest.segments` length grows), their constituent parts
    /// are coalesced into a single `Bytes` blob and inserted into
    /// the cache under each newly-closed segment's rendered URI so
    /// a plain HLS client (ffmpeg, Safari fallback) that walks the
    /// `#EXTINF` lines rather than the `#EXT-X-PART` URIs still
    /// resolves to real bytes. The pre-session-33 cache stored only
    /// partial URIs, which made `GET /seg-<n>.m4s` a 404; the Apple
    /// `mediastreamvalidator` soft-skip workflow surfaced this via
    /// its ffmpeg client-side compliance pass.
    pub async fn push_chunk_bytes(&self, chunk: &CmafChunk, body: Bytes) -> Result<(), HlsError> {
        let mut builder = self.state.builder.write().await;
        let prev_last_seq = builder.manifest().segments.last().map(|s| s.sequence);
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
        let coalesce_work = collect_coalesce_work(builder.manifest(), prev_last_seq);
        let evicted = builder.drain_evicted_uris();
        drop(builder);
        if !uri.is_empty() {
            self.state.cache.write().await.insert(uri, body);
        }
        coalesce_closed_segments(&self.state, coalesce_work).await;
        purge_evicted_uris(&self.state, evicted).await;
        self.state.notify.notify_waiters();
        Ok(())
    }

    /// Force-close the currently open segment. Mirrors
    /// [`PlaylistBuilder::close_pending_segment`] and is exposed
    /// here so end-of-stream paths do not need to reach through the
    /// server into the underlying builder. Applies the same
    /// closed-segment-bytes coalesce [`Self::push_chunk_bytes`]
    /// applies so a fetched `seg-<n>.m4s` is populated even for
    /// the end-of-stream segment the force-close flushes.
    /// Push a SCTE-35 `#EXT-X-DATERANGE` entry into the underlying
    /// playlist. Delegates to [`PlaylistBuilder::push_date_range`].
    /// Subsequent renders include the entry until the playlist's
    /// segment window slides past its `START-DATE`.
    ///
    /// Used by the cli-side `BroadcasterScte35Bridge` to push events
    /// drained from the registry's `"scte35"` track. Notifies all
    /// blocked playlist waiters so LL-HLS clients see the new
    /// DATERANGE on the next manifest refresh.
    pub async fn push_date_range(&self, dr: crate::manifest::DateRange) {
        let mut builder = self.state.builder.write().await;
        builder.push_date_range(dr);
        drop(builder);
        self.state.notify.notify_waiters();
    }

    pub async fn close_pending_segment(&self) {
        let mut builder = self.state.builder.write().await;
        let prev_last_seq = builder.manifest().segments.last().map(|s| s.sequence);
        builder.close_pending_segment();
        let coalesce_work = collect_coalesce_work(builder.manifest(), prev_last_seq);
        let evicted = builder.drain_evicted_uris();
        drop(builder);
        coalesce_closed_segments(&self.state, coalesce_work).await;
        purge_evicted_uris(&self.state, evicted).await;
        self.state.notify.notify_waiters();
    }

    /// Mark this broadcast as ended. Closes the pending segment,
    /// coalesces its bytes, purges any evicted URIs from the cache,
    /// clears the preload hint, and appends `#EXT-X-ENDLIST` to
    /// the rendered playlist so HLS clients stop polling. After
    /// this call the playlist is final: no more segments will
    /// appear, and the retained window becomes a VOD surface that
    /// clients can scrub freely. Calling `finalize()` twice is
    /// harmless.
    pub async fn finalize(&self) {
        let mut builder = self.state.builder.write().await;
        let prev_last_seq = builder.manifest().segments.last().map(|s| s.sequence);
        builder.finalize();
        let coalesce_work = collect_coalesce_work(builder.manifest(), prev_last_seq);
        let evicted = builder.drain_evicted_uris();
        drop(builder);
        coalesce_closed_segments(&self.state, coalesce_work).await;
        purge_evicted_uris(&self.state, evicted).await;
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

/// Snapshot of the `(closed_segment_uri, constituent_part_uris)`
/// pairs for every segment in the manifest whose sequence number is
/// strictly greater than `prev_last_seq`. Called after a
/// `builder.push` / `close_pending_segment` call with the prior
/// tail sequence captured before the mutation, so any segment with
/// a greater sequence is by construction a segment that just
/// closed.
///
/// Sequence-based rather than index-based because the sliding
/// window added in session 34 may evict entries from the front of
/// `manifest.segments` inside the same mutation, which would break
/// a positional walk. The cloned `String` vectors let the caller
/// release the builder lock before taking the cache write lock,
/// which avoids holding both at once.
fn collect_coalesce_work(
    manifest: &crate::manifest::Manifest,
    prev_last_seq: Option<u64>,
) -> Vec<(String, Vec<String>)> {
    manifest
        .segments
        .iter()
        .filter(|seg| match prev_last_seq {
            Some(prev) => seg.sequence > prev,
            None => true,
        })
        .map(|seg| (seg.uri.clone(), seg.parts.iter().map(|p| p.uri.clone()).collect()))
        .collect()
}

/// For each `(seg_uri, part_uris)` pair, concatenate the cached
/// bytes of every part URI and insert the resulting blob under
/// `seg_uri`. Parts that are missing from the cache are skipped
/// (which happens only when a partial was evicted from the
/// sliding window before its segment closed, not in normal flow).
/// Entirely empty results are not inserted so a stray
/// force-close against an empty builder does not plant zero-byte
/// bodies in the cache.
async fn coalesce_closed_segments(state: &Arc<HlsState>, work: Vec<(String, Vec<String>)>) {
    if work.is_empty() {
        return;
    }
    let mut cache = state.cache.write().await;
    for (seg_uri, part_uris) in work {
        // Avoid re-coalescing a segment we already wrote (defensive
        // against a future path that coalesces from multiple sites).
        if cache.contains_key(&seg_uri) {
            continue;
        }
        let total: usize = part_uris.iter().filter_map(|u| cache.get(u).map(|b| b.len())).sum();
        if total == 0 {
            continue;
        }
        let mut buf = bytes::BytesMut::with_capacity(total);
        for pu in &part_uris {
            if let Some(b) = cache.get(pu) {
                buf.extend_from_slice(b);
            }
        }
        cache.insert(seg_uri, buf.freeze());
    }
}

/// Remove every URI the sliding-window eviction dropped from the
/// rendered playlist. Called after every builder mutation so the
/// byte cache stays in lock-step with the manifest: a client that
/// polls the new playlist never sees a URI, and a client that
/// still holds a stale playlist hits the normal 404 path for
/// URIs that are no longer part of the live window.
async fn purge_evicted_uris(state: &Arc<HlsState>, evicted: Vec<String>) {
    if evicted.is_empty() {
        return;
    }
    let mut cache = state.cache.write().await;
    for uri in evicted {
        cache.remove(&uri);
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct BlockingReloadQuery {
    #[serde(rename = "_HLS_msn")]
    hls_msn: Option<u64>,
    #[serde(rename = "_HLS_part")]
    hls_part: Option<u32>,
    /// `_HLS_skip=YES` (or `v2`) asks the server to return a
    /// delta playlist: older segments are replaced by a single
    /// `#EXT-X-SKIP:SKIPPED-SEGMENTS=N` tag. Anything other than
    /// `YES` / `v2` is treated as the absent case. The delta is
    /// still subject to the spec floor in
    /// [`crate::manifest::Manifest::delta_skip_count`]; the query
    /// only *requests* the directive, the manifest decides
    /// whether to honour it.
    #[serde(rename = "_HLS_skip")]
    hls_skip: Option<String>,
}

/// One sibling-rendition report block to append to a rendered
/// media playlist as an `#EXT-X-RENDITION-REPORT` tag. Populated
/// by [`handle_multi_get`] from the sibling [`HlsServer`]'s
/// [`HlsServer::current_rendition_position`] read. The
/// single-broadcast router at [`HlsServer::router`] always passes
/// an empty slice.
#[derive(Debug, Clone)]
struct RenditionReport {
    uri: String,
    last_msn: u64,
    last_part: Option<u32>,
}

/// Append `#EXT-X-RENDITION-REPORT` lines to a rendered playlist
/// body. The spec lets the tag appear anywhere in a playlist; we
/// emit it at the end, after the preload hint, so a client walking
/// the body top-down encounters it after it has seen every segment,
/// partial, and preload hint in the current rendition.
fn append_rendition_reports(body: &mut String, reports: &[RenditionReport]) {
    for r in reports {
        use std::fmt::Write as _;
        let _ = write!(
            body,
            "#EXT-X-RENDITION-REPORT:URI=\"{}\",LAST-MSN={}",
            r.uri, r.last_msn
        );
        if let Some(part) = r.last_part {
            let _ = write!(body, ",LAST-PART={part}");
        }
        body.push('\n');
    }
}

/// Render the playlist response for an [`HlsState`], honouring the
/// LL-HLS blocking reload semantic. Shared between the single-broadcast
/// router and the [`MultiHlsServer`] router so the blocking behaviour
/// lives in exactly one place. `reports` carries any sibling-rendition
/// reports [`handle_multi_get`] computed before calling this function;
/// it is empty for the single-broadcast router where there is no
/// sibling.
async fn render_playlist(
    state: &Arc<HlsState>,
    hls_msn: Option<u64>,
    hls_part: Option<u32>,
    hls_skip: Option<&str>,
    reports: &[RenditionReport],
) -> Response {
    let want_delta = matches!(hls_skip, Some(v) if v.eq_ignore_ascii_case("YES") || v.eq_ignore_ascii_case("v2"));
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
            let manifest = builder.manifest();
            let skip = if want_delta { manifest.delta_skip_count() } else { 0 };
            let mut body = manifest.render_with_skip(skip);
            drop(builder);
            append_rendition_reports(&mut body, reports);
            return ([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")], body).into_response();
        }
        let notified = state.notify.notified();
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let builder = state.builder.read().await;
            let manifest = builder.manifest();
            let skip = if want_delta { manifest.delta_skip_count() } else { 0 };
            let mut body = manifest.render_with_skip(skip);
            drop(builder);
            append_rendition_reports(&mut body, reports);
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
    // Single-broadcast router has no sibling renditions; the
    // multi-broadcast router handles that path.
    render_playlist(&state, q.hls_msn, q.hls_part, q.hls_skip.as_deref(), &[]).await
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
#[derive(Clone)]
pub struct MultiHlsServer {
    inner: Arc<MultiHlsState>,
}

impl std::fmt::Debug for MultiHlsServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultiHlsServer")
            .field("broadcast_count", &self.broadcast_count())
            .field(
                "owner_resolver",
                &self.inner.owner_resolver.as_ref().map(|_| "<resolver>"),
            )
            .finish()
    }
}

/// Future returned by an [`OwnerResolver`]. Producing an owned
/// `String` keeps the callback object-safe without needing HRTBs
/// on the return type.
pub type RedirectFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>>;

/// Callback that resolves an unknown broadcast name to the base
/// URL of the owning node's HLS endpoint (e.g.
/// `"http://a.local:8888"`). Returning `Some(url)` triggers a 302
/// to `"<url>/hls/<broadcast>/<tail>"`; returning `None` falls
/// through to the existing 404 path.
///
/// The resolver is called from inside the axum handler so it
/// should be fast-ish; the expected implementation is a chitchat
/// KV lookup via [`lvqr_cluster::Cluster::find_owner_endpoints`],
/// which takes a single mutex acquisition.
pub type OwnerResolver = Arc<dyn Fn(String) -> RedirectFuture + Send + Sync>;

struct MultiHlsState {
    config: PlaylistBuilderConfig,
    broadcasts: std::sync::Mutex<HashMap<String, BroadcastEntry>>,
    /// Optional callback consulted when an incoming request names a
    /// broadcast this node does not host. See [`OwnerResolver`].
    owner_resolver: Option<OwnerResolver>,
    /// ABR ladder metadata the master-playlist composer uses to
    /// emit one `#EXT-X-STREAM-INF` per rendition sibling of a
    /// source broadcast. Populated at server startup by the
    /// `lvqr-cli` composition root from the operator-supplied
    /// `--transcode-rendition` list (Tier 4 item 4.6 session 106
    /// C). Empty when transcode is disabled or no ladder is
    /// configured; master playlist falls back to the single-variant
    /// source-only path.
    ladder: std::sync::RwLock<Vec<crate::master::RenditionMeta>>,
    /// Operator override for the source variant's advertised
    /// `BANDWIDTH`. Defaults to `highest_rung_bps * 1.2` when
    /// `None` and the ladder is non-empty; `2_500_000` when the
    /// ladder is empty (pre-session 106 C behavior).
    source_bandwidth_bps: std::sync::RwLock<Option<u64>>,
}

/// Per-broadcast state tracked by [`MultiHlsServer`].
///
/// Video is always present (broadcasts are created on the first
/// `ensure_video` call); audio is optional and appears once
/// `ensure_audio` is called for the same broadcast name. The
/// optional `subtitles` field is populated on the first
/// `ensure_subtitles` call (Tier 4 item 4.5 session C); when
/// present, the master playlist gains an
/// `EXT-X-MEDIA TYPE=SUBTITLES` rendition group and the variant
/// stream gets a `SUBTITLES="subs"` attribute.
#[derive(Debug, Clone)]
struct BroadcastEntry {
    video: HlsServer,
    audio: Option<HlsServer>,
    subtitles: Option<SubtitlesServer>,
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
                owner_resolver: None,
                ladder: std::sync::RwLock::new(Vec::new()),
                source_bandwidth_bps: std::sync::RwLock::new(None),
            }),
        }
    }

    /// Build a new multi-broadcast server with an
    /// [`OwnerResolver`] already installed. Used by `lvqr-cli`
    /// when clustering is enabled; the resolver wraps a
    /// `Cluster::find_owner_endpoints` lookup so requests for a
    /// broadcast hosted on a peer redirect with `302` instead of
    /// `404`.
    pub fn with_owner_resolver(config: PlaylistBuilderConfig, resolver: OwnerResolver) -> Self {
        Self {
            inner: Arc::new(MultiHlsState {
                config,
                broadcasts: std::sync::Mutex::new(HashMap::new()),
                owner_resolver: Some(resolver),
                ladder: std::sync::RwLock::new(Vec::new()),
                source_bandwidth_bps: std::sync::RwLock::new(None),
            }),
        }
    }

    /// Resolve a redirect target for `broadcast`. Returns `None`
    /// when no resolver is installed or when the resolver yields
    /// `None`. Pulled out of the handler so it is unit-testable
    /// without spinning an axum request.
    async fn resolve_redirect_base(&self, broadcast: &str) -> Option<String> {
        let resolver = self.inner.owner_resolver.as_ref()?;
        resolver(broadcast.to_string()).await
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
        let cfg = stamp_pdt_now(&self.inner.config);
        let entry = BroadcastEntry {
            video: HlsServer::new(cfg),
            audio: None,
            subtitles: None,
        };
        let video = entry.video.clone();
        map.insert(broadcast.to_string(), entry);
        video
    }

    /// Producer-side entry point for the audio rendition of
    /// `broadcast`. Returns a cheap clone of the per-broadcast
    /// audio [`HlsServer`], creating both the broadcast entry and
    /// the audio rendition if either does not yet exist.
    ///
    /// `timescale` is the track's native sample rate (44_100 for
    /// typical AAC-LC, 48_000 for Opus / HE-AAC etc.). The derived
    /// audio `PlaylistBuilderConfig` uses it so `#EXT-X-PART:DURATION`
    /// values in the rendered audio playlist report the real
    /// wall-clock duration of each partial rather than the
    /// session-13 hardcoded 48 kHz approximation that overstated
    /// 44.1 kHz AAC partial durations by ~8.8%.
    ///
    /// Audio renditions are also configured with a distinct init-
    /// segment URI (`audio-init.mp4`) and chunk URI prefix
    /// (`audio-`) so they never collide with the video rendition's
    /// cache.
    pub fn ensure_audio(&self, broadcast: &str, timescale: u32) -> HlsServer {
        let mut map = self
            .inner
            .broadcasts
            .lock()
            .expect("multi hls broadcasts mutex poisoned");
        let entry = map.entry(broadcast.to_string()).or_insert_with(|| {
            let cfg = stamp_pdt_now(&self.inner.config);
            BroadcastEntry {
                video: HlsServer::new(cfg),
                audio: None,
                subtitles: None,
            }
        });
        if entry.audio.is_none() {
            let cfg = stamp_pdt_now(&self.inner.config);
            entry.audio = Some(HlsServer::new(audio_config_from(&cfg, timescale)));
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

    /// Producer-side entry point for the subtitles rendition of
    /// `broadcast`. Returns a cheap clone of the per-broadcast
    /// [`SubtitlesServer`], creating both the broadcast entry
    /// and the subtitles state on first call. Tier 4 item 4.5
    /// session C: the WhisperCaptionsAgent's captions feed the
    /// returned server's `push_cue` via `BroadcasterCaptionsBridge`.
    /// Push a SCTE-35 `#EXT-X-DATERANGE` entry into the video
    /// rendition of `broadcast`. Lazily creates the broadcast entry
    /// (the video rendition gets a default config); the next manifest
    /// render carries the new DATERANGE alongside any prior live
    /// entries. No-op if the playlist's segment window has already
    /// advanced past the entry's `START-DATE`.
    pub async fn push_date_range(&self, broadcast: &str, dr: crate::manifest::DateRange) {
        let video = self.ensure_video(broadcast);
        video.push_date_range(dr).await;
    }

    pub fn ensure_subtitles(&self, broadcast: &str) -> SubtitlesServer {
        let mut map = self
            .inner
            .broadcasts
            .lock()
            .expect("multi hls broadcasts mutex poisoned");
        let entry = map.entry(broadcast.to_string()).or_insert_with(|| {
            let cfg = stamp_pdt_now(&self.inner.config);
            BroadcastEntry {
                video: HlsServer::new(cfg),
                audio: None,
                subtitles: None,
            }
        });
        if entry.subtitles.is_none() {
            entry.subtitles = Some(SubtitlesServer::new());
        }
        entry.subtitles.clone().expect("subtitles just assigned")
    }

    /// Consumer-side lookup for the subtitles rendition of
    /// `broadcast`. Returns `None` when the broadcast has no
    /// captions (either the broadcast is unknown or no
    /// captions agent has been wired in).
    pub fn subtitles(&self, broadcast: &str) -> Option<SubtitlesServer> {
        self.inner
            .broadcasts
            .lock()
            .expect("multi hls broadcasts mutex poisoned")
            .get(broadcast)
            .and_then(|e| e.subtitles.clone())
    }

    /// Mark a broadcast as ended. Calls [`HlsServer::finalize`] on
    /// both the video and audio renditions (if present), which closes
    /// the pending segment, coalesces its bytes, appends
    /// `#EXT-X-ENDLIST`, and wakes any parked blocking-reload
    /// subscribers. No-op if the broadcast is unknown.
    pub async fn finalize_broadcast(&self, broadcast: &str) {
        let (video, audio, subs) = {
            let map = self
                .inner
                .broadcasts
                .lock()
                .expect("multi hls broadcasts mutex poisoned");
            match map.get(broadcast) {
                Some(entry) => (Some(entry.video.clone()), entry.audio.clone(), entry.subtitles.clone()),
                None => return,
            }
        };
        if let Some(v) = video {
            v.finalize().await;
        }
        if let Some(a) = audio {
            a.finalize().await;
        }
        if let Some(s) = subs {
            s.finalize();
        }
    }

    /// Register the ABR ladder the master-playlist composer consults
    /// when a request for `<source>/master.m3u8` arrives. Each entry's
    /// `name` is matched against sibling broadcasts of the form
    /// `<source>/<name>`; siblings that have published at least a
    /// video rendition produce one `#EXT-X-STREAM-INF` variant line
    /// with the ladder entry's `BANDWIDTH`, `RESOLUTION`, and
    /// `CODECS`. Calling again replaces the entire ladder.
    ///
    /// Empty ladder -> master playlist falls back to the single-
    /// variant source-only shape the pre-session-106-C version
    /// emitted. Tier 4 item 4.6 session 106 C.
    pub fn set_ladder(&self, ladder: Vec<crate::master::RenditionMeta>) {
        *self.inner.ladder.write().expect("multi hls ladder lock poisoned") = ladder;
    }

    /// Current ladder snapshot. Read-only clone; useful for tests
    /// and admin diagnostics.
    pub fn ladder(&self) -> Vec<crate::master::RenditionMeta> {
        self.inner
            .ladder
            .read()
            .expect("multi hls ladder lock poisoned")
            .clone()
    }

    /// Override the advertised `BANDWIDTH` for the source variant in
    /// the master playlist. Defaults (`None`) to
    /// `highest_rung_bps * 1.2` when a ladder is configured, and to
    /// `2_500_000` when the ladder is empty. Tier 4 item 4.6
    /// session 106 C.
    pub fn set_source_bandwidth_bps(&self, bps: Option<u64>) {
        *self
            .inner
            .source_bandwidth_bps
            .write()
            .expect("multi hls source_bandwidth lock poisoned") = bps;
    }

    /// Current source-variant bandwidth override, if any.
    pub fn source_bandwidth_bps(&self) -> Option<u64> {
        *self
            .inner
            .source_bandwidth_bps
            .read()
            .expect("multi hls source_bandwidth lock poisoned")
    }

    /// Return the set of `<name>` suffixes for every broadcast of
    /// shape `<source>/<name>` that has a video rendition tracked
    /// today. Used by [`handle_master_playlist`] to find sibling
    /// rendition broadcasts without needing to name the specific
    /// ladder rungs up-front. Test-facing.
    pub(crate) fn variant_siblings(&self, source: &str) -> Vec<String> {
        let map = self
            .inner
            .broadcasts
            .lock()
            .expect("multi hls broadcasts mutex poisoned");
        let prefix = format!("{source}/");
        let mut siblings: Vec<String> = map
            .keys()
            .filter_map(|key| key.strip_prefix(&prefix).map(|s| s.to_string()))
            .filter(|suffix| !suffix.contains('/'))
            .collect();
        siblings.sort();
        siblings
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
    /// * `/hls/{broadcast}/captions/playlist.m3u8` -- subtitles
    ///   media playlist (Tier 4 item 4.5 session C). Plain HLS
    ///   playlist with no LL-HLS partials and no `EXT-X-MAP`;
    ///   each `#EXTINF` entry references a per-cue `.vtt` file.
    /// * `/hls/{broadcast}/captions/seg-<msn>.vtt` -- the
    ///   `.vtt` body for media-sequence `msn`. 404 when the cue
    ///   has been evicted from the sliding window.
    /// * `/hls/{broadcast}/<uri>` -- video chunk (everything else).
    pub fn router(&self) -> Router {
        Router::new()
            .route(&format!("{MULTI_HLS_PREFIX}/{{*path}}"), get(handle_multi_get))
            .with_state(self.clone())
    }
}

/// Derive an audio-rendition [`PlaylistBuilderConfig`] from the
/// video template and the real audio track timescale. Uses the
/// video template's timing parameters but swaps the `map_uri`,
/// `uri_prefix`, and timescale to values that match the audio
/// track. Callers plumb `timescale` from the FLV AAC sequence
/// header's `AudioConfig::sample_rate`.
/// Stamp the current wall-clock time into a cloned config so the
/// builder emits `#EXT-X-PROGRAM-DATE-TIME` from this point on.
/// Called once per broadcast at `ensure_video` / `ensure_audio`
/// creation time so each broadcast anchors independently.
fn stamp_pdt_now(config: &PlaylistBuilderConfig) -> PlaylistBuilderConfig {
    let now_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    PlaylistBuilderConfig {
        program_date_time_base: Some(now_millis),
        ..config.clone()
    }
}

fn audio_config_from(video: &PlaylistBuilderConfig, timescale: u32) -> PlaylistBuilderConfig {
    PlaylistBuilderConfig {
        timescale,
        starting_sequence: video.starting_sequence,
        map_uri: "audio-init.mp4".into(),
        uri_prefix: "audio-".into(),
        target_duration_secs: video.target_duration_secs,
        part_target_secs: video.part_target_secs,
        max_segments: video.max_segments,
        program_date_time_base: video.program_date_time_base,
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

    // If the broadcast is unknown on this node AND an owner resolver
    // is wired (clustering enabled), try to redirect the subscriber
    // to the owning node's HLS surface before 404'ing. Resolver
    // misses fall through to the existing not-found paths.
    if multi.video(broadcast).is_none() && multi.audio(broadcast).is_none() && multi.subtitles(broadcast).is_none() {
        if let Some(base) = multi.resolve_redirect_base(broadcast).await {
            return redirect_to_owner(&base, &path);
        }
    }

    if tail == "master.m3u8" {
        return handle_master_playlist(&multi, broadcast).await;
    }
    // Captions URIs are sub-pathed `<broadcast>/captions/<file>`.
    // The split_broadcast_path helper splits at the last `/`, so
    // for `live/cam1/captions/playlist.m3u8` it returns
    // `("live/cam1/captions", "playlist.m3u8")`. Detect the
    // captions tail by stripping the `/captions` suffix off the
    // broadcast and re-routing.
    if let Some(real_broadcast) = broadcast.strip_suffix("/captions") {
        return handle_captions(&multi, real_broadcast, tail).await;
    }
    let video = multi.video(broadcast);
    let audio = multi.audio(broadcast);
    let (server_opt, video_uri): (Option<HlsServer>, bool) = match tail {
        "playlist.m3u8" | "init.mp4" => (video.clone(), true),
        "audio.m3u8" | "audio-init.mp4" => (audio.clone(), false),
        other if other.starts_with("audio-") => (audio.clone(), false),
        _ => (video.clone(), true),
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
        "playlist.m3u8" | "audio.m3u8" => {
            // Build a sibling-rendition report so a client polling
            // the video playlist discovers the audio rendition's
            // live position without an extra round trip, and vice
            // versa. The sibling is whichever rendition we are NOT
            // currently rendering. Read the sibling's position
            // before taking the target rendition's lock inside
            // `render_playlist` so the two reads stay unordered --
            // eventual consistency is acceptable per the LL-HLS
            // spec ("as up-to-date as the Playlist that contains
            // it").
            let reports = if video_uri {
                build_sibling_reports(audio.as_ref(), "audio.m3u8").await
            } else {
                build_sibling_reports(video.as_ref(), "playlist.m3u8").await
            };
            render_playlist(&state, q.hls_msn, q.hls_part, q.hls_skip.as_deref(), &reports).await
        }
        "init.mp4" | "audio-init.mp4" => render_init(&state).await,
        other => render_uri(&state, other).await,
    }
}

/// Build a 302 response redirecting to `<base>/hls/<path>`.
/// `base` is expected to already carry the scheme + authority
/// (e.g. `"http://a.local:8888"`) with no trailing slash; the
/// helper is tolerant of one stray trailing slash and silently
/// strips it. `path` is the tail the handler received, already
/// `{broadcast}/{filename}` shaped.
fn redirect_to_owner(base: &str, path: &str) -> Response {
    let base = base.trim_end_matches('/');
    let location = format!("{base}/hls/{path}");
    (
        StatusCode::FOUND,
        [(axum::http::header::LOCATION, location)],
        "broadcast lives on another cluster node",
    )
        .into_response()
}

/// Build a one-element rendition report slice for a sibling
/// rendition, or an empty slice if the sibling does not exist or
/// has not yet seen any chunks. `sibling_uri` is the URI the target
/// playlist should use to reference the sibling (relative to the
/// broadcast's HLS base).
async fn build_sibling_reports(sibling: Option<&HlsServer>, sibling_uri: &str) -> Vec<RenditionReport> {
    let Some(s) = sibling else {
        return Vec::new();
    };
    let Some((last_msn, last_part)) = s.current_rendition_position().await else {
        return Vec::new();
    };
    vec![RenditionReport {
        uri: sibling_uri.to_string(),
        last_msn,
        last_part,
    }]
}

/// Handle a `/hls/{broadcast}/captions/<file>` request. Routes:
///
/// * `playlist.m3u8` -- the captions media playlist.
/// * `seg-<msn>.vtt` -- the per-cue WebVTT segment body.
async fn handle_captions(multi: &MultiHlsServer, broadcast: &str, tail: &str) -> Response {
    let Some(subs) = multi.subtitles(broadcast) else {
        return (
            StatusCode::NOT_FOUND,
            format!("unknown captions rendition for broadcast {broadcast}"),
        )
            .into_response();
    };
    if tail == "playlist.m3u8" {
        let body = subs.render_playlist();
        return ([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")], body).into_response();
    }
    if let Some(seq) = SubtitlesServer::parse_segment_uri(tail)
        && let Some(body) = subs.render_segment(seq)
    {
        return ([(header::CONTENT_TYPE, "text/vtt; charset=utf-8")], body).into_response();
    }
    (StatusCode::NOT_FOUND, format!("unknown captions URI {tail}")).into_response()
}

async fn handle_master_playlist(multi: &MultiHlsServer, broadcast: &str) -> Response {
    let Some(video) = multi.video(broadcast) else {
        return (StatusCode::NOT_FOUND, format!("unknown broadcast {broadcast}")).into_response();
    };
    let mut master = crate::master::MasterPlaylist::default();
    let audio_group_id = "audio";
    let audio_server = multi.audio(broadcast);
    let has_audio = audio_server.is_some();
    if has_audio {
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
    let subtitles_group_id = "subs";
    let has_subtitles = multi.subtitles(broadcast).is_some();
    if has_subtitles {
        master.renditions.push(crate::master::MediaRendition {
            rendition_type: crate::master::MediaRenditionType::Subtitles,
            group_id: subtitles_group_id.into(),
            name: "English".into(),
            uri: "captions/playlist.m3u8".into(),
            default: true,
            autoselect: true,
            language: Some("en".into()),
        });
    }
    // Video codec string comes from the init segment parsed at
    // `push_init` time via `lvqr_cmaf::detect_video_codec_string`.
    // Falls back to the session-13 default when no init segment
    // has arrived yet so a client hitting master.m3u8 before the
    // first fragment still sees a syntactically valid variant.
    let video_codec = video
        .video_codec_string()
        .await
        .unwrap_or_else(|| "avc1.640020".to_string());
    // Audio codec string is pulled the same way from the audio
    // sibling server's cached init-segment decode. Session 30
    // added `detect_audio_codec_string` and the parallel
    // `audio_codec_string()` accessor; the fallback is the
    // pre-session-30 hardcode (`mp4a.40.2`) so a client hitting
    // master.m3u8 before the audio init lands still sees a
    // syntactically valid variant.
    let audio_codec = match audio_server {
        Some(audio) => audio
            .audio_codec_string()
            .await
            .unwrap_or_else(|| "mp4a.40.2".to_string()),
        None => String::new(),
    };
    let codecs = if has_audio {
        format!("{video_codec},{audio_codec}")
    } else {
        video_codec
    };

    // Session 106 C: multi-variant master playlist. Scan the tracked
    // broadcasts for `<source>/<name>` siblings that have a live
    // video rendition; emit one variant per sibling, sorted highest-
    // to-lowest bandwidth per the HLS ABR-client convention. The
    // source variant is always included as the top-or-bottom entry;
    // we bias it highest so clients that honour playlist order pick
    // the source first.
    let ladder = multi.ladder();
    let siblings = multi.variant_siblings(broadcast);
    let mut sibling_variants: Vec<(u64, crate::master::VariantStream)> = Vec::new();
    for suffix in &siblings {
        let Some(meta) = ladder.iter().find(|m| &m.name == suffix) else {
            continue;
        };
        // Only emit the sibling if the rendition broadcaster has a
        // live video rendition; empty siblings are in-progress and
        // should not show up as a variant until they have one frame.
        let sibling_broadcast = format!("{broadcast}/{suffix}");
        if multi.video(&sibling_broadcast).is_none() {
            continue;
        }
        let has_sibling_audio = multi.audio(&sibling_broadcast).is_some();
        // Each rendition is self-contained (video + audio served by
        // its own per-broadcast HLS surface), so the master playlist
        // does NOT reference the top-level audio group from the
        // rendition variants. The rendition's audio playlist is
        // reachable at `<rendition>/audio.m3u8`, the same relative
        // shape the source variant uses.
        let variant = crate::master::VariantStream {
            bandwidth_bps: meta.bandwidth_bps,
            codecs: meta.codecs.clone(),
            resolution: meta.resolution,
            audio_group: None,
            subtitles_group: has_subtitles.then(|| subtitles_group_id.to_string()),
            uri: format!("./{suffix}/playlist.m3u8"),
        };
        let _ = has_sibling_audio;
        sibling_variants.push((meta.bandwidth_bps, variant));
    }

    let source_bandwidth_bps = multi.source_bandwidth_bps().unwrap_or_else(|| {
        if let Some(top) = ladder.iter().map(|m| m.bandwidth_bps).max() {
            top + top / 5
        } else {
            // Pre-session-106-C default: 2.5 Mbps flat estimate.
            2_500_000
        }
    });
    let source_variant = crate::master::VariantStream {
        bandwidth_bps: source_bandwidth_bps,
        codecs,
        resolution: None,
        audio_group: has_audio.then(|| audio_group_id.to_string()),
        subtitles_group: has_subtitles.then(|| subtitles_group_id.to_string()),
        uri: "playlist.m3u8".into(),
    };

    // Sort siblings highest-to-lowest; the source variant is
    // inserted at the front so clients honouring playlist order pick
    // the source first.
    sibling_variants.sort_by_key(|b| std::cmp::Reverse(b.0));
    master.variants.push(source_variant);
    for (_, variant) in sibling_variants {
        master.variants.push(variant);
    }

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

    #[tokio::test]
    async fn second_push_init_marks_next_segment_discontinuity() {
        // First push_init is the start of stream and must NOT mark a
        // discontinuity; the second push_init (publisher reconnect on
        // the same broadcast slot, RFC 8216bis §4.4.4.4 trigger) must.
        let server = HlsServer::new(PlaylistBuilderConfig::default());
        // Initial init: first segment renders without discontinuity.
        server.push_init(Bytes::from_static(b"init-v1")).await;
        let seg0 = mk_chunk(0, 90_000, CmafChunkKind::Segment);
        server
            .push_chunk_bytes(&seg0, Bytes::from_static(b"seg0"))
            .await
            .unwrap();
        server.state.builder.write().await.close_pending_segment();
        let m1 = server.state.builder.read().await.manifest().render();
        assert!(
            !m1.contains("#EXT-X-DISCONTINUITY"),
            "first segment must not be a discontinuity boundary; got:\n{m1}"
        );
        // Replacement init -> next-closed segment must carry the tag.
        server.push_init(Bytes::from_static(b"init-v2")).await;
        let seg1 = mk_chunk(90_000, 90_000, CmafChunkKind::Segment);
        server
            .push_chunk_bytes(&seg1, Bytes::from_static(b"seg1"))
            .await
            .unwrap();
        server.state.builder.write().await.close_pending_segment();
        let m2 = server.state.builder.read().await.manifest().render();
        assert!(
            m2.contains("#EXT-X-DISCONTINUITY"),
            "replacement init must mark next-closed segment as discontinuity; got:\n{m2}"
        );
    }

    #[tokio::test]
    async fn redirect_to_owner_includes_full_path_and_302() {
        let resp = redirect_to_owner("http://a.local:8888", "live/test/master.m3u8");
        assert_eq!(resp.status(), StatusCode::FOUND);
        let loc = resp
            .headers()
            .get(axum::http::header::LOCATION)
            .expect("location")
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(loc, "http://a.local:8888/hls/live/test/master.m3u8");
    }

    #[tokio::test]
    async fn redirect_to_owner_tolerates_trailing_slash_on_base() {
        let resp = redirect_to_owner("http://a.local:8888/", "x/init.mp4");
        let loc = resp
            .headers()
            .get(axum::http::header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(loc, "http://a.local:8888/hls/x/init.mp4");
    }

    #[tokio::test]
    async fn resolve_redirect_base_returns_none_without_resolver() {
        let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());
        assert!(multi.resolve_redirect_base("live/test").await.is_none());
    }

    #[tokio::test]
    async fn resolve_redirect_base_invokes_installed_resolver() {
        let resolver: OwnerResolver = Arc::new(|broadcast| {
            Box::pin(async move {
                if broadcast == "live/test" {
                    Some("http://a.local:8888".to_string())
                } else {
                    None
                }
            })
        });
        let multi = MultiHlsServer::with_owner_resolver(PlaylistBuilderConfig::default(), resolver);
        assert_eq!(
            multi.resolve_redirect_base("live/test").await,
            Some("http://a.local:8888".to_string())
        );
        assert_eq!(multi.resolve_redirect_base("live/other").await, None);
    }

    #[tokio::test]
    async fn unknown_broadcast_redirects_to_owner_via_router() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let resolver: OwnerResolver = Arc::new(|broadcast| {
            Box::pin(async move {
                if broadcast == "live/test" {
                    Some("http://a.local:8888".to_string())
                } else {
                    None
                }
            })
        });
        let multi = MultiHlsServer::with_owner_resolver(PlaylistBuilderConfig::default(), resolver);
        let app = multi.router();

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/hls/live/test/master.m3u8")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request");

        assert_eq!(resp.status(), StatusCode::FOUND);
        let loc = resp
            .headers()
            .get(axum::http::header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(loc, "http://a.local:8888/hls/live/test/master.m3u8");
    }

    #[tokio::test]
    async fn unknown_broadcast_without_resolver_match_returns_404() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let resolver: OwnerResolver = Arc::new(|_b| Box::pin(async move { None }));
        let multi = MultiHlsServer::with_owner_resolver(PlaylistBuilderConfig::default(), resolver);
        let app = multi.router();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/hls/unknown/master.m3u8")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn master_playlist_emits_one_variant_per_rendition_sibling() {
        use crate::master::RenditionMeta;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());
        // Source broadcast + three rendition siblings. Each rendition
        // ensures a video rendition so the composer emits a variant
        // for it.
        let _ = multi.ensure_video("live/demo");
        let _ = multi.ensure_video("live/demo/720p");
        let _ = multi.ensure_video("live/demo/480p");
        let _ = multi.ensure_video("live/demo/240p");

        multi.set_ladder(vec![
            RenditionMeta {
                name: "720p".into(),
                bandwidth_bps: RenditionMeta::bandwidth_bps_with_overhead(2_500),
                resolution: Some((1280, 720)),
                codecs: "avc1.640028,mp4a.40.2".into(),
            },
            RenditionMeta {
                name: "480p".into(),
                bandwidth_bps: RenditionMeta::bandwidth_bps_with_overhead(1_200),
                resolution: Some((854, 480)),
                codecs: "avc1.640028,mp4a.40.2".into(),
            },
            RenditionMeta {
                name: "240p".into(),
                bandwidth_bps: RenditionMeta::bandwidth_bps_with_overhead(400),
                resolution: Some((426, 240)),
                codecs: "avc1.640028,mp4a.40.2".into(),
            },
        ]);

        let app = multi.router();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/hls/live/demo/master.m3u8")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request");
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.expect("body");
        let body = std::str::from_utf8(&body_bytes).expect("utf8").to_string();

        // Four variants: source + three renditions.
        let stream_inf_count = body.matches("#EXT-X-STREAM-INF").count();
        assert_eq!(stream_inf_count, 4, "master playlist should have 4 variants: {body}");
        // Each rendition's URI is the relative form the briefing locks in.
        assert!(body.contains("./720p/playlist.m3u8"), "body: {body}");
        assert!(body.contains("./480p/playlist.m3u8"), "body: {body}");
        assert!(body.contains("./240p/playlist.m3u8"), "body: {body}");
        // Rendition BANDWIDTH + RESOLUTION attributes round-tripped.
        assert!(body.contains("BANDWIDTH=2750000"), "720p kbps*1.1 = 2750000: {body}");
        assert!(body.contains("RESOLUTION=1280x720"));
        assert!(body.contains("RESOLUTION=854x480"));
        assert!(body.contains("RESOLUTION=426x240"));
        // Source bandwidth default: max(ladder) * 1.2 = 2750000 * 1.2 = 3300000.
        assert!(
            body.contains("BANDWIDTH=3300000"),
            "source variant bandwidth missing: {body}"
        );
        // Source variant first (highest bandwidth, honouring playlist order).
        let source_pos = body.find("\nplaylist.m3u8").expect("source uri present");
        let first_rend_pos = body.find("./720p/playlist.m3u8").expect("720p uri present");
        assert!(
            source_pos < first_rend_pos,
            "source variant must be emitted first: {body}"
        );
    }

    #[tokio::test]
    async fn master_playlist_source_bandwidth_override_applies() {
        use crate::master::RenditionMeta;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());
        let _ = multi.ensure_video("live/demo");
        let _ = multi.ensure_video("live/demo/720p");
        multi.set_ladder(vec![RenditionMeta {
            name: "720p".into(),
            bandwidth_bps: 2_750_000,
            resolution: Some((1280, 720)),
            codecs: "avc1.640028,mp4a.40.2".into(),
        }]);
        multi.set_source_bandwidth_bps(Some(9_000_000));

        let app = multi.router();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/hls/live/demo/master.m3u8")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body = std::str::from_utf8(&body_bytes).unwrap().to_string();
        assert!(body.contains("BANDWIDTH=9000000"), "override not applied: {body}");
    }

    #[tokio::test]
    async fn known_broadcast_skips_resolver_and_serves_locally() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        // Resolver would redirect if consulted, but the broadcast
        // is hosted locally so the redirect path MUST NOT trigger.
        let resolver: OwnerResolver =
            Arc::new(|_b| Box::pin(async move { Some("http://elsewhere.invalid".to_string()) }));
        let multi = MultiHlsServer::with_owner_resolver(PlaylistBuilderConfig::default(), resolver);
        // Ensure a local broadcast exists so the "unknown" guard
        // is not triggered. master.m3u8 for a fresh broadcast with
        // no init segment responds with a syntactically valid
        // variant (video codec falls through to the default).
        let _video = multi.ensure_video("live/local");
        let app = multi.router();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/hls/live/local/master.m3u8")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request");
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

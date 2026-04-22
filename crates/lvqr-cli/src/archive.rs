//! Archive fragment indexer: records every broadcaster-emitted
//! fragment into a `lvqr_archive::RedbSegmentIndex` and writes the
//! payload bytes to an on-disk file the index row points at.
//!
//! Wired by `lib.rs::start` when `ServeConfig::archive_dir` is
//! `Some`. Session 59 switched this path off the observer hook and
//! onto the broadcaster registry surface; session 60 finished the
//! HLS + DASH consumer switchovers so the registry is now the sole
//! dispatch path.
//!
//! Each fragment becomes one row. The LVQR bridge currently emits
//! one `moof+mdat` Fragment per video NAL / per AAC access unit, so
//! the index granularity matches the smallest addressable media
//! unit. Range scans return rows ordered by `start_dts`, which is
//! exactly the DVR scrub primitive the archive is for.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
#[cfg(feature = "c2pa")]
use lvqr_archive::provenance::{C2paConfig, finalize_broadcast_signed};
#[cfg(feature = "c2pa")]
use lvqr_archive::writer::init_segment_path;
use lvqr_archive::writer::{write_init, write_segment};
use lvqr_archive::{RedbSegmentIndex, SegmentIndex, SegmentRef};
use lvqr_auth::{AuthContext, AuthDecision, SharedAuth};
use lvqr_fragment::{FragmentBroadcasterRegistry, FragmentStream};
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;

/// Broadcaster-native archive indexer. Session 59 consumer-side
/// switchover: replaces [`IndexingFragmentObserver`]'s
/// FragmentObserver-hook dispatch with a
/// [`FragmentBroadcasterRegistry::on_entry_created`] callback that
/// spawns one tokio task per `(broadcast, track)` and drains
/// `next_fragment()` into the same on-disk layout and redb index the
/// observer-based path used.
///
/// Install via [`BroadcasterArchiveIndexer::install`] exactly once at
/// server startup, *after* every ingest crate has been constructed
/// with the same shared [`FragmentBroadcasterRegistry`]. The callback
/// runs on fresh insertion into the registry; every fragment emitted
/// subsequently is written to disk + indexed by the spawned drain
/// task. No state lives on the indexer itself -- it is a one-shot
/// wiring helper.
///
/// Differences from the observer-based version:
///
/// * No explicit `on_init` message. The init segment bytes are not
///   written to disk as a standalone file (they were not under the
///   observer path either). `timescale` is captured from the
///   broadcaster's meta, which is installed at
///   `FragmentBroadcaster::new` time and never changes.
///
/// * No shared-state map. Each `(broadcast, track)` gets its own
///   drain task with its own segment_seq counter; tasks terminate
///   cleanly when the producer side drops the broadcaster (i.e.
///   `registry.remove` + every ingest clone dropped).
pub(crate) struct BroadcasterArchiveIndexer;

impl BroadcasterArchiveIndexer {
    /// Register an `on_entry_created` callback on the supplied
    /// registry. The callback spawns a drain task per broadcaster on
    /// the current tokio runtime; callers must invoke this from
    /// inside a tokio runtime.
    ///
    /// `c2pa_config` is only meaningful when the `c2pa` feature is
    /// on; it is threaded through to the drain task so broadcast-
    /// end finalize runs via
    /// [`lvqr_archive::provenance::finalize_broadcast_signed`] when
    /// the drain loop terminates (i.e. when every producer-side
    /// clone of the broadcaster has dropped, typically after the
    /// RtmpMoqBridge's `on_unpublish` callback calls
    /// `registry.remove`). On non-c2pa builds the argument is
    /// accepted as a unit-typed placeholder so the call-site
    /// signature stays identical -- session 94 B3.
    #[cfg(feature = "c2pa")]
    pub fn install(
        archive_dir: PathBuf,
        index: Arc<RedbSegmentIndex>,
        registry: &FragmentBroadcasterRegistry,
        c2pa_config: Option<C2paConfig>,
    ) {
        Self::install_inner(archive_dir, index, registry, c2pa_config);
    }

    /// Non-c2pa variant: same install semantics without the
    /// broadcast-end finalize hook.
    #[cfg(not(feature = "c2pa"))]
    pub fn install(archive_dir: PathBuf, index: Arc<RedbSegmentIndex>, registry: &FragmentBroadcasterRegistry) {
        Self::install_inner(archive_dir, index, registry, ());
    }

    fn install_inner(
        archive_dir: PathBuf,
        index: Arc<RedbSegmentIndex>,
        registry: &FragmentBroadcasterRegistry,
        #[cfg(feature = "c2pa")] c2pa_config: Option<C2paConfig>,
        #[cfg(not(feature = "c2pa"))] _c2pa_config: (),
    ) {
        let dir_root = archive_dir;
        let index_root = index;
        #[cfg(feature = "c2pa")]
        let c2pa_root = c2pa_config;
        registry.on_entry_created(move |broadcast, track, bc| {
            let dir = dir_root.clone();
            let index = Arc::clone(&index_root);
            let broadcast = broadcast.to_string();
            let track = track.to_string();
            // Subscribe synchronously inside the callback so no emit
            // can race ahead of the drain loop. The subscription is
            // handed to the spawned task by move capture.
            let sub = bc.subscribe();
            let timescale = bc.meta().timescale;
            let handle = match Handle::try_current() {
                Ok(h) => h,
                Err(_) => {
                    tracing::warn!(
                        broadcast = %broadcast,
                        track = %track,
                        "BroadcasterArchiveIndexer: callback fired outside tokio runtime; drain not spawned",
                    );
                    return;
                }
            };
            // Deliberately DO NOT hold an Arc<FragmentBroadcaster> in the
            // drain task. Doing so would keep the broadcast::Sender alive
            // via the Arc and the recv loop would never see Closed after
            // every ingest-side clone dropped -- the task would leak and
            // redb would hold its exclusive lock forever, exactly the
            // shutdown bug the session 54 draft discovered for the
            // subscribe path. The BroadcasterStream already owns only
            // the Receiver side, so this is correct.
            #[cfg(feature = "c2pa")]
            {
                let c2pa = c2pa_root.clone();
                handle.spawn(Self::drain(dir, index, broadcast, track, timescale, sub, c2pa));
            }
            #[cfg(not(feature = "c2pa"))]
            {
                handle.spawn(Self::drain(dir, index, broadcast, track, timescale, sub));
            }
        });
    }

    async fn drain(
        dir: PathBuf,
        index: Arc<RedbSegmentIndex>,
        broadcast: String,
        track: String,
        timescale: u32,
        mut sub: lvqr_fragment::BroadcasterStream,
        #[cfg(feature = "c2pa")] c2pa_config: Option<C2paConfig>,
    ) {
        let mut segment_seq: u64 = 0;
        // Track whether the init segment has been persisted to disk.
        // The RTMP bridge sets the init segment on the broadcaster's
        // meta AFTER the FragmentBroadcaster is created (the FLV
        // sequence header lands after the first get_or_create), so
        // the subscription's initial snapshot does not carry it. On
        // each fragment iteration we refresh meta and, if init is
        // now available and not yet persisted, fire a spawn_blocking
        // `write_init` to land `<archive>/<broadcast>/<track>/init.mp4`.
        // Session 94 B3: this is the on-disk surface the drain-
        // terminated C2PA finalize path reads back.
        let mut init_persisted = false;
        while let Some(fragment) = sub.next_fragment().await {
            if !init_persisted {
                sub.refresh_meta();
                if let Some(init_bytes) = sub.meta().init_segment.as_ref() {
                    let dir_for_task = dir.clone();
                    let broadcast_for_task = broadcast.clone();
                    let track_for_task = track.clone();
                    let init_vec = init_bytes.to_vec();
                    let join = tokio::task::spawn_blocking(move || {
                        write_init(&dir_for_task, &broadcast_for_task, &track_for_task, &init_vec)
                    })
                    .await;
                    match join {
                        Ok(Ok(_)) => {
                            init_persisted = true;
                        }
                        Ok(Err(e)) => {
                            tracing::warn!(
                                error = %e,
                                broadcast = %broadcast,
                                track = %track,
                                "broadcaster archive: write_init failed",
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                broadcast = %broadcast,
                                track = %track,
                                "broadcaster archive: write_init join error",
                            );
                        }
                    }
                }
            }
            if fragment.duration == 0 {
                continue;
            }
            segment_seq += 1;
            let start_dts = fragment.dts;
            let end_dts = fragment.dts.saturating_add(fragment.duration);
            let keyframe_start = fragment.flags.keyframe;
            let payload = fragment.payload.clone();
            let length = payload.len() as u64;
            let dir_for_task = dir.clone();
            let broadcast_seg = broadcast.clone();
            let track_seg = track.clone();
            let index_task = Arc::clone(&index);
            // Session 88 A1: the segment layout + synchronous
            // write live in `lvqr_archive::writer::write_segment`.
            // `spawn_blocking` is still this caller's job because
            // the multi-thread tokio runtime can absorb the
            // blocking syscall on a worker thread; session 88 A2
            // will swap the body of `write_segment` behind an
            // `io-uring` feature without touching this call site.
            tokio::task::spawn_blocking(move || {
                let path = match write_segment(&dir_for_task, &broadcast_seg, &track_seg, segment_seq, payload.as_ref())
                {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            broadcast = %broadcast_seg,
                            track = %track_seg,
                            seq = segment_seq,
                            "broadcaster archive: write_segment failed",
                        );
                        return;
                    }
                };
                let path_str = match path.to_str() {
                    Some(s) => s.to_string(),
                    None => {
                        tracing::warn!(
                            path = %path.display(),
                            "broadcaster archive: path is not valid utf-8"
                        );
                        return;
                    }
                };
                let seg = SegmentRef {
                    broadcast: broadcast_seg,
                    track: track_seg,
                    segment_seq,
                    start_dts,
                    end_dts,
                    timescale,
                    keyframe_start,
                    path: path_str,
                    byte_offset: 0,
                    length,
                };
                if let Err(e) = index_task.record(&seg) {
                    tracing::warn!(error = ?e, "broadcaster archive: index.record failed");
                }
            });
        }
        tracing::info!(
            broadcast = %broadcast,
            track = %track,
            "BroadcasterArchiveIndexer: drain terminated (producers closed)",
        );

        // Tier 4 item 4.3 session B3: drain-terminated C2PA finalize.
        // The while loop above exits when every producer-side clone of
        // the broadcaster has been dropped -- typically driven by the
        // RtmpMoqBridge's on_unpublish callback calling
        // `registry.remove`. That is the per-broadcast moment we
        // finalize + sign the concatenated asset.
        #[cfg(feature = "c2pa")]
        if let Some(config) = c2pa_config {
            Self::finalize_c2pa(dir, index, broadcast, track, config).await;
        }
    }

    /// Drain-termination C2PA finalize. Reads
    /// `<archive>/<broadcast>/<track>/init.mp4`, walks the redb
    /// segment index for this `(broadcast, track)` in `start_dts`
    /// order, and calls
    /// [`lvqr_archive::provenance::finalize_broadcast_signed`] inside
    /// `tokio::task::spawn_blocking` (the finalize primitive is sync
    /// and does on-disk reads + signing, so it needs to leave the
    /// reactor). Writes `finalized.mp4` + `finalized.c2pa` next to
    /// the segment files.
    ///
    /// Errors are logged at `warn!`; no retry. Operators re-derive
    /// by inspecting the archive and re-signing manually per the
    /// session 94 B3 decision.
    #[cfg(feature = "c2pa")]
    async fn finalize_c2pa(
        dir: PathBuf,
        index: Arc<RedbSegmentIndex>,
        broadcast: String,
        track: String,
        config: C2paConfig,
    ) {
        let join = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let init_path = init_segment_path(&dir, &broadcast, &track);
            let init_bytes = match std::fs::read(&init_path) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(format!(
                        "init segment not persisted at {}; skipping finalize",
                        init_path.display()
                    ));
                }
                Err(e) => {
                    return Err(format!("read init {}: {e}", init_path.display()));
                }
            };
            let rows = index
                .find_range(&broadcast, &track, 0, u64::MAX)
                .map_err(|e| format!("index find_range: {e}"))?;
            if rows.is_empty() {
                return Err("no archived segments for (broadcast, track); skipping finalize".to_string());
            }
            let segment_paths: Vec<PathBuf> = rows.into_iter().map(|r| PathBuf::from(r.path)).collect();
            let asset_path = dir.join(&broadcast).join(&track).join("finalized.mp4");
            let manifest_path = dir.join(&broadcast).join(&track).join("finalized.c2pa");
            let signed = finalize_broadcast_signed(
                &config,
                &init_bytes,
                &segment_paths,
                "video/mp4",
                &asset_path,
                &manifest_path,
            )
            .map_err(|e| format!("finalize_broadcast_signed: {e}"))?;
            tracing::info!(
                broadcast = %broadcast,
                track = %track,
                asset = %asset_path.display(),
                manifest = %manifest_path.display(),
                manifest_bytes = signed.manifest_bytes.len(),
                segments = segment_paths.len(),
                "C2PA finalize: wrote signed asset and manifest",
            );
            Ok(())
        })
        .await;
        match join {
            Ok(Ok(())) => {}
            Ok(Err(msg)) => tracing::warn!(error = %msg, "broadcaster archive: c2pa finalize failed"),
            Err(e) => tracing::warn!(error = %e, "broadcaster archive: c2pa finalize join error"),
        }
    }
}

/// JSON-shaped mirror of [`SegmentRef`] for the playback endpoint.
/// A dedicated DTO keeps the wire format stable even if the
/// in-memory struct gains new fields.
#[derive(Debug, Serialize)]
pub(crate) struct PlaybackSegment {
    pub broadcast: String,
    pub track: String,
    pub segment_seq: u64,
    pub start_dts: u64,
    pub end_dts: u64,
    pub timescale: u32,
    pub keyframe_start: bool,
    pub path: String,
    pub byte_offset: u64,
    pub length: u64,
}

impl From<SegmentRef> for PlaybackSegment {
    fn from(seg: SegmentRef) -> Self {
        Self {
            broadcast: seg.broadcast,
            track: seg.track,
            segment_seq: seg.segment_seq,
            start_dts: seg.start_dts,
            end_dts: seg.end_dts,
            timescale: seg.timescale,
            keyframe_start: seg.keyframe_start,
            path: seg.path,
            byte_offset: seg.byte_offset,
            length: seg.length,
        }
    }
}

/// Query parameters for `GET /playback/{*broadcast}`. `track`
/// defaults to `0.mp4` (video) so a caller who only wants the
/// video rendition can omit it; `from` defaults to `0` and `to`
/// to `u64::MAX` so omitting both returns every recorded segment
/// for the stream.
#[derive(Debug, Deserialize)]
pub(crate) struct PlaybackQuery {
    #[serde(default)]
    pub track: Option<String>,
    #[serde(default)]
    pub from: Option<u64>,
    #[serde(default)]
    pub to: Option<u64>,
    #[serde(default)]
    pub token: Option<String>,
}

/// Router state shared between the three `/playback/*` handlers.
/// Carries a canonicalized copy of the archive directory so the
/// `file` handler can reject path traversal in constant time,
/// the shared [`SharedAuth`] provider so every handler honors
/// the same subscribe-token semantics the WS relay uses, and
/// the shared `RedbSegmentIndex` handle so the sync scans do
/// not race redb's exclusive-file lock against the writer.
#[derive(Clone)]
pub(crate) struct ArchiveState {
    pub dir: Arc<PathBuf>,
    pub canonical_dir: Arc<PathBuf>,
    pub index: Arc<RedbSegmentIndex>,
    pub auth: SharedAuth,
}

/// Extract a bearer token from an incoming request. Two
/// transports are honored, matching the WS relay's resolver:
///
/// 1. `Authorization: Bearer <token>` header -- the HTTP-native
///    form that every `curl` / `reqwest` / browser can produce.
/// 2. `?token=<token>` query parameter -- accepted as a fallback
///    for clients that cannot set custom headers (e.g. a plain
///    `<video src>` tag or a simple file URL), at the cost of
///    leaking the token into access logs. The WS handlers log a
///    deprecation warning on this path; the playback surface
///    accepts it silently for now because it is much newer and
///    the deprecation plan is tracked separately.
fn extract_bearer(headers: &HeaderMap, token_query: &Option<String>) -> Option<String> {
    if let Some(hv) = headers.get(header::AUTHORIZATION)
        && let Ok(raw) = hv.to_str()
        && let Some(tok) = raw.strip_prefix("Bearer ")
        && !tok.is_empty()
    {
        return Some(tok.to_string());
    }
    token_query.as_ref().filter(|t| !t.is_empty()).cloned()
}

/// Run the `SharedAuth` subscribe check for a playback request.
/// Returns `None` on allow; returns `Some(Response)` with a 401
/// body when the provider denies, so callers can `return` it
/// directly.
fn playback_auth_gate(auth: &SharedAuth, broadcast: &str, token: Option<String>) -> Option<Response> {
    let decision = auth.check(&AuthContext::Subscribe {
        token,
        broadcast: broadcast.to_string(),
    });
    if let AuthDecision::Deny { reason } = decision {
        metrics::counter!("lvqr_auth_failures_total", "entry" => "playback").increment(1);
        return Some((StatusCode::UNAUTHORIZED, reason).into_response());
    }
    None
}

async fn playback_handler(
    State(state): State<ArchiveState>,
    headers: HeaderMap,
    AxumPath(broadcast): AxumPath<String>,
    Query(params): Query<PlaybackQuery>,
) -> Response {
    let token = extract_bearer(&headers, &params.token);
    if let Some(resp) = playback_auth_gate(&state.auth, &broadcast, token) {
        return resp;
    }

    let track = params.track.as_deref().unwrap_or("0.mp4");
    let from = params.from.unwrap_or(0);
    let to = params.to.unwrap_or(u64::MAX);

    // redb is synchronous and holds an exclusive file lock, so the
    // scan itself is fast but still blocks the current task.
    // `spawn_blocking` keeps the admin axum runtime responsive for
    // other requests while the scan runs.
    let index = Arc::clone(&state.index);
    let broadcast_owned = broadcast.clone();
    let track_owned = track.to_string();
    let rows = tokio::task::spawn_blocking(move || index.find_range(&broadcast_owned, &track_owned, from, to)).await;

    match rows {
        Ok(Ok(rows)) => {
            let body: Vec<PlaybackSegment> = rows.into_iter().map(Into::into).collect();
            Json(body).into_response()
        }
        Ok(Err(e)) => {
            tracing::warn!(broadcast = %broadcast, track, error = %e, "playback: index query failed");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("index error: {e}")).into_response()
        }
        Err(e) => {
            tracing::warn!(broadcast = %broadcast, track, error = %e, "playback: join error");
            (StatusCode::INTERNAL_SERVER_ERROR, "scan task panicked").into_response()
        }
    }
}

/// Query parameters for `GET /playback/latest/{*broadcast}`.
#[derive(Debug, Deserialize)]
pub(crate) struct LatestQuery {
    #[serde(default)]
    pub track: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

async fn latest_handler(
    State(state): State<ArchiveState>,
    headers: HeaderMap,
    AxumPath(broadcast): AxumPath<String>,
    Query(params): Query<LatestQuery>,
) -> Response {
    let token = extract_bearer(&headers, &params.token);
    if let Some(resp) = playback_auth_gate(&state.auth, &broadcast, token) {
        return resp;
    }

    let track = params.track.as_deref().unwrap_or("0.mp4").to_string();
    let index = Arc::clone(&state.index);
    let broadcast_owned = broadcast.clone();
    let track_owned = track.clone();
    let row = tokio::task::spawn_blocking(move || index.latest(&broadcast_owned, &track_owned)).await;

    match row {
        Ok(Ok(Some(seg))) => Json::<PlaybackSegment>(seg.into()).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "no segments for (broadcast, track)").into_response(),
        Ok(Err(e)) => {
            tracing::warn!(broadcast = %broadcast, track = %track, error = %e, "playback: latest query failed");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("index error: {e}")).into_response()
        }
        Err(e) => {
            tracing::warn!(broadcast = %broadcast, track = %track, error = %e, "playback: latest join error");
            (StatusCode::INTERNAL_SERVER_ERROR, "scan task panicked").into_response()
        }
    }
}

/// Query parameters for `GET /playback/file/{*rel}`. The only
/// field today is the token fallback; `rel` itself is the URL
/// path component.
#[derive(Debug, Deserialize)]
pub(crate) struct FileQuery {
    #[serde(default)]
    pub token: Option<String>,
}

/// Serve an archived fragment file by relative path, e.g.
/// `GET /playback/file/live/dvr/0.mp4/00000001.m4s`. `rel` is
/// joined onto the configured archive directory; the joined
/// path is canonicalized and rejected if it escapes the archive
/// root. Returns `application/octet-stream` bytes on success,
/// `404` when the file is missing, and `400` when the path
/// traversal guard trips.
///
/// Auth: the "broadcast" the request authorizes against is the
/// leading path component of `rel`. The writer's canonical
/// layout is `<broadcast_components...>/<track>/<seq>.m4s`,
/// where broadcasts typically contain exactly one slash
/// (`live/dvr`). The auth check treats everything up to the
/// track's `0.mp4` / `1.mp4` suffix as the broadcast; a JWT
/// with `sub:live/dvr` therefore authorizes every segment file
/// under that stream's archive subtree without authorizing
/// sibling streams.
async fn file_handler(
    State(state): State<ArchiveState>,
    headers: HeaderMap,
    AxumPath(rel): AxumPath<String>,
    Query(params): Query<FileQuery>,
) -> Response {
    let token = extract_bearer(&headers, &params.token);
    let broadcast_for_auth = broadcast_from_rel(&rel);
    if let Some(resp) = playback_auth_gate(&state.auth, broadcast_for_auth, token) {
        return resp;
    }

    let joined = state.dir.join(&rel);
    // Canonicalize and confirm the resolved path is still under
    // the canonicalized archive root. `canonicalize` fails with
    // `NotFound` when the file does not exist; treat that as a
    // 404 rather than a 500.
    let canonical = match std::fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (StatusCode::NOT_FOUND, format!("archive file not found: {rel}")).into_response();
        }
        Err(e) => {
            tracing::warn!(path = %joined.display(), error = %e, "archive: canonicalize failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("canonicalize error: {e}")).into_response();
        }
    };
    if !canonical.starts_with(state.canonical_dir.as_path()) {
        tracing::warn!(
            rel = %rel,
            resolved = %canonical.display(),
            "archive: file request escaped archive root"
        );
        return (StatusCode::BAD_REQUEST, "path escapes archive root").into_response();
    }

    let bytes = match tokio::fs::read(&canonical).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (StatusCode::NOT_FOUND, format!("archive file not found: {rel}")).into_response();
        }
        Err(e) => {
            tracing::warn!(path = %canonical.display(), error = %e, "archive: read failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("read error: {e}")).into_response();
        }
    };

    let total = bytes.len();

    // RFC 7233 range-request support. Most HTML5 `<video>` tags
    // issue `Range: bytes=0-` on the first request and then scrub
    // via `Range: bytes=N-` as the viewer seeks; without this
    // branch every seek re-downloads the whole segment from byte
    // zero. Multi-range requests (`bytes=0-10,20-30`) would need
    // `multipart/byteranges` encoding which is rare in the wild;
    // we fall through to 200 OK + full body on them so a client
    // that asks for multiple ranges still receives a valid
    // response, just not partitioned the way it expected.
    if let Some(hv) = headers.get(header::RANGE) {
        match parse_single_range(hv, total) {
            ParsedRange::Single(start, end) => {
                // Inclusive on both ends per RFC 7233; a request for
                // `bytes=0-0` returns exactly byte 0 (one byte).
                let slice = bytes[start..=end].to_vec();
                return Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .header(header::CONTENT_LENGTH, slice.len())
                    .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}"))
                    .header(header::ACCEPT_RANGES, "bytes")
                    .body(Body::from(slice))
                    .expect("valid response");
            }
            ParsedRange::Unsatisfiable => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                    .body(Body::empty())
                    .expect("valid response");
            }
            ParsedRange::Ignored => {
                // Fall through to the full-body response below.
            }
        }
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, total)
        .header(header::ACCEPT_RANGES, "bytes")
        .body(Body::from(bytes))
        .expect("valid response")
}

/// Outcome of attempting to parse a `Range:` header. `Single`
/// carries an inclusive `[start, end]` byte range; `Unsatisfiable`
/// means the header was well-formed but the range lies outside the
/// resource (416); `Ignored` means the header is malformed or
/// requests a form we do not implement (multi-range), in which case
/// the handler falls through to a normal 200 response so the client
/// still receives the full body.
#[derive(Debug, PartialEq, Eq)]
enum ParsedRange {
    Single(usize, usize),
    Unsatisfiable,
    Ignored,
}

fn parse_single_range(raw: &axum::http::HeaderValue, total: usize) -> ParsedRange {
    let Ok(s) = raw.to_str() else {
        return ParsedRange::Ignored;
    };
    let Some(spec) = s.trim().strip_prefix("bytes=") else {
        return ParsedRange::Ignored;
    };
    // Multi-range requests use comma-separated specs; fall back to
    // a full-body 200 OK rather than implementing
    // `multipart/byteranges`.
    if spec.contains(',') {
        return ParsedRange::Ignored;
    }
    let Some((start_s, end_s)) = spec.split_once('-') else {
        return ParsedRange::Ignored;
    };
    let start_s = start_s.trim();
    let end_s = end_s.trim();
    if total == 0 {
        // Empty file: every range is unsatisfiable.
        return ParsedRange::Unsatisfiable;
    }
    match (start_s.is_empty(), end_s.is_empty()) {
        (false, false) => {
            let Ok(start) = start_s.parse::<usize>() else {
                return ParsedRange::Ignored;
            };
            let Ok(end) = end_s.parse::<usize>() else {
                return ParsedRange::Ignored;
            };
            if start > end || start >= total {
                return ParsedRange::Unsatisfiable;
            }
            // Clamp the end to the final byte per RFC 7233: a
            // client asking for `bytes=0-999999` against a 100-byte
            // file gets the full 100 bytes.
            let end = end.min(total - 1);
            ParsedRange::Single(start, end)
        }
        (false, true) => {
            // `bytes=A-`: from A to end of resource.
            let Ok(start) = start_s.parse::<usize>() else {
                return ParsedRange::Ignored;
            };
            if start >= total {
                return ParsedRange::Unsatisfiable;
            }
            ParsedRange::Single(start, total - 1)
        }
        (true, false) => {
            // `bytes=-N`: suffix range, last N bytes.
            let Ok(n) = end_s.parse::<usize>() else {
                return ParsedRange::Ignored;
            };
            if n == 0 {
                return ParsedRange::Unsatisfiable;
            }
            let n = n.min(total);
            ParsedRange::Single(total - n, total - 1)
        }
        (true, true) => ParsedRange::Ignored,
    }
}

#[cfg(test)]
mod range_tests {
    use super::{ParsedRange, parse_single_range};
    use axum::http::HeaderValue;

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).unwrap()
    }

    #[test]
    fn bytes_a_b_inclusive() {
        assert_eq!(parse_single_range(&hv("bytes=0-99"), 200), ParsedRange::Single(0, 99));
        assert_eq!(
            parse_single_range(&hv("bytes=50-150"), 200),
            ParsedRange::Single(50, 150)
        );
        // Single byte: bytes=5-5 should return exactly byte 5.
        assert_eq!(parse_single_range(&hv("bytes=5-5"), 10), ParsedRange::Single(5, 5));
    }

    #[test]
    fn bytes_a_open_tail() {
        assert_eq!(
            parse_single_range(&hv("bytes=100-"), 200),
            ParsedRange::Single(100, 199)
        );
        assert_eq!(parse_single_range(&hv("bytes=0-"), 200), ParsedRange::Single(0, 199));
    }

    #[test]
    fn bytes_suffix_last_n() {
        assert_eq!(parse_single_range(&hv("bytes=-50"), 200), ParsedRange::Single(150, 199));
        // Suffix larger than resource: clamp to whole file.
        assert_eq!(parse_single_range(&hv("bytes=-9999"), 200), ParsedRange::Single(0, 199));
    }

    #[test]
    fn end_clamped_to_last_byte() {
        // Over-long end: RFC 7233 says clamp.
        assert_eq!(
            parse_single_range(&hv("bytes=0-99999"), 100),
            ParsedRange::Single(0, 99)
        );
    }

    #[test]
    fn start_at_or_beyond_length_is_unsatisfiable() {
        assert_eq!(
            parse_single_range(&hv("bytes=200-300"), 200),
            ParsedRange::Unsatisfiable
        );
        assert_eq!(parse_single_range(&hv("bytes=500-"), 200), ParsedRange::Unsatisfiable);
    }

    #[test]
    fn zero_suffix_is_unsatisfiable() {
        assert_eq!(parse_single_range(&hv("bytes=-0"), 200), ParsedRange::Unsatisfiable);
    }

    #[test]
    fn backwards_range_is_unsatisfiable() {
        assert_eq!(parse_single_range(&hv("bytes=100-50"), 200), ParsedRange::Unsatisfiable);
    }

    #[test]
    fn malformed_headers_are_ignored() {
        // Wrong unit (bytes= prefix missing).
        assert_eq!(parse_single_range(&hv("0-99"), 200), ParsedRange::Ignored);
        // Non-numeric.
        assert_eq!(parse_single_range(&hv("bytes=abc-def"), 200), ParsedRange::Ignored);
        // Missing dash.
        assert_eq!(parse_single_range(&hv("bytes=100"), 200), ParsedRange::Ignored);
        // Empty on both sides.
        assert_eq!(parse_single_range(&hv("bytes=-"), 200), ParsedRange::Ignored);
    }

    #[test]
    fn multi_range_requests_fall_through() {
        // Multi-range is legal per RFC 7233 but we do not implement
        // `multipart/byteranges`, so the handler falls back to a
        // full-body 200 via the `Ignored` branch.
        assert_eq!(parse_single_range(&hv("bytes=0-10,20-30"), 200), ParsedRange::Ignored);
    }

    #[test]
    fn empty_file_every_range_unsatisfiable() {
        assert_eq!(parse_single_range(&hv("bytes=0-10"), 0), ParsedRange::Unsatisfiable);
        assert_eq!(parse_single_range(&hv("bytes=-10"), 0), ParsedRange::Unsatisfiable);
    }
}

/// Derive the broadcast key a `/playback/file/{*rel}` request
/// should authorize against. The writer's canonical layout is
/// `<broadcast>/<track>/<seq>.m4s` where `track` matches the
/// `N.mp4` MoQ convention. We walk backward from the end of
/// `rel`, drop the file segment and the track segment, and hand
/// the remaining prefix to `auth.check`. If the layout does not
/// match (only one component, or the track lookup fails), we
/// fall back to the full `rel` so a misconfigured request still
/// runs through the auth gate rather than slipping through.
fn broadcast_from_rel(rel: &str) -> &str {
    let trimmed = rel.trim_end_matches('/');
    // Strip the trailing `.../<seq>.m4s` component.
    let Some((head, _file)) = trimmed.rsplit_once('/') else {
        return trimmed;
    };
    // Strip the trailing `.../<track>` component. Only accept
    // `N.mp4`-shaped tracks so we do not over-trim a legitimate
    // path whose layout happens not to match.
    let Some((broadcast, track)) = head.rsplit_once('/') else {
        return trimmed;
    };
    if track.ends_with(".mp4") && track.len() > 4 && track[..track.len() - 4].chars().all(|c| c.is_ascii_digit()) {
        broadcast
    } else {
        trimmed
    }
}

/// Build the `/playback` router. Merged into the admin axum router
/// in `lib.rs::start` when `ServeConfig::archive_dir` is set.
///
/// Routes:
/// * `GET /playback/latest/{*broadcast}` -- single most-recent
///   segment for the stream, or 404 if none.
/// * `GET /playback/file/{*rel}` -- raw bytes of an archived
///   fragment file under the archive directory, guarded against
///   path traversal.
/// * `GET /playback/{*broadcast}` -- every segment overlapping the
///   `[from, to)` window (defaults `[0, u64::MAX)`), ordered by
///   `start_dts`.
///
/// The `latest` and `file` routes are declared first so axum's
/// more-specific match wins over the trailing catch-all on
/// `{*broadcast}`.
pub(crate) fn playback_router(dir: PathBuf, index: Arc<RedbSegmentIndex>, auth: SharedAuth) -> Router {
    let canonical_dir = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    let state = ArchiveState {
        dir: Arc::new(dir),
        canonical_dir: Arc::new(canonical_dir),
        index,
        auth,
    };
    Router::new()
        .route("/playback/latest/{*broadcast}", get(latest_handler))
        .route("/playback/file/{*rel}", get(file_handler))
        .route("/playback/{*broadcast}", get(playback_handler))
        .with_state(state)
}

/// Router state for `/playback/verify/{broadcast}`. Only the archive
/// directory + auth provider are needed; the verify handler reads the
/// signed asset + sidecar manifest from disk and calls
/// [`c2pa::Reader`] directly rather than touching the redb index.
#[cfg(feature = "c2pa")]
#[derive(Clone)]
pub(crate) struct VerifyState {
    pub dir: Arc<PathBuf>,
    pub auth: SharedAuth,
}

/// Response shape for `GET /playback/verify/{broadcast}`.
#[cfg(feature = "c2pa")]
#[derive(Debug, Serialize)]
pub(crate) struct VerifyResponse {
    /// Signer identity as reported by `c2pa::Manifest::issuer`. The
    /// returned string is the subject of the signing certificate
    /// (typically the operator's org name or a broadcast identifier);
    /// `null` when c2pa-rs could not extract a signer (e.g. the
    /// signature's certificate chain is malformed beyond the
    /// profile check).
    pub signer: Option<String>,
    /// ISO-8601 timestamp as reported by `c2pa::Manifest::time` (the
    /// RFC 3161 TSA countersignature when present, otherwise the
    /// signer's local claim-generator time). `null` when the manifest
    /// carries no signing timestamp.
    pub signed_at: Option<String>,
    /// `true` iff `c2pa::Reader::validation_state` returned `Valid`
    /// or `Trusted` (cryptographic integrity checks passed; trust
    /// is an operator-trust-list concern, not a cryptographic one).
    /// `false` for `Invalid` -- manifest parse failure, bad
    /// signature, or a non-severe validation error that the
    /// response's `errors` field details.
    pub valid: bool,
    /// `c2pa::Reader::validation_state` as a stable string
    /// (`"Invalid" | "Valid" | "Trusted"`). The stable form is
    /// exposed so clients can distinguish trust-list-validated
    /// manifests from cryptographically-valid-but-untrusted ones
    /// without relying on the enum's Rust discriminant.
    pub validation_state: &'static str,
    /// Per-failure validation messages from
    /// `c2pa::Reader::validation_status`. Contains success and
    /// informational codes as well; the verify route filters to
    /// failures only so client callers do not need to know the
    /// status-code taxonomy to decide "did this pass". Empty when
    /// no failures were reported.
    pub errors: Vec<String>,
}

/// Query parameters for `GET /playback/verify/{*broadcast}`. `track`
/// defaults to `0.mp4` (video) to match the sister playback routes.
#[cfg(feature = "c2pa")]
#[derive(Debug, Deserialize)]
pub(crate) struct VerifyQuery {
    #[serde(default)]
    pub track: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

/// `GET /playback/verify/{*broadcast}` -- read the drain-terminated
/// C2PA finalize pair (`finalized.mp4` + `finalized.c2pa`) from the
/// archive directory and verify the manifest via `c2pa::Reader`.
///
/// Returns [`VerifyResponse`] on success, `404` when either file is
/// missing (i.e. finalize has not run for this stream yet), `500`
/// when the manifest cannot be parsed even at the structural level.
/// Auth runs the same subscribe-token gate the other `/playback/*`
/// handlers use; operators who want a separate admin-token flow can
/// layer a stricter `AuthProvider` through `SharedAuth`.
#[cfg(feature = "c2pa")]
async fn verify_handler(
    State(state): State<VerifyState>,
    headers: HeaderMap,
    AxumPath(broadcast): AxumPath<String>,
    Query(params): Query<VerifyQuery>,
) -> Response {
    let token = extract_bearer(&headers, &params.token);
    if let Some(resp) = playback_auth_gate(&state.auth, &broadcast, token) {
        return resp;
    }

    let track = params.track.as_deref().unwrap_or("0.mp4").to_string();
    let dir = Arc::clone(&state.dir);
    let broadcast_owned = broadcast.clone();
    let track_owned = track.clone();
    let join = tokio::task::spawn_blocking(move || verify_on_disk(dir.as_path(), &broadcast_owned, &track_owned)).await;

    match join {
        Ok(Ok(resp)) => Json(resp).into_response(),
        Ok(Err(VerifyError::NotFound(path))) => (
            StatusCode::NOT_FOUND,
            format!("c2pa finalize artefact not found: {path}"),
        )
            .into_response(),
        Ok(Err(VerifyError::Read { path, error })) => {
            tracing::warn!(path = %path, error = %error, "verify: filesystem read failed");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("read {path}: {error}")).into_response()
        }
        Ok(Err(VerifyError::Parse(msg))) => {
            tracing::warn!(error = %msg, "verify: c2pa parse failed");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("c2pa parse error: {msg}")).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "verify: join error");
            (StatusCode::INTERNAL_SERVER_ERROR, "verify task panicked").into_response()
        }
    }
}

#[cfg(feature = "c2pa")]
enum VerifyError {
    NotFound(String),
    Read { path: String, error: std::io::Error },
    Parse(String),
}

#[cfg(feature = "c2pa")]
fn verify_on_disk(archive_dir: &std::path::Path, broadcast: &str, track: &str) -> Result<VerifyResponse, VerifyError> {
    let asset_path = archive_dir.join(broadcast).join(track).join("finalized.mp4");
    let manifest_path = archive_dir.join(broadcast).join(track).join("finalized.c2pa");
    let asset_bytes = match std::fs::read(&asset_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(VerifyError::NotFound(asset_path.display().to_string()));
        }
        Err(e) => {
            return Err(VerifyError::Read {
                path: asset_path.display().to_string(),
                error: e,
            });
        }
    };
    let manifest_bytes = match std::fs::read(&manifest_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(VerifyError::NotFound(manifest_path.display().to_string()));
        }
        Err(e) => {
            return Err(VerifyError::Read {
                path: manifest_path.display().to_string(),
                error: e,
            });
        }
    };

    let reader = c2pa::Reader::from_context(c2pa::Context::new())
        .with_manifest_data_and_stream(&manifest_bytes, "video/mp4", std::io::Cursor::new(asset_bytes))
        .map_err(|e| VerifyError::Parse(e.to_string()))?;

    let validation_state = reader.validation_state();
    let validation_state_str = match validation_state {
        c2pa::ValidationState::Invalid => "Invalid",
        c2pa::ValidationState::Valid => "Valid",
        c2pa::ValidationState::Trusted => "Trusted",
    };
    let valid = matches!(
        validation_state,
        c2pa::ValidationState::Valid | c2pa::ValidationState::Trusted
    );

    let signer = reader.active_manifest().and_then(|m| m.issuer());
    let signed_at = reader.active_manifest().and_then(|m| m.time());

    // `validation_status()` returns a flat list of validation codes;
    // `validation_results()` is a richer map when populated. Prefer
    // the richer map and fall back to the flat list. In both cases,
    // filter out `signingCredential.untrusted` so it does not appear
    // as a hard error: c2pa-rs itself treats it as non-fatal (see
    // `Reader::validation_state` in c2pa 0.80's reader.rs -- the
    // state is still Valid/Trusted when the only codes are
    // SIGNING_CREDENTIAL_UNTRUSTED). Callers that care about
    // "cryptographically valid vs. trust-list-validated" read the
    // `validation_state` field instead.
    let format_code = |s: &c2pa::validation_status::ValidationStatus| -> String {
        let code = s.code();
        match s.explanation() {
            Some(exp) => format!("{code}: {exp}"),
            None => code.to_string(),
        }
    };
    let is_hard_failure = |s: &c2pa::validation_status::ValidationStatus| -> bool {
        s.code() != c2pa::validation_status::SIGNING_CREDENTIAL_UNTRUSTED
    };
    let errors: Vec<String> = if let Some(results) = reader.validation_results() {
        results
            .active_manifest()
            .map(|m| m.failure())
            .into_iter()
            .flatten()
            .filter(|s| is_hard_failure(s))
            .map(format_code)
            .collect()
    } else if let Some(statuses) = reader.validation_status() {
        statuses
            .iter()
            .filter(|s| is_hard_failure(s))
            .map(format_code)
            .collect()
    } else {
        Vec::new()
    };

    Ok(VerifyResponse {
        signer,
        signed_at,
        valid,
        validation_state: validation_state_str,
        errors,
    })
}

/// Build the `/playback/verify` router. Merged into the admin axum
/// router in `lib.rs::start` when the `c2pa` feature is on AND the
/// archive directory is configured. Tier 4 item 4.3 session B3.
#[cfg(feature = "c2pa")]
pub(crate) fn verify_router(dir: PathBuf, auth: SharedAuth) -> Router {
    let state = VerifyState {
        dir: Arc::new(dir),
        auth,
    };
    Router::new()
        .route("/playback/verify/{*broadcast}", get(verify_handler))
        .with_state(state)
}

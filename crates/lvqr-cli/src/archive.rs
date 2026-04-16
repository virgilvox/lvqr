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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
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
    pub fn install(archive_dir: PathBuf, index: Arc<RedbSegmentIndex>, registry: &FragmentBroadcasterRegistry) {
        let dir_root = archive_dir;
        let index_root = index;
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
            handle.spawn(Self::drain(dir, index, broadcast, track, timescale, sub));
        });
    }

    async fn drain(
        dir: PathBuf,
        index: Arc<RedbSegmentIndex>,
        broadcast: String,
        track: String,
        timescale: u32,
        mut sub: lvqr_fragment::BroadcasterStream,
    ) {
        let mut segment_seq: u64 = 0;
        while let Some(fragment) = sub.next_fragment().await {
            if fragment.duration == 0 {
                continue;
            }
            segment_seq += 1;
            let start_dts = fragment.dts;
            let end_dts = fragment.dts.saturating_add(fragment.duration);
            let keyframe_start = fragment.flags.keyframe;
            let path = Self::segment_path(&dir, &broadcast, &track, segment_seq);
            let payload = fragment.payload.clone();
            let length = payload.len() as u64;
            let broadcast = broadcast.clone();
            let track = track.clone();
            let index = Arc::clone(&index);
            let path_for_task = path.clone();
            tokio::task::spawn_blocking(move || {
                if let Some(parent) = path_for_task.parent()
                    && let Err(e) = std::fs::create_dir_all(parent)
                {
                    tracing::warn!(error = ?e, dir = %parent.display(), "broadcaster archive: mkdir failed");
                    return;
                }
                if let Err(e) = std::fs::write(&path_for_task, payload.as_ref()) {
                    tracing::warn!(error = ?e, path = %path_for_task.display(), "broadcaster archive: fs::write failed");
                    return;
                }
                let path_str = match path_for_task.to_str() {
                    Some(s) => s.to_string(),
                    None => {
                        tracing::warn!(
                            path = %path_for_task.display(),
                            "broadcaster archive: path is not valid utf-8"
                        );
                        return;
                    }
                };
                let seg = SegmentRef {
                    broadcast,
                    track,
                    segment_seq,
                    start_dts,
                    end_dts,
                    timescale,
                    keyframe_start,
                    path: path_str,
                    byte_offset: 0,
                    length,
                };
                if let Err(e) = index.record(&seg) {
                    tracing::warn!(error = ?e, "broadcaster archive: index.record failed");
                }
            });
        }
        tracing::info!(
            broadcast = %broadcast,
            track = %track,
            "BroadcasterArchiveIndexer: drain terminated (producers closed)",
        );
    }

    fn segment_path(root: &Path, broadcast: &str, track: &str, seq: u64) -> PathBuf {
        root.join(broadcast).join(track).join(format!("{seq:08}.m4s"))
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

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(Body::from(bytes))
        .expect("valid response")
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

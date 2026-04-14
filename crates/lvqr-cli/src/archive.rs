//! Archive fragment observer: records every bridge-emitted fragment
//! into a `lvqr_archive::RedbSegmentIndex` and writes the payload
//! bytes to an on-disk file the index row points at.
//!
//! Wired by `lib.rs::start` when `ServeConfig::archive_dir` is
//! `Some`. The observer is attached to the bridge through the same
//! `FragmentObserver` pattern the LL-HLS bridge uses, composed via
//! [`TeeFragmentObserver`] when HLS is also enabled so both
//! consumers see every fragment.
//!
//! Each fragment becomes one row. The LVQR bridge currently emits
//! one `moof+mdat` Fragment per video NAL / per AAC access unit, so
//! the index granularity matches the smallest addressable media
//! unit. Range scans return rows ordered by `start_dts`, which is
//! exactly the DVR scrub primitive the archive is for.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{FromRef, Path as AxumPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use bytes::Bytes;
use lvqr_archive::{RedbSegmentIndex, SegmentIndex, SegmentRef};
use lvqr_fragment::Fragment;
use lvqr_ingest::{FragmentObserver, SharedFragmentObserver};
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;

/// Per-track state captured at `on_init` and consulted on every
/// subsequent `on_fragment`. The timescale comes from the bridge's
/// `on_init` signature (90 kHz for video, the AAC sample rate for
/// audio). `segment_seq` counts monotonic writes so the on-disk
/// filename is stable per-track.
struct TrackState {
    timescale: u32,
    segment_seq: u64,
}

/// Fragment observer that writes every fragment to
/// `<archive_dir>/<broadcast>/<track>/<seq>.m4s` and records a
/// `SegmentRef` into a shared `RedbSegmentIndex`.
pub(crate) struct IndexingFragmentObserver {
    archive_dir: PathBuf,
    index: Arc<RedbSegmentIndex>,
    tracks: Mutex<HashMap<(String, String), TrackState>>,
}

impl IndexingFragmentObserver {
    pub fn new(archive_dir: PathBuf, index: Arc<RedbSegmentIndex>) -> Self {
        Self {
            archive_dir,
            index,
            tracks: Mutex::new(HashMap::new()),
        }
    }

    fn segment_path(root: &Path, broadcast: &str, track: &str, seq: u64) -> PathBuf {
        root.join(broadcast).join(track).join(format!("{seq:08}.m4s"))
    }
}

impl FragmentObserver for IndexingFragmentObserver {
    fn on_init(&self, broadcast: &str, track: &str, timescale: u32, _init: Bytes) {
        let mut map = self.tracks.lock().expect("archive observer mutex poisoned");
        map.insert(
            (broadcast.to_string(), track.to_string()),
            TrackState {
                timescale,
                segment_seq: 0,
            },
        );
    }

    fn on_fragment(&self, broadcast: &str, track: &str, fragment: &Fragment) {
        if fragment.duration == 0 {
            return;
        }

        let (timescale, seq) = {
            let mut map = self.tracks.lock().expect("archive observer mutex poisoned");
            let Some(state) = map.get_mut(&(broadcast.to_string(), track.to_string())) else {
                // Fragment arrived before its init. Defensive branch;
                // the bridge invariant fires on_init first.
                return;
            };
            state.segment_seq += 1;
            (state.timescale, state.segment_seq)
        };

        let start_dts = fragment.dts;
        let end_dts = fragment.dts.saturating_add(fragment.duration);
        let keyframe_start = fragment.flags.keyframe;
        let path = Self::segment_path(&self.archive_dir, broadcast, track, seq);
        let payload = fragment.payload.clone();
        let length = payload.len() as u64;
        let broadcast_owned = broadcast.to_string();
        let track_owned = track.to_string();
        let index = Arc::clone(&self.index);

        let Ok(handle) = Handle::try_current() else {
            tracing::warn!("archive observer on_fragment outside tokio runtime; dropping fragment");
            return;
        };
        handle.spawn_blocking(move || {
            if let Some(parent) = path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                tracing::warn!(error = ?e, dir = %parent.display(), "archive: mkdir failed");
                return;
            }
            if let Err(e) = std::fs::write(&path, payload.as_ref()) {
                tracing::warn!(error = ?e, path = %path.display(), "archive: fs::write failed");
                return;
            }
            let path_str = match path.to_str() {
                Some(s) => s.to_string(),
                None => {
                    tracing::warn!(path = %path.display(), "archive: path is not valid utf-8");
                    return;
                }
            };
            let seg = SegmentRef {
                broadcast: broadcast_owned,
                track: track_owned,
                segment_seq: seq,
                start_dts,
                end_dts,
                timescale,
                keyframe_start,
                path: path_str,
                byte_offset: 0,
                length,
            };
            if let Err(e) = index.record(&seg) {
                tracing::warn!(error = ?e, "archive: index.record failed");
            }
        });
    }
}

/// Fan-out `FragmentObserver` that forwards every call to every
/// inner observer in registration order. Used by `lvqr-cli` to
/// compose the LL-HLS bridge with the archive indexer without
/// widening the bridge's single-observer API.
pub(crate) struct TeeFragmentObserver {
    inner: Vec<SharedFragmentObserver>,
}

impl TeeFragmentObserver {
    pub fn new(inner: Vec<SharedFragmentObserver>) -> Self {
        Self { inner }
    }
}

impl FragmentObserver for TeeFragmentObserver {
    fn on_init(&self, broadcast: &str, track: &str, timescale: u32, init: Bytes) {
        for obs in &self.inner {
            obs.on_init(broadcast, track, timescale, init.clone());
        }
    }

    fn on_fragment(&self, broadcast: &str, track: &str, fragment: &Fragment) {
        for obs in &self.inner {
            obs.on_fragment(broadcast, track, fragment);
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
}

/// Router state shared between the three `/playback/*` handlers.
/// Carries a canonicalized copy of the archive directory so the
/// `file` handler can reject path traversal in constant time
/// without touching the filesystem twice on every request.
#[derive(Clone)]
pub(crate) struct ArchiveState {
    pub dir: Arc<PathBuf>,
    pub canonical_dir: Arc<PathBuf>,
    pub index: Arc<RedbSegmentIndex>,
}

impl FromRef<ArchiveState> for Arc<RedbSegmentIndex> {
    fn from_ref(state: &ArchiveState) -> Arc<RedbSegmentIndex> {
        Arc::clone(&state.index)
    }
}

async fn playback_handler(
    State(index): State<Arc<RedbSegmentIndex>>,
    AxumPath(broadcast): AxumPath<String>,
    Query(params): Query<PlaybackQuery>,
) -> Response {
    let track = params.track.as_deref().unwrap_or("0.mp4");
    let from = params.from.unwrap_or(0);
    let to = params.to.unwrap_or(u64::MAX);

    // redb is synchronous and holds an exclusive file lock, so the
    // scan itself is fast but still blocks the current task.
    // `spawn_blocking` keeps the admin axum runtime responsive for
    // other requests while the scan runs.
    let index = Arc::clone(&index);
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
}

async fn latest_handler(
    State(index): State<Arc<RedbSegmentIndex>>,
    AxumPath(broadcast): AxumPath<String>,
    Query(params): Query<LatestQuery>,
) -> Response {
    let track = params.track.as_deref().unwrap_or("0.mp4").to_string();
    let index = Arc::clone(&index);
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

/// Serve an archived fragment file by relative path, e.g.
/// `GET /playback/file/live/dvr/0.mp4/00000001.m4s`. `rel` is
/// joined onto the configured archive directory; the joined
/// path is canonicalized and rejected if it escapes the archive
/// root. Returns `application/octet-stream` bytes on success,
/// `404` when the file is missing, and `400` when the path
/// traversal guard trips.
async fn file_handler(State(state): State<ArchiveState>, AxumPath(rel): AxumPath<String>) -> Response {
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
pub(crate) fn playback_router(dir: PathBuf, index: Arc<RedbSegmentIndex>) -> Router {
    let canonical_dir = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    let state = ArchiveState {
        dir: Arc::new(dir),
        canonical_dir: Arc::new(canonical_dir),
        index,
    };
    Router::new()
        .route("/playback/latest/{*broadcast}", get(latest_handler))
        .route("/playback/file/{*rel}", get(file_handler))
        .route("/playback/{*broadcast}", get(playback_handler))
        .with_state(state)
}

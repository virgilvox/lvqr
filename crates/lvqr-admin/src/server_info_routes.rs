//! `GET /api/v1/server-info` admin route.
//!
//! Returns a snapshot of the relay's runtime shape so the admin UI can
//! auto-populate connection profiles + render accurate Server Settings
//! views without the operator hand-typing port overrides. The shape
//! is intentionally additive: every field is `Option<...>` or carries
//! `#[serde(default)]` so a future server can return more keys without
//! breaking older clients (and a younger client polling an older
//! server gets `None` / empty values rather than a deserialise error).
//!
//! Mounted on the admin auth middleware like every other
//! `/api/v1/*` route -- the response includes the bearer token's
//! configured-but-still-secret presence (a `bool`, never the literal),
//! so leaking the JSON does not leak credentials.

use crate::routes::{AdminError, AdminState};
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

/// Snapshot of every protocol's bound listener address. Each field is
/// `None` when the corresponding `--<protocol>-port` flag was not
/// passed (the protocol is disabled). The string shape is the
/// `SocketAddr::to_string()` form (e.g. `"0.0.0.0:8443"`); the admin
/// UI splits on the colon to extract the port.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoundAddresses {
    #[serde(default)]
    pub admin: Option<String>,
    #[serde(default)]
    pub rtmp: Option<String>,
    #[serde(default)]
    pub whip: Option<String>,
    #[serde(default)]
    pub whep: Option<String>,
    #[serde(default)]
    pub hls: Option<String>,
    #[serde(default)]
    pub dash: Option<String>,
    #[serde(default)]
    pub srt: Option<String>,
    #[serde(default)]
    pub rtsp: Option<String>,
    /// MoQ relay (the QUIC/WebTransport listener; `--port`).
    #[serde(default)]
    pub moq: Option<String>,
    /// WebRTC signaling endpoint when mesh is enabled. Shares the
    /// admin port.
    #[serde(default)]
    pub signal: Option<String>,
}

/// Runtime feature snapshot derived from the parsed `ServeConfig`.
/// Tells the admin UI which surfaces are enabled without it having to
/// guess from observed traffic.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeFeatures {
    #[serde(default)]
    pub mesh_enabled: bool,
    #[serde(default)]
    pub cluster_enabled: bool,
    /// Resolved archive directory when configured; `None` means DVR
    /// archive is disabled.
    #[serde(default)]
    pub archive_dir: Option<String>,
    /// Configured `--record-dir`. Independent of `archive_dir`; both
    /// can be set, set one, or both `None`.
    #[serde(default)]
    pub record_dir: Option<String>,
    /// Number of WASM filters in the chain (process-startup; cannot be
    /// changed via config-reload yet).
    #[serde(default)]
    pub wasm_filter_chain_length: usize,
    /// Coarse auth-provider classifier: `"noop" | "static" | "jwt" |
    /// "jwks" | "webhook" | "multi"`. Never carries token literals.
    #[serde(default)]
    pub auth_mode: String,
    /// `true` when the operator passed `--hmac-playback-secret` (or it
    /// is set in the config file). Lets the admin UI's signed-URL
    /// generator surface a "secret already configured" hint instead of
    /// asking for it.
    #[serde(default)]
    pub hmac_playback_secret_configured: bool,
    /// `true` when `--no-streamkeys` was NOT passed; the
    /// `/api/v1/streamkeys/*` routes are mounted.
    #[serde(default)]
    pub stream_keys_enabled: bool,
}

/// Body of `GET /api/v1/server-info`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerInfo {
    /// `CARGO_PKG_VERSION` baked at build time.
    #[serde(default)]
    pub version: String,
    /// Cargo features compiled into the binary (e.g. `["c2pa",
    /// "transcode", "jwks", "webhook"]`).
    #[serde(default)]
    pub build_features: Vec<String>,
    /// Wall-clock seconds since `lvqr serve` startup. Drives the
    /// dashboard "Uptime" tile.
    #[serde(default)]
    pub uptime_secs: u64,
    #[serde(default)]
    pub bound: BoundAddresses,
    #[serde(default)]
    pub features: RuntimeFeatures,
    /// Resolved `--config <path>` when set; `None` when the relay is
    /// running purely off CLI flags. The admin UI uses this to decide
    /// whether the `/api/v1/config` GET/PUT routes will work.
    #[serde(default)]
    pub config_path: Option<String>,
    /// Path to the WASM filter file(s), in chain order. Empty when
    /// `wasm_filter_chain_length == 0`.
    #[serde(default)]
    pub wasm_filter_paths: Vec<String>,
}

/// Closure shape for the `GET /api/v1/server-info` route. lvqr-cli
/// installs a closure that reads the parsed `ServeConfig` + every
/// listener's bound address + the cargo crate version. The closure
/// is sync because every value it reads is either constant or a
/// cheap atomic load.
pub type ServerInfoFn = Arc<dyn Fn() -> ServerInfo + Send + Sync>;

/// Default factory: returns a stub `ServerInfo` with version =
/// `CARGO_PKG_VERSION`. Used when the CLI did not call
/// `AdminState::with_server_info` (e.g. unit tests).
pub fn default_server_info() -> ServerInfo {
    ServerInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        ..Default::default()
    }
}

/// Helper that returns a `ServerInfoFn` which records uptime relative
/// to a captured `Instant`. The CLI composition root captures
/// `Instant::now()` at startup + threads it through this builder.
pub fn server_info_fn_with_uptime(start: Instant, base: ServerInfo) -> ServerInfoFn {
    Arc::new(move || {
        let mut info = base.clone();
        info.uptime_secs = start.elapsed().as_secs();
        info
    })
}

/// `GET /api/v1/server-info` handler. Always 200; an unwired
/// `ServerInfo` returns the stub default rather than 503 so polling
/// clients get a stable shape regardless of the operator's wiring.
pub async fn get_server_info(State(state): State<AdminState>) -> Result<Json<ServerInfo>, AdminError> {
    Ok(Json(state.server_info()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::build_router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use lvqr_core::RelayStats;
    use tower::ServiceExt;

    #[tokio::test]
    async fn server_info_returns_stub_default_when_unwired() {
        let state = AdminState::new(RelayStats::default, Vec::<crate::StreamInfo>::new);
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/server-info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let info: ServerInfo = serde_json::from_slice(&body).unwrap();
        // The stub default carries the workspace version (matches
        // CARGO_PKG_VERSION at compile time) and empty bound + features.
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.bound, BoundAddresses::default());
        assert_eq!(info.features.wasm_filter_chain_length, 0);
    }

    #[tokio::test]
    async fn server_info_reflects_wired_closure() {
        let state =
            AdminState::new(RelayStats::default, Vec::<crate::StreamInfo>::new).with_server_info(Arc::new(|| {
                ServerInfo {
                    version: "1.2.3".into(),
                    build_features: vec!["c2pa".into(), "transcode".into()],
                    uptime_secs: 0,
                    bound: BoundAddresses {
                        admin: Some("0.0.0.0:18090".into()),
                        rtmp: Some("0.0.0.0:1935".into()),
                        whip: Some("0.0.0.0:8443".into()),
                        whep: Some("0.0.0.0:8444".into()),
                        hls: Some("0.0.0.0:8788".into()),
                        dash: Some("0.0.0.0:8889".into()),
                        moq: Some("0.0.0.0:4443".into()),
                        ..Default::default()
                    },
                    features: RuntimeFeatures {
                        mesh_enabled: true,
                        cluster_enabled: false,
                        archive_dir: Some("/tmp/lvqr/archive".into()),
                        wasm_filter_chain_length: 1,
                        auth_mode: "noop".into(),
                        stream_keys_enabled: true,
                        ..Default::default()
                    },
                    config_path: Some("/etc/lvqr.toml".into()),
                    wasm_filter_paths: vec!["/path/to/frame-counter.wasm".into()],
                }
            }));
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/server-info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let info: ServerInfo = serde_json::from_slice(&body).unwrap();
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.bound.whip.as_deref(), Some("0.0.0.0:8443"));
        assert_eq!(info.bound.hls.as_deref(), Some("0.0.0.0:8788"));
        assert!(info.features.mesh_enabled);
        assert_eq!(info.features.wasm_filter_chain_length, 1);
        assert_eq!(info.config_path.as_deref(), Some("/etc/lvqr.toml"));
    }

    #[tokio::test]
    async fn server_info_uptime_helper_records_time_since_start() {
        let start = Instant::now() - std::time::Duration::from_secs(42);
        let f = server_info_fn_with_uptime(start, default_server_info());
        let info = f();
        assert!(info.uptime_secs >= 42);
    }
}

//! `/api/v1/config-reload` admin routes (session 147).
//!
//! Two handlers:
//!
//! ```text
//! GET  /api/v1/config-reload  -> 200 ConfigReloadStatus (always)
//! POST /api/v1/config-reload  -> 200 ConfigReloadStatus | 500 (build failed)
//!                             | 503 (no --config configured at boot)
//! ```
//!
//! `ConfigReloadStatus` is intentionally defined here (not in
//! lvqr-cli) so it serves dual duty: lvqr-admin owns the wire shape,
//! lvqr-cli's `ConfigReloadHandle` returns it from `reload` /
//! `status` methods. The dependency direction stays
//! `lvqr-cli -> lvqr-admin` (no cycle).

use crate::routes::{AdminError, AdminState};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Wire shape for `GET /api/v1/config-reload` and the success body
/// of `POST /api/v1/config-reload`. Every Optional field carries
/// `#[serde(default)]` so SDK clients older than the server keep
/// parsing forward when later sessions add sibling fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigReloadStatus {
    /// Resolved path of the configured `--config` file. `None`
    /// when the server booted without `--config`.
    #[serde(default)]
    pub config_path: Option<String>,
    /// Unix milliseconds at the most recent successful reload.
    /// `None` until the first reload succeeds.
    #[serde(default)]
    pub last_reload_at_ms: Option<u64>,
    /// `"sighup"`, `"admin_post"`, `"boot"`, or `None` when no
    /// reload has occurred yet.
    #[serde(default)]
    pub last_reload_kind: Option<String>,
    /// Keys the most recent reload effectively re-applied.
    /// Currently always `["auth"]` on success; future increments
    /// add `mesh_ice` and `hmac_secret`.
    #[serde(default)]
    pub applied_keys: Vec<String>,
    /// Operator-facing warnings -- e.g. structural-key diffs that
    /// require a server restart, or deferred-reload sections
    /// (`jwks_url`, `webhook_auth_url`, `mesh_ice_servers`,
    /// `hmac_playback_secret`).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Closure shape for the `GET` handler. lvqr-cli installs a closure
/// that calls `ConfigReloadHandle::status()`.
pub type ConfigReloadStatusFn = Arc<dyn Fn() -> ConfigReloadStatus + Send + Sync>;

/// Closure shape for the `POST` handler. lvqr-cli installs a closure
/// that calls `ConfigReloadHandle::reload("admin_post")`. Returns
/// the new status on success or a string error on failure.
pub type ConfigReloadTriggerFn = Arc<dyn Fn() -> Result<ConfigReloadStatus, String> + Send + Sync>;

/// `GET /api/v1/config-reload`. Always returns 200; the response
/// distinguishes "no config-reload wired" (no path, no last
/// timestamp) from "wired but no reload yet" (path present, no
/// timestamp) from "wired and reloaded" (path + timestamp).
pub async fn get_config_reload(State(state): State<AdminState>) -> Json<ConfigReloadStatus> {
    Json(state.config_reload_status())
}

/// `POST /api/v1/config-reload`. Triggers a reload via the wired
/// closure. 503 when no `--config` was passed at boot (closure
/// absent); 500 with the build-error message when reload fails;
/// 200 with the new status on success.
pub async fn trigger_config_reload(State(state): State<AdminState>) -> Result<Response, AdminError> {
    let Some(trigger) = state.config_reload_trigger() else {
        // 503 maps to AdminError::Internal(...) which renders 500;
        // we want a distinct 503 here so callers can disambiguate
        // "feature off" from "feature on but failed". Direct
        // construction.
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "config reload not configured (server booted without --config)"
            })),
        )
            .into_response());
    };
    match trigger() {
        Ok(status) => Ok((StatusCode::OK, Json(status)).into_response()),
        Err(reason) => Err(AdminError::Internal(format!("config reload failed: {reason}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::build_router;
    use axum::body::Body;
    use axum::http::{Request, header};
    use std::sync::Mutex;
    use tower::ServiceExt;

    fn ok_state() -> (AdminState, Arc<Mutex<u32>> /* trigger call count */) {
        let calls = Arc::new(Mutex::new(0u32));
        let calls_for_status = calls.clone();
        let calls_for_trigger = calls.clone();
        let state = AdminState::new(lvqr_core::RelayStats::default, Vec::<crate::StreamInfo>::new)
            .with_config_reload_status(Arc::new(move || ConfigReloadStatus {
                config_path: Some("/etc/lvqr.toml".into()),
                last_reload_at_ms: Some(*calls_for_status.lock().unwrap() as u64 * 1000),
                last_reload_kind: Some("admin_post".into()),
                applied_keys: vec!["auth".into()],
                warnings: Vec::new(),
            }))
            .with_config_reload_trigger(Arc::new(move || {
                let mut n = calls_for_trigger.lock().unwrap();
                *n += 1;
                Ok(ConfigReloadStatus {
                    config_path: Some("/etc/lvqr.toml".into()),
                    last_reload_at_ms: Some(*n as u64 * 1000),
                    last_reload_kind: Some("admin_post".into()),
                    applied_keys: vec!["auth".into()],
                    warnings: Vec::new(),
                })
            }));
        (state, calls)
    }

    #[tokio::test]
    async fn get_returns_default_when_not_wired() {
        let state = AdminState::new(lvqr_core::RelayStats::default, Vec::<crate::StreamInfo>::new);
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/config-reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: ConfigReloadStatus = serde_json::from_slice(&body).unwrap();
        assert!(parsed.config_path.is_none());
        assert!(parsed.last_reload_at_ms.is_none());
    }

    #[tokio::test]
    async fn post_returns_503_when_not_wired() {
        let state = AdminState::new(lvqr_core::RelayStats::default, Vec::<crate::StreamInfo>::new);
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/config-reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_reflects_status_closure() {
        let (state, _calls) = ok_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/config-reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: ConfigReloadStatus = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.config_path.as_deref(), Some("/etc/lvqr.toml"));
        assert_eq!(parsed.last_reload_kind.as_deref(), Some("admin_post"));
    }

    #[tokio::test]
    async fn post_invokes_trigger_closure_and_returns_200() {
        let (state, calls) = ok_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/config-reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: ConfigReloadStatus = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.applied_keys, vec!["auth".to_string()]);
        assert_eq!(*calls.lock().unwrap(), 1, "trigger closure must be called once");
    }

    #[tokio::test]
    async fn post_returns_500_when_trigger_returns_err() {
        let state = AdminState::new(lvqr_core::RelayStats::default, Vec::<crate::StreamInfo>::new)
            .with_config_reload_trigger(Arc::new(|| Err("forced reload failure for test".into())));
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/config-reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn config_reload_routes_respect_admin_auth() {
        use lvqr_auth::{SharedAuth, StaticAuthConfig, StaticAuthProvider};
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            ..Default::default()
        }));
        let (state, _calls) = ok_state();
        let state = state.with_auth(auth);
        let app = build_router(state);
        // Missing bearer -- 401.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/config-reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // Correct bearer -- 200.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/config-reload")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

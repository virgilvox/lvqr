use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing::get};
use lvqr_auth::{AuthContext, AuthDecision, NoopAuthProvider, SharedAuth};
use lvqr_core::RelayStats;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

/// Stream info returned by the API.
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamInfo {
    pub name: String,
    pub subscribers: usize,
}

/// Mesh state returned by the API.
#[derive(Debug, Serialize, Deserialize)]
pub struct MeshState {
    pub enabled: bool,
    pub peer_count: usize,
    pub offload_percentage: f64,
}

/// Provider for /metrics endpoint output. Returns Prometheus text-format
/// metrics. Set up by Phase 4 (metrics).
pub type MetricsRender = Arc<dyn Fn() -> String + Send + Sync>;

/// Shared state for the admin API.
///
/// Uses callbacks so the admin crate doesn't depend on relay or ingest types.
/// The CLI wires these to real relay metrics and bridge state.
#[derive(Clone)]
pub struct AdminState {
    get_stats: Arc<dyn Fn() -> RelayStats + Send + Sync>,
    get_streams: Arc<dyn Fn() -> Vec<StreamInfo> + Send + Sync>,
    get_mesh: Arc<dyn Fn() -> MeshState + Send + Sync>,
    auth: SharedAuth,
    metrics_render: Option<MetricsRender>,
}

impl AdminState {
    pub fn new(
        get_stats: impl Fn() -> RelayStats + Send + Sync + 'static,
        get_streams: impl Fn() -> Vec<StreamInfo> + Send + Sync + 'static,
    ) -> Self {
        Self {
            get_stats: Arc::new(get_stats),
            get_streams: Arc::new(get_streams),
            get_mesh: Arc::new(|| MeshState {
                enabled: false,
                peer_count: 0,
                offload_percentage: 0.0,
            }),
            auth: Arc::new(NoopAuthProvider),
            metrics_render: None,
        }
    }

    /// Set the mesh state provider.
    pub fn with_mesh(mut self, get_mesh: impl Fn() -> MeshState + Send + Sync + 'static) -> Self {
        self.get_mesh = Arc::new(get_mesh);
        self
    }

    /// Install an auth provider that gates `/api/v1/*` routes.
    pub fn with_auth(mut self, auth: SharedAuth) -> Self {
        self.auth = auth;
        self
    }

    /// Install a Prometheus metrics renderer for the `/metrics` endpoint.
    pub fn with_metrics(mut self, render: MetricsRender) -> Self {
        self.metrics_render = Some(render);
        self
    }
}

/// Structured error responses for the admin API.
#[derive(Debug)]
pub enum AdminError {
    Unauthorized(String),
    NotFound(String),
    Internal(String),
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AdminError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            AdminError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            AdminError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

/// Build the admin API router.
pub fn build_router(state: AdminState) -> Router {
    let auth = state.auth.clone();
    let api_routes = Router::new()
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/streams", get(list_streams))
        .route("/api/v1/mesh", get(get_mesh))
        .layer(middleware::from_fn_with_state(auth, auth_middleware));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_handler))
        .merge(api_routes)
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

/// Prometheus scrape endpoint. Always unauthenticated; if no metrics renderer
/// is installed, returns an empty body.
async fn metrics_handler(State(state): State<AdminState>) -> Response {
    let body = match &state.metrics_render {
        Some(render) => render(),
        None => String::new(),
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
        .into_response()
}

async fn get_stats(State(state): State<AdminState>) -> Result<Json<RelayStats>, AdminError> {
    Ok(Json((state.get_stats)()))
}

async fn list_streams(State(state): State<AdminState>) -> Result<Json<Vec<StreamInfo>>, AdminError> {
    Ok(Json((state.get_streams)()))
}

async fn get_mesh(State(state): State<AdminState>) -> Result<Json<MeshState>, AdminError> {
    Ok(Json((state.get_mesh)()))
}

/// Middleware that validates the `Authorization: Bearer` header against the
/// admin auth provider. Skips when `NoopAuthProvider` (always allows).
async fn auth_middleware(State(auth): State<SharedAuth>, req: Request<axum::body::Body>, next: Next) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::to_string)
        .unwrap_or_default();
    let decision = auth.check(&AuthContext::Admin { token });
    if let AuthDecision::Deny { reason } = decision {
        tracing::debug!(reason = %reason, "admin request denied");
        return AdminError::Unauthorized(reason).into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use lvqr_auth::{StaticAuthConfig, StaticAuthProvider};
    use tower::ServiceExt;

    fn test_state(streams: Vec<(&'static str, usize)>) -> AdminState {
        let streams_for_stats = streams.clone();
        AdminState::new(
            move || {
                let total_subs: u64 = streams_for_stats.iter().map(|(_, s)| *s as u64).sum();
                RelayStats {
                    tracks: streams_for_stats.len() as u64 * 2,
                    subscribers: total_subs,
                    publishers: streams_for_stats.len() as u64,
                    ..Default::default()
                }
            },
            move || {
                streams
                    .iter()
                    .map(|(name, subs)| StreamInfo {
                        name: name.to_string(),
                        subscribers: *subs,
                    })
                    .collect()
            },
        )
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let state = test_state(vec![]);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn stats_empty() {
        let state = test_state(vec![]);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/stats").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let stats: RelayStats = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.tracks, 0);
        assert_eq!(stats.subscribers, 0);
        assert_eq!(stats.publishers, 0);
    }

    #[tokio::test]
    async fn stats_with_active_streams() {
        let state = test_state(vec![("live/test", 5)]);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/stats").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let stats: RelayStats = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.publishers, 1);
        assert_eq!(stats.subscribers, 5);
        assert_eq!(stats.tracks, 2);
    }

    #[tokio::test]
    async fn list_streams_returns_active() {
        let state = test_state(vec![("live/stream1", 2), ("live/stream2", 3)]);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/streams").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let streams: Vec<StreamInfo> = serde_json::from_slice(&body).unwrap();
        assert_eq!(streams.len(), 2);

        let mut names: Vec<&str> = streams.iter().map(|s| s.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["live/stream1", "live/stream2"]);
    }

    #[tokio::test]
    async fn mesh_disabled_by_default() {
        let state = test_state(vec![]);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/mesh").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let mesh: MeshState = serde_json::from_slice(&body).unwrap();
        assert!(!mesh.enabled);
        assert_eq!(mesh.peer_count, 0);
    }

    #[tokio::test]
    async fn mesh_with_peers() {
        let state = test_state(vec![]).with_mesh(|| MeshState {
            enabled: true,
            peer_count: 42,
            offload_percentage: 73.5,
        });
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/mesh").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let mesh: MeshState = serde_json::from_slice(&body).unwrap();
        assert!(mesh.enabled);
        assert_eq!(mesh.peer_count, 42);
        assert!((mesh.offload_percentage - 73.5).abs() < 0.01);
    }

    #[tokio::test]
    async fn admin_api_rejects_without_token_when_configured() {
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            ..Default::default()
        }));
        let state = test_state(vec![]).with_auth(auth);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/stats").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_api_accepts_valid_token() {
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            ..Default::default()
        }));
        let state = test_state(vec![]).with_auth(auth);
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/stats")
                    .header("Authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn healthz_open_even_with_auth() {
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            ..Default::default()
        }));
        let state = test_state(vec![]).with_auth(auth);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_endpoint_present_even_without_renderer() {
        let state = test_state(vec![]);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_endpoint_uses_renderer() {
        let state = test_state(vec![]).with_metrics(Arc::new(|| "lvqr_test 1\n".to_string()));
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("lvqr_test 1"));
    }
}

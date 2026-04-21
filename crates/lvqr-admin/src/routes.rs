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
    /// Optional cluster handle. Populated by [`AdminState::with_cluster`];
    /// consumed by the `/api/v1/cluster/*` routes defined in
    /// [`crate::cluster_routes`]. Feature-gated so callers that do
    /// not run clustering pay no cost for the dep.
    #[cfg(feature = "cluster")]
    cluster: Option<Arc<lvqr_cluster::Cluster>>,
    /// Optional federation status handle. Populated by
    /// [`AdminState::with_federation_status`]; consumed by the
    /// `GET /api/v1/cluster/federation` route in
    /// [`crate::cluster_routes`]. `None` means the caller did not
    /// start a [`lvqr_cluster::FederationRunner`] (no
    /// `federation_links` configured); the route then returns an
    /// empty link list.
    #[cfg(feature = "cluster")]
    federation_status: Option<lvqr_cluster::FederationStatusHandle>,
    /// Optional latency SLO tracker. Populated by
    /// [`AdminState::with_slo`]; consumed by the
    /// `GET /api/v1/slo` route. `None` means the caller did not
    /// wire the tracker into any egress surface; the route then
    /// returns an empty broadcast list. Tier 4 item 4.7 session A.
    slo: Option<crate::slo::LatencyTracker>,
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
            #[cfg(feature = "cluster")]
            cluster: None,
            #[cfg(feature = "cluster")]
            federation_status: None,
            slo: None,
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

    /// Wire an `Arc<Cluster>` so the `/api/v1/cluster/*` routes can
    /// answer against it. Without this call, the cluster routes
    /// return 503.
    #[cfg(feature = "cluster")]
    pub fn with_cluster(mut self, cluster: Arc<lvqr_cluster::Cluster>) -> Self {
        self.cluster = Some(cluster);
        self
    }

    /// Borrow the configured cluster handle, if any. Used by the
    /// `cluster_routes` module.
    #[cfg(feature = "cluster")]
    pub(crate) fn cluster(&self) -> Option<&Arc<lvqr_cluster::Cluster>> {
        self.cluster.as_ref()
    }

    /// Wire a [`lvqr_cluster::FederationStatusHandle`] so the
    /// `GET /api/v1/cluster/federation` route can expose per-link
    /// state (connecting / connected / failed) to operators.
    /// Without this call the route returns an empty link list.
    /// Tier 4 item 4.4 session C.
    #[cfg(feature = "cluster")]
    pub fn with_federation_status(mut self, status: lvqr_cluster::FederationStatusHandle) -> Self {
        self.federation_status = Some(status);
        self
    }

    /// Borrow the configured federation status handle, if any.
    /// Used by the `cluster_routes` module.
    #[cfg(feature = "cluster")]
    pub(crate) fn federation_status(&self) -> Option<&lvqr_cluster::FederationStatusHandle> {
        self.federation_status.as_ref()
    }

    /// Wire a [`crate::slo::LatencyTracker`] so the
    /// `GET /api/v1/slo` route can expose per-(broadcast, transport)
    /// p50 / p95 / p99 / max latency drawn from the tracker's
    /// rolling sample window. Without this call the route returns
    /// an empty broadcast list. Tier 4 item 4.7 session A.
    pub fn with_slo(mut self, tracker: crate::slo::LatencyTracker) -> Self {
        self.slo = Some(tracker);
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
    let mut api_routes: Router<AdminState> = Router::new()
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/streams", get(list_streams))
        .route("/api/v1/mesh", get(get_mesh))
        .route("/api/v1/slo", get(get_slo));

    #[cfg(feature = "cluster")]
    {
        api_routes = api_routes.merge(crate::cluster_routes::cluster_router());
    }

    let api_routes = api_routes.layer(middleware::from_fn_with_state(auth, auth_middleware));

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

/// `GET /api/v1/slo` handler. Returns `{ "broadcasts": [SloEntry..] }`
/// drawn from the [`crate::slo::LatencyTracker`] wired via
/// [`AdminState::with_slo`]. When no tracker is configured the
/// route returns an empty broadcast list (shape stable for
/// dashboards that pre-bake the response structure). Tier 4 item
/// 4.7 session A.
async fn get_slo(State(state): State<AdminState>) -> Result<Json<serde_json::Value>, AdminError> {
    let broadcasts = match state.slo.as_ref() {
        Some(tracker) => tracker.snapshot(),
        None => Vec::new(),
    };
    Ok(Json(json!({ "broadcasts": broadcasts })))
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
        // Emit `lvqr_auth_failures_total{entry="admin"}` so brute-force
        // admin-token attempts are visible to Prometheus scrapers on the
        // same counter the RTMP / MoQ / WS ingest paths use. Without
        // this, the admin surface was the only auth entry point that
        // denied silently, which the internal audit flagged as a
        // monitoring blind spot.
        metrics::counter!("lvqr_auth_failures_total", "entry" => "admin").increment(1);
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

    #[tokio::test]
    async fn slo_route_empty_without_tracker() {
        let state = test_state(vec![]);
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/api/v1/slo").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let broadcasts = v.get("broadcasts").expect("broadcasts field present");
        assert!(broadcasts.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn slo_route_exposes_tracker_snapshot() {
        let tracker = crate::slo::LatencyTracker::new();
        for ms in 1..=50u64 {
            tracker.record("live/demo", "hls", ms * 2);
        }
        let state = test_state(vec![]).with_slo(tracker);
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/api/v1/slo").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let broadcasts = v.get("broadcasts").unwrap().as_array().unwrap();
        assert_eq!(broadcasts.len(), 1);
        let first = &broadcasts[0];
        assert_eq!(first["broadcast"], "live/demo");
        assert_eq!(first["transport"], "hls");
        assert_eq!(first["sample_count"], 50);
        assert_eq!(first["total_observed"], 50);
        assert!(first["p50_ms"].as_u64().unwrap() > 0);
        assert!(first["max_ms"].as_u64().unwrap() >= first["p99_ms"].as_u64().unwrap());
    }

    #[tokio::test]
    async fn slo_route_respects_admin_auth() {
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            ..Default::default()
        }));
        let tracker = crate::slo::LatencyTracker::new();
        let state = test_state(vec![]).with_auth(auth).with_slo(tracker);
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/api/v1/slo").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

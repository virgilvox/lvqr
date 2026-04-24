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
///
/// `peers` carries per-peer intended-vs-actual offload counters; the
/// `intended_children` value comes from the topology planner's
/// assignment, while `forwarded_frames` is the cumulative count the
/// client reported via the `/signal` `ForwardReport` message (see
/// `lvqr_signal::SignalMessage::ForwardReport`). When mesh is disabled
/// the vec is empty. Added in session 141 with `#[serde(default)]`
/// so pre-141 clients still deserialize new-server bodies (and vice
/// versa on the Python side via `.get("peers", [])`).
#[derive(Debug, Serialize, Deserialize)]
pub struct MeshState {
    pub enabled: bool,
    pub peer_count: usize,
    pub offload_percentage: f64,
    /// Per-peer intended-vs-actual offload stats. Empty when mesh is
    /// disabled. Session 141 -- actual-vs-intended offload reporting.
    #[serde(default)]
    pub peers: Vec<MeshPeerStats>,
}

/// Per-peer offload stats exposed via `GET /api/v1/mesh`.
///
/// `intended_children` is the count of tree children the topology
/// planner assigned to this peer (derived from `PeerInfo.children`).
/// `forwarded_frames` is the cumulative count of fragments the peer
/// has reported forwarding to its DataChannel children via the
/// `/signal` `ForwardReport` message; the server replaces rather
/// than accumulates so a client reconnect cannot inflate the
/// displayed value indefinitely. Session 141.
#[derive(Debug, Serialize, Deserialize)]
pub struct MeshPeerStats {
    pub peer_id: String,
    pub role: String,
    pub parent: Option<String>,
    pub depth: u32,
    pub intended_children: usize,
    pub forwarded_frames: u64,
}

/// WASM filter chain state returned by `GET /api/v1/wasm-filter`.
///
/// `enabled` mirrors whether `--wasm-filter` was configured at
/// `lvqr serve` time. When false, `chain_length` is `0` and
/// both `broadcasts` and `slots` are empty; the route still
/// returns 200 OK so dashboards pre-baking the response shape do
/// not need a separate 404 handler.
///
/// `slots` carries per-filter-position counters (index 0 is the
/// first filter in the chain, N-1 is the last). Short-circuit
/// semantics mean later slots naturally report fewer `seen`
/// fragments than earlier slots when an earlier slot drops --
/// operators use this to pinpoint which filter is denying in a
/// drop-heavy chain. PLAN Phase D session 140.
#[derive(Debug, Serialize, Deserialize)]
pub struct WasmFilterState {
    pub enabled: bool,
    pub chain_length: usize,
    pub broadcasts: Vec<WasmFilterBroadcastStats>,
    /// Per-slot counters in insertion order. Always contains
    /// `chain_length` entries when `enabled` is true.
    #[serde(default)]
    pub slots: Vec<WasmFilterSlotStats>,
}

/// Per-`(broadcast, track)` WASM filter counters. Values are atomic
/// snapshots at read time and may drift by one or two fragments
/// between the different counter reads for the same key.
#[derive(Debug, Serialize, Deserialize)]
pub struct WasmFilterBroadcastStats {
    pub broadcast: String,
    pub track: String,
    pub seen: u64,
    pub kept: u64,
    pub dropped: u64,
}

/// Per-slot WASM filter counters. `index` is the filter's position
/// in the chain (0-based). `seen` / `kept` / `dropped` describe
/// what THAT slot observed -- later slots in a chain will show
/// smaller `seen` counts when an earlier slot drops, because the
/// chain short-circuits on the first `None`. PLAN Phase D session
/// 140.
#[derive(Debug, Serialize, Deserialize)]
pub struct WasmFilterSlotStats {
    pub index: usize,
    pub seen: u64,
    pub kept: u64,
    pub dropped: u64,
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
    /// Snapshot callback for the `GET /api/v1/wasm-filter` route.
    /// Populated by [`AdminState::with_wasm_filter`]; defaults to a
    /// "no filter configured" closure that returns an empty
    /// [`WasmFilterState`] with `enabled: false`. The indirection
    /// keeps `lvqr-admin` free of a `lvqr-wasm` dep so builds that
    /// turn off the filter stack pay no graph cost. PLAN Phase D
    /// session 137.
    get_wasm_filter: Arc<dyn Fn() -> WasmFilterState + Send + Sync>,
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
                peers: Vec::new(),
            }),
            auth: Arc::new(NoopAuthProvider),
            metrics_render: None,
            #[cfg(feature = "cluster")]
            cluster: None,
            #[cfg(feature = "cluster")]
            federation_status: None,
            slo: None,
            get_wasm_filter: Arc::new(|| WasmFilterState {
                enabled: false,
                chain_length: 0,
                broadcasts: Vec::new(),
                slots: Vec::new(),
            }),
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

    /// Wire a snapshot closure backing the `GET /api/v1/wasm-filter`
    /// route. The CLI's composition root passes a closure that reads
    /// `chain_length` + per-broadcast counters off the
    /// `WasmFilterBridgeHandle` stored on [`lvqr_cli::ServerHandle`].
    /// Without this call the route returns `{enabled: false,
    /// chain_length: 0, broadcasts: []}`. PLAN Phase D session 137.
    pub fn with_wasm_filter(mut self, get: impl Fn() -> WasmFilterState + Send + Sync + 'static) -> Self {
        self.get_wasm_filter = Arc::new(get);
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
        .route("/api/v1/slo", get(get_slo))
        .route("/api/v1/wasm-filter", get(get_wasm_filter));

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

/// `GET /api/v1/wasm-filter` handler. Returns the chain length +
/// per-`(broadcast, track)` counters for the configured WASM filter
/// chain, or an empty "disabled" body when `--wasm-filter` is unset.
/// PLAN Phase D session 137.
async fn get_wasm_filter(State(state): State<AdminState>) -> Result<Json<WasmFilterState>, AdminError> {
    Ok(Json((state.get_wasm_filter)()))
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
        assert!(mesh.peers.is_empty());
    }

    #[tokio::test]
    async fn mesh_with_peers() {
        let state = test_state(vec![]).with_mesh(|| MeshState {
            enabled: true,
            peer_count: 42,
            offload_percentage: 73.5,
            peers: vec![
                MeshPeerStats {
                    peer_id: "root-1".into(),
                    role: "Root".into(),
                    parent: None,
                    depth: 0,
                    intended_children: 3,
                    forwarded_frames: 1200,
                },
                MeshPeerStats {
                    peer_id: "relay-7".into(),
                    role: "Relay".into(),
                    parent: Some("root-1".into()),
                    depth: 1,
                    intended_children: 1,
                    forwarded_frames: 400,
                },
            ],
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
        assert_eq!(mesh.peers.len(), 2);

        // Session 141: the admin body surfaces the intended-vs-actual
        // split per peer.
        let root = mesh
            .peers
            .iter()
            .find(|p| p.peer_id == "root-1")
            .expect("root-1 present");
        assert_eq!(root.role, "Root");
        assert!(root.parent.is_none());
        assert_eq!(root.depth, 0);
        assert_eq!(root.intended_children, 3);
        assert_eq!(root.forwarded_frames, 1200);

        let relay = mesh
            .peers
            .iter()
            .find(|p| p.peer_id == "relay-7")
            .expect("relay-7 present");
        assert_eq!(relay.role, "Relay");
        assert_eq!(relay.parent.as_deref(), Some("root-1"));
        assert_eq!(relay.depth, 1);
        assert_eq!(relay.intended_children, 1);
        assert_eq!(relay.forwarded_frames, 400);
    }

    #[tokio::test]
    async fn mesh_state_deserializes_pre_141_body_without_peers() {
        // Session 141 compat: `peers` has `#[serde(default)]`, so a
        // pre-141 server body that omits the field entirely must still
        // parse into an empty vec on a new client.
        let body = br#"{"enabled":true,"peer_count":3,"offload_percentage":66.6}"#;
        let mesh: MeshState = serde_json::from_slice(body).unwrap();
        assert!(mesh.enabled);
        assert_eq!(mesh.peer_count, 3);
        assert!((mesh.offload_percentage - 66.6).abs() < 0.01);
        assert!(mesh.peers.is_empty(), "missing peers field must default to empty");
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

    #[tokio::test]
    async fn wasm_filter_route_defaults_to_disabled_when_unconfigured() {
        let state = test_state(vec![]);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/wasm-filter")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let st: WasmFilterState = serde_json::from_slice(&body).unwrap();
        assert!(!st.enabled);
        assert_eq!(st.chain_length, 0);
        assert!(st.broadcasts.is_empty());
        assert!(st.slots.is_empty());
    }

    #[tokio::test]
    async fn wasm_filter_route_renders_configured_snapshot() {
        let state = test_state(vec![]).with_wasm_filter(|| WasmFilterState {
            enabled: true,
            chain_length: 2,
            broadcasts: vec![WasmFilterBroadcastStats {
                broadcast: "live/demo".into(),
                track: "0.mp4".into(),
                seen: 10,
                kept: 9,
                dropped: 1,
            }],
            slots: vec![
                WasmFilterSlotStats {
                    index: 0,
                    seen: 10,
                    kept: 10,
                    dropped: 0,
                },
                WasmFilterSlotStats {
                    index: 1,
                    seen: 10,
                    kept: 9,
                    dropped: 1,
                },
            ],
        });
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/wasm-filter")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let st: WasmFilterState = serde_json::from_slice(&body).unwrap();
        assert!(st.enabled);
        assert_eq!(st.chain_length, 2);
        assert_eq!(st.broadcasts.len(), 1);
        assert_eq!(st.broadcasts[0].broadcast, "live/demo");
        assert_eq!(st.broadcasts[0].track, "0.mp4");
        assert_eq!(st.broadcasts[0].seen, 10);
        assert_eq!(st.broadcasts[0].kept, 9);
        assert_eq!(st.broadcasts[0].dropped, 1);
        // Per-slot stats: slot 0 keeps everything, slot 1 drops one.
        assert_eq!(st.slots.len(), 2);
        assert_eq!(st.slots[0].index, 0);
        assert_eq!(st.slots[0].seen, 10);
        assert_eq!(st.slots[0].kept, 10);
        assert_eq!(st.slots[0].dropped, 0);
        assert_eq!(st.slots[1].index, 1);
        assert_eq!(st.slots[1].seen, 10);
        assert_eq!(st.slots[1].kept, 9);
        assert_eq!(st.slots[1].dropped, 1);
    }

    #[tokio::test]
    async fn wasm_filter_route_respects_admin_auth() {
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            ..Default::default()
        }));
        let state = test_state(vec![]).with_auth(auth);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/wasm-filter")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

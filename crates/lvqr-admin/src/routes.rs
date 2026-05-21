use axum::extract::{Path, State};
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{
    Json, Router,
    routing::{delete, get, post},
};
use lvqr_auth::{AuthContext, AuthDecision, NoopAuthProvider, SharedAuth, SharedStreamKeyStore};
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

/// Per-broadcast detail returned by `GET /api/v1/streams/{name}`.
///
/// Built by introspecting the `FragmentBroadcasterRegistry` for every
/// `(broadcast, track)` entry whose broadcast matches `name`. The list
/// route `GET /api/v1/streams` stays a lean `{name, subscribers}` shape;
/// this route carries the heavier per-track breakdown so the operator
/// console's stream-detail view does not pay for it on every poll of the
/// list. `subscribers` is the maximum per-track subscriber count across
/// the broadcast's tracks (each egress transport subscribes per track, so
/// the busiest track is the truest "viewers" proxy without a dedicated
/// session counter). Returns 404 when no track for `name` exists.
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamDetailInfo {
    pub name: String,
    pub subscribers: usize,
    pub tracks: Vec<TrackInfo>,
}

/// Per-track counters for a broadcast, exposed via
/// `GET /api/v1/streams/{name}`. `kind` is derived from the track id and
/// RFC 6381 `codec` string (video / audio / captions / scte35 / catalog /
/// data) for display grouping; the relay treats every track uniformly so
/// `kind` is a presentation hint, not a wire distinction. Counters are
/// atomic snapshots and may drift by a fragment or two between reads.
#[derive(Debug, Serialize, Deserialize)]
pub struct TrackInfo {
    pub track: String,
    pub kind: String,
    pub codec: String,
    pub timescale: u32,
    /// Total fragments emitted on this track since broadcast start.
    pub fragments: u64,
    /// Live subscriber count on this track's broadcast channel.
    pub subscribers: usize,
    /// Fragments dropped to lagging subscribers (broadcast-channel skips).
    pub lagged_skips: u64,
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
///
/// `capacity` (added in session 144) is the per-peer self-reported
/// max-children value the client advertised on its
/// `SignalMessage::Register`, clamped at register time to the
/// operator-configured `MeshConfig.max_children`. `None` means the
/// client did not advertise a value and the planner uses the global
/// ceiling for that peer. Behind `#[serde(default)]` so pre-144
/// admin clients deserializing new-server bodies (and new clients
/// parsing pre-144 bodies) both work without a schema-version bump.
#[derive(Debug, Serialize, Deserialize)]
pub struct MeshPeerStats {
    pub peer_id: String,
    pub role: String,
    pub parent: Option<String>,
    pub depth: u32,
    pub intended_children: usize,
    pub forwarded_frames: u64,
    /// Per-peer self-reported relay capacity. Session 144.
    #[serde(default)]
    pub capacity: Option<u32>,
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

/// Transcode ladder state returned by `GET /api/v1/transcode/ladders`.
///
/// `enabled` reflects whether the binary was built with the `transcode`
/// feature AND at least one `--transcode-rendition` was configured. When
/// false, `encoder` is empty and both lists are empty; the route still
/// returns 200 OK so dashboards do not need a separate 404 path. Read-only
/// introspection: the ladder is configured at process startup, so this
/// surface lists what is configured plus live per-output counters; it does
/// not mutate the ladder.
///
/// `renditions` is the configured ladder in declaration order.
/// `active` carries one entry per live `(transcoder, rendition, broadcast,
/// track)` the runner has observed -- so an operator can see which source
/// broadcasts are currently being transcoded into which renditions and how
/// many fragments each output has produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeState {
    pub enabled: bool,
    /// Encoder backend label: `"software"`, `"videotoolbox"`, `"nvenc"`,
    /// `"vaapi"`, or `"qsv"`. Empty when `enabled` is false.
    pub encoder: String,
    pub renditions: Vec<RenditionInfo>,
    #[serde(default)]
    pub active: Vec<TranscodeActiveStats>,
}

/// One configured rendition in the transcode ladder. Mirrors the operator
/// `lvqr_transcode::RenditionSpec` without the admin crate taking a
/// `lvqr-transcode` dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenditionInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
}

/// Live per-output transcode counters. `fragments_seen` is the cumulative
/// fragment count the named transcoder has produced for `rendition` from
/// `(broadcast, track)`; `panics` is the count of caught panics across the
/// transcoder's lifecycle hooks (a non-zero value flags an unhealthy
/// encoder). Atomic snapshots; may drift a fragment or two between reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeActiveStats {
    pub transcoder: String,
    pub rendition: String,
    pub broadcast: String,
    pub track: String,
    pub fragments_seen: u64,
    pub panics: u64,
}

/// In-process agent state returned by `GET /api/v1/agents`.
///
/// `enabled` reflects whether the binary was built with an agent feature
/// (e.g. `whisper`) AND at least one agent was configured at startup. When
/// false, both lists are empty and the route still returns 200. Read-only
/// introspection of a startup-configured agent set; runtime start/stop is a
/// separate CRUD surface for a later slice.
///
/// `agents` is the configured agent set; `active` carries one entry per live
/// `(agent, broadcast, track)` the runner has observed, so an operator can
/// see which broadcasts each agent is attached to plus its fragment / panic
/// counters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub enabled: bool,
    pub agents: Vec<AgentInfo>,
    #[serde(default)]
    pub active: Vec<AgentActiveStats>,
}

/// One configured in-process agent. `name` is the agent's registry name
/// (the track-id it publishes under, e.g. `"captions"`); `kind` is a
/// display grouping. `model` / `window_ms` are agent-specific config (set
/// for the Whisper captions agent); both are `None` for agents that do not
/// use them. No secrets are exposed -- `model` is a local file path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub window_ms: Option<u32>,
}

/// Live per-attachment agent counters. `fragments_seen` is the cumulative
/// fragment count the named agent has processed for `(broadcast, track)`;
/// `panics` counts caught panics across the agent's lifecycle hooks (a
/// non-zero value flags an unhealthy agent). Atomic snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentActiveStats {
    pub agent: String,
    pub broadcast: String,
    pub track: String,
    pub fragments_seen: u64,
    pub panics: u64,
}

/// Provider for /metrics endpoint output. Returns Prometheus text-format
/// metrics. Set up by Phase 4 (metrics).
pub type MetricsRender = Arc<dyn Fn() -> String + Send + Sync>;

/// Snapshot closure backing `GET /api/v1/streams/{name}`. Takes a broadcast
/// name and returns its per-track detail, or `None` (-> 404) when no track
/// for that broadcast is registered.
pub type StreamDetailFn = Arc<dyn Fn(&str) -> Option<StreamDetailInfo> + Send + Sync>;

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
    /// Optional runtime stream-key store backing the
    /// `/api/v1/streamkeys/*` admin routes (session 146). `None`
    /// means the operator booted with `--no-streamkeys` (or an
    /// embedder did not wire the store); the list endpoint then
    /// returns `{"keys":[]}` and the mutating endpoints return
    /// 500 with a clear "store not configured" message.
    streamkey_store: Option<SharedStreamKeyStore>,
    /// Optional `GET /api/v1/config-reload` status closure
    /// (session 147). Populated by `lvqr-cli`'s composition root
    /// when `--config` is set. `None` -> the route returns a
    /// default `ConfigReloadStatus` (every Optional unset).
    config_reload_status: Option<crate::config_reload_routes::ConfigReloadStatusFn>,
    /// Optional `POST /api/v1/config-reload` trigger closure
    /// (session 147). `None` -> the POST returns 503.
    config_reload_trigger: Option<crate::config_reload_routes::ConfigReloadTriggerFn>,
    /// Optional snapshot closure backing `GET /api/v1/server-info`
    /// (session 164). lvqr-cli's composition root passes a closure
    /// that reads the parsed `ServeConfig` + every listener's bound
    /// address + the cargo crate version + `Instant::now()` for
    /// uptime. `None` -> the route returns the stub
    /// `default_server_info()` (just `CARGO_PKG_VERSION` + empty
    /// bound + features) so dashboards can pre-bake the response
    /// shape before the CLI is wired.
    server_info: Option<crate::server_info_routes::ServerInfoFn>,
    /// Per-broadcast detail closure backing `GET /api/v1/streams/{name}`.
    /// Populated by [`AdminState::with_stream_detail`]; lvqr-cli's
    /// composition root passes a closure that introspects the
    /// `FragmentBroadcasterRegistry`. The default returns `None` for
    /// every name so the route replies 404 until wired -- keeping the
    /// admin crate free of a `lvqr-fragment` dependency.
    get_stream_detail: StreamDetailFn,
    /// Snapshot callback for the `GET /api/v1/transcode/ladders` route.
    /// Populated by [`AdminState::with_transcode`]; defaults to a
    /// "no ladder configured" closure returning `enabled: false`. The
    /// indirection keeps `lvqr-admin` free of a `lvqr-transcode`
    /// dependency so non-transcode builds pay no graph cost.
    get_transcode: Arc<dyn Fn() -> TranscodeState + Send + Sync>,
    /// Snapshot callback for the `GET /api/v1/agents` route. Populated by
    /// [`AdminState::with_agents`]; defaults to a "no agents configured"
    /// closure returning `enabled: false`. Keeps `lvqr-admin` free of any
    /// agent-crate dependency.
    get_agents: Arc<dyn Fn() -> AgentState + Send + Sync>,
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
            streamkey_store: None,
            config_reload_status: None,
            config_reload_trigger: None,
            server_info: None,
            get_stream_detail: Arc::new(|_name| None),
            get_transcode: Arc::new(|| TranscodeState {
                enabled: false,
                encoder: String::new(),
                renditions: Vec::new(),
                active: Vec::new(),
            }),
            get_agents: Arc::new(|| AgentState {
                enabled: false,
                agents: Vec::new(),
                active: Vec::new(),
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

    /// Wire the per-broadcast detail closure backing
    /// `GET /api/v1/streams/{name}`. The closure receives the requested
    /// broadcast name and returns `Some(detail)` when at least one track
    /// for that broadcast exists, `None` otherwise (the route maps `None`
    /// to 404). Without this call every name resolves to 404.
    pub fn with_stream_detail(
        mut self,
        get: impl Fn(&str) -> Option<StreamDetailInfo> + Send + Sync + 'static,
    ) -> Self {
        self.get_stream_detail = Arc::new(get);
        self
    }

    /// Wire the transcode-ladder snapshot closure backing
    /// `GET /api/v1/transcode/ladders`. The CLI composition root calls
    /// this only on `transcode`-feature builds with a configured ladder;
    /// without it the route reports `enabled: false`.
    pub fn with_transcode(mut self, get: impl Fn() -> TranscodeState + Send + Sync + 'static) -> Self {
        self.get_transcode = Arc::new(get);
        self
    }

    /// Wire the in-process agent snapshot closure backing
    /// `GET /api/v1/agents`. The CLI composition root calls this only on
    /// agent-feature builds (e.g. `whisper`) with a configured agent;
    /// without it the route reports `enabled: false`.
    pub fn with_agents(mut self, get: impl Fn() -> AgentState + Send + Sync + 'static) -> Self {
        self.get_agents = Arc::new(get);
        self
    }

    /// Wire the runtime stream-key store backing the
    /// `/api/v1/streamkeys/*` admin routes (session 146). Without
    /// this call the list endpoint serves an empty array and the
    /// mutating endpoints return 500 -- which is what the operator
    /// gets when they boot with `--no-streamkeys`. The CLI's
    /// composition root always calls this when streamkeys are
    /// enabled (the default).
    pub fn with_streamkey_store(mut self, store: SharedStreamKeyStore) -> Self {
        self.streamkey_store = Some(store);
        self
    }

    /// Borrow the configured stream-key store, if any. Used by
    /// [`crate::streamkey_routes`] handlers.
    pub(crate) fn streamkey_store(&self) -> Option<&SharedStreamKeyStore> {
        self.streamkey_store.as_ref()
    }

    /// Wire a `GET /api/v1/config-reload` status closure
    /// (session 147). lvqr-cli's composition root passes a
    /// closure that calls `ConfigReloadHandle::status()`.
    pub fn with_config_reload_status(mut self, f: crate::config_reload_routes::ConfigReloadStatusFn) -> Self {
        self.config_reload_status = Some(f);
        self
    }

    /// Wire a `POST /api/v1/config-reload` trigger closure
    /// (session 147). lvqr-cli's composition root passes a
    /// closure that calls `ConfigReloadHandle::reload("admin_post")`.
    pub fn with_config_reload_trigger(mut self, f: crate::config_reload_routes::ConfigReloadTriggerFn) -> Self {
        self.config_reload_trigger = Some(f);
        self
    }

    /// Borrow the wired status closure (or return a default
    /// `ConfigReloadStatus` when not wired).
    pub(crate) fn config_reload_status(&self) -> crate::config_reload_routes::ConfigReloadStatus {
        match self.config_reload_status.as_ref() {
            Some(f) => f(),
            None => crate::config_reload_routes::ConfigReloadStatus::default(),
        }
    }

    /// Borrow the wired trigger closure (or `None` when not wired).
    pub(crate) fn config_reload_trigger(&self) -> Option<&crate::config_reload_routes::ConfigReloadTriggerFn> {
        self.config_reload_trigger.as_ref()
    }

    /// Wire a `GET /api/v1/server-info` snapshot closure (session 164).
    /// lvqr-cli's composition root calls this with a closure built via
    /// [`crate::server_info_routes::server_info_fn_with_uptime`] over
    /// the parsed `ServeConfig` + bound listener addresses.
    pub fn with_server_info(mut self, f: crate::server_info_routes::ServerInfoFn) -> Self {
        self.server_info = Some(f);
        self
    }

    /// Snapshot the wired server-info closure, or return the stub
    /// `default_server_info()` when none is wired. Always returns a
    /// `ServerInfo` (never `None`) so the route handler can stay
    /// trivially infallible.
    pub(crate) fn server_info(&self) -> crate::server_info_routes::ServerInfo {
        match self.server_info.as_ref() {
            Some(f) => f(),
            None => crate::server_info_routes::default_server_info(),
        }
    }
}

/// Structured error responses for the admin API.
#[derive(Debug)]
pub enum AdminError {
    Unauthorized(String),
    NotFound(String),
    Internal(String),
    /// Session 156 follow-up: 400 for client-supplied JSON whose
    /// shape parsed but whose values fail server-side validation
    /// (negative latency, oversized strings, bogus transport label).
    BadRequest(String),
    /// Session 156 follow-up: 503 for routes that depend on a
    /// runtime handle the operator did not wire (e.g.
    /// `POST /api/v1/slo/client-sample` when the SLO tracker was
    /// not installed via `AdminState::with_slo`).
    Unavailable(String),
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AdminError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            AdminError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            AdminError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
            AdminError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AdminError::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
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
        .route("/api/v1/streams/{name}", get(get_stream_detail))
        .route("/api/v1/mesh", get(get_mesh))
        .route("/api/v1/slo", get(get_slo))
        .route("/api/v1/wasm-filter", get(get_wasm_filter))
        .route("/api/v1/transcode/ladders", get(get_transcode))
        .route("/api/v1/agents", get(get_agents))
        .route(
            "/api/v1/streamkeys",
            get(crate::streamkey_routes::list_streamkeys).post(crate::streamkey_routes::mint_streamkey),
        )
        .route(
            "/api/v1/streamkeys/{id}",
            delete(crate::streamkey_routes::revoke_streamkey),
        )
        .route(
            "/api/v1/streamkeys/{id}/rotate",
            post(crate::streamkey_routes::rotate_streamkey),
        )
        .route(
            "/api/v1/config-reload",
            get(crate::config_reload_routes::get_config_reload)
                .post(crate::config_reload_routes::trigger_config_reload),
        )
        .route("/api/v1/server-info", get(crate::server_info_routes::get_server_info));

    #[cfg(feature = "cluster")]
    {
        api_routes = api_routes.merge(crate::cluster_routes::cluster_router());
    }

    let api_routes = api_routes.layer(middleware::from_fn_with_state(auth, auth_middleware));

    // Session 156 follow-up: `/api/v1/slo/client-sample` lives off
    // the admin-only middleware so subscribe-token-bearing clients
    // (Tier 5 SDKs) can push samples without holding an admin
    // token. The handler's own dual-auth check accepts EITHER an
    // admin token or a subscribe token validated against the
    // broadcast in the request body.
    let api_dual_auth_routes: Router<AdminState> =
        Router::new().route("/api/v1/slo/client-sample", post(post_client_sample));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_handler))
        .merge(api_routes)
        .merge(api_dual_auth_routes)
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

/// `GET /api/v1/streams/{name}` -- per-broadcast track breakdown. Returns
/// 404 when no track for the broadcast is currently registered.
async fn get_stream_detail(
    State(state): State<AdminState>,
    Path(name): Path<String>,
) -> Result<Json<StreamDetailInfo>, AdminError> {
    match (state.get_stream_detail)(&name) {
        Some(detail) => Ok(Json(detail)),
        None => Err(AdminError::NotFound(format!("no active broadcast named {name:?}"))),
    }
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

/// JSON body for [`post_client_sample`]. Tier 5 client SDK callers
/// (browser / Rust / Python subscribers) record their own
/// glass-to-glass latency on every received frame and POST a
/// summary sample here so the relay's per-(broadcast, transport)
/// tracker can include MoQ / WHEP / etc. egress in its SLO
/// snapshot.
///
/// Session 156 follow-up: the v1.1-B scoping call rejected a MoQ
/// wire-format change that would let the server measure pure-MoQ
/// subscriber latency directly (would break foreign MoQ clients).
/// This endpoint is the documented path forward: clients push back
/// sampled timestamps, the server records them into the same
/// [`crate::slo::LatencyTracker`] surface that already powers the
/// existing `lvqr_subscriber_glass_to_glass_ms` Prometheus
/// histogram.
#[derive(Debug, Deserialize)]
struct ClientLatencySample {
    /// Source broadcast name (e.g. `"live/demo"`).
    broadcast: String,
    /// Egress surface: `"moq"`, `"whep"`, `"hls"`, `"dash"`, `"ws"`.
    /// Free-form to keep the contract additive for future
    /// transports; the tracker stores the label verbatim.
    transport: String,
    /// Wall-clock UNIX-ms timestamp anchored on the publisher's
    /// frame. Recovery is transport-specific:
    ///
    /// * HLS subscribers lift it from `#EXT-X-PROGRAM-DATE-TIME`
    ///   via `HTMLMediaElement.getStartDate() + currentTime`. See
    ///   `bindings/js/packages/dvr-player/src/slo-sampler.ts` for
    ///   the reference client.
    /// * MoQ subscribers do not have a per-frame wall-clock channel
    ///   on the wire today. `lvqr_fragment::MoqTrackSink::push`
    ///   writes only the fMP4 payload bytes
    ///   (`crates/lvqr-fragment/src/moq_sink.rs`); the v1.1-B
    ///   scoping call rejected adding a per-frame wall-clock to the
    ///   in-band frame format. Closing pure-MoQ glass-to-glass is
    ///   tracked as a v1.2 follow-up; the session 157 audit
    ///   (`tracking/SESSION_157_BRIEFING.md`) sketches a sibling
    ///   `<broadcast>/0.timing` MoQ track as the candidate
    ///   non-breaking design.
    ///
    /// `lvqr_fragment::Fragment::ingest_time_ms` carries the same
    /// stamp on the in-memory ingest pipeline (server-side
    /// stamping for the existing
    /// `lvqr_subscriber_glass_to_glass_ms` histogram), but it is
    /// NOT part of any wire format -- a network client cannot lift
    /// it from a received frame.
    ingest_ts_ms: u64,
    /// Wall-clock UNIX-ms timestamp the client recorded on render
    /// (the moment the frame's pixels reached the consumer's
    /// pipeline -- usually `videoEl.currentTime` advancing past the
    /// frame, or the equivalent on a non-browser client).
    render_ts_ms: u64,
}

/// Upper bound on the latency value the tracker will accept (5 min).
/// Anything beyond this is almost certainly clock skew between
/// publisher + subscriber, not real glass-to-glass latency, and
/// would corrupt the percentile histogram. Reject with 400 so the
/// caller can fix their clock-sync setup.
const MAX_ACCEPTED_LATENCY_MS: u64 = 300_000;

/// Cap on the broadcast-name string length. Matches LVQR's existing
/// broadcast-key constraints throughout the workspace.
const MAX_BROADCAST_LEN: usize = 256;

/// Cap on the transport-label length. Real labels are <=8 chars
/// (`moq`, `whep`, `hls`, `dash`, `ws`); 32 is generous headroom for
/// future qualifiers (e.g. `moq-v2`).
const MAX_TRANSPORT_LEN: usize = 32;

/// `POST /api/v1/slo/client-sample` handler. Validates the JSON
/// body, computes `latency_ms = render_ts_ms - ingest_ts_ms`, and
/// records into the configured [`crate::slo::LatencyTracker`].
/// Returns 204 No Content on success, 400 on validation failure,
/// 503 when no tracker is wired (the GET route returns an empty
/// list silently in that case; POST cannot do the equivalent
/// because the sample has nowhere to land), 401 when the
/// dual-auth check rejects both admin AND subscribe paths.
///
/// **Auth (dual)**: the handler is mounted OFF the admin-only
/// middleware so subscribe-token-bearing clients (the natural
/// future Tier 5 client SDK) can push samples without holding an
/// admin scope. The bearer token is checked against
/// [`AuthContext::Admin`] first; on deny the same token is
/// re-checked against
/// `AuthContext::Subscribe { broadcast: <body.broadcast> }`. Either
/// path opens the sample. The auth provider's existing
/// per-broadcast subscribe-token logic naturally enforces
/// "subscribers can only push samples for broadcasts they're
/// allowed to subscribe to", which prevents token-laundering /
/// sample-pollution from a subscriber pushing samples against a
/// broadcast it has no permission to read.
async fn post_client_sample(
    State(state): State<AdminState>,
    headers: axum::http::HeaderMap,
    Json(sample): Json<ClientLatencySample>,
) -> Result<StatusCode, AdminError> {
    let tracker = state
        .slo
        .as_ref()
        .ok_or_else(|| AdminError::Unavailable("SLO tracker not configured on this server".into()))?;

    let broadcast = sample.broadcast.trim();
    if broadcast.is_empty() {
        return Err(AdminError::BadRequest("broadcast must be non-empty".into()));
    }
    if broadcast.len() > MAX_BROADCAST_LEN {
        return Err(AdminError::BadRequest(format!(
            "broadcast exceeds max length ({MAX_BROADCAST_LEN} chars)"
        )));
    }
    let transport = sample.transport.trim();
    if transport.is_empty() {
        return Err(AdminError::BadRequest("transport must be non-empty".into()));
    }
    if transport.len() > MAX_TRANSPORT_LEN {
        return Err(AdminError::BadRequest(format!(
            "transport exceeds max length ({MAX_TRANSPORT_LEN} chars)"
        )));
    }
    if sample.render_ts_ms < sample.ingest_ts_ms {
        return Err(AdminError::BadRequest(
            "render_ts_ms must be >= ingest_ts_ms (client clock skew?)".into(),
        ));
    }
    let latency_ms = sample.render_ts_ms - sample.ingest_ts_ms;
    if latency_ms > MAX_ACCEPTED_LATENCY_MS {
        return Err(AdminError::BadRequest(format!(
            "latency_ms {latency_ms} exceeds {MAX_ACCEPTED_LATENCY_MS} ms cap (clock skew or bad sample)"
        )));
    }

    // Dual auth: try admin first (current operator-facing surface),
    // then subscribe (Tier 5 client SDK path) against the broadcast
    // in the body. The auth provider's existing per-broadcast
    // subscribe-token logic enforces that subscribers can only push
    // samples for broadcasts they're allowed to subscribe to.
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::to_string)
        .unwrap_or_default();
    let admin_decision = state.auth.check(&AuthContext::Admin { token: token.clone() });
    let allowed = matches!(admin_decision, AuthDecision::Allow) || {
        let sub_decision = state.auth.check(&AuthContext::Subscribe {
            token: if token.is_empty() { None } else { Some(token.clone()) },
            broadcast: broadcast.to_string(),
        });
        matches!(sub_decision, AuthDecision::Allow)
    };
    if !allowed {
        metrics::counter!(
            "lvqr_auth_failures_total",
            "entry" => "slo_client_sample",
        )
        .increment(1);
        return Err(AdminError::Unauthorized(
            "admin or subscribe token required to push SLO samples".into(),
        ));
    }

    tracker.record(broadcast, transport, latency_ms);
    metrics::counter!(
        "lvqr_slo_client_samples_total",
        "transport" => transport.to_string(),
    )
    .increment(1);
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/wasm-filter` handler. Returns the chain length +
/// per-`(broadcast, track)` counters for the configured WASM filter
/// chain, or an empty "disabled" body when `--wasm-filter` is unset.
/// PLAN Phase D session 137.
async fn get_wasm_filter(State(state): State<AdminState>) -> Result<Json<WasmFilterState>, AdminError> {
    Ok(Json((state.get_wasm_filter)()))
}

/// `GET /api/v1/transcode/ladders` -- configured transcode ladder plus live
/// per-output counters. Returns `enabled: false` when no ladder is wired.
async fn get_transcode(State(state): State<AdminState>) -> Result<Json<TranscodeState>, AdminError> {
    Ok(Json((state.get_transcode)()))
}

/// `GET /api/v1/agents` -- configured in-process agents plus live
/// per-attachment counters. Returns `enabled: false` when none are wired.
async fn get_agents(State(state): State<AdminState>) -> Result<Json<AgentState>, AdminError> {
    Ok(Json((state.get_agents)()))
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
                    capacity: Some(5),
                },
                MeshPeerStats {
                    peer_id: "relay-7".into(),
                    role: "Relay".into(),
                    parent: Some("root-1".into()),
                    depth: 1,
                    intended_children: 1,
                    forwarded_frames: 400,
                    capacity: None,
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
        // Session 144: the admin body now also surfaces the per-peer
        // self-reported capacity. The Root advertised 5; the Relay
        // omitted the field (None on the wire).
        assert_eq!(root.capacity, Some(5));

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
        assert!(relay.capacity.is_none(), "relay omitted capacity on the wire");
    }

    /// Session 144: a pre-144 admin body that omits the per-peer
    /// `capacity` field must deserialize into a new client with
    /// `capacity = None` via `#[serde(default)]`.
    #[tokio::test]
    async fn mesh_peer_stats_deserializes_pre_144_body_without_capacity() {
        let body = br#"{"enabled":true,"peer_count":1,"offload_percentage":0.0,"peers":[
            {"peer_id":"root-1","role":"Root","parent":null,"depth":0,
             "intended_children":0,"forwarded_frames":0}
        ]}"#;
        let mesh: MeshState = serde_json::from_slice(body).unwrap();
        assert_eq!(mesh.peers.len(), 1);
        assert!(
            mesh.peers[0].capacity.is_none(),
            "missing capacity field must default to None"
        );
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
    async fn client_sample_records_into_tracker_and_returns_204() {
        let tracker = crate::slo::LatencyTracker::new();
        let state = test_state(vec![]).with_slo(tracker.clone());
        let app = build_router(state);
        let body = serde_json::json!({
            "broadcast": "live/demo",
            "transport": "moq",
            "ingest_ts_ms": 1_714_066_800_000_u64,
            "render_ts_ms": 1_714_066_800_120_u64,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/slo/client-sample")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let snap = tracker.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].broadcast, "live/demo");
        assert_eq!(snap[0].transport, "moq");
        assert_eq!(snap[0].sample_count, 1);
        assert_eq!(snap[0].max_ms, 120);
    }

    #[tokio::test]
    async fn client_sample_returns_503_without_tracker() {
        let state = test_state(vec![]);
        let app = build_router(state);
        let body = serde_json::json!({
            "broadcast": "live/demo",
            "transport": "moq",
            "ingest_ts_ms": 0_u64,
            "render_ts_ms": 100_u64,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/slo/client-sample")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn client_sample_rejects_negative_latency_400() {
        let tracker = crate::slo::LatencyTracker::new();
        let state = test_state(vec![]).with_slo(tracker);
        let app = build_router(state);
        let body = serde_json::json!({
            "broadcast": "live/demo",
            "transport": "moq",
            "ingest_ts_ms": 200_u64,
            "render_ts_ms": 100_u64,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/slo/client-sample")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn client_sample_rejects_oversized_latency_400() {
        let tracker = crate::slo::LatencyTracker::new();
        let state = test_state(vec![]).with_slo(tracker);
        let app = build_router(state);
        // 10 minutes apart -- well past the 5-minute clock-skew cap.
        let body = serde_json::json!({
            "broadcast": "live/demo",
            "transport": "moq",
            "ingest_ts_ms": 0_u64,
            "render_ts_ms": 600_000_u64,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/slo/client-sample")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn client_sample_rejects_empty_broadcast_400() {
        let tracker = crate::slo::LatencyTracker::new();
        let state = test_state(vec![]).with_slo(tracker);
        let app = build_router(state);
        let body = serde_json::json!({
            "broadcast": "",
            "transport": "moq",
            "ingest_ts_ms": 0_u64,
            "render_ts_ms": 100_u64,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/slo/client-sample")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn client_sample_rejects_oversized_transport_label_400() {
        let tracker = crate::slo::LatencyTracker::new();
        let state = test_state(vec![]).with_slo(tracker);
        let app = build_router(state);
        // > 32 chars
        let big_label: String = "a".repeat(64);
        let body = serde_json::json!({
            "broadcast": "live/demo",
            "transport": big_label,
            "ingest_ts_ms": 0_u64,
            "render_ts_ms": 100_u64,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/slo/client-sample")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn client_sample_rejects_request_with_no_token_when_auth_is_configured() {
        // Session 156 follow-up: dual-auth contract. With static
        // tokens configured (both admin + subscribe), an unauthed
        // request must be rejected. Mirrors the auth gate's
        // `AuthDecision::Deny` for both contexts.
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            subscribe_token: Some("subtok".into()),
            ..Default::default()
        }));
        let tracker = crate::slo::LatencyTracker::new();
        let state = test_state(vec![]).with_auth(auth).with_slo(tracker);
        let app = build_router(state);
        let body = serde_json::json!({
            "broadcast": "live/demo",
            "transport": "moq",
            "ingest_ts_ms": 0_u64,
            "render_ts_ms": 100_u64,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/slo/client-sample")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn client_sample_admin_token_is_accepted() {
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            subscribe_token: Some("subtok".into()),
            ..Default::default()
        }));
        let tracker = crate::slo::LatencyTracker::new();
        let state = test_state(vec![]).with_auth(auth).with_slo(tracker.clone());
        let app = build_router(state);
        let body = serde_json::json!({
            "broadcast": "live/demo",
            "transport": "moq",
            "ingest_ts_ms": 0_u64,
            "render_ts_ms": 100_u64,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/slo/client-sample")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(tracker.snapshot().len(), 1);
    }

    #[tokio::test]
    async fn client_sample_subscribe_token_is_accepted() {
        // Session 156 follow-up: subscribers shouldn't need an
        // admin token to push their own latency samples. The
        // dual-auth path lets a subscriber-scoped token through
        // when it's valid for the broadcast in the body.
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("admintok".into()),
            subscribe_token: Some("subtok".into()),
            ..Default::default()
        }));
        let tracker = crate::slo::LatencyTracker::new();
        let state = test_state(vec![]).with_auth(auth).with_slo(tracker.clone());
        let app = build_router(state);
        let body = serde_json::json!({
            "broadcast": "live/demo",
            "transport": "moq",
            "ingest_ts_ms": 1_000_u64,
            "render_ts_ms": 1_120_u64,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/slo/client-sample")
                    .header(header::CONTENT_TYPE, "application/json")
                    // Subscribe-scope bearer; the static provider's
                    // Subscribe match accepts it for any broadcast.
                    .header(header::AUTHORIZATION, "Bearer subtok")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let snap = tracker.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].max_ms, 120);
    }

    #[tokio::test]
    async fn client_sample_wrong_token_is_rejected() {
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("admintok".into()),
            subscribe_token: Some("subtok".into()),
            ..Default::default()
        }));
        let tracker = crate::slo::LatencyTracker::new();
        let state = test_state(vec![]).with_auth(auth).with_slo(tracker);
        let app = build_router(state);
        let body = serde_json::json!({
            "broadcast": "live/demo",
            "transport": "moq",
            "ingest_ts_ms": 0_u64,
            "render_ts_ms": 100_u64,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/slo/client-sample")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, "Bearer not-the-token")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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

    #[tokio::test]
    async fn stream_detail_404_when_unwired() {
        let state = test_state(vec![("live/demo", 1)]);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/streams/live%2Fdemo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn stream_detail_renders_wired_snapshot() {
        let state = test_state(vec![]).with_stream_detail(|name| {
            if name != "live/demo" {
                return None;
            }
            Some(StreamDetailInfo {
                name: name.to_string(),
                subscribers: 3,
                tracks: vec![
                    TrackInfo {
                        track: "0.mp4".into(),
                        kind: "video".into(),
                        codec: "avc1.640028".into(),
                        timescale: 90_000,
                        fragments: 120,
                        subscribers: 3,
                        lagged_skips: 0,
                    },
                    TrackInfo {
                        track: "1.mp4".into(),
                        kind: "audio".into(),
                        codec: "mp4a.40.2".into(),
                        timescale: 48_000,
                        fragments: 240,
                        subscribers: 2,
                        lagged_skips: 1,
                    },
                ],
            })
        });
        let app = build_router(state);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/streams/live%2Fdemo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let detail: StreamDetailInfo = serde_json::from_slice(&body).unwrap();
        assert_eq!(detail.name, "live/demo");
        assert_eq!(detail.subscribers, 3);
        assert_eq!(detail.tracks.len(), 2);
        assert_eq!(detail.tracks[0].track, "0.mp4");
        assert_eq!(detail.tracks[0].kind, "video");
        assert_eq!(detail.tracks[0].fragments, 120);
        assert_eq!(detail.tracks[1].kind, "audio");
        assert_eq!(detail.tracks[1].lagged_skips, 1);

        // Unknown broadcast -> 404.
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/streams/live%2Fmissing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn transcode_route_defaults_to_disabled_when_unconfigured() {
        let state = test_state(vec![]);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/transcode/ladders")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let st: TranscodeState = serde_json::from_slice(&body).unwrap();
        assert!(!st.enabled);
        assert!(st.encoder.is_empty());
        assert!(st.renditions.is_empty());
        assert!(st.active.is_empty());
    }

    #[tokio::test]
    async fn transcode_route_renders_configured_ladder() {
        let state = test_state(vec![]).with_transcode(|| TranscodeState {
            enabled: true,
            encoder: "software".into(),
            renditions: vec![
                RenditionInfo {
                    name: "720p".into(),
                    width: 1280,
                    height: 720,
                    video_bitrate_kbps: 2500,
                    audio_bitrate_kbps: 128,
                },
                RenditionInfo {
                    name: "480p".into(),
                    width: 854,
                    height: 480,
                    video_bitrate_kbps: 1200,
                    audio_bitrate_kbps: 96,
                },
            ],
            active: vec![TranscodeActiveStats {
                transcoder: "software".into(),
                rendition: "720p".into(),
                broadcast: "live/demo".into(),
                track: "0.mp4".into(),
                fragments_seen: 42,
                panics: 0,
            }],
        });
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/transcode/ladders")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let st: TranscodeState = serde_json::from_slice(&body).unwrap();
        assert!(st.enabled);
        assert_eq!(st.encoder, "software");
        assert_eq!(st.renditions.len(), 2);
        assert_eq!(st.renditions[0].name, "720p");
        assert_eq!(st.renditions[0].height, 720);
        assert_eq!(st.active.len(), 1);
        assert_eq!(st.active[0].rendition, "720p");
        assert_eq!(st.active[0].fragments_seen, 42);
        assert_eq!(st.active[0].panics, 0);
    }

    #[tokio::test]
    async fn transcode_route_respects_admin_auth() {
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            ..Default::default()
        }));
        let state = test_state(vec![]).with_auth(auth);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/transcode/ladders")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn agents_route_defaults_to_disabled_when_unconfigured() {
        let state = test_state(vec![]);
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/api/v1/agents").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let st: AgentState = serde_json::from_slice(&body).unwrap();
        assert!(!st.enabled);
        assert!(st.agents.is_empty());
        assert!(st.active.is_empty());
    }

    #[tokio::test]
    async fn agents_route_renders_configured_agents() {
        let state = test_state(vec![]).with_agents(|| AgentState {
            enabled: true,
            agents: vec![AgentInfo {
                name: "captions".into(),
                kind: "captions".into(),
                model: Some("/models/ggml-tiny.en.bin".into()),
                window_ms: Some(5_000),
            }],
            active: vec![AgentActiveStats {
                agent: "captions".into(),
                broadcast: "live/demo".into(),
                track: "1.mp4".into(),
                fragments_seen: 17,
                panics: 0,
            }],
        });
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/api/v1/agents").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let st: AgentState = serde_json::from_slice(&body).unwrap();
        assert!(st.enabled);
        assert_eq!(st.agents.len(), 1);
        assert_eq!(st.agents[0].name, "captions");
        assert_eq!(st.agents[0].model.as_deref(), Some("/models/ggml-tiny.en.bin"));
        assert_eq!(st.agents[0].window_ms, Some(5_000));
        assert_eq!(st.active.len(), 1);
        assert_eq!(st.active[0].broadcast, "live/demo");
        assert_eq!(st.active[0].fragments_seen, 17);
    }

    #[tokio::test]
    async fn agents_route_respects_admin_auth() {
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            ..Default::default()
        }));
        let state = test_state(vec![]).with_auth(auth);
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/api/v1/agents").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

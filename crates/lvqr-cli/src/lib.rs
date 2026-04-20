//! LVQR server library entry point.
//!
//! `main.rs` parses CLI args and hands off to [`start`]; tests and embedders
//! can call [`start`] directly with a pre-built [`ServeConfig`]. Every
//! listener (MoQ/QUIC, RTMP/TCP, admin/TCP) is bound inside `start` before
//! it returns, so callers who pass `port: 0` can read the real bound
//! addresses back off the returned [`ServerHandle`] and point test clients
//! at them without polling.
//!
//! This is the library target used by `lvqr-test-utils::TestServer` to
//! spin up a full-stack LVQR instance on ephemeral ports inside
//! integration tests.

mod archive;
mod captions;
#[cfg(feature = "cluster")]
pub mod cluster_claim;
mod hls;

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;
use lvqr_auth::{AuthContext, AuthDecision, NoopAuthProvider, SharedAuth, extract};
use lvqr_core::{EventBus, RelayEvent};
use lvqr_dash::{BroadcasterDashBridge, DashConfig};
use lvqr_fragment::FragmentBroadcasterRegistry;
use lvqr_hls::{MultiHlsServer, PlaylistBuilderConfig};
use lvqr_moq::Track;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;

#[cfg(feature = "c2pa")]
use crate::archive::verify_router;
use crate::archive::{BroadcasterArchiveIndexer, playback_router};
use crate::hls::BroadcasterHlsBridge;

/// Configuration passed to [`start`] to bring up a full-stack LVQR server.
///
/// Every `*_addr` field accepts port `0` for ephemeral port binding; the
/// real bound address is reported back through [`ServerHandle`].
#[derive(Clone)]
pub struct ServeConfig {
    /// QUIC/MoQ relay bind address.
    pub relay_addr: SocketAddr,
    /// RTMP ingest bind address.
    pub rtmp_addr: SocketAddr,
    /// Admin HTTP (and WS relay/ingest) bind address.
    pub admin_addr: SocketAddr,
    /// Optional LL-HLS HTTP bind address. When `Some`, `start()` spins up a
    /// dedicated `HlsServer` axum router on this address that observes the
    /// RTMP bridge's fragment output and serves `/playlist.m3u8`,
    /// `/init.mp4`, and the per-chunk media URIs the playlist references.
    /// When `None`, no HLS surface is exposed.
    pub hls_addr: Option<SocketAddr>,
    /// DVR window depth in seconds for the LL-HLS sliding-window
    /// eviction. Translated to `max_segments = dvr_secs /
    /// target_duration_secs` at server construction. 0 means
    /// unbounded (no eviction). Default: 120 (60 segments at 2 s).
    pub hls_dvr_window_secs: u32,
    /// LL-HLS target segment duration in seconds. Controls both the
    /// `EXT-X-TARGETDURATION` declaration and the CMAF segmenter's
    /// segment-close policy. Default: 2.
    pub hls_target_duration_secs: u32,
    /// LL-HLS target partial (chunk) duration in milliseconds.
    /// Controls both `EXT-X-PART-INF:PART-TARGET` and the CMAF
    /// segmenter's partial-close policy. Default: 200.
    pub hls_part_target_ms: u32,
    /// Optional WHEP (WebRTC HTTP Egress Protocol) HTTP bind address.
    /// When `Some`, `start()` constructs a `Str0mAnswerer` and a
    /// `WhepServer`, attaches the server as a `RawSampleObserver` on
    /// the RTMP bridge, and spins up an axum router on this address
    /// that accepts `POST /whep/{broadcast}` SDP offers, answers
    /// them via `str0m`, and fans each ingest sample into every
    /// subscribed session. When `None`, no WHEP surface is exposed
    /// and no `str0m` state is constructed.
    pub whep_addr: Option<SocketAddr>,
    /// Optional WHIP (WebRTC HTTP Ingest Protocol) HTTP bind
    /// address. When `Some`, `start()` constructs a
    /// `Str0mIngestAnswerer` and a `WhipMoqBridge`, attaches it
    /// as an ingest sink, and spins up an axum router on this
    /// address that accepts `POST /whip/{broadcast}` SDP offers.
    /// When `None`, no WHIP surface is exposed.
    pub whip_addr: Option<SocketAddr>,
    /// Optional MPEG-DASH HTTP bind address. When `Some`, `start()`
    /// spins up a `MultiDashServer` axum router on this address
    /// that observes the same fragment stream the LL-HLS bridge
    /// observes and serves `/dash/{broadcast}/manifest.mpd` plus
    /// numbered segment URIs. RTMP and WHIP publishers both feed
    /// the DASH egress without any per-protocol wiring.
    pub dash_addr: Option<SocketAddr>,
    /// Optional RTSP ingest bind address. When `Some`, `start()`
    /// spins up an `RtspServer` on this TCP address that accepts
    /// RTSP ANNOUNCE/RECORD sessions with interleaved RTP and fans
    /// depacketized H.264/HEVC through the fragment observer chain.
    pub rtsp_addr: Option<SocketAddr>,
    /// Optional SRT ingest bind address. When `Some`, `start()`
    /// spins up an `SrtIngestServer` on this UDP address that
    /// accepts MPEG-TS streams and fans them through the fragment
    /// observer chain.
    pub srt_addr: Option<SocketAddr>,
    /// Enable the peer mesh coordinator and `/signal` endpoint.
    pub mesh_enabled: bool,
    /// Max children per mesh parent when `mesh_enabled`.
    pub max_peers: usize,
    /// Pre-built auth provider. `None` means open access (`NoopAuthProvider`).
    pub auth: Option<SharedAuth>,
    /// Recording directory. `None` disables recording.
    pub record_dir: Option<PathBuf>,
    /// DVR archive directory. When `Some`, `start()` opens a
    /// `RedbSegmentIndex` under `<archive_dir>/archive.redb` and
    /// attaches an archiving fragment observer to the RTMP bridge
    /// that writes every emitted fragment to
    /// `<archive_dir>/<broadcast>/<track>/<seq>.m4s` and records a
    /// `SegmentRef` against the index. The index + segment files
    /// back the DVR scrub / time-range playback surface (Tier 2.4).
    pub archive_dir: Option<PathBuf>,
    /// Optional C2PA provenance configuration. When set, every
    /// `(broadcast, track)` drained by the archive indexer runs the
    /// broadcast-end finalize path on drain termination (the moment
    /// every producer-side clone of the broadcaster drops), which
    /// writes `finalized.mp4` + `finalized.c2pa` next to the segment
    /// files. The admin router also mounts `GET /playback/verify/
    /// {broadcast}` for verifying the resulting manifest. Feature-
    /// gated: the field is accessible only when `lvqr-cli` is built
    /// with `--features c2pa` so the `c2pa` transitive closure does
    /// not leak into deployments that do not need provenance.
    /// Tier 4 item 4.3 session B3.
    #[cfg(feature = "c2pa")]
    pub c2pa: Option<lvqr_archive::provenance::C2paConfig>,
    /// Optional path to a whisper.cpp `ggml-*.bin` model. When
    /// set, `start()` constructs a
    /// `lvqr_agent_whisper::WhisperCaptionsFactory` wired against
    /// the shared `FragmentBroadcasterRegistry` (so the generated
    /// caption cues flow through the same
    /// `(broadcast, "captions")` track the LL-HLS subtitle
    /// rendition drains) and installs it on a throwaway
    /// `lvqr_agent::AgentRunner`; the returned
    /// `AgentRunnerHandle` is held on `ServerHandle` for the
    /// server lifetime. Without a value the factory is skipped
    /// entirely and no AI-adjacent state is constructed.
    /// Feature-gated: accessible only when `lvqr-cli` is built
    /// with `--features whisper` so the whisper.cpp + symphonia
    /// transitive closure stays out of deployments that do not
    /// want captions. Tier 4 item 4.5 session D.
    #[cfg(feature = "whisper")]
    pub whisper_model: Option<PathBuf>,
    /// Optional path to a WASM fragment filter module. When set,
    /// `start()` loads + compiles the module via
    /// `lvqr_wasm::WasmFilter::load` and installs a filter tap
    /// on the shared `FragmentBroadcasterRegistry` before any
    /// ingest listener starts accepting traffic. The tap
    /// observes every fragment flowing through every
    /// broadcaster and drives
    /// `lvqr_wasm_fragments_total{outcome=keep|drop}` counters;
    /// in v1 it does NOT modify what downstream subscribers
    /// receive (session-86 scope narrowing). Omit to disable.
    pub wasm_filter: Option<PathBuf>,
    /// Install the global Prometheus recorder. Must be `false` in tests
    /// because `metrics-exporter-prometheus` panics on the second install
    /// in a process. `main.rs` sets this to `true`.
    pub install_prometheus: bool,
    /// Pre-built OTLP metrics recorder handed off by
    /// `lvqr_observability::init` when `LVQR_OTLP_ENDPOINT` is
    /// set. When `Some`, `start()` installs it as the global
    /// `metrics`-crate recorder -- either on its own or composed
    /// with the Prometheus recorder via
    /// `metrics_util::layers::FanoutBuilder` when
    /// `install_prometheus` is also true. When `None`, only the
    /// Prometheus path runs (legacy behavior).
    pub otel_metrics_recorder: Option<lvqr_observability::OtelMetricsRecorder>,
    /// Path to TLS certificate (PEM). Reserved; not consumed yet. The
    /// relay auto-generates self-signed certs when unset.
    pub tls_cert: Option<PathBuf>,
    /// Path to TLS private key (PEM). Reserved; not consumed yet.
    pub tls_key: Option<PathBuf>,
    /// Optional cluster gossip bind address. When `Some`, `start()`
    /// bootstraps an `lvqr_cluster::Cluster` on this address, wires
    /// it into the admin router so `/api/v1/cluster/*` answers, and
    /// installs an `OwnerResolver` on the HLS server so subscribers
    /// hitting this node for a peer-owned broadcast receive a 302
    /// pointing at the owner. `None` (default) disables clustering
    /// and the node behaves as a standalone single-process server.
    ///
    /// Feature-gated on `cluster`; the field is present regardless
    /// so `ServeConfig` stays ABI-stable across feature flips.
    pub cluster_listen: Option<SocketAddr>,
    /// Cluster peer seed addresses. Each entry is an `ip:port`
    /// string the new node gossips to on boot. Ignored when
    /// [`cluster_listen`](Self::cluster_listen) is `None`.
    pub cluster_seeds: Vec<String>,
    /// Cluster-node identifier. `None` auto-generates a random
    /// `lvqr-<16 alphanumeric>` id at bootstrap.
    pub cluster_node_id: Option<String>,
    /// Cluster tag gossipped in every SYN. Chitchat rejects
    /// cross-cluster gossip so two LVQR deployments on the same
    /// subnet stay isolated. Empty string falls back to the
    /// crate-default (`"lvqr"`).
    pub cluster_id: Option<String>,
    /// Externally-reachable HLS base URL this node advertises
    /// (e.g. `"http://a.local:8888"`). When clustering is enabled,
    /// `start()` writes this URL into the per-node `endpoints` KV
    /// so peers redirecting subscribers know where to send them.
    /// `None` skips the publish; peers will then 404 rather than
    /// redirect for this node's broadcasts.
    pub cluster_advertise_hls: Option<String>,
    /// Externally-reachable DASH base URL this node advertises
    /// (e.g. `"http://a.local:8888"`). Shape matches
    /// [`cluster_advertise_hls`](Self::cluster_advertise_hls);
    /// peers use this when composing a 302 `Location` for
    /// `/dash/...` requests.
    pub cluster_advertise_dash: Option<String>,
    /// Externally-reachable RTSP base URL this node advertises
    /// (e.g. `"rtsp://a.local:8554"`). Used by the RTSP 302
    /// redirect-to-owner path on DESCRIBE / PLAY for peer-owned
    /// broadcasts.
    pub cluster_advertise_rtsp: Option<String>,
    /// Cross-cluster federation links. Each link opens a single
    /// outbound MoQ session to a peer cluster's relay and
    /// re-publishes matching broadcasts into the local origin.
    /// Empty list disables federation. Feature-gated on `cluster`
    /// since `FederationLink` lives in `lvqr-cluster`. Tier 4 item
    /// 4.4.
    #[cfg(feature = "cluster")]
    pub federation_links: Vec<lvqr_cluster::FederationLink>,
}

impl ServeConfig {
    /// Minimal loopback config for tests: every listener on `127.0.0.1:0`,
    /// open access, no recording, no Prometheus install.
    pub fn loopback_ephemeral() -> Self {
        let loopback: std::net::IpAddr = std::net::Ipv4Addr::LOCALHOST.into();
        Self {
            relay_addr: (loopback, 0).into(),
            rtmp_addr: (loopback, 0).into(),
            admin_addr: (loopback, 0).into(),
            hls_addr: Some((loopback, 0).into()),
            hls_dvr_window_secs: 120,
            hls_target_duration_secs: 2,
            hls_part_target_ms: 200,
            whep_addr: None,
            whip_addr: None,
            dash_addr: None,
            rtsp_addr: None,
            srt_addr: None,
            mesh_enabled: false,
            max_peers: 3,
            auth: None,
            record_dir: None,
            archive_dir: None,
            #[cfg(feature = "c2pa")]
            c2pa: None,
            #[cfg(feature = "whisper")]
            whisper_model: None,
            wasm_filter: None,
            install_prometheus: false,
            otel_metrics_recorder: None,
            tls_cert: None,
            tls_key: None,
            cluster_listen: None,
            cluster_seeds: Vec::new(),
            cluster_node_id: None,
            cluster_id: None,
            cluster_advertise_hls: None,
            cluster_advertise_dash: None,
            cluster_advertise_rtsp: None,
            #[cfg(feature = "cluster")]
            federation_links: Vec::new(),
        }
    }
}

/// Handle to a running LVQR server. Dropping the handle cancels the shared
/// shutdown token but does not block on subsystem drain; call [`shutdown`]
/// explicitly in tests that need deterministic teardown before the next
/// fixture starts.
///
/// [`shutdown`]: ServerHandle::shutdown
pub struct ServerHandle {
    relay_addr: SocketAddr,
    rtmp_addr: SocketAddr,
    admin_addr: SocketAddr,
    hls_addr: Option<SocketAddr>,
    whep_addr: Option<SocketAddr>,
    whip_addr: Option<SocketAddr>,
    dash_addr: Option<SocketAddr>,
    rtsp_addr: Option<SocketAddr>,
    srt_addr: Option<SocketAddr>,
    shutdown: CancellationToken,
    join: Option<tokio::task::JoinHandle<()>>,
    /// Cluster handle kept alive for the server's lifetime when
    /// clustering is configured. Feature-gated; `None` when the
    /// `cluster` feature is on but `ServeConfig::cluster_listen` is
    /// `None`, and absent entirely when the feature is off.
    #[cfg(feature = "cluster")]
    cluster: Option<std::sync::Arc<lvqr_cluster::Cluster>>,
    /// WASM fragment-filter tap handle kept alive for the
    /// server's lifetime when `--wasm-filter` is configured.
    /// Tests read fragment counters off this handle to assert
    /// the filter actually saw the ingested broadcasts.
    wasm_filter: Option<lvqr_wasm::WasmFilterBridgeHandle>,
    /// `AgentRunner` handle kept alive for the server's
    /// lifetime when `--whisper-model` is configured. Dropping
    /// the handle aborts every per-broadcast drain task the
    /// runner spawned; holding it on `ServerHandle` mirrors the
    /// `wasm_filter` pattern so the lifetime semantics are
    /// identical. Tier 4 item 4.5 session D. Feature-gated
    /// `#[cfg(feature = "whisper")]`.
    #[cfg(feature = "whisper")]
    agent_runner: Option<lvqr_agent::AgentRunnerHandle>,
    /// WASM filter hot-reload watcher kept alive for the
    /// server's lifetime when `--wasm-filter` is configured.
    /// On every debounced change to the module path, the
    /// reloader recompiles the module and swaps it into the
    /// shared filter; in-flight `apply` calls complete on the
    /// old module, subsequent calls see the new one. Unused
    /// directly; held for its `Drop` side effect of stopping
    /// the watcher thread on shutdown.
    _wasm_reloader: Option<lvqr_wasm::WasmFilterReloader>,
    /// Shared `(broadcast, track)`-keyed registry every
    /// ingest crate publishes into and every consumer
    /// (HLS / DASH / archive / WASM filter / captions
    /// bridge) subscribes through. Exposed on the handle so
    /// integration tests can publish synthetic fragments
    /// (e.g. caption cues for the captions track) without
    /// driving a real ingest protocol. Tier 4 item 4.5
    /// session C added this accessor.
    fragment_registry: FragmentBroadcasterRegistry,
    /// Clone of the relay-backing `OriginProducer`. Federation
    /// tests use this to inject synthetic broadcasts on one
    /// server and verify they propagate to another via a
    /// configured [`lvqr_cluster::FederationLink`]. Always
    /// present since every server has an origin; feature gating
    /// lives on the callers that construct broadcasts through
    /// this handle.
    origin: lvqr_moq::OriginProducer,
    /// `FederationRunner` holding outbound MoQ sessions to peer
    /// clusters open for the server's lifetime. `None` when
    /// `ServeConfig::federation_links` is empty. Feature-gated
    /// on `cluster` since `FederationRunner` lives in
    /// `lvqr-cluster`. Tier 4 item 4.4 session B.
    #[cfg(feature = "cluster")]
    federation_runner: Option<lvqr_cluster::FederationRunner>,
}

impl ServerHandle {
    /// Bound QUIC/MoQ relay address.
    pub fn relay_addr(&self) -> SocketAddr {
        self.relay_addr
    }

    /// Bound RTMP ingest address.
    pub fn rtmp_addr(&self) -> SocketAddr {
        self.rtmp_addr
    }

    /// Bound admin HTTP (and WS relay/ingest) address.
    pub fn admin_addr(&self) -> SocketAddr {
        self.admin_addr
    }

    /// Bound LL-HLS HTTP address, when HLS composition is enabled.
    pub fn hls_addr(&self) -> Option<SocketAddr> {
        self.hls_addr
    }

    /// Bound WHEP HTTP address, when WHEP egress is enabled.
    pub fn whep_addr(&self) -> Option<SocketAddr> {
        self.whep_addr
    }

    /// Bound WHIP HTTP address, when WHIP ingest is enabled.
    pub fn whip_addr(&self) -> Option<SocketAddr> {
        self.whip_addr
    }

    /// Bound MPEG-DASH HTTP address, when DASH egress is enabled.
    pub fn dash_addr(&self) -> Option<SocketAddr> {
        self.dash_addr
    }

    /// Bound RTSP ingest TCP address, when RTSP ingest is enabled.
    pub fn rtsp_addr(&self) -> Option<SocketAddr> {
        self.rtsp_addr
    }

    /// Bound SRT ingest UDP address, when SRT ingest is enabled.
    pub fn srt_addr(&self) -> Option<SocketAddr> {
        self.srt_addr
    }

    /// Cloneable handle to the shared
    /// [`FragmentBroadcasterRegistry`] every ingest crate
    /// publishes into and every consumer (HLS, DASH, archive,
    /// WASM filter, captions bridge) subscribes through.
    /// Useful in integration tests that want to publish
    /// synthetic fragments onto a track (e.g. captions cues
    /// for the captions track) without driving a real
    /// ingest protocol. Tier 4 item 4.5 session C exposed
    /// this for the captions HLS E2E test.
    pub fn fragment_registry(&self) -> &FragmentBroadcasterRegistry {
        &self.fragment_registry
    }

    /// Cloneable handle to the relay-backing
    /// [`lvqr_moq::OriginProducer`]. Used by Tier 4 item 4.4
    /// integration tests to inject synthetic MoQ broadcasts on
    /// one server and verify that a configured
    /// [`lvqr_cluster::FederationLink`] propagates them to a
    /// peer server.
    pub fn origin(&self) -> &lvqr_moq::OriginProducer {
        &self.origin
    }

    /// Cluster handle backing this server, when
    /// [`ServeConfig::cluster_listen`] was `Some` at `start()`
    /// time. Returns `None` for single-node servers. Callers
    /// typically drive the handle to claim broadcasts or inspect
    /// membership; the `shutdown()` method on this crate's
    /// `ServerHandle` already tears the cluster down gracefully.
    #[cfg(feature = "cluster")]
    pub fn cluster(&self) -> Option<&std::sync::Arc<lvqr_cluster::Cluster>> {
        self.cluster.as_ref()
    }

    /// `FederationRunner` handle backing this server, when
    /// [`ServeConfig::federation_links`] was non-empty at
    /// `start()` time. Returns `None` otherwise. Tier 4 item
    /// 4.4 session B.
    #[cfg(feature = "cluster")]
    pub fn federation_runner(&self) -> Option<&lvqr_cluster::FederationRunner> {
        self.federation_runner.as_ref()
    }

    /// HTTP URL pointing at a path on the DASH surface, e.g.
    /// `dash_url("/dash/live/test/manifest.mpd")`. Returns `None`
    /// when DASH is not enabled.
    pub fn dash_url(&self, path: &str) -> Option<String> {
        let addr = self.dash_addr?;
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        Some(format!("http://{addr}{path}"))
    }

    /// HTTP base URL for the admin / WS surface.
    pub fn http_base(&self) -> String {
        format!("http://{}", self.admin_addr)
    }

    /// HTTP URL pointing at a path on the LL-HLS surface, e.g.
    /// `hls_url("/playlist.m3u8")`. Returns `None` when HLS is not
    /// enabled.
    pub fn hls_url(&self, path: &str) -> Option<String> {
        let addr = self.hls_addr?;
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        Some(format!("http://{addr}{path}"))
    }

    /// Construct the WebSocket subscribe URL for a broadcast.
    pub fn ws_url(&self, broadcast: &str) -> String {
        format!("ws://{}/ws/{broadcast}", self.admin_addr)
    }

    /// Construct the WebSocket ingest URL for a broadcast.
    pub fn ws_ingest_url(&self, broadcast: &str) -> String {
        format!("ws://{}/ingest/{broadcast}", self.admin_addr)
    }

    /// Construct the RTMP publish URL for an app + stream key.
    pub fn rtmp_url(&self, app: &str, stream_key: &str) -> String {
        format!("rtmp://{}/{app}/{stream_key}", self.rtmp_addr)
    }

    /// Snapshot of per-`(broadcast, track)` WASM filter tap
    /// counters. Returns `None` when `--wasm-filter` is not set.
    /// Tests read this after an RTMP publish completes to assert
    /// the filter actually observed the broadcast.
    pub fn wasm_filter(&self) -> Option<&lvqr_wasm::WasmFilterBridgeHandle> {
        self.wasm_filter.as_ref()
    }

    /// `AgentRunner` handle backing this server, when
    /// [`ServeConfig::whisper_model`] was `Some` at `start()`
    /// time. Returns `None` when captions are not wired. Tests
    /// read per-`(agent, broadcast, track)` fragment counters
    /// off this handle to assert the whisper agent actually
    /// observed the broadcast. Feature-gated on `whisper`.
    #[cfg(feature = "whisper")]
    pub fn agent_runner(&self) -> Option<&lvqr_agent::AgentRunnerHandle> {
        self.agent_runner.as_ref()
    }

    /// Trigger graceful shutdown and wait for every subsystem to drain.
    pub async fn shutdown(mut self) -> Result<()> {
        self.shutdown.cancel();
        if let Some(join) = self.join.take()
            && let Err(e) = join.await
            && !e.is_cancelled()
        {
            return Err(anyhow::anyhow!("server task panicked: {e}"));
        }
        Ok(())
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        // Best-effort: signal shutdown so the background tasks wind down
        // even if the caller forgot to `.shutdown().await`. We cannot block
        // on the join handle from a sync drop inside an async runtime
        // without risking a deadlock, so we just cancel and return.
        self.shutdown.cancel();
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

/// Start a full-stack LVQR server. All listeners are bound before the
/// function returns, so the [`ServerHandle`] immediately reports real
/// addresses even when the config requested ephemeral ports.
///
/// The returned handle owns a background task that runs the relay, RTMP,
/// and admin subsystems under a shared cancellation token. Use
/// [`ServerHandle::shutdown`] for deterministic teardown.
pub async fn start(config: ServeConfig) -> Result<ServerHandle> {
    tracing::info!(
        relay = %config.relay_addr,
        rtmp = %config.rtmp_addr,
        admin = %config.admin_addr,
        mesh = config.mesh_enabled,
        "starting LVQR server"
    );

    // Metrics recorder install. Process-wide, must be skipped in
    // tests (all four permutations below call
    // `metrics::set_global_recorder` which panics or errors on
    // second install). The four cases are:
    //   Prom + OTel:   install a FanoutBuilder of both.
    //   Prom only:     install the Prometheus recorder (legacy).
    //   OTel only:     install the OTel-forwarding recorder.
    //   Neither:       install nothing; metrics calls are no-ops.
    // The `PrometheusRecorder` handle is exposed on
    // `ServerHandle` for the admin `/metrics` scrape route, so
    // we always capture it before handing the recorder off to a
    // Fanout layer.
    let prom_handle = match (config.install_prometheus, config.otel_metrics_recorder.clone()) {
        (true, Some(otel_recorder)) => {
            let prom_recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
            let handle = prom_recorder.handle();
            let fanout = metrics_util::layers::FanoutBuilder::default()
                .add_recorder(prom_recorder)
                .add_recorder(otel_recorder)
                .build();
            metrics::set_global_recorder(fanout)
                .map_err(|e| anyhow::anyhow!("failed to install metrics fanout recorder: {e}"))?;
            Some(handle)
        }
        (true, None) => Some(
            metrics_exporter_prometheus::PrometheusBuilder::new()
                .install_recorder()
                .map_err(|e| anyhow::anyhow!("failed to install Prometheus recorder: {e}"))?,
        ),
        (false, Some(otel_recorder)) => {
            metrics::set_global_recorder(otel_recorder)
                .map_err(|e| anyhow::anyhow!("failed to install OTLP metrics recorder: {e}"))?;
            None
        }
        (false, None) => None,
    };

    let shutdown = CancellationToken::new();

    // Auth provider: caller-provided, or fall back to open access.
    let auth: SharedAuth = config
        .auth
        .clone()
        .unwrap_or_else(|| Arc::new(NoopAuthProvider) as SharedAuth);

    // Shared lifecycle bus: bridge and WS ingest emit
    // BroadcastStarted/Stopped here; recorder subscribes to it.
    let events = EventBus::default();

    // MoQ relay: init_server() binds the QUIC socket and reports the real
    // bound address, which we surface back through ServerHandle.
    let relay_config = lvqr_relay::RelayConfig::new(config.relay_addr);
    let mut relay = lvqr_relay::RelayServer::new(relay_config);
    relay.set_auth_provider(auth.clone());
    let (mut moq_server, relay_bound) = relay.init_server()?;
    tracing::info!(addr = %relay_bound, "MoQ relay bound");

    // Optional cluster bootstrap. Resolver for `MultiHlsServer` is
    // built up-front so the HLS constructor below can install it in
    // one shot instead of patching the server after the fact.
    #[cfg(feature = "cluster")]
    let cluster = if let Some(listen) = config.cluster_listen {
        let ccfg = lvqr_cluster::ClusterConfig {
            listen,
            seeds: config.cluster_seeds.clone(),
            node_id: config.cluster_node_id.clone().map(lvqr_cluster::NodeId::new),
            cluster_id: config
                .cluster_id
                .clone()
                .unwrap_or_else(|| lvqr_cluster::ClusterConfig::default().cluster_id),
            ..lvqr_cluster::ClusterConfig::default()
        };
        let c = lvqr_cluster::Cluster::bootstrap(ccfg)
            .await
            .map_err(|e| anyhow::anyhow!("cluster bootstrap failed: {e}"))?;
        let c = std::sync::Arc::new(c);
        if config.cluster_advertise_hls.is_some()
            || config.cluster_advertise_dash.is_some()
            || config.cluster_advertise_rtsp.is_some()
        {
            let endpoints = lvqr_cluster::NodeEndpoints {
                hls: config.cluster_advertise_hls.clone(),
                dash: config.cluster_advertise_dash.clone(),
                rtsp: config.cluster_advertise_rtsp.clone(),
            };
            c.set_endpoints(&endpoints)
                .await
                .map_err(|e| anyhow::anyhow!("cluster set_endpoints failed: {e}"))?;
        }
        tracing::info!(
            node = %c.self_id(),
            %listen,
            advertise_hls = ?config.cluster_advertise_hls,
            advertise_dash = ?config.cluster_advertise_dash,
            advertise_rtsp = ?config.cluster_advertise_rtsp,
            "cluster enabled"
        );
        Some(c)
    } else {
        None
    };
    #[cfg(feature = "cluster")]
    let hls_owner_resolver: Option<lvqr_hls::OwnerResolver> = cluster.as_ref().map(|c| {
        let c = c.clone();
        let resolver: lvqr_hls::OwnerResolver = std::sync::Arc::new(move |broadcast: String| {
            let c = c.clone();
            Box::pin(async move {
                let (_, endpoints) = c.find_owner_endpoints(&broadcast).await?;
                endpoints.hls
            })
        });
        resolver
    });
    #[cfg(not(feature = "cluster"))]
    let hls_owner_resolver: Option<lvqr_hls::OwnerResolver> = None;
    #[cfg(feature = "cluster")]
    let dash_owner_resolver: Option<lvqr_dash::OwnerResolver> = cluster.as_ref().map(|c| {
        let c = c.clone();
        let resolver: lvqr_dash::OwnerResolver = std::sync::Arc::new(move |broadcast: String| {
            let c = c.clone();
            Box::pin(async move {
                let (_, endpoints) = c.find_owner_endpoints(&broadcast).await?;
                endpoints.dash
            })
        });
        resolver
    });
    #[cfg(not(feature = "cluster"))]
    let dash_owner_resolver: Option<lvqr_dash::OwnerResolver> = None;
    #[cfg(feature = "cluster")]
    let rtsp_owner_resolver: Option<lvqr_rtsp::OwnerResolver> = cluster.as_ref().map(|c| {
        let c = c.clone();
        let resolver: lvqr_rtsp::OwnerResolver = std::sync::Arc::new(move |broadcast: String| {
            let c = c.clone();
            Box::pin(async move {
                let (_, endpoints) = c.find_owner_endpoints(&broadcast).await?;
                endpoints.rtsp
            })
        });
        resolver
    });
    #[cfg(not(feature = "cluster"))]
    let rtsp_owner_resolver: Option<lvqr_rtsp::OwnerResolver> = None;

    // Optional multi-broadcast LL-HLS server. The broadcaster-native
    // HLS bridge (installed below) subscribes on the shared registry
    // and pumps fragments into the shared `MultiHlsServer` state.
    // Each broadcast gets its own per-broadcast `HlsServer` on first
    // publish; the axum router demultiplexes requests under
    // `/hls/{broadcast}/...`.
    //
    // When clustering is enabled, an `OwnerResolver` redirects
    // subscribers of peer-owned broadcasts to the owning node's HLS
    // URL instead of returning 404.
    let target_dur = config.hls_target_duration_secs;
    let part_target_secs = config.hls_part_target_ms as f32 / 1000.0;
    let max_segments = if config.hls_dvr_window_secs == 0 || target_dur == 0 {
        None
    } else {
        Some((config.hls_dvr_window_secs / target_dur) as usize)
    };
    let hls_server = config.hls_addr.map(|_| {
        let playlist_cfg = PlaylistBuilderConfig {
            target_duration_secs: target_dur,
            part_target_secs,
            max_segments,
            ..PlaylistBuilderConfig::default()
        };
        match hls_owner_resolver.clone() {
            Some(r) => MultiHlsServer::with_owner_resolver(playlist_cfg, r),
            None => MultiHlsServer::new(playlist_cfg),
        }
    });

    // Optional multi-broadcast MPEG-DASH server. Sibling of the
    // LL-HLS fan-out above: a single `MultiDashServer` subscribes
    // on the shared registry and projects fragments onto a
    // per-broadcast axum router mounted under `/dash/{broadcast}/...`.
    // Every ingest protocol (RTMP, WHIP, SRT, RTSP) feeds DASH via
    // the same `BroadcasterDashBridge` install below.
    let dash_server = config.dash_addr.map(|_| match dash_owner_resolver.clone() {
        Some(r) => lvqr_dash::MultiDashServer::with_owner_resolver(DashConfig::default(), r),
        None => lvqr_dash::MultiDashServer::new(DashConfig::default()),
    });

    // Shared FragmentBroadcasterRegistry used by every ingest crate
    // and every consumer. Session 60 completed the Tier 2.1 migration:
    // every ingest protocol publishes to this one registry, and every
    // consumer (archive, LL-HLS, DASH) installs an on_entry_created
    // callback against it.
    let shared_registry = FragmentBroadcasterRegistry::new();

    // Auto-claim every new broadcast against the cluster so peers
    // redirect correctly without the operator having to call
    // `Cluster::claim_broadcast` by hand. The bridge holds the
    // `Claim` alive until every ingest publisher for that
    // broadcast disconnects. Feature-gated; no-op when
    // single-node.
    #[cfg(feature = "cluster")]
    if let Some(ref c) = cluster {
        cluster_claim::install_cluster_claim_bridge(c.clone(), cluster_claim::DEFAULT_CLAIM_LEASE, &shared_registry);
    }

    // Optional WASM fragment filter tap. Installed BEFORE any
    // ingest listener accepts traffic so the very first fragment
    // of the first broadcast flows through the filter. The
    // reloader watches the module path for changes and calls
    // `SharedFilter::replace` atomically when the file changes;
    // in-flight fragments finish on the old module and the next
    // fragment sees the new one.
    let (wasm_filter_handle, wasm_reloader_handle) = if let Some(ref path) = config.wasm_filter {
        let filter = lvqr_wasm::WasmFilter::load(path)
            .map_err(|e| anyhow::anyhow!("WASM filter load at {} failed: {e}", path.display()))?;
        tracing::info!(path = %path.display(), "WASM fragment filter loaded");
        let shared = lvqr_wasm::SharedFilter::new(filter);
        let bridge = lvqr_wasm::install_wasm_filter_bridge(&shared_registry, shared.clone());
        let reloader = lvqr_wasm::WasmFilterReloader::spawn(path, shared)
            .map_err(|e| anyhow::anyhow!("WASM filter hot-reload watcher at {} failed: {e}", path.display()))?;
        (Some(bridge), Some(reloader))
    } else {
        (None, None)
    };

    // RTMP ingest bridged to MoQ. Pre-bind the TCP listener so we can
    // report the real bound port (for ephemeral-port test setups).
    let mut bridge_builder = lvqr_ingest::RtmpMoqBridge::with_auth(relay.origin().clone(), auth.clone())
        .with_events(events.clone())
        .with_registry(shared_registry.clone());

    // Optional DVR archive index. Opened before the bridge is frozen
    // so the BroadcasterArchiveIndexer can install its on_entry_created
    // callback on the shared registry. The index file lives at
    // `<archive_dir>/archive.redb`; the directory is created on
    // demand if it does not already exist.
    let archive_index = if let Some(ref dir) = config.archive_dir {
        std::fs::create_dir_all(dir)
            .map_err(|e| anyhow::anyhow!("archive: failed to create {}: {e}", dir.display()))?;
        let db_path = dir.join("archive.redb");
        let index = lvqr_archive::RedbSegmentIndex::open(&db_path)
            .map_err(|e| anyhow::anyhow!("archive: failed to open {}: {e}", db_path.display()))?;
        tracing::info!(dir = %dir.display(), "DVR archive index enabled");
        Some((dir.clone(), Arc::new(index)))
    } else {
        None
    };

    // Install the broadcaster-based archive indexer on the shared
    // registry. Every subsequent ingest-side emit is drained to disk +
    // redb by a per-broadcaster tokio task the indexer spawns.
    if let Some((ref dir, ref index)) = archive_index {
        #[cfg(feature = "c2pa")]
        BroadcasterArchiveIndexer::install(dir.clone(), Arc::clone(index), &shared_registry, config.c2pa.clone());
        #[cfg(not(feature = "c2pa"))]
        BroadcasterArchiveIndexer::install(dir.clone(), Arc::clone(index), &shared_registry);
    }

    // Install the broadcaster-based LL-HLS composition bridge on the
    // shared registry. Every ingest crate's first `publish_init` for a
    // `(broadcast, track)` pair fires the callback; the callback
    // subscribes and spawns a drain task that projects fragments onto
    // the shared `MultiHlsServer`. Session 60 consumer-side switchover:
    // replaces the FragmentObserver path the HLS bridge used through
    // session 59.
    if let Some(ref hls) = hls_server {
        BroadcasterHlsBridge::install(
            hls.clone(),
            config.hls_target_duration_secs * 1000,
            config.hls_part_target_ms,
            &shared_registry,
        );
        // Tier 4 item 4.5 session C: feed the captions
        // sub-track into the per-broadcast subtitles
        // rendition. The bridge no-ops on every track that
        // is not `"captions"`, so it composes safely with
        // the LL-HLS bridge above.
        captions::BroadcasterCaptionsBridge::install(hls.clone(), &shared_registry);
    }

    // Tier 4 item 4.5 session D: if the operator passed
    // `--whisper-model <PATH>`, build the
    // `WhisperCaptionsFactory` + `AgentRunner` and install it
    // onto the shared registry so every new
    // `(broadcast, "1.mp4")` triggers a WhisperCaptionsAgent.
    // The agent republishes each caption cue onto
    // `(broadcast, "captions")` where the
    // `BroadcasterCaptionsBridge` above picks it up and feeds
    // the HLS subtitle rendition. Without the flag (or without
    // the `whisper` feature at all) no AI state is constructed.
    #[cfg(feature = "whisper")]
    let agent_runner_handle = if let Some(ref path) = config.whisper_model {
        if hls_server.is_none() {
            // The captions track reaches browser players only via
            // the HLS subtitle rendition that `BroadcasterCaptionsBridge`
            // wires above. With HLS disabled the WhisperCaptionsAgent
            // still runs and publishes cues onto the registry and the
            // in-process `CaptionStream`, but browser subscribers see
            // nothing. Warn so misconfigured deployments surface early
            // rather than through silent captions loss.
            tracing::warn!(
                path = %path.display(),
                "whisper captions agent enabled without HLS surface; browser clients will not receive captions"
            );
        }
        let factory =
            lvqr_agent_whisper::WhisperCaptionsFactory::new(lvqr_agent_whisper::WhisperConfig::new(path.clone()))
                .with_caption_registry(shared_registry.clone());
        tracing::info!(path = %path.display(), "whisper captions agent enabled");
        Some(
            lvqr_agent::AgentRunner::new()
                .with_factory(factory)
                .install(&shared_registry),
        )
    } else {
        None
    };

    // Install the broadcaster-based DASH composition bridge. Same
    // pattern as LL-HLS: the callback spawns a drain task per
    // `(broadcast, track)` that stamps a monotonic `$Number$` counter
    // onto every observed fragment and pushes it into the per-broadcast
    // `DashServer`. Session 60: completes the consumer-side switchover.
    if let Some(ref dash) = dash_server {
        BroadcasterDashBridge::install(dash.clone(), &shared_registry);
    }

    // Tier 4 item 4.4 session B: start a FederationRunner against any
    // configured peer-cluster MoQ relays. Each runner task opens an
    // outbound MoQ session, drains the remote origin's announcement
    // stream, filters by the link's forwarded_broadcasts list, and
    // re-publishes matched broadcasts into the local relay's origin
    // producer so every MoQ subscriber on this node sees them as if
    // they were ingested locally. No-op when the links list is empty.
    // Feature-gated on `cluster` so single-node builds stay thin.
    #[cfg(feature = "cluster")]
    let federation_runner_handle = if config.federation_links.is_empty() {
        None
    } else {
        tracing::info!(links = config.federation_links.len(), "starting federation runner");
        Some(lvqr_cluster::FederationRunner::start(
            config.federation_links.clone(),
            relay.origin().clone(),
            shutdown.clone(),
        ))
    };

    // Optional WHEP surface. Constructed before the bridge is
    // frozen into an `Arc` so we can attach the `WhepServer` as a
    // `RawSampleObserver`; both the observer clone and the axum
    // router clone share the same underlying session registry, so
    // a POST on the router is immediately visible to the raw-sample
    // fanout path.
    let whep_server = if let Some(addr) = config.whep_addr {
        let str0m_cfg = lvqr_whep::Str0mConfig { host_ip: addr.ip() };
        let answerer = Arc::new(lvqr_whep::Str0mAnswerer::new(str0m_cfg)) as Arc<dyn lvqr_whep::SdpAnswerer>;
        let server = lvqr_whep::WhepServer::new(answerer);
        let observer: lvqr_ingest::SharedRawSampleObserver = Arc::new(server.clone());
        bridge_builder = bridge_builder.with_raw_sample_observer(observer);
        Some(server)
    } else {
        None
    };

    // Optional WHIP ingest surface. The bridge side is a sibling
    // of `RtmpMoqBridge`: it owns its own `BroadcastProducer` state
    // but publishes fragments onto the same shared registry, so every
    // existing egress (MoQ, LL-HLS, DASH, disk record, DVR archive)
    // picks up WHIP publishers with zero additional wiring.
    let (whip_server, whip_bridge) = if let Some(addr) = config.whip_addr {
        let mut whip_bridge = lvqr_whip::WhipMoqBridge::new(relay.origin().clone())
            .with_events(events.clone())
            .with_registry(shared_registry.clone());
        if let Some(ref server) = whep_server {
            let raw_observer: lvqr_ingest::SharedRawSampleObserver = Arc::new(server.clone());
            whip_bridge = whip_bridge.with_raw_sample_observer(raw_observer);
        }
        let whip_bridge_arc = Arc::new(whip_bridge);
        let sink = whip_bridge_arc.clone() as Arc<dyn lvqr_whip::IngestSampleSink>;
        let str0m_cfg = lvqr_whip::Str0mIngestConfig { host_ip: addr.ip() };
        let answerer =
            Arc::new(lvqr_whip::Str0mIngestAnswerer::new(str0m_cfg, sink)) as Arc<dyn lvqr_whip::SdpAnswerer>;
        let server = lvqr_whip::WhipServer::with_auth_provider(answerer, auth.clone());
        (Some(server), Some(whip_bridge_arc))
    } else {
        (None, None)
    };

    // Optional SRT ingest server. Publishes to the shared registry;
    // every broadcaster-native consumer picks up SRT publishers
    // automatically.
    let (srt_server, srt_bound) = if let Some(addr) = config.srt_addr {
        let mut server =
            lvqr_srt::SrtIngestServer::with_registry(addr, shared_registry.clone()).with_auth(auth.clone());
        let bound = server.bind().await?;
        tracing::info!(addr = %bound, "SRT ingest bound");
        (Some(server), Some(bound))
    } else {
        (None, None)
    };
    let srt_events_clone = events.clone();
    let srt_shutdown_token = shutdown.clone();

    // Optional RTSP ingest server. Publishes to the shared registry
    // alongside every other ingest protocol. When clustering is
    // enabled, the owner resolver redirects DESCRIBE / PLAY for
    // peer-owned broadcasts with RTSP 302.
    let (rtsp_server, rtsp_bound) = if let Some(addr) = config.rtsp_addr {
        let mut server = lvqr_rtsp::RtspServer::with_registry(addr, shared_registry.clone()).with_auth(auth.clone());
        if let Some(r) = rtsp_owner_resolver.clone() {
            server = server.with_owner_resolver(r);
        }
        let bound = server.bind().await?;
        tracing::info!(addr = %bound, "RTSP ingest bound");
        (Some(server), Some(bound))
    } else {
        (None, None)
    };
    let rtsp_events_clone = events.clone();
    let rtsp_shutdown_token = shutdown.clone();

    let bridge = Arc::new(bridge_builder);
    let rtmp_config = lvqr_ingest::RtmpConfig {
        bind_addr: config.rtmp_addr,
    };
    let rtmp_server = bridge.create_rtmp_server(rtmp_config);
    let rtmp_listener = tokio::net::TcpListener::bind(config.rtmp_addr).await?;
    let rtmp_bound = rtmp_listener.local_addr()?;
    tracing::info!(addr = %rtmp_bound, "RTMP ingest bound");

    // Admin listener: pre-bind to capture the real port.
    let admin_listener = tokio::net::TcpListener::bind(config.admin_addr).await?;
    let admin_bound = admin_listener.local_addr()?;
    tracing::info!(addr = %admin_bound, "admin HTTP bound");

    // HLS listener: pre-bind so the test harness can read the
    // ephemeral port back via `ServerHandle::hls_addr` immediately
    // after `start()` returns.
    let (hls_listener, hls_bound) = if let Some(addr) = config.hls_addr {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        tracing::info!(addr = %bound, "LL-HLS HTTP bound");
        (Some(listener), Some(bound))
    } else {
        (None, None)
    };

    // WHEP listener: pre-bind the same way so test harnesses can
    // read the ephemeral port back immediately. `whep_server` was
    // built earlier and is `None` if `config.whep_addr` is `None`.
    let (whep_listener, whep_bound) = if let Some(addr) = config.whep_addr {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        tracing::info!(addr = %bound, "WHEP HTTP bound");
        (Some(listener), Some(bound))
    } else {
        (None, None)
    };

    // DASH listener: pre-bind so ephemeral-port test harnesses can
    // read the real port back via `ServerHandle::dash_addr`
    // immediately after `start()` returns.
    let (dash_listener, dash_bound) = if let Some(addr) = config.dash_addr {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        tracing::info!(addr = %bound, "MPEG-DASH HTTP bound");
        (Some(listener), Some(bound))
    } else {
        (None, None)
    };

    // WHIP listener: pre-bind for the same reason. Keeping the
    // bridge arc alive for the lifetime of the server task is
    // important: dropping it would tear down every active MoQ
    // broadcast produced by a WHIP publisher.
    let (whip_listener, whip_bound) = if let Some(addr) = config.whip_addr {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        tracing::info!(addr = %bound, "WHIP HTTP bound");
        (Some(listener), Some(bound))
    } else {
        (None, None)
    };

    // Optional disk recorder.
    if let Some(ref dir) = config.record_dir {
        let recorder = lvqr_record::BroadcastRecorder::new(dir);
        let origin = relay.origin().clone();
        let event_rx = events.subscribe();
        let record_shutdown = shutdown.clone();
        tracing::info!(dir = %dir.display(), "recording enabled");
        tokio::spawn(async move {
            spawn_recordings(recorder, origin, event_rx, record_shutdown).await;
        });
    }

    // HLS finalization subscriber: when a broadcaster disconnects
    // the RTMP bridge emits BroadcastStopped, and this task calls
    // MultiHlsServer::finalize_broadcast so the playlist gains
    // EXT-X-ENDLIST and the retained window becomes a VOD surface.
    if let Some(ref hls) = hls_server {
        let hls_for_finalize = hls.clone();
        let mut hls_event_rx = events.subscribe();
        let hls_finalize_shutdown = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = hls_finalize_shutdown.cancelled() => break,
                    msg = hls_event_rx.recv() => {
                        match msg {
                            Ok(RelayEvent::BroadcastStopped { name }) => {
                                tracing::info!(broadcast = %name, "finalizing HLS broadcast");
                                hls_for_finalize.finalize_broadcast(&name).await;
                            }
                            Ok(_) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(missed = n, "HLS finalize subscriber lagged");
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }

    // DASH finalization subscriber: same pattern as HLS above.
    // Switches the MPD from type="dynamic" to type="static" so
    // DASH clients stop polling for new segments.
    if let Some(ref dash) = dash_server {
        let dash_for_finalize = dash.clone();
        let mut dash_event_rx = events.subscribe();
        let dash_finalize_shutdown = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = dash_finalize_shutdown.cancelled() => break,
                    msg = dash_event_rx.recv() => {
                        match msg {
                            Ok(RelayEvent::BroadcastStopped { name }) => {
                                tracing::info!(broadcast = %name, "finalizing DASH broadcast");
                                dash_for_finalize.finalize_broadcast(&name);
                            }
                            Ok(_) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(missed = n, "DASH finalize subscriber lagged");
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }

    // Admin HTTP state and router.
    let metrics_state = relay.metrics().clone();
    let bridge_for_stats = bridge.clone();
    let bridge_for_streams = bridge.clone();

    let admin_state = lvqr_admin::AdminState::new(
        move || {
            let active = bridge_for_stats.active_stream_count() as u64;
            lvqr_core::RelayStats {
                publishers: active,
                tracks: active * 2,
                subscribers: metrics_state.connections_active.load(Ordering::Relaxed),
                bytes_received: 0,
                bytes_sent: 0,
                uptime_secs: 0,
            }
        },
        move || {
            bridge_for_streams
                .stream_names()
                .into_iter()
                .map(|name| lvqr_admin::StreamInfo { name, subscribers: 0 })
                .collect()
        },
    )
    .with_auth(auth.clone());
    let admin_state = if let Some(prom) = prom_handle {
        admin_state.with_metrics(Arc::new(move || prom.render()))
    } else {
        admin_state
    };
    // Wire the cluster into `/api/v1/cluster/*`. Without this the
    // feature-gated routes in `lvqr-admin` reply 500 with a
    // "cluster not wired" message.
    #[cfg(feature = "cluster")]
    let admin_state = match cluster.as_ref() {
        Some(c) => admin_state.with_cluster(c.clone()),
        None => admin_state,
    };

    // WebSocket fMP4 relay + WebSocket ingest state.
    let ws_state = WsRelayState {
        origin: relay.origin().clone(),
        init_segments: Arc::new(dashmap::DashMap::new()),
        auth: auth.clone(),
        events: events.clone(),
    };
    let ws_router = axum::Router::new()
        .route("/ws/{*broadcast}", get(ws_relay_handler))
        .route("/ingest/{*broadcast}", get(ws_ingest_handler))
        .with_state(ws_state);

    let combined_router = {
        let admin_router = if config.mesh_enabled {
            let mesh_config = lvqr_mesh::MeshConfig {
                max_children: config.max_peers,
                ..Default::default()
            };
            let mesh = Arc::new(lvqr_mesh::MeshCoordinator::new(mesh_config));

            let mesh_for_cb = mesh.clone();
            relay.set_connection_callback(Arc::new(move |conn_id, connected| {
                let peer_id = format!("conn-{conn_id}");
                if connected {
                    match mesh_for_cb.add_peer(peer_id.clone(), "default".to_string()) {
                        Ok(a) => {
                            tracing::info!(peer = %peer_id, role = ?a.role, depth = a.depth, "mesh: peer assigned");
                        }
                        Err(e) => {
                            tracing::warn!(peer = %peer_id, error = ?e, "mesh: assign failed");
                        }
                    }
                } else {
                    let orphans = mesh_for_cb.remove_peer(&peer_id);
                    for orphan in orphans {
                        let _ = mesh_for_cb.reassign_peer(&orphan);
                    }
                }
            }));

            let mesh_for_reaper = mesh.clone();
            let reaper_shutdown = shutdown.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            let dead = mesh_for_reaper.find_dead_peers();
                            for peer_id in dead {
                                tracing::info!(peer = %peer_id, "mesh: removing dead peer");
                                let orphans = mesh_for_reaper.remove_peer(&peer_id);
                                for orphan in orphans {
                                    let _ = mesh_for_reaper.reassign_peer(&orphan);
                                }
                            }
                        }
                        _ = reaper_shutdown.cancelled() => {
                            tracing::debug!("mesh reaper shutting down");
                            break;
                        }
                    }
                }
            });

            let mesh_for_signal = mesh.clone();
            let mut signal = lvqr_signal::SignalServer::new();
            signal.set_peer_callback(Arc::new(move |peer_id, track, connected| {
                if connected {
                    match mesh_for_signal.add_peer(peer_id.to_string(), track.to_string()) {
                        Ok(a) => {
                            tracing::info!(peer = %peer_id, role = ?a.role, depth = a.depth, "mesh: signal peer assigned");
                            Some(lvqr_signal::SignalMessage::AssignParent {
                                peer_id: peer_id.to_string(),
                                role: format!("{:?}", a.role),
                                parent_id: a.parent,
                                depth: a.depth,
                            })
                        }
                        Err(e) => {
                            tracing::warn!(peer = %peer_id, error = ?e, "mesh: signal assign failed");
                            None
                        }
                    }
                } else {
                    let orphans = mesh_for_signal.remove_peer(peer_id);
                    for orphan in orphans {
                        let _ = mesh_for_signal.reassign_peer(&orphan);
                    }
                    None
                }
            }));

            let mesh_for_admin = mesh.clone();
            let admin_with_mesh = admin_state.with_mesh(move || lvqr_admin::MeshState {
                enabled: true,
                peer_count: mesh_for_admin.peer_count(),
                offload_percentage: mesh_for_admin.offload_percentage(),
            });

            tracing::info!(
                max_children = config.max_peers,
                "peer mesh enabled (/signal endpoint active)"
            );

            let router = lvqr_admin::build_router(admin_with_mesh);
            router.merge(signal.router())
        } else {
            lvqr_admin::build_router(admin_state)
        };

        let combined = admin_router.merge(ws_router);
        let combined = if let Some((ref dir, ref index)) = archive_index {
            combined.merge(playback_router(dir.clone(), Arc::clone(index), auth.clone()))
        } else {
            combined
        };
        // Tier 4 item 4.3 session B3: feature-gated `/playback/verify/
        // {broadcast}` admin route. Mounted only when the `c2pa`
        // feature is on AND an archive directory is configured (the
        // verify route reads `<archive>/<broadcast>/<track>/
        // finalized.*` off disk, so an archive is a hard prerequisite).
        #[cfg(feature = "c2pa")]
        let combined = if let Some((ref dir, _)) = archive_index {
            combined.merge(verify_router(dir.clone(), auth.clone()))
        } else {
            combined
        };
        combined
    }
    .layer(CorsLayer::permissive());

    // Spawn a single background task that joins relay + RTMP + admin and
    // signals the shared shutdown token if any subsystem exits early.
    let relay_shutdown = shutdown.clone();
    let rtmp_shutdown = shutdown.clone();
    let admin_shutdown = shutdown.clone();
    let hls_shutdown = shutdown.clone();
    let dash_shutdown = shutdown.clone();
    let whep_shutdown = shutdown.clone();
    let whip_shutdown = shutdown.clone();
    let bg_shutdown_for_task = shutdown.clone();
    let hls_router_pair =
        hls_listener.map(|listener| (listener, hls_server.expect("hls_server set when listener is set")));
    let dash_router_pair =
        dash_listener.map(|listener| (listener, dash_server.expect("dash_server set when listener is set")));
    let whep_router_pair =
        whep_listener.map(|listener| (listener, whep_server.expect("whep_server set when listener is set")));
    let whip_router_pair =
        whip_listener.map(|listener| (listener, whip_server.expect("whip_server set when listener is set")));
    // Moved into the spawned task below so it lives as long as
    // the WHIP poll loops; see `drop(_whip_bridge_keepalive)` at
    // the end of the join block.
    let whip_bridge_keepalive = whip_bridge;

    // Clone the relay's OriginProducer for the ServerHandle. `relay`
    // itself moves into the accept-loop below, so the clone is how
    // callers (federation tests, admin consumers) reach the origin
    // for the server's lifetime.
    let relay_origin = relay.origin().clone();

    let join = tokio::spawn(async move {
        let shutdown_on_exit_relay = bg_shutdown_for_task.clone();
        let relay_fut = async move {
            if let Err(e) = relay.accept_loop(&mut moq_server, relay_shutdown).await {
                tracing::error!(error = %e, "relay server error");
            }
            shutdown_on_exit_relay.cancel();
        };

        let shutdown_on_exit_rtmp = bg_shutdown_for_task.clone();
        let rtmp_server_task = rtmp_server;
        let rtmp_fut = async move {
            if let Err(e) = rtmp_server_task.run_with_listener(rtmp_listener, rtmp_shutdown).await {
                tracing::error!(error = %e, "RTMP server error");
            }
            shutdown_on_exit_rtmp.cancel();
        };

        let shutdown_on_exit_admin = bg_shutdown_for_task.clone();
        let admin_fut = async move {
            let result = axum::serve(admin_listener, combined_router)
                .with_graceful_shutdown(async move { admin_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "admin server error");
            }
            shutdown_on_exit_admin.cancel();
        };

        let shutdown_on_exit_hls = bg_shutdown_for_task.clone();
        let hls_fut = async move {
            let Some((listener, server)) = hls_router_pair else {
                return;
            };
            let router = server.router().layer(CorsLayer::permissive());
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move { hls_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "HLS server error");
            }
            shutdown_on_exit_hls.cancel();
        };

        let shutdown_on_exit_dash = bg_shutdown_for_task.clone();
        let dash_fut = async move {
            let Some((listener, server)) = dash_router_pair else {
                return;
            };
            let router = server.router().layer(CorsLayer::permissive());
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move { dash_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "DASH server error");
            }
            shutdown_on_exit_dash.cancel();
        };

        let shutdown_on_exit_whep = bg_shutdown_for_task.clone();
        let whep_fut = async move {
            let Some((listener, server)) = whep_router_pair else {
                return;
            };
            let router = lvqr_whep::router_for(server);
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move { whep_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "WHEP server error");
            }
            shutdown_on_exit_whep.cancel();
        };

        let shutdown_on_exit_whip = bg_shutdown_for_task.clone();
        let whip_fut = async move {
            let Some((listener, server)) = whip_router_pair else {
                return;
            };
            let router = lvqr_whip::router_for(server);
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move { whip_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "WHIP server error");
            }
            shutdown_on_exit_whip.cancel();
        };

        let srt_shutdown = bg_shutdown_for_task.clone();
        let srt_events = srt_events_clone;
        let srt_cancel = srt_shutdown_token;
        let srt_fut = async move {
            let Some(server) = srt_server else { return };
            if let Err(e) = server.run(srt_events, srt_cancel).await {
                tracing::error!(error = %e, "SRT server error");
            }
            srt_shutdown.cancel();
        };

        let rtsp_shutdown = bg_shutdown_for_task.clone();
        let rtsp_events = rtsp_events_clone;
        let rtsp_cancel = rtsp_shutdown_token;
        let rtsp_fut = async move {
            let Some(server) = rtsp_server else { return };
            if let Err(e) = server.run(rtsp_events, rtsp_cancel).await {
                tracing::error!(error = %e, "RTSP server error");
            }
            rtsp_shutdown.cancel();
        };

        let _ = tokio::join!(
            relay_fut, rtmp_fut, admin_fut, hls_fut, dash_fut, whep_fut, whip_fut, srt_fut, rtsp_fut
        );
        drop(whip_bridge_keepalive);
        tracing::info!("shutdown complete");
    });

    Ok(ServerHandle {
        relay_addr: relay_bound,
        rtmp_addr: rtmp_bound,
        admin_addr: admin_bound,
        hls_addr: hls_bound,
        whep_addr: whep_bound,
        whip_addr: whip_bound,
        dash_addr: dash_bound,
        rtsp_addr: rtsp_bound,
        srt_addr: srt_bound,
        shutdown,
        join: Some(join),
        #[cfg(feature = "cluster")]
        cluster,
        wasm_filter: wasm_filter_handle,
        _wasm_reloader: wasm_reloader_handle,
        #[cfg(feature = "whisper")]
        agent_runner: agent_runner_handle,
        fragment_registry: shared_registry,
        origin: relay_origin,
        #[cfg(feature = "cluster")]
        federation_runner: federation_runner_handle,
    })
}

// =====================================================================
// Internal WS relay + WS ingest handlers
// =====================================================================

/// Shared state for WebSocket relay and ingest handlers.
#[derive(Clone)]
struct WsRelayState {
    origin: lvqr_moq::OriginProducer,
    /// Stored init segments per broadcast, so viewers get them immediately
    /// on connect.
    init_segments: Arc<dashmap::DashMap<String, Bytes>>,
    /// Authentication provider applied to WS subscribe and ingest sessions.
    auth: SharedAuth,
    /// Lifecycle event bus.
    events: EventBus,
}

async fn ws_relay_handler(
    ws: WebSocketUpgrade,
    State(state): State<WsRelayState>,
    Path(broadcast): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    tracing::info!(broadcast = %broadcast, "WebSocket relay request");
    let resolved = resolve_ws_token(&headers, &params, "ws_subscribe");
    let decision = state.auth.check(&AuthContext::Subscribe {
        token: resolved.token,
        broadcast: broadcast.clone(),
    });
    if let AuthDecision::Deny { reason } = decision {
        tracing::warn!(broadcast = %broadcast, reason = %reason, "WS relay denied");
        metrics::counter!("lvqr_auth_failures_total", "entry" => "ws").increment(1);
        return (StatusCode::UNAUTHORIZED, reason).into_response();
    }
    metrics::counter!("lvqr_ws_connections_total", "direction" => "subscribe").increment(1);
    let ws = match resolved.offered_subprotocol {
        Some(ref p) => ws.protocols(std::iter::once(p.clone())),
        None => ws,
    };
    ws.on_upgrade(move |socket| ws_relay_session(socket, state, broadcast))
        .into_response()
}

async fn ws_relay_session(mut socket: WebSocket, state: WsRelayState, broadcast: String) {
    let consumer = state.origin.consume();
    let Some(bc) = consumer.consume_broadcast(&broadcast) else {
        tracing::warn!(broadcast = %broadcast, "broadcast not found for WS relay");
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4404,
                reason: "broadcast not found".into(),
            })))
            .await;
        return;
    };

    tracing::info!(broadcast = %broadcast, "WS relay session started");

    let video_track = bc.subscribe_track(&Track::new("0.mp4")).ok();
    let audio_track = bc.subscribe_track(&Track::new("1.mp4")).ok();

    if video_track.is_none() && audio_track.is_none() {
        tracing::warn!(broadcast = %broadcast, "no playable tracks for WS relay");
        return;
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<(u8, Bytes)>(64);
    let cancel = CancellationToken::new();

    if let Some(track) = video_track {
        let tx = tx.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            relay_track(track, 0u8, tx, cancel).await;
        });
    }
    if let Some(track) = audio_track {
        let tx = tx.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            relay_track(track, 1u8, tx, cancel).await;
        });
    }
    drop(tx);

    while let Some((track_id, payload)) = rx.recv().await {
        let mut framed = Vec::with_capacity(1 + payload.len());
        framed.push(track_id);
        framed.extend_from_slice(&payload);
        let len = framed.len() as u64;
        if let Err(e) = socket.send(Message::Binary(framed.into())).await {
            tracing::debug!(error = ?e, "WS send error");
            break;
        }
        metrics::counter!("lvqr_frames_relayed_total", "transport" => "ws").increment(1);
        metrics::counter!("lvqr_bytes_relayed_total", "transport" => "ws").increment(len);
    }

    cancel.cancel();
    tracing::info!(broadcast = %broadcast, "WS relay session ended");
}

async fn relay_track(
    mut track: lvqr_moq::TrackConsumer,
    track_id: u8,
    tx: tokio::sync::mpsc::Sender<(u8, Bytes)>,
    cancel: CancellationToken,
) {
    loop {
        let group = tokio::select! {
            res = track.next_group() => res,
            _ = cancel.cancelled() => return,
        };
        let mut group = match group {
            Ok(Some(g)) => g,
            Ok(None) => return,
            Err(e) => {
                tracing::debug!(track_id, error = ?e, "track error");
                return;
            }
        };
        loop {
            let frame = tokio::select! {
                res = group.read_frame() => res,
                _ = cancel.cancelled() => return,
            };
            match frame {
                Ok(Some(bytes)) => {
                    if tx.send((track_id, bytes)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!(track_id, error = ?e, "group read error");
                    return;
                }
            }
        }
    }
}

async fn ws_ingest_handler(
    ws: WebSocketUpgrade,
    State(state): State<WsRelayState>,
    Path(broadcast): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    tracing::info!(broadcast = %broadcast, "WebSocket ingest request");
    let resolved = resolve_ws_token(&headers, &params, "ws_ingest");
    let decision = state
        .auth
        .check(&extract::extract_ws_ingest(resolved.token.as_deref(), &broadcast));
    if let AuthDecision::Deny { reason } = decision {
        tracing::warn!(broadcast = %broadcast, reason = %reason, "WS ingest denied");
        metrics::counter!("lvqr_auth_failures_total", "entry" => "ws_ingest").increment(1);
        return (StatusCode::UNAUTHORIZED, reason).into_response();
    }
    metrics::counter!("lvqr_ws_connections_total", "direction" => "publish").increment(1);
    let ws = match resolved.offered_subprotocol {
        Some(ref p) => ws.protocols(std::iter::once(p.clone())),
        None => ws,
    };
    ws.on_upgrade(move |socket| ws_ingest_session(socket, state, broadcast))
        .into_response()
}

/// Result of extracting a bearer token from a WebSocket upgrade request.
///
/// The preferred transport is the `Sec-WebSocket-Protocol` header with a
/// value of `lvqr.bearer.<token>`. When the client offers that, the
/// matching subprotocol string is echoed back so axum's upgrade handshake
/// accepts it. The legacy `?token=` query parameter is still accepted as
/// a fallback but logs a deprecation warning.
struct WsTokenResolution {
    token: Option<String>,
    offered_subprotocol: Option<String>,
}

fn resolve_ws_token(headers: &HeaderMap, params: &HashMap<String, String>, entry: &'static str) -> WsTokenResolution {
    if let Some(hv) = headers.get("sec-websocket-protocol")
        && let Ok(raw) = hv.to_str()
    {
        for item in raw.split(',') {
            let proto = item.trim();
            if let Some(tok) = proto.strip_prefix("lvqr.bearer.")
                && !tok.is_empty()
            {
                return WsTokenResolution {
                    token: Some(tok.to_string()),
                    offered_subprotocol: Some(proto.to_string()),
                };
            }
        }
    }

    if let Some(tok) = params.get("token").filter(|t| !t.is_empty()) {
        tracing::warn!(
            entry = entry,
            "deprecated: ?token= query parameter; migrate to Sec-WebSocket-Protocol: lvqr.bearer.<token>"
        );
        return WsTokenResolution {
            token: Some(tok.clone()),
            offered_subprotocol: None,
        };
    }

    WsTokenResolution {
        token: None,
        offered_subprotocol: None,
    }
}

async fn ws_ingest_session(mut socket: WebSocket, state: WsRelayState, broadcast: String) {
    use lvqr_ingest::remux;

    tracing::info!(broadcast = %broadcast, "WS ingest session started");

    let Some(mut bc) = state.origin.create_broadcast(&broadcast) else {
        tracing::warn!(broadcast = %broadcast, "broadcast creation failed");
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 4409,
                reason: "broadcast already exists".into(),
            })))
            .await;
        return;
    };

    state.events.emit(RelayEvent::BroadcastStarted {
        name: broadcast.clone(),
    });

    let Ok(mut video_track) = bc.create_track(Track::new("0.mp4")) else {
        tracing::warn!("failed to create video track");
        return;
    };
    let Ok(mut catalog_track) = bc.create_track(Track::new(".catalog")) else {
        tracing::warn!("failed to create catalog track");
        return;
    };

    let mut _video_config: Option<remux::VideoConfig> = None;
    let mut video_init: Option<Bytes> = None;
    let mut video_group: Option<lvqr_moq::GroupProducer> = None;
    let mut video_seq: u32 = 0;
    let _ = socket.send(Message::Text(r#"{"status":"ready"}"#.into())).await;

    while let Some(msg) = socket.recv().await {
        let data = match msg {
            Ok(Message::Binary(data)) => data,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(e) => {
                tracing::debug!(error = ?e, "WS ingest recv error");
                break;
            }
        };

        if data.len() < 5 {
            continue;
        }

        let msg_type = data[0];
        let timestamp = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
        let payload = Bytes::from(data[5..].to_vec());

        match msg_type {
            0 => {
                if payload.len() < 6 {
                    continue;
                }
                let vid_width = u16::from_be_bytes([payload[0], payload[1]]);
                let vid_height = u16::from_be_bytes([payload[2], payload[3]]);
                let avcc_data = &payload[4..];

                match parse_avcc_record(avcc_data) {
                    Some(config) => {
                        tracing::info!(
                            broadcast = %broadcast,
                            codec = %config.codec_string(),
                            width = vid_width,
                            height = vid_height,
                            "WS ingest: video config received"
                        );
                        let init = remux::video_init_segment_with_size(&config, vid_width, vid_height);
                        _video_config = Some(config.clone());
                        video_init = Some(init.clone());
                        state.init_segments.insert(broadcast.clone(), init);

                        let json = remux::generate_catalog(Some(&config), None);
                        if let Ok(mut group) = catalog_track.append_group() {
                            let _ = group.write_frame(Bytes::from(json));
                            let _ = group.finish();
                        }
                    }
                    None => {
                        tracing::warn!("invalid AVCC record from browser");
                    }
                }
            }
            1 => {
                let Some(ref init) = video_init else { continue };
                if let Some(mut g) = video_group.take() {
                    let _ = g.finish();
                }
                video_seq += 1;
                let base_dts = (timestamp as u64) * 90;
                let sample = lvqr_cmaf::RawSample {
                    track_id: 1,
                    dts: base_dts,
                    cts_offset: 0,
                    duration: 3000,
                    payload,
                    keyframe: true,
                };
                if let Ok(mut group) = video_track.append_group() {
                    let _ = group.write_frame(init.clone());
                    let seg = lvqr_cmaf::build_moof_mdat(video_seq, 1, base_dts, &[sample]);
                    let _ = group.write_frame(seg);
                    video_group = Some(group);
                }
            }
            2 => {
                if video_init.is_none() {
                    continue;
                }
                video_seq += 1;
                let base_dts = (timestamp as u64) * 90;
                let sample = lvqr_cmaf::RawSample {
                    track_id: 1,
                    dts: base_dts,
                    cts_offset: 0,
                    duration: 3000,
                    payload,
                    keyframe: false,
                };
                if let Some(ref mut group) = video_group {
                    let seg = lvqr_cmaf::build_moof_mdat(video_seq, 1, base_dts, &[sample]);
                    let _ = group.write_frame(seg);
                }
            }
            _ => {}
        }
    }

    if let Some(mut g) = video_group.take() {
        let _ = g.finish();
    }
    state.events.emit(RelayEvent::BroadcastStopped {
        name: broadcast.clone(),
    });
    tracing::info!(broadcast = %broadcast, "WS ingest session ended");
}

/// Background task that listens on the event bus for new broadcasts and
/// starts a recorder for each one. Event-driven so WS-ingested broadcasts
/// are recorded identically to RTMP-ingested ones.
async fn spawn_recordings(
    recorder: lvqr_record::BroadcastRecorder,
    origin: lvqr_moq::OriginProducer,
    mut events: tokio::sync::broadcast::Receiver<RelayEvent>,
    shutdown: CancellationToken,
) {
    let mut active: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        let event = tokio::select! {
            res = events.recv() => res,
            _ = shutdown.cancelled() => return,
        };
        let event = match event {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(missed = n, "recorder event stream lagged");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        match event {
            RelayEvent::BroadcastStarted { name } => {
                if !active.insert(name.clone()) {
                    continue;
                }
                let consumer = origin.consume();
                let Some(broadcast) = consumer.consume_broadcast(&name) else {
                    tracing::warn!(broadcast = %name, "recorder: broadcast not resolvable yet");
                    active.remove(&name);
                    continue;
                };
                let recorder = recorder.clone();
                let cancel = shutdown.clone();
                tracing::info!(broadcast = %name, "starting recording");
                let name_clone = name.clone();
                tokio::spawn(async move {
                    let _ = recorder
                        .record_broadcast(&name_clone, broadcast, lvqr_record::RecordOptions::default(), cancel)
                        .await;
                });
            }
            RelayEvent::BroadcastStopped { name } => {
                active.remove(&name);
            }
            RelayEvent::ViewerJoined { .. } | RelayEvent::ViewerLeft { .. } => {}
        }
    }
}

/// Parse an AVCDecoderConfigurationRecord from a WS ingest `type=0` payload.
fn parse_avcc_record(data: &[u8]) -> Option<lvqr_ingest::remux::VideoConfig> {
    if data.len() < 6 {
        return None;
    }
    let profile = data[1];
    let compat = data[2];
    let level = data[3];
    let nalu_length_size = (data[4] & 0x03) + 1;

    let num_sps = (data[5] & 0x1F) as usize;
    let mut offset = 6;
    let mut sps_list = Vec::with_capacity(num_sps);
    for _ in 0..num_sps {
        if offset + 2 > data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + len > data.len() {
            return None;
        }
        sps_list.push(data[offset..offset + len].to_vec());
        offset += len;
    }

    if offset >= data.len() {
        return None;
    }
    let num_pps = data[offset] as usize;
    offset += 1;
    let mut pps_list = Vec::with_capacity(num_pps);
    for _ in 0..num_pps {
        if offset + 2 > data.len() {
            return None;
        }
        let len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        if offset + len > data.len() {
            return None;
        }
        pps_list.push(data[offset..offset + len].to_vec());
        offset += len;
    }

    if sps_list.is_empty() || pps_list.is_empty() {
        return None;
    }

    Some(lvqr_ingest::remux::VideoConfig {
        sps_list,
        pps_list,
        profile,
        compat,
        level,
        nalu_length_size,
    })
}

#[cfg(all(test, feature = "whisper"))]
mod whisper_serve_config_tests {
    use super::ServeConfig;
    use std::path::PathBuf;

    #[test]
    fn loopback_ephemeral_defaults_whisper_model_to_none() {
        let cfg = ServeConfig::loopback_ephemeral();
        assert!(cfg.whisper_model.is_none());
    }

    #[test]
    fn whisper_model_round_trips_through_serve_config() {
        let path = PathBuf::from("/nonexistent/ggml-tiny.en.bin");
        let cfg = ServeConfig {
            whisper_model: Some(path.clone()),
            ..ServeConfig::loopback_ephemeral()
        };
        assert_eq!(cfg.whisper_model.as_deref(), Some(path.as_path()));
    }
}

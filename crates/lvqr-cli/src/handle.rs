//! `ServerHandle` struct + accessors + `Drop`.
//!
//! Extracted out of `lib.rs` in the session-111-B1 follow-up refactor
//! so the composition root stays focused on wiring. The public type
//! [`ServerHandle`] is re-exported from `crate::lib` so external
//! callers (`main.rs`, `lvqr-test-utils`, embedders) continue to name
//! it as `lvqr_cli::ServerHandle` with no API churn.
//!
//! Struct fields are `pub(crate)` so the composition root in
//! `lib::start` can continue to build the handle via a struct literal
//! without a 30-argument constructor.

use anyhow::Result;
use lvqr_fragment::FragmentBroadcasterRegistry;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Handle to a running LVQR server. Dropping the handle cancels the shared
/// shutdown token but does not block on subsystem drain; call [`shutdown`]
/// explicitly in tests that need deterministic teardown before the next
/// fixture starts.
///
/// [`shutdown`]: ServerHandle::shutdown
pub struct ServerHandle {
    pub(crate) relay_addr: SocketAddr,
    pub(crate) rtmp_addr: SocketAddr,
    pub(crate) admin_addr: SocketAddr,
    pub(crate) hls_addr: Option<SocketAddr>,
    pub(crate) whep_addr: Option<SocketAddr>,
    pub(crate) whip_addr: Option<SocketAddr>,
    pub(crate) dash_addr: Option<SocketAddr>,
    pub(crate) rtsp_addr: Option<SocketAddr>,
    pub(crate) srt_addr: Option<SocketAddr>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) join: Option<tokio::task::JoinHandle<()>>,
    /// Cluster handle kept alive for the server's lifetime when
    /// clustering is configured. Feature-gated; `None` when the
    /// `cluster` feature is on but `ServeConfig::cluster_listen` is
    /// `None`, and absent entirely when the feature is off.
    #[cfg(feature = "cluster")]
    pub(crate) cluster: Option<std::sync::Arc<lvqr_cluster::Cluster>>,
    /// WASM fragment-filter tap handle kept alive for the
    /// server's lifetime when `--wasm-filter` is configured.
    /// Tests read fragment counters off this handle to assert
    /// the filter actually saw the ingested broadcasts.
    pub(crate) wasm_filter: Option<lvqr_wasm::WasmFilterBridgeHandle>,
    /// `AgentRunner` handle kept alive for the server's
    /// lifetime when `--whisper-model` is configured. Dropping
    /// the handle aborts every per-broadcast drain task the
    /// runner spawned; holding it on `ServerHandle` mirrors the
    /// `wasm_filter` pattern so the lifetime semantics are
    /// identical. Tier 4 item 4.5 session D. Feature-gated
    /// `#[cfg(feature = "whisper")]`.
    #[cfg(feature = "whisper")]
    pub(crate) agent_runner: Option<lvqr_agent::AgentRunnerHandle>,
    /// `TranscodeRunner` handle kept alive for the server's
    /// lifetime when [`ServeConfig::transcode_renditions`] is
    /// non-empty. Dropping the handle aborts every per-rendition
    /// drain task the runner spawned; holding it on `ServerHandle`
    /// mirrors the `agent_runner` and `wasm_filter` patterns so
    /// the lifetime semantics are identical. Tier 4 item 4.6
    /// session 106 C. Feature-gated `#[cfg(feature = "transcode")]`.
    #[cfg(feature = "transcode")]
    pub(crate) transcode_runner: Option<lvqr_transcode::TranscodeRunnerHandle>,
    /// Shared latency SLO tracker backing the `/api/v1/slo` admin
    /// route. Cloned into every instrumented egress surface at
    /// start() so the route sees per-(broadcast, transport) samples
    /// drawn from the whole server. Tier 4 item 4.7 session A.
    pub(crate) slo: lvqr_admin::LatencyTracker,
    /// Mesh coordinator backing the `/api/v1/mesh` admin route
    /// and the `/signal` WebSocket. `Some` when
    /// [`ServeConfig::mesh_enabled`] was `true` at `start()`,
    /// `None` otherwise. Integration tests use
    /// [`ServerHandle::mesh_coordinator`] to assert on tree
    /// state after driving WS subscribers through the relay.
    /// Session 111-B1.
    pub(crate) mesh_coordinator: Option<Arc<lvqr_mesh::MeshCoordinator>>,
    /// One WASM filter hot-reload watcher per entry in the
    /// configured filter chain. Empty when `--wasm-filter` is
    /// unset; otherwise holds N watchers in the same order as the
    /// chain. On every debounced change to a watched path, that
    /// slot's reloader recompiles its module and swaps it into its
    /// own `SharedFilter`; in-flight `apply` calls on that slot
    /// complete on the old module, subsequent calls see the new
    /// one, and the other slots in the chain are unaffected.
    /// Unused directly; held for each watcher's `Drop` side effect
    /// of stopping its background thread on shutdown.
    pub(crate) _wasm_reloaders: Vec<lvqr_wasm::WasmFilterReloader>,
    /// Shared `(broadcast, track)`-keyed registry every
    /// ingest crate publishes into and every consumer
    /// (HLS / DASH / archive / WASM filter / captions
    /// bridge) subscribes through. Exposed on the handle so
    /// integration tests can publish synthetic fragments
    /// (e.g. caption cues for the captions track) without
    /// driving a real ingest protocol. Tier 4 item 4.5
    /// session C added this accessor.
    pub(crate) fragment_registry: FragmentBroadcasterRegistry,
    /// Clone of the relay-backing `OriginProducer`. Federation
    /// tests use this to inject synthetic broadcasts on one
    /// server and verify they propagate to another via a
    /// configured [`lvqr_cluster::FederationLink`]. Always
    /// present since every server has an origin; feature gating
    /// lives on the callers that construct broadcasts through
    /// this handle.
    pub(crate) origin: lvqr_moq::OriginProducer,
    /// `FederationRunner` holding outbound MoQ sessions to peer
    /// clusters open for the server's lifetime. `None` when
    /// `ServeConfig::federation_links` is empty. Feature-gated
    /// on `cluster` since `FederationRunner` lives in
    /// `lvqr-cluster`. Tier 4 item 4.4 session B.
    #[cfg(feature = "cluster")]
    pub(crate) federation_runner: Option<lvqr_cluster::FederationRunner>,
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

    /// `TranscodeRunner` handle backing this server, when
    /// [`ServeConfig::transcode_renditions`] was non-empty at
    /// `start()` time. Returns `None` when no ladder is
    /// configured. Tests read per-`(transcoder, rendition,
    /// broadcast, track)` counters off this handle to assert
    /// the ladder factories actually observed the source
    /// broadcast. Feature-gated on `transcode`. Tier 4 item
    /// 4.6 session 106 C.
    #[cfg(feature = "transcode")]
    pub fn transcode_runner(&self) -> Option<&lvqr_transcode::TranscodeRunnerHandle> {
        self.transcode_runner.as_ref()
    }

    /// Cloneable handle to the shared
    /// [`lvqr_admin::LatencyTracker`] that powers the
    /// `/api/v1/slo` admin route. Tests snapshot this directly
    /// to assert the instrumented egress surfaces are recording
    /// samples. Tier 4 item 4.7 session A.
    pub fn slo(&self) -> &lvqr_admin::LatencyTracker {
        &self.slo
    }

    /// Mesh coordinator backing the `/api/v1/mesh` admin route
    /// and the `/signal` WebSocket. Returns `None` when the
    /// server was started with `mesh_enabled = false`. Tests
    /// call `peer_count()` + `offload_percentage()` directly on
    /// the coordinator to assert on tree state without going
    /// through the admin HTTP surface. Session 111-B1.
    pub fn mesh_coordinator(&self) -> Option<&Arc<lvqr_mesh::MeshCoordinator>> {
        self.mesh_coordinator.as_ref()
    }

    /// Construct the WebSocket `/signal` URL. The returned URL
    /// points at the admin port (the same axum service that
    /// mounts `/signal`). Tests add a
    /// `Sec-WebSocket-Protocol: lvqr.bearer.<token>` header or a
    /// `?token=<token>` query parameter for the subscribe-auth
    /// gate. Session 111-B1.
    pub fn signal_url(&self) -> String {
        format!("ws://{}/signal", self.admin_addr)
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

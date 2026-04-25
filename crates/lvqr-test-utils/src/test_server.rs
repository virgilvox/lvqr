//! Spawnable full-stack LVQR server for integration tests.
//!
//! Every integration test in the workspace used to roll its own server
//! setup: pre-bind an RTMP port, pre-bind a WS port, stand up a bespoke
//! `axum::Router`, wire mocks for everything else. That boilerplate
//! drifted across test files and silently diverged from the production
//! `serve()` path, which is exactly the "theatrical tests" anti-pattern
//! the Tier 0 audit flagged.
//!
//! `TestServer` collapses all of that into one call:
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! use lvqr_test_utils::TestServer;
//!
//! let server = TestServer::start(Default::default()).await?;
//! let rtmp_url = server.rtmp_url("live", "test");
//! let ws_url = server.ws_url("live/test");
//! // drive real clients against rtmp_url / ws_url...
//! server.shutdown().await?;
//! # Ok(())
//! # }
//! ```
//!
//! The handle drives the exact same `lvqr_cli::start` path the production
//! `lvqr serve` command runs, so any drift between the test wiring and the
//! real server is impossible by construction.

use anyhow::Result;
use lvqr_auth::SharedAuth;
use lvqr_cli::{ServeConfig, ServerHandle, start};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

/// Per-protocol toggles plus auth injection for a [`TestServer`].
///
/// Defaults are the common case for integration tests: every LVQR
/// protocol enabled on `127.0.0.1:0` including the LL-HLS surface,
/// no mesh, no recording, open access. Override individual fields
/// through the builder methods.
#[derive(Default, Clone)]
pub struct TestServerConfig {
    mesh_enabled: bool,
    max_peers: Option<usize>,
    auth: Option<SharedAuth>,
    record_dir: Option<PathBuf>,
    archive_dir: Option<PathBuf>,
    hmac_playback_secret: Option<String>,
    wasm_filter: Vec<PathBuf>,
    hls_disabled: bool,
    dash_enabled: bool,
    srt_enabled: bool,
    rtsp_enabled: bool,
    whip_enabled: bool,
    whep_enabled: bool,
    #[cfg(feature = "c2pa")]
    c2pa: Option<lvqr_archive::provenance::C2paConfig>,
    #[cfg(feature = "whisper")]
    whisper_model: Option<PathBuf>,
    #[cfg(feature = "transcode")]
    transcode_renditions: Vec<lvqr_transcode::RenditionSpec>,
    #[cfg(feature = "transcode")]
    source_bandwidth_kbps: Option<u32>,
    federation_links: Vec<lvqr_cluster::FederationLink>,
    relay_addr: Option<SocketAddr>,
    no_auth_live_playback: bool,
    no_auth_signal: bool,
    mesh_root_peer_count: Option<usize>,
    mesh_ice_servers: Vec<lvqr_signal::IceServer>,
    /// Session 146: when true, disable the runtime stream-key CRUD
    /// admin API. Default `false` means the wrap is on (matches
    /// `lvqr-cli`'s `lvqr serve` default), so existing tests that
    /// run `TestServer::start(TestServerConfig::new())` exercise
    /// the same code path that production deployments take. Tests
    /// that want pre-146 behavior verbatim flip this with
    /// `with_no_streamkeys()`.
    no_streamkeys: bool,
    /// Session 147: optional config-file path. When `Some`, the
    /// TestServer's `start()` path applies the file at boot via the
    /// reload pipeline, installs the SIGHUP listener, and mounts
    /// `/api/v1/config-reload`. `None` means the routes are still
    /// mounted but GET returns a default `ConfigReloadStatus` and
    /// POST returns 503.
    config_file: Option<PathBuf>,
}

impl TestServerConfig {
    /// Create a fresh test-server config with all defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable the peer mesh coordinator and `/signal` endpoint.
    pub fn with_mesh(mut self, max_peers: usize) -> Self {
        self.mesh_enabled = true;
        self.max_peers = Some(max_peers);
        self
    }

    /// Install a pre-built auth provider. When unset, the server runs
    /// with open access (`NoopAuthProvider`).
    pub fn with_auth(mut self, auth: SharedAuth) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Enable disk recording into the given directory.
    pub fn with_record_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.record_dir = Some(dir.into());
        self
    }

    /// Enable the DVR archive index under the given directory. The
    /// server opens `<dir>/archive.redb` and writes fragment bytes
    /// into `<dir>/<broadcast>/<track>/<seq>.m4s` as they arrive.
    pub fn with_archive_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.archive_dir = Some(dir.into());
        self
    }

    /// Install an HMAC signing secret for short-lived playback
    /// URLs (PLAN v1.1 row 121). When set, every `/playback/*`
    /// handler accepts an alternative auth path via
    /// `?exp=<ts>&sig=<b64url>` query params that short-circuits
    /// the `SharedAuth` subscribe gate. Used by the signed-URL
    /// integration tests to exercise the valid / tampered /
    /// expired / missing-sig scenarios.
    pub fn with_hmac_playback_secret(mut self, secret: impl Into<String>) -> Self {
        self.hmac_playback_secret = Some(secret.into());
        self
    }

    /// Install a pre-built `C2paConfig` so the TestServer's archive
    /// drain task runs broadcast-end finalize (writes
    /// `finalized.mp4` + `finalized.c2pa`) and the admin router
    /// mounts `/playback/verify/{broadcast}`. Requires
    /// `with_archive_dir(..)` to be called as well; without an
    /// archive directory the drain task does not spawn and the
    /// verify route has nothing to read. Gated on
    /// `feature = "c2pa"` on `lvqr-test-utils`. Tier 4 item 4.3
    /// session B3.
    #[cfg(feature = "c2pa")]
    pub fn with_c2pa(mut self, c2pa: lvqr_archive::provenance::C2paConfig) -> Self {
        self.c2pa = Some(c2pa);
        self
    }

    /// Install a whisper.cpp model path on the `ServeConfig` so the
    /// TestServer's `start()` path constructs a
    /// `WhisperCaptionsFactory` + `AgentRunner` against the shared
    /// fragment registry. Gated on `feature = "whisper"` on
    /// `lvqr-test-utils`. Tier 4 item 4.5 session D.
    #[cfg(feature = "whisper")]
    pub fn with_whisper_model(mut self, path: impl Into<PathBuf>) -> Self {
        self.whisper_model = Some(path.into());
        self
    }

    /// Install a transcode ABR ladder on the TestServer's
    /// `ServeConfig`. The server builds one
    /// `SoftwareTranscoderFactory` + one
    /// `AudioPassthroughTranscoderFactory` per rendition against
    /// the shared fragment registry; the LL-HLS master-playlist
    /// composer emits one `#EXT-X-STREAM-INF` per rendition
    /// sibling the transcoder produces. Gated on
    /// `feature = "transcode"` on `lvqr-test-utils`. Tier 4 item
    /// 4.6 session 106 C.
    #[cfg(feature = "transcode")]
    pub fn with_transcode_ladder(mut self, renditions: Vec<lvqr_transcode::RenditionSpec>) -> Self {
        self.transcode_renditions = renditions;
        self
    }

    /// Override the advertised source-variant `BANDWIDTH` in the
    /// master playlist (in kilobits per second). Defaults (None)
    /// to `highest_rung_kbps * 1.2`. Only meaningful alongside
    /// [`with_transcode_ladder`](Self::with_transcode_ladder).
    /// Tier 4 item 4.6 session 106 C.
    #[cfg(feature = "transcode")]
    pub fn with_source_bandwidth_kbps(mut self, kbps: u32) -> Self {
        self.source_bandwidth_kbps = Some(kbps);
        self
    }

    /// Add a cross-cluster federation link to the TestServer's
    /// `ServeConfig`. Multiple calls append; call order matches
    /// `ServeConfig::federation_links` ordering. Tier 4 item 4.4
    /// session B.
    pub fn with_federation_link(mut self, link: lvqr_cluster::FederationLink) -> Self {
        self.federation_links.push(link);
        self
    }

    /// Override the MoQ relay bind address. Defaults to
    /// `127.0.0.1:0` (ephemeral). Tier 4 item 4.4 session C's
    /// reconnect integration test uses this to restart a
    /// shutdown peer on the exact same QUIC/UDP port so the
    /// reconnect path sees a recovered peer rather than picking
    /// up a fresh cluster. UDP (unlike TCP) has no TIME_WAIT so
    /// port reuse across a shutdown / restart works without
    /// SO_REUSEADDR gymnastics.
    pub fn with_relay_addr(mut self, addr: SocketAddr) -> Self {
        self.relay_addr = Some(addr);
        self
    }

    /// Turn off the LL-HLS HTTP surface. Tests that do not exercise
    /// HLS can opt out to skip the extra TCP listener.
    pub fn without_hls(mut self) -> Self {
        self.hls_disabled = true;
        self
    }

    /// Enable the MPEG-DASH HTTP surface. Off by default; tests
    /// that exercise the `/dash/{broadcast}/...` routes flip it on
    /// via this builder so the server pre-binds an ephemeral
    /// loopback listener.
    pub fn with_dash(mut self) -> Self {
        self.dash_enabled = true;
        self
    }

    pub fn with_srt(mut self) -> Self {
        self.srt_enabled = true;
        self
    }

    pub fn with_rtsp(mut self) -> Self {
        self.rtsp_enabled = true;
        self
    }

    /// Enable the WHIP ingest HTTP surface. Needed for cross-protocol
    /// integration tests that exercise the `POST /whip/{broadcast}`
    /// ingest path. Tier 4 item 4.8 session A added this builder to
    /// support session 96 B's one-token-all-protocols E2E.
    pub fn with_whip(mut self) -> Self {
        self.whip_enabled = true;
        self
    }

    /// Enable the WHEP egress HTTP surface. Needed for cross-protocol
    /// integration tests that exercise the `POST /whep/{broadcast}`
    /// signaling path. Session 115 added this builder to support the
    /// RTMP-to-WHEP audio E2E test.
    pub fn with_whep(mut self) -> Self {
        self.whep_enabled = true;
        self
    }

    /// Install a WASM fragment filter from a file path before
    /// any ingest listener starts. The server holds the
    /// `WasmFilterBridgeHandle` on `ServerHandle`; tests read
    /// per-broadcast counters off it.
    ///
    /// Repeated calls chain filters in insertion order so tests
    /// can exercise the multi-filter pipeline that CLI's
    /// `--wasm-filter` flag accepts (PLAN Phase D, session 136).
    /// A single call preserves the legacy single-filter shape.
    pub fn with_wasm_filter(mut self, path: impl Into<PathBuf>) -> Self {
        self.wasm_filter.push(path.into());
        self
    }

    /// Disable the subscribe-auth gate on live HLS and DASH
    /// routes. Mirrors the CLI's `--no-auth-live-playback` flag.
    /// When unset (default), the TestServer wires the same
    /// `SubscribeAuth` gate on the HLS + DASH routers that
    /// `/ws/*` and `/playback/*` already use. Integration tests
    /// that want to exercise the "open live playback with gated
    /// ingest" shape flip this on. Session 112.
    pub fn without_live_playback_auth(mut self) -> Self {
        self.no_auth_live_playback = true;
        self
    }

    /// Disable the subscribe-auth gate on the mesh `/signal`
    /// WebSocket. Mirrors the CLI's `--no-auth-signal` flag.
    /// When unset (default, and `with_mesh` is also set), the
    /// TestServer requires a valid subscribe token via the
    /// `?token=<token>` query parameter on every `/signal`
    /// upgrade. Session 111-B1.
    pub fn without_signal_auth(mut self) -> Self {
        self.no_auth_signal = true;
        self
    }

    /// Disable the runtime stream-key CRUD admin API. Mirrors the
    /// CLI's `--no-streamkeys` flag. When unset (default), the
    /// TestServer wraps the configured auth provider in a
    /// [`lvqr_auth::MultiKeyAuthProvider`] backed by an in-memory
    /// store and mounts `/api/v1/streamkeys/*`. Session 146.
    pub fn with_no_streamkeys(mut self) -> Self {
        self.no_streamkeys = true;
        self
    }

    /// Configure a `--config <path>` for hot reload (session 147).
    /// The file is applied at boot and re-applied on SIGHUP /
    /// `POST /api/v1/config-reload`. When unset (default), reload
    /// is disabled (GET returns default status; POST returns 503).
    pub fn with_config_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_file = Some(path.into());
        self
    }

    /// Override the mesh `root_peer_count` so tests can exercise
    /// the `AssignParent` path with a small number of peers.
    /// Defaults to the `lvqr_mesh::MeshConfig::default()` value
    /// of 30; tests that want the second peer to become a child
    /// of the first set this to 1. Only meaningful when
    /// `with_mesh` is set. Session 111-B1.
    pub fn with_mesh_root_peer_count(mut self, count: usize) -> Self {
        self.mesh_root_peer_count = Some(count);
        self
    }

    /// Configure the mesh `--mesh-ice-servers` snapshot pushed via
    /// `AssignParent`. Empty (default) emits `ice_servers: []` so
    /// JS clients fall back to their constructor default. Session
    /// 143 -- TURN deployment recipe.
    pub fn with_mesh_ice_servers(mut self, servers: Vec<lvqr_signal::IceServer>) -> Self {
        self.mesh_ice_servers = servers;
        self
    }
}

/// A running LVQR server bound to ephemeral loopback ports.
///
/// Drop cancels the shutdown token eagerly; call [`shutdown`] to wait for
/// every subsystem to drain, which is what deterministic tests want.
///
/// [`shutdown`]: TestServer::shutdown
pub struct TestServer {
    handle: ServerHandle,
}

impl TestServer {
    /// Start a test server with the supplied config. Every listener is
    /// bound before this function returns, so [`rtmp_addr`], [`ws_url`]
    /// and friends are valid immediately.
    ///
    /// [`rtmp_addr`]: TestServer::rtmp_addr
    /// [`ws_url`]: TestServer::ws_url
    pub async fn start(config: TestServerConfig) -> Result<Self> {
        let loopback: IpAddr = Ipv4Addr::LOCALHOST.into();
        let ephemeral: SocketAddr = (loopback, 0).into();
        let serve_config = ServeConfig {
            relay_addr: config.relay_addr.unwrap_or(ephemeral),
            rtmp_addr: ephemeral,
            admin_addr: ephemeral,
            hls_addr: if config.hls_disabled { None } else { Some(ephemeral) },
            hls_dvr_window_secs: 120,
            hls_target_duration_secs: 2,
            hls_part_target_ms: 200,
            whep_addr: if config.whep_enabled { Some(ephemeral) } else { None },
            whip_addr: if config.whip_enabled { Some(ephemeral) } else { None },
            dash_addr: if config.dash_enabled { Some(ephemeral) } else { None },
            rtsp_addr: if config.rtsp_enabled { Some(ephemeral) } else { None },
            srt_addr: if config.srt_enabled { Some(ephemeral) } else { None },
            mesh_enabled: config.mesh_enabled,
            max_peers: config.max_peers.unwrap_or(3),
            auth: config.auth,
            record_dir: config.record_dir,
            archive_dir: config.archive_dir,
            hmac_playback_secret: config.hmac_playback_secret,
            #[cfg(feature = "c2pa")]
            c2pa: config.c2pa,
            #[cfg(feature = "whisper")]
            whisper_model: config.whisper_model,
            #[cfg(feature = "transcode")]
            transcode_renditions: config.transcode_renditions,
            #[cfg(feature = "transcode")]
            source_bandwidth_kbps: config.source_bandwidth_kbps,
            wasm_filter: config.wasm_filter,
            // Prometheus install is process-wide and panics on second
            // call, so tests always disable it. Metrics macros still
            // fire; they're just dropped on the floor instead of being
            // rendered through a /metrics endpoint.
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
            federation_links: config.federation_links,
            no_auth_live_playback: config.no_auth_live_playback,
            no_auth_signal: config.no_auth_signal,
            mesh_root_peer_count: config.mesh_root_peer_count,
            mesh_ice_servers: config.mesh_ice_servers,
            streamkeys_enabled: !config.no_streamkeys,
            // Session 147: when the test set `with_config_file(path)`,
            // wire a ConfigReloadSeed with all-None CLI defaults so
            // the file's `[auth]` section is the sole source of
            // truth. start()'s boot-time reload applies it.
            config_reload: config.config_file.map(|path| lvqr_cli::ConfigReloadSeed {
                path,
                auth_boot_defaults: lvqr_cli::AuthBootDefaults::default(),
                jwks_boot: None,
                webhook_boot: None,
            }),
        };
        let handle = start(serve_config).await?;
        Ok(Self { handle })
    }

    /// Bound QUIC/MoQ relay address.
    pub fn relay_addr(&self) -> SocketAddr {
        self.handle.relay_addr()
    }

    /// Bound RTMP ingest address.
    pub fn rtmp_addr(&self) -> SocketAddr {
        self.handle.rtmp_addr()
    }

    /// Bound admin HTTP address (also hosts `/ws/*` and `/ingest/*`).
    pub fn admin_addr(&self) -> SocketAddr {
        self.handle.admin_addr()
    }

    /// Bound LL-HLS HTTP address. Panics if HLS was disabled via
    /// [`TestServerConfig::without_hls`].
    pub fn hls_addr(&self) -> SocketAddr {
        self.handle
            .hls_addr()
            .expect("HLS surface disabled on this TestServer; remove without_hls() to enable")
    }

    /// Build an HTTP URL pointing at a path on the LL-HLS surface, e.g.
    /// `hls_url("/playlist.m3u8")`. Panics if HLS was disabled.
    pub fn hls_url(&self, path: &str) -> String {
        self.handle
            .hls_url(path)
            .expect("HLS surface disabled on this TestServer; remove without_hls() to enable")
    }

    /// Bound MPEG-DASH HTTP address. Panics if DASH was not
    /// enabled via [`TestServerConfig::with_dash`].
    pub fn dash_addr(&self) -> SocketAddr {
        self.handle
            .dash_addr()
            .expect("DASH surface not enabled on this TestServer; call with_dash() to enable")
    }

    /// Bound RTSP ingest TCP address. Panics if RTSP was not enabled.
    pub fn rtsp_addr(&self) -> SocketAddr {
        self.handle
            .rtsp_addr()
            .expect("RTSP ingest not enabled on this TestServer; call with_rtsp() to enable")
    }

    /// Bound WHIP ingest HTTP address. Panics if WHIP was not enabled.
    pub fn whip_addr(&self) -> SocketAddr {
        self.handle
            .whip_addr()
            .expect("WHIP ingest not enabled on this TestServer; call with_whip() to enable")
    }

    /// Bound WHEP egress HTTP address. Panics if WHEP was not enabled.
    pub fn whep_addr(&self) -> SocketAddr {
        self.handle
            .whep_addr()
            .expect("WHEP egress not enabled on this TestServer; call with_whep() to enable")
    }

    /// WASM filter tap handle (read-only). Returns `None` when
    /// `with_wasm_filter` was not used on this TestServer.
    pub fn wasm_filter(&self) -> Option<&lvqr_wasm::WasmFilterBridgeHandle> {
        self.handle.wasm_filter()
    }

    /// AgentRunner handle (read-only). Returns `None` when
    /// [`TestServerConfig::with_whisper_model`] was not used.
    /// Tests read per-`(agent, broadcast, track)` fragment
    /// counters off this handle to assert the whisper agent
    /// actually observed the RTMP audio. Gated on
    /// `feature = "whisper"`. Tier 4 item 4.5 session D.
    #[cfg(feature = "whisper")]
    pub fn agent_runner(&self) -> Option<&lvqr_agent::AgentRunnerHandle> {
        self.handle.agent_runner()
    }

    /// `TranscodeRunner` handle (read-only). Returns `None` when
    /// [`TestServerConfig::with_transcode_ladder`] was not used.
    /// Tests read per-`(transcoder, rendition, broadcast, track)`
    /// counters off this handle to assert the ladder factories
    /// actually observed the RTMP source. Gated on
    /// `feature = "transcode"`. Tier 4 item 4.6 session 106 C.
    #[cfg(feature = "transcode")]
    pub fn transcode_runner(&self) -> Option<&lvqr_transcode::TranscodeRunnerHandle> {
        self.handle.transcode_runner()
    }

    /// Shared latency SLO tracker (read-only). Tests snapshot
    /// this to assert the server's egress surfaces are recording
    /// per-(broadcast, transport) latency samples. Tier 4 item
    /// 4.7 session A.
    pub fn slo(&self) -> &lvqr_cli::LatencyTracker {
        self.handle.slo()
    }

    /// Cloneable handle to the server's relay-backing
    /// [`lvqr_moq::OriginProducer`]. Tier 4 item 4.4 session B
    /// federation tests use this to inject synthetic MoQ
    /// broadcasts on one TestServer and assert they propagate to
    /// another via a configured federation link.
    pub fn origin(&self) -> &lvqr_moq::OriginProducer {
        self.handle.origin()
    }

    /// FederationRunner handle. Returns `None` when no
    /// [`with_federation_link`](TestServerConfig::with_federation_link)
    /// builder was invoked. Tier 4 item 4.4 session B.
    pub fn federation_runner(&self) -> Option<&lvqr_cluster::FederationRunner> {
        self.handle.federation_runner()
    }

    /// Bound SRT ingest UDP address. Panics if SRT was not enabled.
    pub fn srt_addr(&self) -> SocketAddr {
        self.handle
            .srt_addr()
            .expect("SRT ingest not enabled on this TestServer; call with_srt() to enable")
    }

    /// Build an HTTP URL pointing at a path on the DASH surface,
    /// e.g. `dash_url("/dash/live/test/manifest.mpd")`. Panics if
    /// DASH was not enabled.
    pub fn dash_url(&self, path: &str) -> String {
        self.handle
            .dash_url(path)
            .expect("DASH surface not enabled on this TestServer; call with_dash() to enable")
    }

    /// Admin HTTP base URL (e.g. `http://127.0.0.1:34921`).
    pub fn http_base(&self) -> String {
        self.handle.http_base()
    }

    /// WebSocket subscribe URL for a broadcast (e.g. `"live/test"`).
    pub fn ws_url(&self, broadcast: &str) -> String {
        self.handle.ws_url(broadcast)
    }

    /// Mesh `/signal` WebSocket URL. The underlying `ServerHandle`
    /// mounts `/signal` on the admin port when `mesh_enabled` is
    /// set. Integration tests append a `?token=<token>` query
    /// parameter to carry the subscribe bearer. Session 111-B1.
    pub fn signal_url(&self) -> String {
        self.handle.signal_url()
    }

    /// Mesh coordinator handle. `None` when the TestServer was
    /// started without `with_mesh(..)`. Tests inspect
    /// `peer_count()` + `offload_percentage()` directly on the
    /// coordinator to assert on tree state. Session 111-B1.
    pub fn mesh_coordinator(&self) -> Option<&std::sync::Arc<lvqr_mesh::MeshCoordinator>> {
        self.handle.mesh_coordinator()
    }

    /// WebSocket ingest URL for a broadcast.
    pub fn ws_ingest_url(&self, broadcast: &str) -> String {
        self.handle.ws_ingest_url(broadcast)
    }

    /// RTMP publish URL for an `app` and `stream_key`.
    pub fn rtmp_url(&self, app: &str, stream_key: &str) -> String {
        self.handle.rtmp_url(app, stream_key)
    }

    /// Cloneable handle to the shared `FragmentBroadcasterRegistry`.
    /// Useful in tests that want to publish synthetic fragments
    /// directly onto a track (e.g. caption cues for the captions
    /// track) without driving a real ingest protocol. Tier 4
    /// item 4.5 session C surfaced this for the captions HLS
    /// E2E test.
    pub fn fragment_registry(&self) -> &lvqr_fragment::FragmentBroadcasterRegistry {
        self.handle.fragment_registry()
    }

    /// Trigger graceful shutdown and wait for every subsystem to drain.
    pub async fn shutdown(self) -> Result<()> {
        self.handle.shutdown().await
    }
}

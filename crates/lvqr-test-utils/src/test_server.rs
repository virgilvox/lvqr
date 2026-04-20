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
    wasm_filter: Option<PathBuf>,
    hls_disabled: bool,
    dash_enabled: bool,
    srt_enabled: bool,
    rtsp_enabled: bool,
    whip_enabled: bool,
    #[cfg(feature = "c2pa")]
    c2pa: Option<lvqr_archive::provenance::C2paConfig>,
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

    /// Install a WASM fragment filter from a file path before
    /// any ingest listener starts. The server holds the
    /// `WasmFilterBridgeHandle` on `ServerHandle`; tests read
    /// per-broadcast counters off it.
    pub fn with_wasm_filter(mut self, path: impl Into<PathBuf>) -> Self {
        self.wasm_filter = Some(path.into());
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
            relay_addr: ephemeral,
            rtmp_addr: ephemeral,
            admin_addr: ephemeral,
            hls_addr: if config.hls_disabled { None } else { Some(ephemeral) },
            hls_dvr_window_secs: 120,
            hls_target_duration_secs: 2,
            hls_part_target_ms: 200,
            whep_addr: None,
            whip_addr: if config.whip_enabled { Some(ephemeral) } else { None },
            dash_addr: if config.dash_enabled { Some(ephemeral) } else { None },
            rtsp_addr: if config.rtsp_enabled { Some(ephemeral) } else { None },
            srt_addr: if config.srt_enabled { Some(ephemeral) } else { None },
            mesh_enabled: config.mesh_enabled,
            max_peers: config.max_peers.unwrap_or(3),
            auth: config.auth,
            record_dir: config.record_dir,
            archive_dir: config.archive_dir,
            #[cfg(feature = "c2pa")]
            c2pa: config.c2pa,
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

    /// WASM filter tap handle (read-only). Returns `None` when
    /// `with_wasm_filter` was not used on this TestServer.
    pub fn wasm_filter(&self) -> Option<&lvqr_wasm::WasmFilterBridgeHandle> {
        self.handle.wasm_filter()
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

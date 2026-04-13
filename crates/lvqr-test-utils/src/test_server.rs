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
/// protocol enabled on `127.0.0.1:0`, no mesh, no recording, open
/// access. Override individual fields through the builder methods.
#[derive(Default, Clone)]
pub struct TestServerConfig {
    mesh_enabled: bool,
    max_peers: Option<usize>,
    auth: Option<SharedAuth>,
    record_dir: Option<PathBuf>,
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
            mesh_enabled: config.mesh_enabled,
            max_peers: config.max_peers.unwrap_or(3),
            auth: config.auth,
            record_dir: config.record_dir,
            // Prometheus install is process-wide and panics on second
            // call, so tests always disable it. Metrics macros still
            // fire; they're just dropped on the floor instead of being
            // rendered through a /metrics endpoint.
            install_prometheus: false,
            tls_cert: None,
            tls_key: None,
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

    /// Trigger graceful shutdown and wait for every subsystem to drain.
    pub async fn shutdown(self) -> Result<()> {
        self.handle.shutdown().await
    }
}

use crate::error::RelayError;
use lvqr_auth::{AuthContext, AuthDecision, NoopAuthProvider, SharedAuth};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Configuration for the relay server.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Address to bind the QUIC/WebTransport listener.
    pub bind_addr: SocketAddr,
    /// Hostnames for self-signed TLS cert generation.
    /// If empty, "localhost" is used.
    pub tls_hostnames: Vec<String>,
}

impl RelayConfig {
    pub fn new(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            tls_hostnames: vec!["localhost".to_string()],
        }
    }
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            bind_addr: ([0, 0, 0, 0], 4443).into(),
            tls_hostnames: vec!["localhost".to_string()],
        }
    }
}

/// Runtime statistics for the relay.
#[derive(Debug, Default)]
pub struct RelayMetrics {
    pub connections_total: AtomicU64,
    pub connections_active: AtomicU64,
}

/// Callback for connection lifecycle events.
/// Called with (connection_id, connected: true/false).
pub type ConnectionCallback = Arc<dyn Fn(u64, bool) + Send + Sync>;

/// The MoQ relay server.
///
/// Uses moq-native to accept WebTransport/QUIC connections and moq-lite's
/// Origin system for zero-copy track fanout.
#[cfg(feature = "quinn-transport")]
pub struct RelayServer {
    config: RelayConfig,
    origin: moq_lite::OriginProducer,
    metrics: Arc<RelayMetrics>,
    on_connection: Option<ConnectionCallback>,
    auth: SharedAuth,
}

#[cfg(feature = "quinn-transport")]
impl RelayServer {
    pub fn new(config: RelayConfig) -> Self {
        Self {
            config,
            origin: moq_lite::OriginProducer::new(),
            metrics: Arc::new(RelayMetrics::default()),
            on_connection: None,
            auth: Arc::new(NoopAuthProvider),
        }
    }

    /// Install an authentication provider. By default `NoopAuthProvider` is used.
    pub fn set_auth_provider(&mut self, auth: SharedAuth) {
        self.auth = auth;
    }

    /// Set a callback for connection lifecycle events.
    /// Called with (conn_id, true) on connect, (conn_id, false) on disconnect.
    pub fn set_connection_callback(&mut self, cb: ConnectionCallback) {
        self.on_connection = Some(cb);
    }

    /// Get the shared Origin for external track injection (e.g., RTMP ingest).
    pub fn origin(&self) -> &moq_lite::OriginProducer {
        &self.origin
    }

    /// Get relay metrics.
    pub fn metrics(&self) -> &Arc<RelayMetrics> {
        &self.metrics
    }

    /// Initialize and return the moq-native Server.
    pub fn init_server(&self) -> Result<(moq_native::Server, SocketAddr), RelayError> {
        let mut server_config = moq_native::ServerConfig::default();
        server_config.bind = Some(self.config.bind_addr);
        server_config.tls.generate = if self.config.tls_hostnames.is_empty() {
            vec!["localhost".to_string()]
        } else {
            self.config.tls_hostnames.clone()
        };

        let server = server_config
            .init()
            .map_err(|e| RelayError::Transport(format!("failed to init server: {e}")))?;

        let local_addr = server
            .local_addr()
            .map_err(|e| RelayError::Transport(format!("failed to get local addr: {e}")))?;

        Ok((server, local_addr))
    }

    /// Run the relay server. Blocks until the cancellation token fires.
    pub async fn run(&self, shutdown: CancellationToken) -> Result<(), RelayError> {
        let (mut server, local_addr) = self.init_server()?;
        info!(addr = %local_addr, "relay listening");

        self.accept_loop(&mut server, shutdown).await
    }

    /// Run the relay on a pre-initialized server until cancellation.
    pub async fn accept_loop(
        &self,
        server: &mut moq_native::Server,
        shutdown: CancellationToken,
    ) -> Result<(), RelayError> {
        let mut conn_id: u64 = 0;

        loop {
            tokio::select! {
                request = server.accept() => {
                    let Some(request) = request else { break };
                    conn_id += 1;
                    self.metrics.connections_total.fetch_add(1, Ordering::Relaxed);
                    metrics::counter!("lvqr_moq_connections_total").increment(1);

                    // Authentication: inspect the requested URL for a token query
                    // parameter and ask the auth provider whether to allow the
                    // session. Reject early via Request::close before the handshake.
                    let (token, broadcast) = parse_url_token(request.url());
                    let auth_decision = self.auth.check(&AuthContext::Subscribe {
                        token: token.clone(),
                        broadcast: broadcast.clone(),
                    });
                    if let AuthDecision::Deny { reason } = auth_decision {
                        warn!(conn = conn_id, reason = %reason, "rejecting unauthenticated session");
                        metrics::counter!("lvqr_auth_failures_total", "entry" => "moq").increment(1);
                        if let Err(e) = request.close(401).await {
                            debug!(error = %e, "request close failed");
                        }
                        continue;
                    }

                    self.metrics.connections_active.fetch_add(1, Ordering::Relaxed);
                    metrics::gauge!("lvqr_active_moq_sessions").increment(1.0);
                    let origin = self.origin.clone();
                    let metrics = self.metrics.clone();
                    let id = conn_id;
                    let on_conn = self.on_connection.clone();

                    if let Some(ref cb) = on_conn {
                        cb(id, true);
                    }

                    tokio::spawn(async move {
                        info!(conn = id, transport = request.transport(), "new connection");

                        let session_result = request.with_publish(origin.consume()).with_consume(origin).ok().await;

                        match session_result {
                            Ok(session) => {
                                if let Err(e) = session.closed().await {
                                    warn!(conn = id, error = %e, "session closed with error");
                                } else {
                                    info!(conn = id, "session closed");
                                }
                            }
                            Err(e) => {
                                error!(conn = id, error = %e, "failed to accept session");
                            }
                        }

                        metrics.connections_active.fetch_sub(1, Ordering::Relaxed);
                        ::metrics::gauge!("lvqr_active_moq_sessions").decrement(1.0);
                        if let Some(ref cb) = on_conn {
                            cb(id, false);
                        }
                    });
                }
                _ = shutdown.cancelled() => {
                    info!("relay shutdown signal received, draining connections");
                    server.close().await;
                    break;
                }
            }
        }

        Ok(())
    }
}

/// Extract the optional `token` query parameter and broadcast path from the
/// MoQ session URL. The MoQ session URL is typically of the form
/// `https://host:port/<broadcast>?token=<token>`.
#[cfg(feature = "quinn-transport")]
fn parse_url_token(url: Option<&url::Url>) -> (Option<String>, String) {
    let Some(url) = url else {
        return (None, String::new());
    };
    let broadcast = url.path().trim_start_matches('/').to_string();
    let token = url
        .query_pairs()
        .find(|(k, _)| k == "token")
        .map(|(_, v)| v.into_owned());
    (token, broadcast)
}

/// Stub server when quinn-transport feature is disabled.
#[cfg(not(feature = "quinn-transport"))]
pub struct RelayServer {
    config: RelayConfig,
}

#[cfg(not(feature = "quinn-transport"))]
impl RelayServer {
    pub fn new(config: RelayConfig) -> Self {
        let _ = config;
        Self { config }
    }

    pub async fn run(&self, _shutdown: CancellationToken) -> Result<(), RelayError> {
        Err(RelayError::Transport(
            "no transport backend enabled (enable quinn-transport feature)".to_string(),
        ))
    }
}

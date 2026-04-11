use crate::error::RelayError;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{error, info, warn};

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
}

#[cfg(feature = "quinn-transport")]
impl RelayServer {
    pub fn new(config: RelayConfig) -> Self {
        Self {
            config,
            origin: moq_lite::OriginProducer::new(),
            metrics: Arc::new(RelayMetrics::default()),
            on_connection: None,
        }
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

    /// Run the relay server. Blocks until shutdown.
    pub async fn run(&self) -> Result<(), RelayError> {
        let (mut server, local_addr) = self.init_server()?;
        info!(addr = %local_addr, "relay listening");

        self.accept_loop(&mut server).await
    }

    /// Run the relay on a pre-initialized server.
    pub async fn accept_loop(&self, server: &mut moq_native::Server) -> Result<(), RelayError> {
        let mut conn_id: u64 = 0;

        while let Some(request) = server.accept().await {
            conn_id += 1;
            self.metrics.connections_total.fetch_add(1, Ordering::Relaxed);
            self.metrics.connections_active.fetch_add(1, Ordering::Relaxed);

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
                if let Some(ref cb) = on_conn {
                    cb(id, false);
                }
            });
        }

        Ok(())
    }
}

/// Stub server when quinn-transport feature is disabled.
#[cfg(not(feature = "quinn-transport"))]
pub struct RelayServer {
    config: RelayConfig,
}

#[cfg(not(feature = "quinn-transport"))]
impl RelayServer {
    pub fn new(config: RelayConfig) -> Self {
        Self { config }
    }

    pub async fn run(&self) -> Result<(), RelayError> {
        Err(RelayError::Transport(
            "no transport backend enabled (enable quinn-transport feature)".to_string(),
        ))
    }
}

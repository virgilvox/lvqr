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

/// The MoQ relay server.
///
/// Uses moq-native to accept WebTransport/QUIC connections and moq-lite's
/// Origin system for zero-copy track fanout. Publishers and subscribers
/// connect to the same Origin; moq-lite handles all data forwarding internally.
///
/// The relay is a thin connection manager. It does NOT parse or copy media data.
/// Data flows through ref-counted `bytes::Bytes` buffers inside moq-lite.
#[cfg(feature = "quinn-transport")]
pub struct RelayServer {
    config: RelayConfig,
    /// Shared Origin: publishers write tracks into this,
    /// subscribers read tracks from it. moq-lite routes everything.
    origin: moq_lite::OriginProducer,
    metrics: Arc<RelayMetrics>,
}

#[cfg(feature = "quinn-transport")]
impl RelayServer {
    pub fn new(config: RelayConfig) -> Self {
        Self {
            config,
            origin: moq_lite::OriginProducer::new(),
            metrics: Arc::new(RelayMetrics::default()),
        }
    }

    /// Get the shared Origin for external track injection (e.g., RTMP ingest).
    ///
    /// An RTMP ingest module can create broadcasts and tracks on this Origin,
    /// and they will be available to all MoQ subscribers automatically.
    pub fn origin(&self) -> &moq_lite::OriginProducer {
        &self.origin
    }

    /// Get relay metrics.
    pub fn metrics(&self) -> &Arc<RelayMetrics> {
        &self.metrics
    }

    /// Initialize and return the moq-native Server.
    ///
    /// Returns the server and the local address it is bound to.
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
    /// Useful for tests where you need the server and local addr before running.
    pub async fn accept_loop(&self, server: &mut moq_native::Server) -> Result<(), RelayError> {
        let mut conn_id: u64 = 0;

        while let Some(request) = server.accept().await {
            conn_id += 1;
            self.metrics.connections_total.fetch_add(1, Ordering::Relaxed);
            self.metrics.connections_active.fetch_add(1, Ordering::Relaxed);

            let origin = self.origin.clone();
            let metrics = self.metrics.clone();
            let id = conn_id;

            tokio::spawn(async move {
                info!(conn = id, transport = request.transport(), "new connection");

                // The publish/subscribe swap: from the relay's perspective,
                // we "publish" what the client wants to subscribe to (OriginConsumer),
                // and we "consume" what the client wants to publish (OriginProducer).
                //
                // Both publishers and subscribers use the same Origin.
                // moq-lite internally routes ANNOUNCE/SUBSCRIBE between them.
                let session_result = request.with_publish(origin.consume()).with_consume(origin).ok().await;

                match session_result {
                    Ok(session) => {
                        // Hold the session open until the client disconnects.
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

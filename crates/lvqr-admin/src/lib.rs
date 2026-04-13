pub mod routes;

pub use routes::{AdminError, AdminState, MeshState, MetricsRender, StreamInfo, build_router};

/// Configuration for the admin HTTP server.
#[derive(Debug, Clone)]
pub struct AdminConfig {
    /// Port to listen on (default: 8080).
    pub port: u16,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self { port: 8080 }
    }
}

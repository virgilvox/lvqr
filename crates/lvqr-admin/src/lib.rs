pub mod config_reload_routes;
pub mod routes;
pub mod slo;
pub mod streamkey_routes;

#[cfg(feature = "cluster")]
pub mod cluster_routes;

pub use config_reload_routes::{ConfigReloadFuture, ConfigReloadStatus, ConfigReloadStatusFn, ConfigReloadTriggerFn};
pub use routes::{
    AdminError, AdminState, MeshPeerStats, MeshState, MetricsRender, StreamInfo, WasmFilterBroadcastStats,
    WasmFilterSlotStats, WasmFilterState, build_router,
};
pub use slo::{LatencyTracker, SloEntry};
pub use streamkey_routes::StreamKeyList;

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

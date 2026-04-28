//! HTTP API, stats, health, and SLO surface for LVQR.
//!
//! Composes the `/api/v1/*` admin router off [`AdminState`] +
//! [`build_router`]. Route trees mounted today (12 total):
//! `/healthz`, `/api/v1/{stats,streams,mesh,slo,wasm-filter}`, the
//! cluster-gated `/api/v1/cluster/{nodes,broadcasts,config,federation}`,
//! `/api/v1/streamkeys/*` (session 146), and
//! `/api/v1/config-reload` (sessions 147-149).
//!
//! ## Latency SLO
//!
//! [`LatencyTracker`] is the per-broadcast ring buffer behind both
//! `GET /api/v1/slo` (snapshot of p50/p95/p99/max) and the
//! `lvqr_subscriber_glass_to_glass_ms` Prometheus histogram. Every
//! egress crate (`lvqr-hls`, `lvqr-dash`, `lvqr-whep`, the WS relay
//! in `lvqr-cli`) records one sample per delivered fragment. The
//! `POST /api/v1/slo/client-sample` route (session 156 follow-up,
//! see [`routes::ClientLatencySample`]) accepts client-pushed
//! latency from any subscriber under dual-auth (admin OR
//! per-broadcast subscribe scope).
//!
//! ## Hot config reload
//!
//! [`ConfigReloadStatus`] surfaces SIGHUP / `POST /api/v1/config-reload`
//! results (last applied path, applied_keys diff, warnings).
//! `lvqr-cli` builds the [`ConfigReloadStatusFn`] +
//! [`ConfigReloadTriggerFn`] closures that thread the swap through
//! the auth chain, mesh ICE list, HMAC playback secret, JWKS / webhook
//! URLs.

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

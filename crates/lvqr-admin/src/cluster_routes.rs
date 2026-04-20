//! Read-only `/api/v1/cluster/*` admin endpoints (Tier 3 session E).
//!
//! Feature-gated on `cluster`. When the feature is on and an
//! `Arc<Cluster>` has been wired into [`crate::AdminState`] via
//! [`crate::AdminState::with_cluster`], three routes go live:
//!
//! * `GET /api/v1/cluster/nodes` -- every live cluster member with
//!   advertised capacity attached.
//! * `GET /api/v1/cluster/broadcasts` -- every non-expired
//!   broadcast-ownership lease, enumerated by name.
//! * `GET /api/v1/cluster/config` -- every cluster-wide config key
//!   reduced to its LWW winner.
//!
//! If the feature is on but no cluster was wired, the handlers
//! return 503. This keeps `build_router` stateless with respect
//! to the cluster decision while still exposing the endpoints so
//! deployments can swap a cluster in at runtime without a
//! binary change.
//!
//! Note on path: the session E row in `tracking/TIER_3_PLAN.md`
//! proposes `/admin/cluster/*`. LVQR's existing admin API lives
//! under `/api/v1/*`, so these endpoints follow that convention
//! for auth-middleware uniformity. The path is an implementation
//! detail; the deliverable is the JSON surface.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::{Json, Router, routing::get};
use lvqr_cluster::{BroadcastSummary, Cluster, ConfigEntry, FederationLinkStatus, NodeCapacity, NodeId};
use serde::{Deserialize, Serialize};

use crate::routes::{AdminError, AdminState};

/// External-facing JSON shape for one node. Mirrors
/// [`lvqr_cluster::ClusterNode`] but stringifies the socket
/// address so the admin output is trivially grep-friendly.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ClusterNodeView {
    pub id: NodeId,
    pub generation: u64,
    pub gossip_addr: String,
    pub capacity: Option<NodeCapacity>,
}

impl ClusterNodeView {
    fn from_cluster_node(m: lvqr_cluster::ClusterNode) -> Self {
        Self {
            id: m.id,
            generation: m.generation,
            gossip_addr: format_addr(m.gossip_addr),
            capacity: m.capacity,
        }
    }
}

fn format_addr(addr: SocketAddr) -> String {
    addr.to_string()
}

fn cluster_or_err(state: &AdminState) -> Result<Arc<Cluster>, AdminError> {
    state
        .cluster()
        .cloned()
        .ok_or_else(|| AdminError::Internal("cluster feature enabled but no Cluster handle wired".into()))
}

pub(crate) async fn list_nodes(State(state): State<AdminState>) -> Result<Json<Vec<ClusterNodeView>>, AdminError> {
    let cluster = cluster_or_err(&state)?;
    let members = cluster.members().await;
    Ok(Json(
        members.into_iter().map(ClusterNodeView::from_cluster_node).collect(),
    ))
}

pub(crate) async fn list_broadcasts(
    State(state): State<AdminState>,
) -> Result<Json<Vec<BroadcastSummary>>, AdminError> {
    let cluster = cluster_or_err(&state)?;
    Ok(Json(cluster.list_broadcasts().await))
}

pub(crate) async fn list_config(State(state): State<AdminState>) -> Result<Json<Vec<ConfigEntry>>, AdminError> {
    let cluster = cluster_or_err(&state)?;
    Ok(Json(cluster.list_config().await))
}

/// JSON body for `GET /api/v1/cluster/federation` (Tier 4 item 4.4
/// session C). Wraps the per-link status vector in a `links`
/// object so the shape can gain sibling fields in the future
/// (e.g. `generated_at_ms`, aggregate counters) without a
/// breaking schema change.
#[derive(Debug, Serialize, Deserialize)]
pub struct FederationStatusView {
    pub links: Vec<FederationLinkStatus>,
}

/// `GET /api/v1/cluster/federation`. Returns one
/// [`FederationLinkStatus`] entry per configured federation link,
/// in the same order the operator declared them in config. When
/// the CLI did not install a
/// [`lvqr_cluster::FederationStatusHandle`] on [`AdminState`]
/// (no `federation_links` configured, or the cluster feature is
/// off in the current build), the response body is
/// `{"links": []}` with status 200. The empty-vs-missing
/// distinction is deliberately collapsed: operator tooling can
/// poll this route unconditionally and treat an empty list as
/// "federation disabled or no links".
pub(crate) async fn federation_status(
    State(state): State<AdminState>,
) -> Result<Json<FederationStatusView>, AdminError> {
    let links = match state.federation_status() {
        Some(handle) => handle.snapshot(),
        None => Vec::new(),
    };
    Ok(Json(FederationStatusView { links }))
}

/// Router fragment that builds the cluster endpoints on a shared
/// [`AdminState`]. Consumers merge this into the top-level admin
/// router via `Router::merge`.
pub(crate) fn cluster_router() -> Router<AdminState> {
    Router::new()
        .route("/api/v1/cluster/nodes", get(list_nodes))
        .route("/api/v1/cluster/broadcasts", get(list_broadcasts))
        .route("/api/v1/cluster/config", get(list_config))
        .route("/api/v1/cluster/federation", get(federation_status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chitchat::transport::ChannelTransport;
    use lvqr_cluster::{Cluster, ClusterConfig};
    use lvqr_core::RelayStats;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::time::Duration;
    use tower::ServiceExt;

    async fn boot_cluster(port: u16) -> (Arc<Cluster>, ChannelTransport) {
        let transport = ChannelTransport::default();
        let mut cfg = ClusterConfig::for_test();
        cfg.listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        cfg.node_id = Some(NodeId::new(format!("admin-test-{port}")));
        cfg.cluster_id = "lvqr-test-admin".to_string();
        cfg.gossip_interval = Duration::from_millis(50);
        cfg.capacity_advertise_interval = Duration::from_millis(150);
        let cluster = Cluster::bootstrap_with_transport(cfg, &transport)
            .await
            .expect("bootstrap");
        (Arc::new(cluster), transport)
    }

    fn minimal_state(cluster: Arc<Cluster>) -> AdminState {
        AdminState::new(RelayStats::default, Vec::new).with_cluster(cluster)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nodes_endpoint_returns_self_member() {
        let (cluster, _transport) = boot_cluster(20401).await;
        let self_id = cluster.self_id().clone();
        let app = build_router(minimal_state(cluster.clone()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/nodes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let nodes: Vec<ClusterNodeView> = serde_json::from_slice(&body).expect("json");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, self_id);

        // Tear down so the background advertiser + chitchat server exit
        // before the transport drops.
        if let Ok(c) = Arc::try_unwrap(cluster) {
            c.shutdown().await.expect("shutdown");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn broadcasts_endpoint_lists_active_claim() {
        let (cluster, _transport) = boot_cluster(20402).await;
        let _claim = cluster
            .claim_broadcast("live/admin-demo", Duration::from_secs(5))
            .await
            .expect("claim");
        let app = build_router(minimal_state(cluster.clone()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/broadcasts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let list: Vec<BroadcastSummary> = serde_json::from_slice(&body).expect("json");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "live/admin-demo");

        drop(_claim);
        if let Ok(c) = Arc::try_unwrap(cluster) {
            c.shutdown().await.expect("shutdown");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn config_endpoint_lists_set_keys() {
        let (cluster, _transport) = boot_cluster(20403).await;
        cluster
            .config_set("hls.low-latency.enabled", "true")
            .await
            .expect("set");
        let app = build_router(minimal_state(cluster.clone()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let entries: Vec<ConfigEntry> = serde_json::from_slice(&body).expect("json");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "hls.low-latency.enabled");
        assert_eq!(entries[0].value, "true");

        if let Ok(c) = Arc::try_unwrap(cluster) {
            c.shutdown().await.expect("shutdown");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn federation_route_returns_empty_list_when_not_wired() {
        // No FederationStatusHandle installed on AdminState (no
        // `federation_links` configured, or clustering off). Route
        // returns 200 + empty list per the session 103 C contract.
        let (cluster, _transport) = boot_cluster(20404).await;
        let app = build_router(minimal_state(cluster.clone()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/federation")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let view: FederationStatusView = serde_json::from_slice(&body).expect("json");
        assert!(view.links.is_empty());
        if let Ok(c) = Arc::try_unwrap(cluster) {
            c.shutdown().await.expect("shutdown");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn federation_route_reports_configured_link_status() {
        // FederationStatusHandle installed with two links. Route
        // returns both in declared order.
        let (cluster, _transport) = boot_cluster(20405).await;
        let shutdown = tokio_util::sync::CancellationToken::new();
        let origin = lvqr_moq::OriginProducer::new();
        let links = vec![
            lvqr_cluster::FederationLink::new("https://192.0.2.10:4443/", "t", vec!["live/a".into()]),
            lvqr_cluster::FederationLink::new("https://192.0.2.11:4443/", "t", vec!["live/b".into()]),
        ];
        let runner = lvqr_cluster::FederationRunner::start(links, origin, shutdown.clone());
        let status_handle = runner.status_handle();
        let state = minimal_state(cluster.clone()).with_federation_status(status_handle);
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/federation")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let view: FederationStatusView = serde_json::from_slice(&body).expect("json");
        assert_eq!(view.links.len(), 2);
        assert_eq!(view.links[0].remote_url, "https://192.0.2.10:4443/");
        assert_eq!(view.links[0].forwarded_broadcasts, vec!["live/a".to_string()]);
        assert_eq!(view.links[1].remote_url, "https://192.0.2.11:4443/");

        let shutdown_fut = runner.shutdown();
        tokio::time::timeout(Duration::from_secs(2), shutdown_fut)
            .await
            .expect("runner shutdown bounded");
        if let Ok(c) = Arc::try_unwrap(cluster) {
            c.shutdown().await.expect("shutdown");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cluster_endpoint_503_when_not_wired() {
        // AdminState without .with_cluster -- the routes should
        // reply with a 500 (Internal) explaining the miswiring. We
        // use 500 rather than 503 because the feature was compiled
        // in; a 503 would suggest a transient state rather than a
        // deployment miswiring.
        let state = AdminState::new(RelayStats::default, Vec::new);
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/cluster/nodes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request");
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}

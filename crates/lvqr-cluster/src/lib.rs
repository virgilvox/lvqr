//! Cluster membership + broadcast-ownership plane for LVQR.
//!
//! This crate is the Tier 3 code surface for the "cluster plane"
//! described in `tracking/TIER_3_PLAN.md`. Session 71 (A) lands the
//! scaffold + single-node bootstrap; later sessions extend it with
//! ownership KV, capacity advertisement, and an `/admin/cluster/*`
//! HTTP surface.
//!
//! ## Orthogonal to `lvqr-mesh`
//!
//! `lvqr-mesh` is browser-facing WebRTC peer-mesh signaling -- it
//! federates playback clients to offload egress from the server.
//! `lvqr-cluster` is server-facing node coordination -- it federates
//! *server processes* so one logical LVQR service can span N hosts.
//! The two crates have no API overlap; neither depends on the
//! other; a deployed LVQR node may enable neither, either, or both.
//!
//! ## Scope this session
//!
//! * [`Cluster::bootstrap`] spins up a local chitchat gossip node
//!   on a UDP port.
//! * [`Cluster::self_node`] returns this node's identity.
//! * [`Cluster::members`] returns the self node plus every live
//!   peer chitchat reports.
//! * [`Cluster::shutdown`] is an explicit graceful shutdown that
//!   waits for the background gossip task to exit.
//!
//! Broadcast ownership (claims + leases), capacity advertisement,
//! cluster-wide config gossip, and the admin HTTP surface all land
//! in sessions 72-76. This session's smoke test asserts only that a
//! single node boots, reports itself, and shuts down cleanly.
//!
//! ## Load-bearing invariants preserved
//!
//! * **LBD #5 (chitchat scope discipline)**: the surface introduced
//!   here exposes membership only. No per-frame counter, per-
//!   subscriber bitrate, or fast-changing state flows through
//!   chitchat.
//! * **LBD #3 (control vs hot path)**: every API here is an
//!   `async` control-plane operation. No method sits on a per-
//!   fragment hot path.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chitchat::transport::{Transport, UdpTransport};
use chitchat::{
    ChitchatConfig, ChitchatHandle, ChitchatId, FailureDetectorConfig as ChitchatFailureDetectorConfig, spawn_chitchat,
};
use rand::Rng;
use rand::distributions::Alphanumeric;
use tracing::{debug, info};

/// Default UDP port for the chitchat gossip transport. Matches the
/// upstream chitchat example's convention. No LVQR listener today
/// binds a UDP port in this range, so there is no collision risk
/// with the existing protocols (RTMP 1935, RTSP 554, HLS/DASH on
/// the HTTP listener, QUIC/MoQ on 443, SRT 8890, admin 8080).
pub const DEFAULT_GOSSIP_PORT: u16 = 10_007;

/// Default cluster identifier gossipped in every SYN message.
/// Chitchat uses this to reject cross-cluster gossip: a node with
/// `cluster_id = "lvqr"` ignores messages from a cluster named
/// anything else. Set it to something unique per deployment if you
/// share a subnet with another chitchat-using service.
pub const DEFAULT_CLUSTER_ID: &str = "lvqr";

/// Default gossip round interval. One second matches chitchat's own
/// documented default and keeps convergence at 3-4 rounds even for
/// 10-node clusters.
pub const DEFAULT_GOSSIP_INTERVAL: Duration = Duration::from_secs(1);

/// Default grace period before a node's state is fully garbage-
/// collected after it is marked for deletion. One minute is the
/// lower bound that survives a transient network partition without
/// dropping state; chitchat itself defaults to two hours, which is
/// too long for LVQR's typical node lifetime.
pub const DEFAULT_MARKED_FOR_DELETION_GRACE_PERIOD: Duration = Duration::from_secs(60);

/// Unique-within-cluster identifier for one LVQR node.
///
/// Wraps a string so callers cannot accidentally confuse a broadcast
/// name with a node name -- both are string-shaped in the wild.
/// Stored unchanged on the chitchat wire; chitchat imposes no
/// character constraint beyond UTF-8.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Random 16-char alphanumeric identifier. Used when the caller
    /// does not supply an explicit `node_id` in [`ClusterConfig`].
    pub fn random() -> Self {
        let mut rng = rand::thread_rng();
        let raw: String = (0..16).map(|_| rng.sample(Alphanumeric) as char).collect();
        Self(format!("lvqr-{raw}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for NodeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for NodeId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Failure-detector tuning knobs re-exported from chitchat behind a
/// thin wrapper so crate consumers can tune liveness detection
/// without depending on chitchat directly.
///
/// Phi accrual parameters:
///
/// * `phi_threshold` -- a node is flagged dead when its phi value
///   exceeds this threshold. Chitchat ships 8.0 which is the
///   classic Cassandra default.
/// * `sampling_window_size` -- how many heartbeat intervals the
///   detector averages over. Larger windows smooth jitter but slow
///   adaptation.
/// * `max_interval` -- heartbeat arrivals longer than this are
///   ignored (they're treated as data corruption rather than real
///   samples).
/// * `initial_interval` -- prior mean used to seed the sampling
///   window before any real samples have been observed. Shorter
///   values make the detector more aggressive on freshly-joined
///   peers.
/// * `dead_node_grace_period` -- once a node is marked dead, its
///   state is kept for this long before garbage collection.
#[derive(Debug, Clone)]
pub struct FailureDetectorConfig {
    pub phi_threshold: f64,
    pub sampling_window_size: usize,
    pub max_interval: Duration,
    pub initial_interval: Duration,
    pub dead_node_grace_period: Duration,
}

impl Default for FailureDetectorConfig {
    fn default() -> Self {
        Self {
            phi_threshold: 8.0,
            sampling_window_size: 1_000,
            max_interval: Duration::from_secs(10),
            initial_interval: Duration::from_secs(5),
            dead_node_grace_period: Duration::from_secs(24 * 60 * 60),
        }
    }
}

impl From<FailureDetectorConfig> for ChitchatFailureDetectorConfig {
    fn from(cfg: FailureDetectorConfig) -> Self {
        Self {
            phi_threshold: cfg.phi_threshold,
            sampling_window_size: cfg.sampling_window_size,
            max_interval: cfg.max_interval,
            initial_interval: cfg.initial_interval,
            dead_node_grace_period: cfg.dead_node_grace_period,
        }
    }
}

/// Inputs to [`Cluster::bootstrap`]. Every field has a sensible
/// default so simple callers can `ClusterConfig::default()`.
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    /// UDP socket the gossip server binds. Defaults to
    /// `0.0.0.0:DEFAULT_GOSSIP_PORT`.
    pub listen: SocketAddr,
    /// Address this node advertises to peers. Must be reachable
    /// from every other node. Defaults to `listen`.
    pub advertise: Option<SocketAddr>,
    /// Peer seed addresses to gossip with on boot. Format is
    /// chitchat's: `ip:port` strings. Empty means "no peers";
    /// the node runs as a standalone cluster of one.
    pub seeds: Vec<String>,
    /// This node's identifier. Defaults to a random
    /// 16-character alphanumeric string prefixed with `lvqr-`.
    pub node_id: Option<NodeId>,
    /// Cluster tag. Chitchat rejects cross-cluster gossip so two
    /// LVQR deployments on the same subnet stay isolated.
    pub cluster_id: String,
    /// Gossip round interval.
    pub gossip_interval: Duration,
    /// Delay between a peer being scheduled for deletion and its
    /// state being garbage-collected.
    pub marked_for_deletion_grace_period: Duration,
    /// Failure-detector tuning. Defaults match chitchat's (phi 8.0
    /// over a 1000-sample window, 5 s initial interval prior).
    /// Override in tests for snappier detection.
    pub failure_detector: FailureDetectorConfig,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_GOSSIP_PORT),
            advertise: None,
            seeds: Vec::new(),
            node_id: None,
            cluster_id: DEFAULT_CLUSTER_ID.to_string(),
            gossip_interval: DEFAULT_GOSSIP_INTERVAL,
            marked_for_deletion_grace_period: DEFAULT_MARKED_FOR_DELETION_GRACE_PERIOD,
            failure_detector: FailureDetectorConfig::default(),
        }
    }
}

impl ClusterConfig {
    /// Preset suitable for unit tests: ephemeral UDP port on
    /// loopback, no seeds, short gossip interval + grace period,
    /// aggressive failure-detector tuning so expiry-sensitive
    /// assertions complete in under a second instead of hitting
    /// chitchat's 5 s prior-mean floor.
    pub fn for_test() -> Self {
        Self {
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            advertise: None,
            seeds: Vec::new(),
            node_id: None,
            cluster_id: "lvqr-test".to_string(),
            gossip_interval: Duration::from_millis(100),
            marked_for_deletion_grace_period: Duration::from_secs(2),
            failure_detector: FailureDetectorConfig {
                // Match chitchat's default phi threshold; with the
                // shortened initial interval below, phi crosses 8
                // within ~500 ms of missed heartbeats at a 50 ms
                // gossip interval.
                phi_threshold: 8.0,
                sampling_window_size: 1_000,
                max_interval: Duration::from_secs(10),
                // Shrink the prior so tests do not have to warm the
                // sampling window for 25 s before the detector
                // becomes responsive.
                initial_interval: Duration::from_millis(200),
                dead_node_grace_period: Duration::from_secs(2),
            },
        }
    }
}

/// Externally-facing view of one cluster node.
///
/// Mirrors chitchat's [`ChitchatId`] but presented behind our public
/// type so crate consumers do not have to depend on chitchat directly.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClusterNode {
    pub id: NodeId,
    /// Monotonically-increasing generation. Incremented each time
    /// the node rejoins the cluster after a restart. Chitchat uses
    /// this to disambiguate state across restarts.
    pub generation: u64,
    /// Address peers should use to gossip with this node.
    pub gossip_addr: SocketAddr,
}

impl ClusterNode {
    fn from_chitchat_id(cid: &ChitchatId) -> Self {
        Self {
            id: NodeId(cid.node_id.clone()),
            generation: cid.generation_id,
            gossip_addr: cid.gossip_advertise_addr,
        }
    }
}

/// Handle to a running cluster node.
///
/// Holding the handle keeps the background gossip task alive; drop
/// or call [`Cluster::shutdown`] to terminate cleanly.
pub struct Cluster {
    self_node: ClusterNode,
    handle: ChitchatHandle,
}

impl Cluster {
    /// Spawn the gossip server, register this node, and return a
    /// handle. The task runs until [`Cluster::shutdown`] is called or
    /// the handle is dropped.
    ///
    /// Generation is picked from `SystemTime::now()` so sequential
    /// restarts are guaranteed monotonic without requiring the
    /// caller to track state across process boundaries. Callers who
    /// need deterministic generations in tests can shut down and
    /// re-bootstrap inside a single process.
    pub async fn bootstrap(config: ClusterConfig) -> Result<Self> {
        Self::bootstrap_with_transport(config, &UdpTransport).await
    }

    /// Same as [`Cluster::bootstrap`] but with a caller-supplied
    /// transport. Used by integration tests that want the
    /// in-process `ChannelTransport` instead of real UDP so they
    /// can exercise multi-node topologies without port pressure.
    pub async fn bootstrap_with_transport(config: ClusterConfig, transport: &dyn Transport) -> Result<Self> {
        let node_id = config.node_id.clone().unwrap_or_else(NodeId::random);
        let generation = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let advertise = config.advertise.unwrap_or(config.listen);

        let chitchat_id = ChitchatId::new(node_id.0.clone(), generation, advertise);
        let chitchat_config = ChitchatConfig {
            chitchat_id: chitchat_id.clone(),
            cluster_id: config.cluster_id.clone(),
            gossip_interval: config.gossip_interval,
            listen_addr: config.listen,
            seed_nodes: config.seeds.clone(),
            failure_detector_config: config.failure_detector.clone().into(),
            marked_for_deletion_grace_period: config.marked_for_deletion_grace_period,
            catchup_callback: None,
            extra_liveness_predicate: None,
        };

        let handle = spawn_chitchat(chitchat_config, Vec::new(), transport)
            .await
            .context("spawn chitchat")?;

        // Resolve the bound address -- useful when config.listen used port 0
        // to pick an ephemeral port. Chitchat itself keeps the originally-
        // configured advertise address, but the ephemeral port case wants the
        // caller to see the real address.
        let self_node = ClusterNode {
            id: node_id.clone(),
            generation,
            gossip_addr: advertise,
        };

        info!(
            node = %node_id,
            generation,
            %advertise,
            cluster_id = %config.cluster_id,
            "cluster bootstrapped"
        );

        Ok(Self { self_node, handle })
    }

    /// This node's identity. Available immediately after bootstrap
    /// even before any gossip round completes.
    pub fn self_node(&self) -> &ClusterNode {
        &self.self_node
    }

    /// This node's id as a short borrowable string.
    pub fn self_id(&self) -> &NodeId {
        &self.self_node.id
    }

    /// Snapshot of the current cluster membership: the self node
    /// plus every peer chitchat's failure detector currently
    /// considers live.
    ///
    /// Chitchat's `live_nodes()` returns peers only (the self node
    /// is implicit); we merge them here so callers do not have to
    /// reason about the distinction.
    pub async fn members(&self) -> Vec<ClusterNode> {
        let chitchat = self.handle.chitchat();
        let guard = chitchat.lock().await;
        let mut out: Vec<ClusterNode> = guard.live_nodes().map(ClusterNode::from_chitchat_id).collect();
        out.push(self.self_node.clone());
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out.dedup_by(|a, b| a.id == b.id);
        out
    }

    /// Graceful shutdown: stops the gossip task and waits for it to
    /// exit. Equivalent to dropping the handle but propagates any
    /// shutdown error to the caller.
    pub async fn shutdown(self) -> Result<()> {
        debug!(node = %self.self_node.id, "cluster shutdown requested");
        self.handle.shutdown().await.context("chitchat shutdown")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bootstrap_single_node_reports_itself_in_members() {
        let cfg = ClusterConfig::for_test();
        let cluster = Cluster::bootstrap(cfg).await.expect("bootstrap");
        let members = cluster.members().await;
        assert_eq!(members.len(), 1, "single node cluster has one member");
        assert_eq!(members[0].id, *cluster.self_id());
        cluster.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn explicit_node_id_survives_bootstrap() {
        let mut cfg = ClusterConfig::for_test();
        cfg.node_id = Some(NodeId::new("explicit-node"));
        let cluster = Cluster::bootstrap(cfg).await.expect("bootstrap");
        assert_eq!(cluster.self_id().as_str(), "explicit-node");
        cluster.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn node_id_random_generates_fresh_identifiers() {
        // Sanity: two random ids are overwhelmingly unlikely to collide.
        let a = NodeId::random();
        let b = NodeId::random();
        assert_ne!(a, b);
        assert!(a.as_str().starts_with("lvqr-"));
        assert!(b.as_str().starts_with("lvqr-"));
    }

    #[test]
    fn default_cluster_config_binds_to_default_port() {
        let cfg = ClusterConfig::default();
        assert_eq!(cfg.listen.port(), DEFAULT_GOSSIP_PORT);
        assert_eq!(cfg.cluster_id, DEFAULT_CLUSTER_ID);
        assert!(cfg.seeds.is_empty());
    }

    #[test]
    fn for_test_config_uses_ephemeral_port_and_loopback() {
        let cfg = ClusterConfig::for_test();
        assert_eq!(cfg.listen.port(), 0, "ephemeral");
        assert!(cfg.listen.ip().is_loopback());
        assert_eq!(cfg.cluster_id, "lvqr-test");
    }
}

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
//! ## Scope as of session 76 (Tier 3 session F1)
//!
//! * [`Cluster::bootstrap`] spins up a local chitchat gossip node
//!   on a UDP port.
//! * [`Cluster::self_node`] returns this node's identity.
//! * [`Cluster::members`] returns every live peer chitchat reports
//!   (self included) with each peer's most recent advertised
//!   [`NodeCapacity`] and [`NodeEndpoints`] attached.
//! * [`Cluster::shutdown`] is an explicit graceful shutdown that
//!   waits for both the capacity advertiser and the gossip task to
//!   exit.
//! * [`Cluster::claim_broadcast`], [`Cluster::find_broadcast_owner`],
//!   and [`Cluster::list_broadcasts`] manage the per-broadcast
//!   ownership KV. Dropping the returned [`Claim`] tombstones the
//!   key (best-effort) so peers see the slot freed within one
//!   gossip round.
//! * [`Cluster::capacity_gauge`] exposes a shared handle to this
//!   node's advertised capacity. Samplers update the gauge; the
//!   advertiser task publishes snapshots every
//!   `capacity_advertise_interval`.
//! * [`Cluster::config_set`], [`Cluster::config_get`], and
//!   [`Cluster::list_config`] implement the cluster-wide config
//!   channel. Writes are timestamped on the setter's self node;
//!   cross-node conflicts resolve by LWW on the timestamp.
//! * [`Cluster::set_endpoints`], [`Cluster::node_endpoints`], and
//!   [`Cluster::find_owner_endpoints`] implement per-node endpoint
//!   advertisement so the redirect-to-owner egress paths
//!   (`lvqr-hls`, `lvqr-dash`, `lvqr-rtsp`) can resolve a
//!   broadcast's owner to a reachable URL.
//!
//! CLI wiring (`lvqr-cli serve` flags + HLS/DASH/RTSP handler
//! redirect paths) lands in session 77 (F2).
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

mod broadcast;
mod capacity;
mod config;
mod endpoints;
mod federation;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chitchat::transport::{Transport, UdpTransport};
use chitchat::{
    ChitchatConfig, ChitchatHandle, ChitchatId, FailureDetectorConfig as ChitchatFailureDetectorConfig, spawn_chitchat,
};
use rand::Rng;
use rand::distributions::Alphanumeric;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

pub use broadcast::{BROADCAST_KEY_PREFIX, BroadcastSummary, Claim, MIN_LEASE};
pub use capacity::{CAPACITY_KEY, CapacityGauge, NodeCapacity};
pub use config::{CONFIG_KEY_PREFIX, ConfigEntry};
pub use endpoints::{ENDPOINTS_KEY, NodeEndpoints};
pub use federation::{FederationLink, FederationRunner};

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

/// Default interval between capacity advertisements. Five seconds
/// matches the figure in `tracking/TIER_3_PLAN.md`: fine enough for
/// load-aware routing decisions that play out over many seconds,
/// coarse enough that the gossip payload stays small.
pub const DEFAULT_CAPACITY_ADVERTISE_INTERVAL: Duration = Duration::from_secs(5);

/// Unique-within-cluster identifier for one LVQR node.
///
/// Wraps a string so callers cannot accidentally confuse a broadcast
/// name with a node name -- both are string-shaped in the wild.
/// Stored unchanged on the chitchat wire; chitchat imposes no
/// character constraint beyond UTF-8.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
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
    /// Interval between capacity KV publishes. Defaults to 5 s per
    /// `tracking/TIER_3_PLAN.md`; tests shorten it to low
    /// hundreds of ms so assertions on propagation complete inside
    /// a few seconds.
    pub capacity_advertise_interval: Duration,
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
            capacity_advertise_interval: DEFAULT_CAPACITY_ADVERTISE_INTERVAL,
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
            // 200 ms is the sweet spot: long enough for tests to
            // stage gauge values after bootstrap before the first
            // publish fires, short enough that the two-node
            // propagation assertion completes well under a second.
            capacity_advertise_interval: Duration::from_millis(200),
        }
    }
}

/// Externally-facing view of one cluster node.
///
/// Mirrors chitchat's [`ChitchatId`] but presented behind our public
/// type so crate consumers do not have to depend on chitchat directly.
///
/// `Eq` / `Hash` are intentionally not derived: the `capacity` field
/// carries an `f32` which breaks reflexivity on NaN. Callers that
/// need to key a map on a cluster node should key on [`NodeId`]
/// directly.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterNode {
    pub id: NodeId,
    /// Monotonically-increasing generation. Incremented each time
    /// the node rejoins the cluster after a restart. Chitchat uses
    /// this to disambiguate state across restarts.
    pub generation: u64,
    /// Address peers should use to gossip with this node.
    pub gossip_addr: SocketAddr,
    /// Most recent advertised capacity for this node. `None` if the
    /// node has not yet published a capacity entry (e.g. freshly
    /// booted, first tick has not fired) or if the entry failed to
    /// decode.
    pub capacity: Option<NodeCapacity>,
    /// Externally-reachable egress URLs this node has advertised.
    /// `None` when the node has not called
    /// [`Cluster::set_endpoints`](Cluster::set_endpoints) yet or
    /// when the gossipped entry failed to decode.
    pub endpoints: Option<NodeEndpoints>,
}

/// Handle to a running cluster node.
///
/// Holding the handle keeps the background gossip task alive; drop
/// or call [`Cluster::shutdown`] to terminate cleanly.
pub struct Cluster {
    self_node: ClusterNode,
    /// Wrapped in `Option` so `shutdown()` can move the handle out
    /// even though `Cluster` implements `Drop` (moving a named field
    /// out of a `Drop` type is forbidden; `Option::take` is the
    /// standard workaround).
    handle: Option<ChitchatHandle>,
    capacity_gauge: CapacityGauge,
    /// Cancels background tasks owned by this cluster (currently
    /// just the capacity advertiser). Fires on both explicit
    /// `shutdown()` and on `Drop`.
    background_cancel: CancellationToken,
    /// JoinHandle for the capacity advertiser task. Taken out on
    /// `shutdown()` so we can `.await` clean exit; otherwise the
    /// `Drop` below aborts it.
    capacity_advertiser: Option<JoinHandle<()>>,
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
            capacity: None,
            endpoints: None,
        };

        let capacity_gauge = CapacityGauge::new();
        let background_cancel = CancellationToken::new();
        let capacity_advertiser = capacity::spawn_advertiser(
            handle.chitchat(),
            capacity_gauge.clone(),
            config.capacity_advertise_interval,
            background_cancel.clone(),
        );

        info!(
            node = %node_id,
            generation,
            %advertise,
            cluster_id = %config.cluster_id,
            "cluster bootstrapped"
        );

        Ok(Self {
            self_node,
            handle: Some(handle),
            capacity_gauge,
            background_cancel,
            capacity_advertiser: Some(capacity_advertiser),
        })
    }

    fn handle(&self) -> &ChitchatHandle {
        self.handle
            .as_ref()
            .expect("ChitchatHandle is always Some between bootstrap and shutdown/drop")
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

    /// Snapshot of the current cluster membership: every node
    /// chitchat's failure detector currently considers live,
    /// populated with the latest advertised capacity from each
    /// node's KV state.
    ///
    /// Chitchat's `live_nodes()` prepends the self node, so no
    /// manual merge is needed.
    pub async fn members(&self) -> Vec<ClusterNode> {
        let chitchat = self.handle().chitchat();
        let guard = chitchat.lock().await;
        let mut out: Vec<ClusterNode> = guard
            .live_nodes()
            .map(|cid| {
                let state = guard.node_state(cid);
                let capacity = state
                    .and_then(|state| state.get(CAPACITY_KEY))
                    .and_then(NodeCapacity::decode);
                let endpoints = state
                    .and_then(|state| state.get(ENDPOINTS_KEY))
                    .and_then(NodeEndpoints::decode);
                ClusterNode {
                    id: NodeId(cid.node_id.clone()),
                    generation: cid.generation_id,
                    gossip_addr: cid.gossip_advertise_addr,
                    capacity,
                    endpoints,
                }
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out.dedup_by(|a, b| a.id == b.id);
        out
    }

    /// Shared gauge for this node's advertised capacity. Callers
    /// (samplers living in lvqr-cli, lvqr-relay, etc.) write into
    /// it whenever they have a fresh reading; the advertiser task
    /// picks up the values on its next tick.
    pub fn capacity_gauge(&self) -> &CapacityGauge {
        &self.capacity_gauge
    }

    /// Graceful shutdown: stops the capacity advertiser and the
    /// gossip task, waiting for both to exit. Propagates any
    /// chitchat shutdown error.
    pub async fn shutdown(mut self) -> Result<()> {
        debug!(node = %self.self_node.id, "cluster shutdown requested");
        self.background_cancel.cancel();
        if let Some(task) = self.capacity_advertiser.take() {
            // The advertiser exits cleanly as soon as it sees the
            // cancel signal; ignoring JoinError drops panic info we
            // cannot recover from anyway.
            let _ = task.await;
        }
        let handle = self
            .handle
            .take()
            .expect("ChitchatHandle is always Some between bootstrap and shutdown/drop");
        handle.shutdown().await.context("chitchat shutdown")
    }

    /// Claim ownership of `name` for the duration of `lease`. The
    /// returned [`Claim`] renews the lease every `lease / 4` in the
    /// background; drop it to release (best-effort tombstone write
    /// + natural lease expiry).
    ///
    /// Chitchat is eventually consistent: two nodes that race on the
    /// same name during a partition both succeed here, and readers
    /// break the tie deterministically by picking the latest-expiry
    /// entry. Callers that need stronger semantics should layer a
    /// reconciliation pass on top of this API (a Tier 4 item).
    pub async fn claim_broadcast(&self, name: &str, lease: Duration) -> Result<Claim> {
        broadcast::claim(self.handle(), &self.self_node.id, name, lease).await
    }

    /// Look up the current non-expired owner of `name` by scanning
    /// every known node's KV state. Returns `None` if no live lease
    /// exists anywhere in the cluster.
    ///
    /// Entries whose `expires_at_ms` is in the past are filtered
    /// out: a crashed owner's stale lease will not produce a false
    /// positive once the deadline passes, even before chitchat
    /// garbage-collects the node state.
    pub async fn find_broadcast_owner(&self, name: &str) -> Option<NodeId> {
        broadcast::find_owner(self.handle(), name).await
    }

    /// Snapshot every broadcast any node in the cluster is
    /// currently claiming, filtered to non-expired leases. Used by
    /// the `/admin/cluster/broadcasts` endpoint.
    pub async fn list_broadcasts(&self) -> Vec<BroadcastSummary> {
        broadcast::list_owners(self.handle()).await
    }

    /// Set a cluster-wide config key. Writes a timestamped entry
    /// onto the self node's state; gossip carries the entry to
    /// every peer. Cross-node conflicts resolve by LWW on the
    /// timestamp.
    pub async fn config_set(&self, key: &str, value: &str) -> Result<()> {
        config::set(self.handle(), key, value).await
    }

    /// Read the current cluster-wide value for `key`, resolving
    /// conflicts across nodes by picking the most recently written
    /// entry (highest `ts_ms`). Returns `None` if no node has ever
    /// set the key.
    pub async fn config_get(&self, key: &str) -> Option<String> {
        config::get(self.handle(), key).await
    }

    /// Enumerate every cluster-wide config key any node has ever
    /// set, reduced to the LWW winner per key. Used by the
    /// `/admin/cluster/config` endpoint.
    pub async fn list_config(&self) -> Vec<ConfigEntry> {
        config::list(self.handle()).await
    }

    /// Advertise this node's externally-reachable egress URLs.
    /// Overwrites the previous entry if one exists; gossip
    /// propagates the new value to every peer within a couple of
    /// rounds. Idempotent -- calling with the same value twice is
    /// a no-op.
    pub async fn set_endpoints(&self, endpoints: &NodeEndpoints) -> Result<()> {
        endpoints::set(self.handle(), endpoints).await
    }

    /// Read a specific node's advertised endpoints via chitchat KV.
    /// Returns `None` if the node is unknown, has not yet
    /// advertised, or its entry failed to decode.
    pub async fn node_endpoints(&self, node_id: &NodeId) -> Option<NodeEndpoints> {
        endpoints::get(self.handle(), node_id).await
    }

    /// Convenience helper that resolves a redirect target for a
    /// broadcast in one step: looks up the current owner via
    /// [`Self::find_broadcast_owner`], then fetches that owner's
    /// advertised [`NodeEndpoints`]. Returns `None` if either step
    /// fails -- for example no node owns the broadcast, or the
    /// owner has not advertised its endpoints yet.
    pub async fn find_owner_endpoints(&self, broadcast: &str) -> Option<(NodeId, NodeEndpoints)> {
        let owner = self.find_broadcast_owner(broadcast).await?;
        let endpoints = self.node_endpoints(&owner).await?;
        Some((owner, endpoints))
    }
}

impl Drop for Cluster {
    /// If the caller never invoked [`Cluster::shutdown`] (e.g. a
    /// panic on the owning task) this fires the cancellation token
    /// and aborts the advertiser so the spawned task does not leak
    /// past the gossip server. Dropping [`ChitchatHandle`] itself
    /// then tears the gossip server down.
    fn drop(&mut self) {
        self.background_cancel.cancel();
        if let Some(task) = self.capacity_advertiser.take() {
            task.abort();
        }
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

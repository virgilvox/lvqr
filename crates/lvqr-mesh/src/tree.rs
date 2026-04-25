/// Peer relay tree data structures and algorithms.
///
/// The mesh organizes viewers into a relay tree rooted at the origin server.
/// Root peers connect directly to the server. Each peer relays to up to
/// `max_children` downstream peers. The tree self-balances to minimize
/// depth and maximize reliability.
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Unique peer identifier.
pub type PeerId = String;

/// Role of a peer in the relay tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerRole {
    /// Directly connected to the origin server.
    Root,
    /// Connected via another peer (relay node).
    Relay,
    /// Leaf node that does not relay to others (low bandwidth).
    Leaf,
}

/// Information about a peer in the mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Unique peer identifier.
    pub id: PeerId,
    /// Track this peer is watching.
    pub track: String,
    /// Role in the relay tree.
    pub role: PeerRole,
    /// Parent peer ID (None for root peers).
    pub parent: Option<PeerId>,
    /// Child peer IDs this peer relays to.
    pub children: Vec<PeerId>,
    /// Depth in the tree (0 = root, directly from server).
    pub depth: u32,
    /// Estimated upload bandwidth in kbps.
    pub upload_kbps: u32,
    /// Cumulative count of fragments this peer has forwarded to its
    /// DataChannel children, as reported by the client's periodic
    /// `ForwardReport` signal message. Session 141 -- actual-vs-
    /// intended offload reporting. `#[serde(default)]` so pre-141
    /// tree snapshots still deserialize.
    #[serde(default)]
    pub forwarded_frames: u64,
    /// Self-reported relay capacity (max children this peer is
    /// willing to serve). `None` means "use the operator's global
    /// `MeshConfig.max_children`". Clamped to `[0, max_children]` at
    /// register time by the lvqr-cli signal bridge so on-disk values
    /// are always within the operator's ceiling. Session 144.
    #[serde(default)]
    pub capacity: Option<u32>,
    /// When this peer joined.
    #[serde(skip)]
    pub joined_at: Option<Instant>,
    /// Last heartbeat received.
    #[serde(skip)]
    pub last_heartbeat: Option<Instant>,
}

impl PeerInfo {
    pub fn new(id: PeerId, track: String) -> Self {
        Self {
            id,
            track,
            role: PeerRole::Leaf,
            parent: None,
            children: Vec::new(),
            depth: 0,
            upload_kbps: 5000, // default 5 Mbps
            forwarded_frames: 0,
            capacity: None,
            joined_at: Some(Instant::now()),
            last_heartbeat: Some(Instant::now()),
        }
    }

    /// Whether this peer can accept more children given the operator's
    /// configured ceiling. Consults the per-peer
    /// [`PeerInfo::capacity`] when set, otherwise falls back to
    /// `default_max`. The capacity is also clamped against
    /// `default_max`, so a misbehaving client claim cannot exceed
    /// the operator's ceiling here even if it slipped past
    /// register-time clamping.
    pub fn can_accept_child(&self, default_max: usize) -> bool {
        self.children.len() < self.effective_capacity(default_max)
    }

    /// Effective per-peer capacity: the operator's ceiling
    /// (`default_max`) by default, reduced to whatever the peer
    /// self-reported via [`PeerInfo::capacity`] when that value is
    /// smaller. The min ensures no capacity ever exceeds the
    /// operator's ceiling. Session 144.
    pub fn effective_capacity(&self, default_max: usize) -> usize {
        self.capacity
            .map(|c| (c as usize).min(default_max))
            .unwrap_or(default_max)
    }

    /// Number of children this peer is relaying to.
    pub fn child_count(&self) -> usize {
        self.children.len()
    }
}

/// Assignment result when a new peer joins the mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerAssignment {
    /// The assigned peer ID.
    pub peer_id: PeerId,
    /// Role assigned.
    pub role: PeerRole,
    /// Parent to connect to (None = connect to server directly).
    pub parent: Option<PeerId>,
    /// Depth in the tree.
    pub depth: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_info_defaults() {
        let peer = PeerInfo::new("peer-1".into(), "live/test".into());
        assert_eq!(peer.id, "peer-1");
        assert_eq!(peer.role, PeerRole::Leaf);
        assert!(peer.parent.is_none());
        assert!(peer.children.is_empty());
        assert_eq!(peer.depth, 0);
        assert_eq!(peer.upload_kbps, 5000);
        assert_eq!(peer.forwarded_frames, 0);
        assert!(peer.capacity.is_none());
    }

    #[test]
    fn can_accept_child() {
        let mut peer = PeerInfo::new("peer-1".into(), "live/test".into());
        assert!(peer.can_accept_child(3));
        peer.children.push("child-1".into());
        peer.children.push("child-2".into());
        peer.children.push("child-3".into());
        assert!(!peer.can_accept_child(3));
    }

    #[test]
    fn effective_capacity_uses_global_when_unset() {
        let peer = PeerInfo::new("peer-1".into(), "live/test".into());
        assert_eq!(peer.effective_capacity(3), 3);
        assert_eq!(peer.effective_capacity(0), 0);
    }

    #[test]
    fn effective_capacity_clamps_oversize_claim() {
        let mut peer = PeerInfo::new("peer-1".into(), "live/test".into());
        peer.capacity = Some(u32::MAX);
        assert_eq!(peer.effective_capacity(5), 5);
    }

    #[test]
    fn effective_capacity_uses_smaller_of_claim_and_default() {
        let mut peer = PeerInfo::new("peer-1".into(), "live/test".into());
        peer.capacity = Some(2);
        assert_eq!(peer.effective_capacity(5), 2);
        assert_eq!(peer.effective_capacity(1), 1);
    }

    #[test]
    fn can_accept_child_respects_capacity_zero() {
        let mut peer = PeerInfo::new("peer-1".into(), "live/test".into());
        peer.capacity = Some(0);
        assert!(
            !peer.can_accept_child(10),
            "capacity=0 must reject children even with default_max=10"
        );
    }

    #[test]
    fn can_accept_child_respects_per_peer_cap_below_default() {
        let mut peer = PeerInfo::new("peer-1".into(), "live/test".into());
        peer.capacity = Some(1);
        assert!(peer.can_accept_child(5));
        peer.children.push("child-1".into());
        assert!(
            !peer.can_accept_child(5),
            "peer with capacity=1 must refuse a second child even when default_max=5"
        );
    }
}

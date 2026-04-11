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
            joined_at: Some(Instant::now()),
            last_heartbeat: Some(Instant::now()),
        }
    }

    /// Whether this peer can accept more children.
    pub fn can_accept_child(&self, max_children: usize) -> bool {
        self.children.len() < max_children
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
}

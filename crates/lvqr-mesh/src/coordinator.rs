use dashmap::DashMap;
use lvqr_core::types::SubscriberId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Configuration for the peer mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshConfig {
    /// Maximum number of children each peer can relay to.
    pub max_children: usize,
    /// Number of root peers directly served by the origin server.
    pub root_peer_count: usize,
    /// Maximum tree depth (hops from server to leaf).
    pub max_depth: usize,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            max_children: 3,
            root_peer_count: 30,
            max_depth: 6,
        }
    }
}

/// Peer information in the mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: SubscriberId,
    pub parent: Option<SubscriberId>,
    pub children: Vec<SubscriberId>,
    pub depth: usize,
    pub upload_capacity_kbps: u32,
}

/// Manages the relay tree topology.
///
/// The coordinator assigns viewers to the mesh tree, balancing
/// load across peers. Root peers connect directly to the server;
/// other peers connect through the tree via WebRTC DataChannels.
pub struct MeshCoordinator {
    config: MeshConfig,
    peers: Arc<DashMap<SubscriberId, PeerInfo>>,
}

impl MeshCoordinator {
    pub fn new(config: MeshConfig) -> Self {
        Self {
            config,
            peers: Arc::new(DashMap::new()),
        }
    }

    pub fn config(&self) -> &MeshConfig {
        &self.config
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }
}

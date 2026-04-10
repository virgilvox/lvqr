use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// WebRTC signaling message types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SignalMessage {
    /// SDP offer from a peer.
    Offer { from: String, to: String, sdp: String },
    /// SDP answer from a peer.
    Answer { from: String, to: String, sdp: String },
    /// ICE candidate.
    IceCandidate {
        from: String,
        to: String,
        candidate: String,
    },
}

/// WebRTC signaling server for mesh peer connections.
///
/// Relays SDP offers/answers and ICE candidates between peers
/// to establish WebRTC DataChannel connections for the mesh.
pub struct SignalServer {
    /// Connected peers by ID.
    peers: Arc<DashMap<String, PeerSession>>,
}

#[derive(Debug)]
struct PeerSession {
    /// Track this peer is interested in.
    #[allow(dead_code)]
    track: String,
}

impl SignalServer {
    pub fn new() -> Self {
        Self {
            peers: Arc::new(DashMap::new()),
        }
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }
}

impl Default for SignalServer {
    fn default() -> Self {
        Self::new()
    }
}

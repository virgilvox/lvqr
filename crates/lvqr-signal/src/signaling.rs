use axum::Router;
/// WebRTC signaling server for mesh peer connections.
///
/// Relays SDP offers/answers and ICE candidates between peers to establish
/// WebRTC DataChannel connections for the mesh. Uses WebSocket as the
/// signaling transport.
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// WebRTC signaling message types exchanged between peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SignalMessage {
    /// Register this peer with the signaling server.
    Register { peer_id: String, track: String },

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

    /// Server assigns a parent peer to connect to.
    AssignParent { parent_id: String, depth: u32 },

    /// Server notifies that a peer has left.
    PeerLeft { peer_id: String },
}

/// A connected peer session with a send channel.
struct PeerSession {
    /// Channel to send messages to this peer's WebSocket.
    tx: mpsc::UnboundedSender<SignalMessage>,
    /// Track this peer is interested in (used for filtering during mesh assignment).
    #[allow(dead_code)]
    track: String,
}

/// WebRTC signaling server for mesh peer connections.
#[derive(Clone)]
pub struct SignalServer {
    /// Connected peers by ID.
    peers: Arc<DashMap<String, PeerSession>>,
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

    /// Build the signaling WebSocket route.
    pub fn router(self) -> Router {
        Router::new().route("/signal", get(ws_handler)).with_state(self)
    }

    /// Register a peer and return a channel to send messages to it.
    fn register_peer(&self, peer_id: &str, track: &str) -> mpsc::UnboundedReceiver<SignalMessage> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.peers.insert(
            peer_id.to_string(),
            PeerSession {
                tx,
                track: track.to_string(),
            },
        );
        info!(peer = peer_id, track = track, "peer registered");
        rx
    }

    /// Remove a peer.
    fn remove_peer(&self, peer_id: &str) {
        self.peers.remove(peer_id);
        debug!(peer = peer_id, "peer removed");
    }

    /// Forward a message to the target peer.
    fn forward_to_peer(&self, target_id: &str, message: SignalMessage) {
        if let Some(entry) = self.peers.get(target_id) {
            if entry.tx.send(message).is_err() {
                warn!(peer = target_id, "failed to forward message, peer channel closed");
            }
        } else {
            debug!(peer = target_id, "target peer not found for forwarding");
        }
    }
}

impl Default for SignalServer {
    fn default() -> Self {
        Self::new()
    }
}

/// WebSocket upgrade handler.
async fn ws_handler(ws: WebSocketUpgrade, State(server): State<SignalServer>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_connection(socket, server))
}

/// Handle a single WebSocket connection.
async fn handle_ws_connection(mut socket: WebSocket, server: SignalServer) {
    let (peer_id, mut outgoing_rx) = match wait_for_register(&mut socket, &server).await {
        Some(result) => result,
        None => return,
    };

    loop {
        tokio::select! {
            // Incoming message from the WebSocket
            Some(msg) = recv_ws_message(&mut socket) => {
                match msg {
                    Ok(signal_msg) => {
                        handle_signal_message(&server, &peer_id, signal_msg);
                    }
                    Err(e) => {
                        debug!(peer = %peer_id, error = %e, "websocket receive error");
                        break;
                    }
                }
            }
            // Outgoing message to send via WebSocket
            Some(msg) = outgoing_rx.recv() => {
                let json = match serde_json::to_string(&msg) {
                    Ok(j) => j,
                    Err(e) => {
                        warn!(peer = %peer_id, error = %e, "failed to serialize message");
                        continue;
                    }
                };
                if socket.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            else => break,
        }
    }

    // Cleanup
    server.remove_peer(&peer_id);

    // Notify other peers in the same track
    let left_msg = SignalMessage::PeerLeft {
        peer_id: peer_id.clone(),
    };
    for entry in server.peers.iter() {
        let _ = entry.value().tx.send(left_msg.clone());
    }
}

/// Wait for the initial Register message from a new WebSocket connection.
async fn wait_for_register(
    socket: &mut WebSocket,
    server: &SignalServer,
) -> Option<(String, mpsc::UnboundedReceiver<SignalMessage>)> {
    // Read the first message
    let msg = socket.recv().await?;
    let msg = match msg {
        Ok(Message::Text(text)) => text,
        Ok(Message::Close(_)) | Err(_) => return None,
        _ => return None,
    };

    let signal: SignalMessage = match serde_json::from_str(&msg) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "invalid register message");
            return None;
        }
    };

    match signal {
        SignalMessage::Register { peer_id, track } => {
            let rx = server.register_peer(&peer_id, &track);
            Some((peer_id, rx))
        }
        _ => {
            warn!("expected Register message, got something else");
            None
        }
    }
}

/// Receive and parse a SignalMessage from the WebSocket.
async fn recv_ws_message(socket: &mut WebSocket) -> Option<Result<SignalMessage, String>> {
    let msg = socket.recv().await?;
    match msg {
        Ok(Message::Text(text)) => match serde_json::from_str::<SignalMessage>(&text) {
            Ok(signal) => Some(Ok(signal)),
            Err(e) => Some(Err(format!("invalid JSON: {e}"))),
        },
        Ok(Message::Close(_)) => None,
        Ok(_) => Some(Err("unexpected message type".into())),
        Err(e) => Some(Err(format!("websocket error: {e}"))),
    }
}

/// Handle an incoming signaling message by forwarding it to the target peer.
fn handle_signal_message(server: &SignalServer, from_peer: &str, msg: SignalMessage) {
    let target = match &msg {
        SignalMessage::Offer { to, .. } | SignalMessage::Answer { to, .. } | SignalMessage::IceCandidate { to, .. } => {
            Some(to.clone())
        }
        _ => None,
    };

    if let Some(to) = target {
        debug!(from = from_peer, to = %to, "forwarding signal");
        server.forward_to_peer(&to, msg);
    } else {
        debug!(from = from_peer, "ignoring non-forwardable message");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_message_serialization() {
        let msg = SignalMessage::Offer {
            from: "peer-1".into(),
            to: "peer-2".into(),
            sdp: "v=0\r\n...".into(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"Offer\""));
        assert!(json.contains("\"from\":\"peer-1\""));

        let parsed: SignalMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            SignalMessage::Offer { from, to, sdp } => {
                assert_eq!(from, "peer-1");
                assert_eq!(to, "peer-2");
                assert_eq!(sdp, "v=0\r\n...");
            }
            _ => panic!("expected Offer"),
        }
    }

    #[test]
    fn register_message_serialization() {
        let msg = SignalMessage::Register {
            peer_id: "abc123".into(),
            track: "live/test".into(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: SignalMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            SignalMessage::Register { peer_id, track } => {
                assert_eq!(peer_id, "abc123");
                assert_eq!(track, "live/test");
            }
            _ => panic!("expected Register"),
        }
    }

    #[test]
    fn server_register_and_forward() {
        let server = SignalServer::new();

        let mut rx1 = server.register_peer("peer-1", "live/test");
        let _rx2 = server.register_peer("peer-2", "live/test");
        assert_eq!(server.peer_count(), 2);

        // Forward a message to peer-1
        let msg = SignalMessage::Offer {
            from: "peer-2".into(),
            to: "peer-1".into(),
            sdp: "test-sdp".into(),
        };
        server.forward_to_peer("peer-1", msg);

        // peer-1 should receive it
        let received = rx1.try_recv().unwrap();
        match received {
            SignalMessage::Offer { from, sdp, .. } => {
                assert_eq!(from, "peer-2");
                assert_eq!(sdp, "test-sdp");
            }
            _ => panic!("expected Offer"),
        }
    }

    #[test]
    fn remove_peer_cleans_up() {
        let server = SignalServer::new();
        server.register_peer("peer-1", "live/test");
        assert_eq!(server.peer_count(), 1);

        server.remove_peer("peer-1");
        assert_eq!(server.peer_count(), 0);
    }
}

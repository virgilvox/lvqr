/// WebRTC signaling server for mesh peer connections.
///
/// Relays SDP offers/answers and ICE candidates between peers to establish
/// WebRTC DataChannel connections for the mesh. Uses WebSocket as the
/// signaling transport.
///
/// Supports server-push: an optional callback on peer register/unregister
/// allows the mesh coordinator to assign tree positions and notify peers.
use axum::Router;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// One ICE server entry pushed to clients via [`SignalMessage::AssignParent`].
///
/// JSON shape mirrors WebRTC's `RTCIceServer` so JS clients can
/// drop the value straight into `new RTCPeerConnection({ iceServers: [...] })`.
/// `urls` is always emitted as an array even when only one URL is
/// configured -- normalizing on the wire keeps the JS-side cast
/// simple. `username` + `credential` are skipped on serialize when
/// `None` so STUN-only entries do not carry empty credential
/// fields. Session 143 -- TURN deployment recipe + server-driven
/// ICE config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

/// WebRTC signaling message types exchanged between peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SignalMessage {
    /// Register this peer with the signaling server.
    ///
    /// `capacity` (added in session 144) carries the client's
    /// self-reported max-children value. `None` -- omitted on the
    /// wire -- tells the server to fall back to the operator's
    /// global `MeshConfig.max_children`. The server clamps the
    /// claim to `[0, max_children]` at register time so a misbehaving
    /// client cannot exceed the operator's ceiling. Behind
    /// `#[serde(default)]` so a pre-144 Register body that omits
    /// the field still deserializes cleanly.
    Register {
        peer_id: String,
        track: String,
        #[serde(default)]
        capacity: Option<u32>,
    },

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
    ///
    /// `ice_servers` (added in session 143) carries the operator-
    /// configured STUN/TURN list from `lvqr serve --mesh-ice-servers
    /// <JSON>`. Empty when the operator did not configure the flag;
    /// JS clients fall back to whatever was passed to the `MeshPeer`
    /// constructor (or its hardcoded Google STUN default). Behind
    /// `#[serde(default)]` so a pre-143 server body that omits the
    /// field still deserializes into a new client cleanly.
    AssignParent {
        peer_id: String,
        role: String,
        parent_id: Option<String>,
        depth: u32,
        #[serde(default)]
        ice_servers: Vec<IceServer>,
    },

    /// Server notifies that a peer has left.
    PeerLeft { peer_id: String },

    /// Structured error returned to the client before the server closes
    /// the connection. Emitted when a Register message carries an invalid
    /// peer_id or track, when a duplicate Register arrives on an
    /// already-registered session, and on any other protocol violation
    /// that causes the server to terminate the session.
    ///
    /// `code` is a short machine-readable tag (e.g. `invalid_peer_id`,
    /// `invalid_track`, `expected_register`, `duplicate_register`) and
    /// `reason` is a human-readable sentence suitable for logging.
    Error { code: String, reason: String },

    /// Client-to-server periodic report of the cumulative count of
    /// fragments the peer has forwarded to its DataChannel children.
    ///
    /// The value is a running total the client maintains locally. The
    /// server replaces rather than accumulates so a reconnect cannot
    /// inflate the displayed offload. The peer_id is resolved from the
    /// WS session state; a peer can only report for itself.
    ///
    /// Session 141 -- actual-vs-intended offload reporting.
    ForwardReport { forwarded_frames: u64 },
}

/// Maximum accepted `peer_id` byte length. Short enough to make brute-force
/// peer table pollution expensive; long enough to accept UUIDs and nanoids.
pub const MAX_PEER_ID_LEN: usize = 64;

/// Maximum accepted `track` byte length. Tracks are path-like (e.g.
/// `live/test`), so the limit is larger than peer_id.
pub const MAX_TRACK_LEN: usize = 128;

/// Validate a client-supplied `peer_id`. The audit flagged that peer IDs
/// flow straight from untrusted JSON into the peer table, into log lines,
/// and into mesh coordinator calls. The rule: ASCII alphanumeric plus
/// `_` and `-`, 1..=[`MAX_PEER_ID_LEN`] bytes.
pub fn is_valid_peer_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_PEER_ID_LEN
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Validate a client-supplied `track` string. Tracks look like
/// `live/test`, so the character set is wider: alphanumeric plus
/// `_`, `-`, `.`, and `/`. `..`, leading or trailing `/`, and
/// embedded control characters are all rejected to keep the value
/// safe as a routing key and log field.
pub fn is_valid_track(s: &str) -> bool {
    if s.is_empty() || s.len() > MAX_TRACK_LEN {
        return false;
    }
    if s.starts_with('/') || s.ends_with('/') || s.contains("..") || s.contains("//") {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/'))
}

/// A connected peer session with a send channel.
struct PeerSession {
    tx: mpsc::UnboundedSender<SignalMessage>,
    #[allow(dead_code)]
    track: String,
}

/// Context delivered to [`PeerCallback`] on register and unregister.
///
/// `capacity` carries the client's self-reported max-children claim
/// from the [`SignalMessage::Register`] message. `None` on
/// disconnect (no capacity is meaningful when a peer leaves) and on
/// pre-144 clients that omit the field.
///
/// Session 144 -- per-peer capacity advertisement.
pub struct PeerEvent<'a> {
    pub peer_id: &'a str,
    pub track: &'a str,
    pub capacity: Option<u32>,
    pub connected: bool,
}

/// Callback invoked when a peer registers or unregisters.
/// Returns an optional message to send back to the peer.
pub type PeerCallback = Arc<dyn Fn(&PeerEvent<'_>) -> Option<SignalMessage> + Send + Sync>;

/// Callback invoked when a registered peer sends a `ForwardReport`.
/// The first argument is the peer_id as resolved from the WS session
/// state (NOT a field on the wire message), and the second is the
/// cumulative forwarded-frame count the client reported.
///
/// Kept as a standalone callback (rather than a channel the coordinator
/// drains) so `lvqr-signal` remains independent of `lvqr-mesh`;
/// `lvqr-cli::start()` wires the bridge into `MeshCoordinator::
/// record_forward_report`.
pub type ForwardReportCallback = Arc<dyn Fn(&str, u64) + Send + Sync>;

/// WebRTC signaling server for mesh peer connections.
#[derive(Clone)]
pub struct SignalServer {
    peers: Arc<DashMap<String, PeerSession>>,
    on_peer: Option<PeerCallback>,
    on_forward_report: Option<ForwardReportCallback>,
}

impl SignalServer {
    pub fn new() -> Self {
        Self {
            peers: Arc::new(DashMap::new()),
            on_peer: None,
            on_forward_report: None,
        }
    }

    /// Set a callback for peer register/unregister events.
    /// Called with (peer_id, track, connected). Returns an optional response
    /// message to send to the peer (e.g., AssignParent).
    pub fn set_peer_callback(&mut self, cb: PeerCallback) {
        self.on_peer = Some(cb);
    }

    /// Set a callback for `ForwardReport` messages from registered peers.
    /// Called with (peer_id, forwarded_frames). `lvqr-cli` wires this
    /// into `MeshCoordinator::record_forward_report`. Session 141.
    pub fn set_forward_report_callback(&mut self, cb: ForwardReportCallback) {
        self.on_forward_report = Some(cb);
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Send a message to a specific peer.
    pub fn send_to_peer(&self, peer_id: &str, msg: SignalMessage) {
        if let Some(entry) = self.peers.get(peer_id) {
            if entry.tx.send(msg).is_err() {
                warn!(peer = peer_id, "failed to send message, peer channel closed");
            }
        } else {
            debug!(peer = peer_id, "target peer not found");
        }
    }

    /// Build the signaling WebSocket route.
    pub fn router(self) -> Router {
        Router::new().route("/signal", get(ws_handler)).with_state(self)
    }

    fn register_peer(
        &self,
        peer_id: &str,
        track: &str,
        capacity: Option<u32>,
    ) -> mpsc::UnboundedReceiver<SignalMessage> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.peers.insert(
            peer_id.to_string(),
            PeerSession {
                tx,
                track: track.to_string(),
            },
        );
        info!(peer = peer_id, track = track, "peer registered");

        // Notify callback and send response
        if let Some(ref cb) = self.on_peer {
            let event = PeerEvent {
                peer_id,
                track,
                capacity,
                connected: true,
            };
            if let Some(response) = cb(&event) {
                self.send_to_peer(peer_id, response);
            }
        }

        rx
    }

    fn remove_peer(&self, peer_id: &str) {
        if let Some((_, session)) = self.peers.remove(peer_id) {
            debug!(peer = peer_id, "peer removed");
            if let Some(ref cb) = self.on_peer {
                let event = PeerEvent {
                    peer_id,
                    track: &session.track,
                    capacity: None,
                    connected: false,
                };
                cb(&event);
            }
        }
    }

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

async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: axum::http::HeaderMap,
    State(server): State<SignalServer>,
) -> impl IntoResponse {
    // Session 111-B3: if the client offered a
    // `Sec-WebSocket-Protocol: lvqr.bearer.<token>` subprotocol,
    // echo it back in the upgrade response so RFC 6455-strict
    // clients accept the handshake. The subprotocol-carried
    // bearer is consumed by the CLI's `signal_auth_middleware`
    // BEFORE this handler runs; by the time we get here, auth
    // has already allowed (or the middleware would have
    // short-circuited with a 401). The echo is therefore purely
    // a handshake-compat concern, not an auth concern.
    let offered = offered_bearer_subprotocol(&headers);
    let ws = match offered {
        Some(ref p) => ws.protocols(std::iter::once(p.clone())),
        None => ws,
    };
    ws.on_upgrade(move |socket| handle_ws_connection(socket, server))
}

/// Pick any `lvqr.bearer.<token>` subprotocol offered by the
/// client on the incoming `Sec-WebSocket-Protocol` header. Used
/// by [`ws_handler`] to echo the protocol back on the upgrade
/// response so the handshake completes. Returns `None` when no
/// qualifying subprotocol was offered.
fn offered_bearer_subprotocol(headers: &axum::http::HeaderMap) -> Option<String> {
    let hv = headers.get("sec-websocket-protocol")?;
    let raw = hv.to_str().ok()?;
    for item in raw.split(',') {
        let proto = item.trim();
        if let Some(tok) = proto.strip_prefix("lvqr.bearer.")
            && !tok.is_empty()
        {
            return Some(proto.to_string());
        }
    }
    None
}

async fn handle_ws_connection(mut socket: WebSocket, server: SignalServer) {
    let (peer_id, mut outgoing_rx) = match wait_for_register(&mut socket, &server).await {
        Some(result) => result,
        None => return,
    };

    loop {
        tokio::select! {
            Some(msg) = recv_ws_message(&mut socket) => {
                match msg {
                    Ok(SignalMessage::Register { .. }) => {
                        // The audit caps registrations per connection at 1.
                        // A client that sends a second Register after the
                        // initial handshake is buggy or malicious; reply
                        // with a structured error and close the session.
                        warn!(peer = %peer_id, "duplicate Register after handshake, closing");
                        let err = SignalMessage::Error {
                            code: "duplicate_register".to_string(),
                            reason: "register may only be sent once per connection".to_string(),
                        };
                        let _ = send_signal(&mut socket, &err).await;
                        break;
                    }
                    Ok(signal_msg) => {
                        handle_signal_message(&server, &peer_id, signal_msg);
                    }
                    Err(e) => {
                        debug!(peer = %peer_id, error = %e, "websocket receive error");
                        break;
                    }
                }
            }
            Some(msg) = outgoing_rx.recv() => {
                if send_signal(&mut socket, &msg).await.is_err() {
                    break;
                }
            }
            else => break,
        }
    }

    server.remove_peer(&peer_id);

    let left_msg = SignalMessage::PeerLeft {
        peer_id: peer_id.clone(),
    };
    for entry in server.peers.iter() {
        let _ = entry.value().tx.send(left_msg.clone());
    }
}

async fn wait_for_register(
    socket: &mut WebSocket,
    server: &SignalServer,
) -> Option<(String, mpsc::UnboundedReceiver<SignalMessage>)> {
    let msg = socket.recv().await?;
    let text = match msg {
        Ok(Message::Text(text)) => text,
        Ok(Message::Close(_)) | Err(_) => return None,
        _ => return None,
    };

    let signal: SignalMessage = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "invalid register JSON");
            let err = SignalMessage::Error {
                code: "invalid_json".to_string(),
                reason: format!("first message must be a valid Register JSON: {e}"),
            };
            let _ = send_signal(socket, &err).await;
            return None;
        }
    };

    match signal {
        SignalMessage::Register {
            peer_id,
            track,
            capacity,
        } => {
            if !is_valid_peer_id(&peer_id) {
                // Do not log the bad peer_id at info level: it is
                // attacker-controlled and may contain control chars that
                // corrupt structured logs. Only the length is safe to
                // record without sanitization.
                warn!(len = peer_id.len(), "register rejected: invalid peer_id");
                let err = SignalMessage::Error {
                    code: "invalid_peer_id".to_string(),
                    reason: format!("peer_id must match [A-Za-z0-9_-]{{1,{MAX_PEER_ID_LEN}}}"),
                };
                let _ = send_signal(socket, &err).await;
                return None;
            }
            if !is_valid_track(&track) {
                warn!(
                    peer = %peer_id,
                    len = track.len(),
                    "register rejected: invalid track"
                );
                let err = SignalMessage::Error {
                    code: "invalid_track".to_string(),
                    reason: format!(
                        "track must match [A-Za-z0-9._/-]{{1,{MAX_TRACK_LEN}}} and must not contain .. or leading/trailing /"
                    ),
                };
                let _ = send_signal(socket, &err).await;
                return None;
            }
            let rx = server.register_peer(&peer_id, &track, capacity);
            Some((peer_id, rx))
        }
        _ => {
            warn!("expected Register message, got something else");
            let err = SignalMessage::Error {
                code: "expected_register".to_string(),
                reason: "first message on a signaling connection must be Register".to_string(),
            };
            let _ = send_signal(socket, &err).await;
            None
        }
    }
}

/// Serialize and send a [`SignalMessage`] over a WebSocket as a Text
/// frame. Used by both the outbound-channel pump and the error-response
/// path so serialization is centralized.
async fn send_signal(socket: &mut WebSocket, msg: &SignalMessage) -> Result<(), axum::Error> {
    let json = match serde_json::to_string(msg) {
        Ok(j) => j,
        Err(e) => {
            warn!(error = %e, "failed to serialize signal message");
            return Ok(());
        }
    };
    socket.send(Message::Text(json.into())).await
}

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

fn handle_signal_message(server: &SignalServer, from_peer: &str, msg: SignalMessage) {
    // ForwardReport is a client-to-server message with no `to` field;
    // the server resolves the sender from its WS session state (a peer
    // can only report for itself). Handle it before the peer-forwarding
    // match below so it cannot be routed to an arbitrary target.
    if let SignalMessage::ForwardReport { forwarded_frames } = msg {
        if let Some(ref cb) = server.on_forward_report {
            cb(from_peer, forwarded_frames);
        } else {
            debug!(from = from_peer, "ForwardReport received with no callback installed");
        }
        return;
    }

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
            capacity: Some(3),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"capacity\":3"));
        let parsed: SignalMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            SignalMessage::Register {
                peer_id,
                track,
                capacity,
            } => {
                assert_eq!(peer_id, "abc123");
                assert_eq!(track, "live/test");
                assert_eq!(capacity, Some(3));
            }
            _ => panic!("expected Register"),
        }
    }

    /// Session 144: a pre-144 Register body that omits the
    /// `capacity` field must deserialize cleanly into a new
    /// SignalMessage with capacity = None via `#[serde(default)]`.
    #[test]
    fn register_deserializes_pre_144_body_without_capacity() {
        let json = r#"{"type":"Register","peer_id":"abc","track":"live/test"}"#;
        let parsed: SignalMessage = serde_json::from_str(json).unwrap();
        match parsed {
            SignalMessage::Register {
                peer_id,
                track,
                capacity,
            } => {
                assert_eq!(peer_id, "abc");
                assert_eq!(track, "live/test");
                assert!(capacity.is_none());
            }
            _ => panic!("expected Register"),
        }
    }

    /// Session 144: a Register body with a wildly inflated capacity
    /// claim still deserializes -- the wire shape accepts any u32 and
    /// the lvqr-cli signal bridge clamps at register time. This test
    /// only proves the serde layer is permissive; the clamp itself is
    /// covered by the integration test in `lvqr-cli`.
    #[test]
    fn register_accepts_oversize_capacity_claim() {
        let json = r#"{"type":"Register","peer_id":"abc","track":"live/test","capacity":4294967295}"#;
        let parsed: SignalMessage = serde_json::from_str(json).unwrap();
        match parsed {
            SignalMessage::Register { capacity, .. } => {
                assert_eq!(capacity, Some(u32::MAX));
            }
            _ => panic!("expected Register"),
        }
    }

    #[test]
    fn assign_parent_serialization() {
        let msg = SignalMessage::AssignParent {
            peer_id: "peer-42".into(),
            role: "Relay".into(),
            parent_id: Some("peer-1".into()),
            depth: 2,
            ice_servers: Vec::new(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"AssignParent\""));
        assert!(json.contains("\"parent_id\":\"peer-1\""));
        assert!(json.contains("\"ice_servers\":[]"));

        let parsed: SignalMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            SignalMessage::AssignParent {
                peer_id,
                role,
                parent_id,
                depth,
                ice_servers,
            } => {
                assert_eq!(peer_id, "peer-42");
                assert_eq!(role, "Relay");
                assert_eq!(parent_id.as_deref(), Some("peer-1"));
                assert_eq!(depth, 2);
                assert!(ice_servers.is_empty());
            }
            _ => panic!("expected AssignParent"),
        }
    }

    #[test]
    fn assign_parent_carries_ice_servers() {
        // Session 143: operator-configured STUN/TURN entries flow
        // through AssignParent down to JS clients, which feed the
        // list directly into RTCPeerConnection({ iceServers: [...] }).
        let msg = SignalMessage::AssignParent {
            peer_id: "peer-7".into(),
            role: "Relay".into(),
            parent_id: Some("peer-1".into()),
            depth: 1,
            ice_servers: vec![
                IceServer {
                    urls: vec!["stun:stun.l.google.com:19302".into()],
                    username: None,
                    credential: None,
                },
                IceServer {
                    urls: vec!["turn:turn.example.com:3478".into()],
                    username: Some("u".into()),
                    credential: Some("p".into()),
                },
            ],
        };

        let json = serde_json::to_string(&msg).unwrap();
        // Round-trip preserves urls + credentials exactly.
        let parsed: SignalMessage = serde_json::from_str(&json).unwrap();
        let SignalMessage::AssignParent { ice_servers, .. } = parsed else {
            panic!("expected AssignParent");
        };
        assert_eq!(ice_servers.len(), 2);
        assert_eq!(ice_servers[0].urls, vec!["stun:stun.l.google.com:19302".to_string()]);
        assert!(ice_servers[0].username.is_none());
        assert!(ice_servers[0].credential.is_none());
        assert_eq!(ice_servers[1].urls, vec!["turn:turn.example.com:3478".to_string()]);
        assert_eq!(ice_servers[1].username.as_deref(), Some("u"));
        assert_eq!(ice_servers[1].credential.as_deref(), Some("p"));

        // STUN-only entries skip credential fields on the wire so
        // pre-143 clients that strict-parse on Optional credential
        // fields do not see a null/empty value where they expect
        // absence.
        assert!(!json.contains("\"username\":null"));
        assert!(!json.contains("\"credential\":null"));
    }

    #[test]
    fn assign_parent_deserializes_pre_143_body_without_ice_servers() {
        // A pre-143 server body that omits ice_servers entirely must
        // still deserialize into a new client; the #[serde(default)]
        // on the field makes ice_servers default to an empty vec.
        let json = r#"{"type":"AssignParent","peer_id":"p","role":"Root","parent_id":null,"depth":0}"#;
        let parsed: SignalMessage = serde_json::from_str(json).unwrap();
        let SignalMessage::AssignParent { ice_servers, .. } = parsed else {
            panic!("expected AssignParent");
        };
        assert!(
            ice_servers.is_empty(),
            "missing ice_servers field must default to empty"
        );
    }

    #[test]
    fn server_register_and_forward() {
        let server = SignalServer::new();

        let mut rx1 = server.register_peer("peer-1", "live/test", None);
        let _rx2 = server.register_peer("peer-2", "live/test", None);
        assert_eq!(server.peer_count(), 2);

        let msg = SignalMessage::Offer {
            from: "peer-2".into(),
            to: "peer-1".into(),
            sdp: "test-sdp".into(),
        };
        server.forward_to_peer("peer-1", msg);

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
    fn server_push_to_peer() {
        let server = SignalServer::new();
        let mut rx = server.register_peer("peer-1", "live/test", None);

        server.send_to_peer(
            "peer-1",
            SignalMessage::AssignParent {
                peer_id: "peer-1".into(),
                role: "Root".into(),
                parent_id: None,
                depth: 0,
                ice_servers: Vec::new(),
            },
        );

        let received = rx.try_recv().unwrap();
        match received {
            SignalMessage::AssignParent {
                peer_id,
                role,
                parent_id,
                depth,
                ice_servers,
            } => {
                assert_eq!(peer_id, "peer-1");
                assert_eq!(role, "Root");
                assert!(parent_id.is_none());
                assert_eq!(depth, 0);
                assert!(ice_servers.is_empty());
            }
            _ => panic!("expected AssignParent"),
        }
    }

    #[test]
    fn peer_callback_on_register() {
        let mut server = SignalServer::new();
        server.set_peer_callback(Arc::new(|event: &PeerEvent<'_>| {
            if event.connected {
                Some(SignalMessage::AssignParent {
                    peer_id: event.peer_id.to_string(),
                    role: "Root".into(),
                    parent_id: None,
                    depth: 0,
                    ice_servers: Vec::new(),
                })
            } else {
                None
            }
        }));

        let mut rx = server.register_peer("peer-1", "live/test", None);

        // Should have received AssignParent via callback
        let received = rx.try_recv().unwrap();
        match received {
            SignalMessage::AssignParent { peer_id, role, .. } => {
                assert_eq!(peer_id, "peer-1");
                assert_eq!(role, "Root");
            }
            _ => panic!("expected AssignParent from callback"),
        }
    }

    #[test]
    fn remove_peer_cleans_up() {
        let server = SignalServer::new();
        server.register_peer("peer-1", "live/test", None);
        assert_eq!(server.peer_count(), 1);

        server.remove_peer("peer-1");
        assert_eq!(server.peer_count(), 0);
    }

    /// Session 144: the `PeerEvent` passed to `PeerCallback` must
    /// carry the capacity claim from the wire on register, and
    /// must carry `None` on disconnect.
    #[test]
    fn peer_callback_receives_capacity_on_register_and_none_on_disconnect() {
        use std::sync::Mutex;
        type Capture = (String, Option<u32>, bool);
        let captured: Arc<Mutex<Vec<Capture>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_cb = Arc::clone(&captured);

        let mut server = SignalServer::new();
        server.set_peer_callback(Arc::new(move |event: &PeerEvent<'_>| {
            captured_cb
                .lock()
                .unwrap()
                .push((event.peer_id.to_string(), event.capacity, event.connected));
            None
        }));

        let _rx = server.register_peer("peer-1", "live/test", Some(2));
        server.remove_peer("peer-1");

        let snapshot = captured.lock().unwrap().clone();
        assert_eq!(
            snapshot,
            vec![
                ("peer-1".to_string(), Some(2), true),
                ("peer-1".to_string(), None, false),
            ]
        );
    }

    #[test]
    fn peer_id_validator_accepts_well_formed() {
        assert!(is_valid_peer_id("peer-1"));
        assert!(is_valid_peer_id("PEER_42"));
        assert!(is_valid_peer_id("abc123"));
        assert!(is_valid_peer_id("a"));
        assert!(is_valid_peer_id(&"x".repeat(MAX_PEER_ID_LEN)));
    }

    #[test]
    fn peer_id_validator_rejects_malformed() {
        assert!(!is_valid_peer_id(""));
        assert!(!is_valid_peer_id(&"x".repeat(MAX_PEER_ID_LEN + 1)));
        assert!(!is_valid_peer_id("peer 1")); // space
        assert!(!is_valid_peer_id("peer/1")); // slash
        assert!(!is_valid_peer_id("peer.1")); // dot
        assert!(!is_valid_peer_id("peer\n1")); // newline
        assert!(!is_valid_peer_id("peer\t1")); // tab
        assert!(!is_valid_peer_id("peer\x00id")); // nul
        assert!(!is_valid_peer_id("peer<script>")); // html
        assert!(!is_valid_peer_id("peerü")); // non-ascii
    }

    #[test]
    fn track_validator_accepts_well_formed() {
        assert!(is_valid_track("live/test"));
        assert!(is_valid_track("live"));
        assert!(is_valid_track("a.b.c"));
        assert!(is_valid_track("a/b/c"));
        assert!(is_valid_track("live-2024/hd.1080p"));
        assert!(is_valid_track(&"a".repeat(MAX_TRACK_LEN)));
    }

    #[test]
    fn track_validator_rejects_traversal_and_garbage() {
        assert!(!is_valid_track(""));
        assert!(!is_valid_track(&"a".repeat(MAX_TRACK_LEN + 1)));
        assert!(!is_valid_track("/leading"));
        assert!(!is_valid_track("trailing/"));
        assert!(!is_valid_track("a//b"));
        assert!(!is_valid_track("a/../b"));
        assert!(!is_valid_track(".."));
        assert!(!is_valid_track("a\\b"));
        assert!(!is_valid_track("live test"));
        assert!(!is_valid_track("liveü"));
    }

    #[test]
    fn forward_report_round_trips() {
        let msg = SignalMessage::ForwardReport { forwarded_frames: 2024 };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"ForwardReport\""));
        assert!(json.contains("\"forwarded_frames\":2024"));
        let parsed: SignalMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            SignalMessage::ForwardReport { forwarded_frames } => {
                assert_eq!(forwarded_frames, 2024);
            }
            _ => panic!("expected ForwardReport"),
        }
    }

    #[test]
    fn forward_report_callback_invoked_with_session_peer_id() {
        use std::sync::Mutex;
        let captured: Arc<Mutex<Vec<(String, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_cb = Arc::clone(&captured);

        let mut server = SignalServer::new();
        server.set_forward_report_callback(Arc::new(move |peer_id, frames| {
            captured_cb.lock().unwrap().push((peer_id.to_string(), frames));
        }));

        let _rx = server.register_peer("peer-alpha", "live/test", None);

        // Route a ForwardReport through the private dispatcher as if it
        // arrived on peer-alpha's WebSocket.
        handle_signal_message(
            &server,
            "peer-alpha",
            SignalMessage::ForwardReport { forwarded_frames: 99 },
        );

        let snapshot = captured.lock().unwrap().clone();
        assert_eq!(snapshot, vec![("peer-alpha".to_string(), 99)]);
    }

    #[test]
    fn forward_report_without_callback_is_silent_noop() {
        let server = SignalServer::new();
        // No panic; no forwarding; no output. The test simply asserts
        // that we can dispatch without a callback installed.
        handle_signal_message(
            &server,
            "peer-anon",
            SignalMessage::ForwardReport { forwarded_frames: 10 },
        );
    }

    #[test]
    fn forward_report_does_not_leak_to_other_peers() {
        let server = SignalServer::new();
        let _rx_a = server.register_peer("peer-a", "live/test", None);
        let mut rx_b = server.register_peer("peer-b", "live/test", None);

        handle_signal_message(&server, "peer-a", SignalMessage::ForwardReport { forwarded_frames: 7 });

        // peer-b's outbound channel should not have received the report.
        assert!(
            rx_b.try_recv().is_err(),
            "ForwardReport must not be forwarded to other peers"
        );
    }

    #[test]
    fn error_variant_round_trips() {
        let err = SignalMessage::Error {
            code: "invalid_peer_id".into(),
            reason: "bad".into(),
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"type\":\"Error\""));
        assert!(json.contains("\"code\":\"invalid_peer_id\""));
        let parsed: SignalMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            SignalMessage::Error { code, reason } => {
                assert_eq!(code, "invalid_peer_id");
                assert_eq!(reason, "bad");
            }
            _ => panic!("expected Error"),
        }
    }
}

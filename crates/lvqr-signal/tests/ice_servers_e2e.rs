//! Session 143: end-to-end test for `--mesh-ice-servers` flowing
//! through the production admin router into the `AssignParent`
//! server-push message.
//!
//! Spins up a real TestServer with mesh enabled and a configured
//! ice_servers snapshot, opens a `tokio-tungstenite` WebSocket
//! client to `/signal`, sends a Register, and asserts that the
//! pushed AssignParent body carries the operator-configured list
//! verbatim. Closes the gap that the lvqr-signal unit tests can't:
//! they exercise the SignalMessage round-trip but not the
//! lvqr-cli signal-callback closure that actually clones the
//! ServeConfig snapshot into every emitted AssignParent.

use futures::{SinkExt, StreamExt};
use lvqr_signal::{IceServer, SignalMessage};
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::time::Duration;
use tokio_tungstenite::tungstenite::protocol::Message;

const TIMEOUT: Duration = Duration::from_secs(5);

fn signal_url(server: &TestServer) -> String {
    format!("ws://{}/signal", server.admin_addr())
}

async fn recv_signal(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
) -> SignalMessage {
    loop {
        let msg = tokio::time::timeout(TIMEOUT, socket.next())
            .await
            .expect("timed out waiting for signal frame")
            .expect("socket closed unexpectedly")
            .expect("ws recv error");
        match msg {
            Message::Text(text) => {
                return serde_json::from_str(&text)
                    .unwrap_or_else(|e| panic!("invalid SignalMessage JSON: {e}\n  text: {text}"));
            }
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            Message::Close(frame) => panic!("socket closed: {frame:?}"),
        }
    }
}

async fn send_text(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    text: &str,
) {
    socket.send(Message::Text(text.into())).await.expect("ws send failed");
}

#[tokio::test]
async fn assign_parent_carries_configured_ice_servers() {
    // STUN-only entry plus a TURN entry with credentials. Together
    // they exercise both the no-credential and full-credential
    // paths through the AssignParent serializer.
    let configured = vec![
        IceServer {
            urls: vec!["stun:stun.l.google.com:19302".into()],
            username: None,
            credential: None,
        },
        IceServer {
            urls: vec!["turn:turn.example.com:3478".into()],
            username: Some("operator-user".into()),
            credential: Some("operator-pass".into()),
        },
    ];

    let server = TestServer::start(
        TestServerConfig::new()
            .with_mesh(3)
            .with_mesh_root_peer_count(1)
            .with_mesh_ice_servers(configured.clone()),
    )
    .await
    .expect("TestServer::start");

    let (mut socket, _resp) = tokio::time::timeout(TIMEOUT, tokio_tungstenite::connect_async(signal_url(&server)))
        .await
        .expect("signal connect timed out")
        .expect("signal connect failed");

    send_text(
        &mut socket,
        r#"{"type":"Register","peer_id":"ice-test-peer","track":"live/test"}"#,
    )
    .await;

    let assignment = recv_signal(&mut socket).await;
    let SignalMessage::AssignParent {
        peer_id,
        role,
        parent_id,
        depth,
        ice_servers,
    } = assignment
    else {
        panic!("expected AssignParent, got {assignment:?}");
    };

    assert_eq!(peer_id, "ice-test-peer");
    // Single peer with mesh_root_peer_count=1 -> Root.
    assert_eq!(role, "Root");
    assert!(parent_id.is_none());
    assert_eq!(depth, 0);

    // Load-bearing assertion: the operator-configured list flows
    // through the lvqr-cli signal callback unchanged.
    assert_eq!(ice_servers, configured);

    server.shutdown().await.expect("shutdown failed");
}

#[tokio::test]
async fn assign_parent_omits_ice_servers_when_unconfigured() {
    // No `with_mesh_ice_servers` call -> empty vec on the wire.
    // Pre-143 clients that ignore the field, and the JS MeshPeer
    // fallback to constructor-provided iceServers, both depend on
    // this empty-vec semantic.
    let server = TestServer::start(TestServerConfig::new().with_mesh(3).with_mesh_root_peer_count(1))
        .await
        .expect("TestServer::start");

    let (mut socket, _resp) = tokio_tungstenite::connect_async(signal_url(&server)).await.unwrap();

    send_text(
        &mut socket,
        r#"{"type":"Register","peer_id":"unconfigured-peer","track":"live/test"}"#,
    )
    .await;

    let assignment = recv_signal(&mut socket).await;
    let SignalMessage::AssignParent { ice_servers, .. } = assignment else {
        panic!("expected AssignParent, got {assignment:?}");
    };

    assert!(
        ice_servers.is_empty(),
        "unconfigured server must emit empty ice_servers, got {ice_servers:?}"
    );

    server.shutdown().await.expect("shutdown failed");
}

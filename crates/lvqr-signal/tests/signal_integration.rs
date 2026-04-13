//! Integration tests for `lvqr-signal` driven through the production
//! admin router.
//!
//! Uses [`lvqr_test_utils::TestServer`] with mesh enabled so the `/signal`
//! endpoint is mounted exactly the same way `lvqr serve --mesh-enabled`
//! mounts it in production. Every test connects a real
//! `tokio-tungstenite` WebSocket client, sends real JSON, and asserts on
//! the server's real response. No mocks.
//!
//! These tests close the audit finding that flagged `lvqr-signal` as
//! deserializing untrusted peer_id and track fields from JSON with no
//! validation. If the validators regress, these tests go red.

use futures::{SinkExt, StreamExt};
use lvqr_signal::SignalMessage;
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::time::Duration;
use tokio_tungstenite::tungstenite::protocol::Message;

const TIMEOUT: Duration = Duration::from_secs(5);

/// Build the ws:// URL for the signal endpoint on a running TestServer.
fn signal_url(server: &TestServer) -> String {
    format!("ws://{}/signal", server.admin_addr())
}

/// Read the next text frame from the socket as a parsed SignalMessage.
/// Panics with a descriptive message on timeout, close, or parse error;
/// tests want those to be hard fails, not silent skips.
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
async fn malformed_peer_id_is_rejected_with_structured_error() {
    let server = TestServer::start(TestServerConfig::new().with_mesh(3))
        .await
        .expect("TestServer::start failed");

    let url = signal_url(&server);
    let (mut socket, _resp) = tokio::time::timeout(TIMEOUT, tokio_tungstenite::connect_async(&url))
        .await
        .expect("signal connect timed out")
        .expect("signal connect failed");

    // "peer 1" has a space, which the validator rejects.
    send_text(
        &mut socket,
        r#"{"type":"Register","peer_id":"peer 1","track":"live/test"}"#,
    )
    .await;

    match recv_signal(&mut socket).await {
        SignalMessage::Error { code, reason } => {
            assert_eq!(code, "invalid_peer_id", "wrong error code: reason={reason}");
            assert!(reason.contains("peer_id"), "reason should mention peer_id: {reason}");
        }
        other => panic!("expected Error, got {other:?}"),
    }

    // After rejecting, the server must close the socket. Draining the
    // stream should terminate within the timeout.
    let drain = tokio::time::timeout(TIMEOUT, async {
        while let Some(msg) = socket.next().await {
            if matches!(msg, Err(_) | Ok(Message::Close(_))) {
                break;
            }
        }
    })
    .await;
    assert!(drain.is_ok(), "server did not close after rejecting invalid peer_id");

    server.shutdown().await.expect("shutdown failed");
}

#[tokio::test]
async fn traversal_track_is_rejected_with_structured_error() {
    let server = TestServer::start(TestServerConfig::new().with_mesh(3)).await.unwrap();

    let (mut socket, _) = tokio_tungstenite::connect_async(signal_url(&server)).await.unwrap();

    send_text(
        &mut socket,
        r#"{"type":"Register","peer_id":"peer-1","track":"live/../../etc"}"#,
    )
    .await;

    match recv_signal(&mut socket).await {
        SignalMessage::Error { code, .. } => {
            assert_eq!(code, "invalid_track");
        }
        other => panic!("expected Error, got {other:?}"),
    }

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn non_register_first_message_is_rejected() {
    let server = TestServer::start(TestServerConfig::new().with_mesh(3)).await.unwrap();

    let (mut socket, _) = tokio_tungstenite::connect_async(signal_url(&server)).await.unwrap();

    // Send an Offer as the first message; the server requires Register.
    send_text(&mut socket, r#"{"type":"Offer","from":"x","to":"y","sdp":"v=0"}"#).await;

    match recv_signal(&mut socket).await {
        SignalMessage::Error { code, .. } => {
            assert_eq!(code, "expected_register");
        }
        other => panic!("expected Error, got {other:?}"),
    }

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn duplicate_register_is_rejected_after_initial_handshake() {
    let server = TestServer::start(TestServerConfig::new().with_mesh(3)).await.unwrap();

    let (mut socket, _) = tokio_tungstenite::connect_async(signal_url(&server)).await.unwrap();

    // First Register is valid. The mesh callback responds with an
    // AssignParent, which we drain off the socket before sending the
    // duplicate.
    send_text(
        &mut socket,
        r#"{"type":"Register","peer_id":"peer-good","track":"live/test"}"#,
    )
    .await;

    match recv_signal(&mut socket).await {
        SignalMessage::AssignParent { peer_id, .. } => {
            assert_eq!(peer_id, "peer-good");
        }
        other => panic!("expected AssignParent, got {other:?}"),
    }

    // Second Register on the same connection is the audit-flagged
    // "cap registrations per connection at 1" case.
    send_text(
        &mut socket,
        r#"{"type":"Register","peer_id":"peer-again","track":"live/test"}"#,
    )
    .await;

    match recv_signal(&mut socket).await {
        SignalMessage::Error { code, .. } => {
            assert_eq!(code, "duplicate_register");
        }
        other => panic!("expected Error, got {other:?}"),
    }

    server.shutdown().await.unwrap();
}

#[tokio::test]
async fn valid_register_succeeds_and_receives_assign_parent() {
    // Happy path: a well-formed Register through the real router returns
    // an AssignParent from the mesh callback that lvqr_cli wires up.
    let server = TestServer::start(TestServerConfig::new().with_mesh(3)).await.unwrap();

    let (mut socket, _) = tokio_tungstenite::connect_async(signal_url(&server)).await.unwrap();

    send_text(
        &mut socket,
        r#"{"type":"Register","peer_id":"peer-42","track":"live/test"}"#,
    )
    .await;

    match recv_signal(&mut socket).await {
        SignalMessage::AssignParent { peer_id, depth, .. } => {
            assert_eq!(peer_id, "peer-42");
            // First peer in an empty mesh is always the root at depth 0.
            assert_eq!(depth, 0);
        }
        other => panic!("expected AssignParent, got {other:?}"),
    }

    server.shutdown().await.unwrap();
}

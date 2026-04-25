//! Session 144: end-to-end test for per-peer capacity advertisement.
//!
//! Drives three real WebSocket clients into a TestServer with mesh
//! enabled, max-peers=5, and root-peer-count=1. The first peer
//! advertises capacity=1 on its `Register`; the other two advertise
//! nothing. The lvqr-cli signal-callback closure clamps the claim
//! and threads it into `MeshCoordinator::add_peer`. The assertions:
//! peer-3 must descend to peer-2 even though `MeshConfig.max_children`
//! is 5, because peer-1's per-peer capacity is 1. A second test
//! sends `capacity = u32::MAX` and asserts the admin route reports
//! the clamped value (the operator's global ceiling), proving the
//! lvqr-cli register-time clamp held.

use futures_util::{SinkExt, StreamExt};
use lvqr_admin::MeshState;
use lvqr_signal::SignalMessage;
use lvqr_test_utils::http::http_get;
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::time::Duration;
use tokio_tungstenite::tungstenite::protocol::Message;

const TIMEOUT: Duration = Duration::from_secs(5);

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn signal_url(server: &TestServer) -> String {
    format!("ws://{}/signal", server.admin_addr())
}

async fn connect_signal(server: &TestServer) -> Socket {
    let (socket, _resp) = tokio::time::timeout(TIMEOUT, tokio_tungstenite::connect_async(signal_url(server)))
        .await
        .expect("signal connect timed out")
        .expect("signal connect failed");
    socket
}

async fn send_text(socket: &mut Socket, text: &str) {
    socket.send(Message::Text(text.into())).await.expect("ws send failed");
}

async fn recv_assignment(socket: &mut Socket) -> SignalMessage {
    loop {
        let msg = tokio::time::timeout(TIMEOUT, socket.next())
            .await
            .expect("timed out waiting for AssignParent")
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

async fn fetch_mesh(server: &TestServer) -> MeshState {
    let resp = http_get(server.admin_addr(), "/api/v1/mesh").await;
    assert_eq!(resp.status, 200, "GET /api/v1/mesh returned {}", resp.status);
    serde_json::from_slice(&resp.body)
        .unwrap_or_else(|e| panic!("invalid MeshState JSON: {e}\n  body: {}", resp.body_text()))
}

#[tokio::test]
async fn capacity_one_forces_descent_for_third_peer() {
    let server = TestServer::start(TestServerConfig::new().with_mesh(5).with_mesh_root_peer_count(1))
        .await
        .expect("TestServer::start");

    // peer-1 self-reports capacity=1 via Register. Despite
    // MeshConfig.max_children = 5, peer-1 will only host one child.
    let mut s1 = connect_signal(&server).await;
    send_text(
        &mut s1,
        r#"{"type":"Register","peer_id":"peer-1","track":"live/test","capacity":1}"#,
    )
    .await;
    let a1 = recv_assignment(&mut s1).await;
    let SignalMessage::AssignParent {
        peer_id,
        role,
        parent_id,
        depth,
        ..
    } = a1
    else {
        panic!("expected AssignParent for peer-1");
    };
    assert_eq!(peer_id, "peer-1");
    assert_eq!(role, "Root");
    assert!(parent_id.is_none());
    assert_eq!(depth, 0);

    // peer-2 (no capacity) joins. It becomes the lone child of
    // peer-1.
    let mut s2 = connect_signal(&server).await;
    send_text(&mut s2, r#"{"type":"Register","peer_id":"peer-2","track":"live/test"}"#).await;
    let a2 = recv_assignment(&mut s2).await;
    let SignalMessage::AssignParent {
        peer_id,
        role,
        parent_id,
        depth,
        ..
    } = a2
    else {
        panic!("expected AssignParent for peer-2");
    };
    assert_eq!(peer_id, "peer-2");
    assert_eq!(role, "Relay");
    assert_eq!(parent_id.as_deref(), Some("peer-1"));
    assert_eq!(depth, 1);

    // peer-3 must descend to peer-2 because peer-1 hit its
    // self-reported capacity of 1.
    let mut s3 = connect_signal(&server).await;
    send_text(&mut s3, r#"{"type":"Register","peer_id":"peer-3","track":"live/test"}"#).await;
    let a3 = recv_assignment(&mut s3).await;
    let SignalMessage::AssignParent {
        peer_id,
        role,
        parent_id,
        depth,
        ..
    } = a3
    else {
        panic!("expected AssignParent for peer-3");
    };
    assert_eq!(peer_id, "peer-3");
    assert_eq!(role, "Relay");
    assert_eq!(
        parent_id.as_deref(),
        Some("peer-2"),
        "peer-3 should descend to peer-2 because peer-1 advertised capacity=1"
    );
    assert_eq!(depth, 2);

    // The admin route surfaces the per-peer capacity.
    let mesh = fetch_mesh(&server).await;
    assert!(mesh.enabled);
    assert_eq!(mesh.peer_count, 3);
    let by_id = |id: &str| -> &lvqr_admin::MeshPeerStats {
        mesh.peers
            .iter()
            .find(|p| p.peer_id == id)
            .unwrap_or_else(|| panic!("{id} missing from /api/v1/mesh"))
    };
    assert_eq!(
        by_id("peer-1").capacity,
        Some(1),
        "peer-1 capacity must round-trip onto the admin route"
    );
    assert!(by_id("peer-2").capacity.is_none());
    assert!(by_id("peer-3").capacity.is_none());

    server.shutdown().await.expect("shutdown failed");
}

#[tokio::test]
async fn oversize_capacity_claim_is_clamped_to_global_max() {
    // max_peers (the operator's global ceiling) = 2. A client claim
    // of u32::MAX must clamp to 2 by the time it lands in
    // PeerInfo.capacity / the admin route.
    let server = TestServer::start(TestServerConfig::new().with_mesh(2).with_mesh_root_peer_count(1))
        .await
        .expect("TestServer::start");

    let mut s1 = connect_signal(&server).await;
    send_text(
        &mut s1,
        r#"{"type":"Register","peer_id":"greedy","track":"live/test","capacity":4294967295}"#,
    )
    .await;
    let _ = recv_assignment(&mut s1).await;

    let mesh = fetch_mesh(&server).await;
    let greedy = mesh
        .peers
        .iter()
        .find(|p| p.peer_id == "greedy")
        .expect("greedy peer present");
    assert_eq!(
        greedy.capacity,
        Some(2),
        "u32::MAX claim must clamp to the operator's global max-peers (2)"
    );

    server.shutdown().await.expect("shutdown failed");
}

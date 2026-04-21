//! WS-relay subscriber mesh registration tests (session 111-B2).
//!
//! Every `ws_relay_session` now registers its subscriber with
//! the `MeshCoordinator` at connect time (when mesh is enabled)
//! and sends the assigned `peer_id`, role, parent, and depth as
//! a leading JSON text frame before any binary MoQ frames flow.
//! This test locks that contract in.
//!
//! `two_ws_subscribers_receive_parent_assignment` boots a
//! TestServer with `mesh_enabled = true` and
//! `mesh_root_peer_count = 1` so the second subscriber becomes
//! a child of the first. It connects two WS clients in
//! sequence, reads the leading text frame from each, and
//! asserts both received a `peer_assignment` frame, the first
//! peer is `Root` with null parent, and the second peer is
//! `Relay` with parent equal to the first peer's id and depth
//! 1.
//!
//! `disconnect_removes_peer_from_coordinator` connects a WS
//! subscriber, reads the leading frame, drops the WS, and
//! asserts `mesh_coordinator().peer_count()` settles back to 0.
//! Guards the teardown path so subscribers leaving do not leak
//! tree entries.
//!
//! Tests ride the real TestServer + origin broadcast path, not
//! mocks: `ws_relay_session` consumes a live MoQ broadcast
//! created on the test thread so the session does not hang up
//! with `4404 broadcast not found` before the leading frame
//! reaches the client.

use std::time::Duration;

use futures_util::StreamExt;
use lvqr_test_utils::{TestServer, TestServerConfig};
use tokio_tungstenite::tungstenite::Message;

/// Read the leading text frame from a newly-upgraded WS and
/// parse it as a `peer_assignment` JSON value. Panics on any
/// protocol deviation (wrong frame type, malformed JSON,
/// unexpected `type` field, etc.) so the assertion surface in
/// the actual tests stays terse.
async fn read_peer_assignment<S>(ws: &mut S) -> serde_json::Value
where
    S: StreamExt<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timed out waiting for leading frame")
        .expect("WS closed before leading frame")
        .expect("WS read error");
    let text = match frame {
        Message::Text(t) => t.to_string(),
        other => panic!("expected leading Text frame, got {other:?}"),
    };
    let value: serde_json::Value = serde_json::from_str(&text).expect("leading frame is not JSON");
    assert_eq!(
        value.get("type").and_then(|v| v.as_str()),
        Some("peer_assignment"),
        "leading frame type mismatch: {value:?}"
    );
    value
}

#[tokio::test]
async fn two_ws_subscribers_receive_parent_assignment() {
    // `mesh_root_peer_count = 1` forces the second peer to become
    // a child of the first so the `AssignParent` path is exercised
    // without having to spin up 30 peers for the default root
    // count.
    let server = TestServer::start(TestServerConfig::new().with_mesh(3).with_mesh_root_peer_count(1))
        .await
        .expect("TestServer::start");

    // Create a MoQ broadcast + track so `ws_relay_session` finds
    // something to consume. Without this the session closes with
    // `4404 broadcast not found` before the leading mesh frame is
    // sent.
    let origin = server.origin();
    let mut moq_broadcast = origin
        .create_broadcast("live/demo")
        .expect("create moq broadcast on origin");
    let _moq_video = moq_broadcast
        .create_track(lvqr_moq::Track::new("0.mp4"))
        .expect("create moq video track");

    let ws_url = server.ws_url("live/demo");

    let (mut ws1, _resp1) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("first WS subscribe");
    let peer1 = read_peer_assignment(&mut ws1).await;
    assert_eq!(
        peer1.get("role").and_then(|v| v.as_str()),
        Some("Root"),
        "first peer should be Root: {peer1:?}"
    );
    assert!(
        peer1.get("parent_id").is_some_and(|v| v.is_null()),
        "first peer should have null parent_id: {peer1:?}"
    );
    assert_eq!(
        peer1.get("depth").and_then(|v| v.as_u64()),
        Some(0),
        "first peer depth should be 0: {peer1:?}"
    );
    let peer1_id = peer1
        .get("peer_id")
        .and_then(|v| v.as_str())
        .expect("first peer missing peer_id")
        .to_string();

    let (mut ws2, _resp2) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("second WS subscribe");
    let peer2 = read_peer_assignment(&mut ws2).await;
    assert_eq!(
        peer2.get("role").and_then(|v| v.as_str()),
        Some("Relay"),
        "second peer should be Relay: {peer2:?}"
    );
    assert_eq!(
        peer2.get("parent_id").and_then(|v| v.as_str()),
        Some(peer1_id.as_str()),
        "second peer should name first as parent: {peer2:?}"
    );
    assert_eq!(
        peer2.get("depth").and_then(|v| v.as_u64()),
        Some(1),
        "second peer depth should be 1: {peer2:?}"
    );

    // ServerHandle::mesh_coordinator accessor confirms the
    // coordinator holds both peers while the WS sessions stay
    // alive.
    let count = server
        .mesh_coordinator()
        .expect("mesh_coordinator is Some")
        .peer_count();
    assert_eq!(count, 2, "coordinator should see both WS peers; got {count}");

    drop(ws1);
    drop(ws2);
    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn disconnect_removes_peer_from_coordinator() {
    let server = TestServer::start(TestServerConfig::new().with_mesh(3))
        .await
        .expect("TestServer::start");

    let origin = server.origin();
    let mut moq_broadcast = origin.create_broadcast("live/demo").expect("create broadcast");
    let _moq_video = moq_broadcast
        .create_track(lvqr_moq::Track::new("0.mp4"))
        .expect("create track");

    let (mut ws, _resp) = tokio_tungstenite::connect_async(server.ws_url("live/demo"))
        .await
        .expect("ws connect");
    let _peer = read_peer_assignment(&mut ws).await;
    assert_eq!(
        server.mesh_coordinator().expect("coordinator").peer_count(),
        1,
        "coordinator should see the WS peer while connected"
    );

    drop(ws);

    // Poll until the coordinator sees the peer depart. The
    // deregistration happens on the server side after the WS
    // session loop exits, which can lag the client-side drop by
    // a handful of ms.
    let coordinator = server.mesh_coordinator().expect("coordinator").clone();
    for _ in 0..100 {
        if coordinator.peer_count() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        coordinator.peer_count(),
        0,
        "coordinator should have released the peer after WS drop"
    );

    server.shutdown().await.expect("shutdown");
}

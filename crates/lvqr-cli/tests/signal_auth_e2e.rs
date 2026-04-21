//! Mesh `/signal` subscribe-auth gate tests (session 111-B1).
//!
//! Before 111-B1, the `/signal` WebSocket was unauthenticated:
//! any client could connect, send `Register { peer_id, track }`,
//! and poll the mesh coordinator for tree assignments. Session
//! 111-B1 wraps the `/signal` router with a `SubscribeAuth` gate
//! that honors the `?token=<token>` query parameter and short-
//! circuits the upgrade with a 401 on deny.
//!
//! Coverage:
//!
//! 1. `signal_requires_token_when_auth_configured` -- configured
//!    subscribe-auth, `/signal` upgrade with no token -> 401
//!    (not a successful WS handshake).
//! 2. `signal_accepts_valid_token_query` -- same deployment with
//!    `?token=<token>` -> successful upgrade.
//! 3. `signal_rejects_wrong_token` -- wrong bearer is 401.
//! 4. `noop_signal_allows_any_upgrade` -- default (noop) auth
//!    provider lets `/signal` upgrade through without a token.
//! 5. `escape_hatch_disables_signal_auth` -- configured auth
//!    plus `without_signal_auth()` lets unauthed `/signal`
//!    upgrades through.
//! 6. `mesh_coordinator_accessor_reports_peers` -- the
//!    `ServerHandle::mesh_coordinator()` accessor returns a
//!    handle whose `peer_count()` increments when a
//!    subscribe-authed client connects to `/signal` and sends
//!    Register.
//!
//! Tests use `tokio-tungstenite` to drive real WebSocket
//! upgrades against the bound admin port. A failed upgrade
//! surfaces as a `tungstenite::Error::Http(401)`.

use std::sync::Arc;
use std::time::Duration;

use lvqr_auth::{SharedAuth, StaticAuthConfig, StaticAuthProvider};
use lvqr_test_utils::{TestServer, TestServerConfig};
use tokio_tungstenite::tungstenite;

fn static_subscribe_auth(token: &str) -> SharedAuth {
    Arc::new(StaticAuthProvider::new(StaticAuthConfig {
        admin_token: None,
        publish_key: None,
        subscribe_token: Some(token.to_string()),
    }))
}

/// Returns the HTTP status code from a failed upgrade or `None`
/// if the upgrade succeeded. Shields tests from the specific
/// error-variant walking that `tungstenite` requires.
async fn upgrade_status(url: &str) -> Option<u16> {
    match tokio_tungstenite::connect_async(url).await {
        Ok(_) => None,
        Err(tungstenite::Error::Http(resp)) => Some(resp.status().as_u16()),
        Err(other) => panic!("unexpected connect error: {other:?}"),
    }
}

#[tokio::test]
async fn signal_requires_token_when_auth_configured() {
    let server = TestServer::start(
        TestServerConfig::new()
            .with_mesh(3)
            .with_auth(static_subscribe_auth("viewer-secret")),
    )
    .await
    .expect("TestServer::start");

    let url = server.signal_url();
    let status = upgrade_status(&url).await;
    assert_eq!(
        status,
        Some(401),
        "expected 401 on /signal without token; got {status:?}"
    );

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn signal_accepts_valid_token_query() {
    let server = TestServer::start(
        TestServerConfig::new()
            .with_mesh(3)
            .with_auth(static_subscribe_auth("viewer-secret")),
    )
    .await
    .expect("TestServer::start");

    let url = format!("{}?token=viewer-secret", server.signal_url());
    let (ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("signal upgrade should have succeeded with valid token");
    drop(ws);

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn signal_rejects_wrong_token() {
    let server = TestServer::start(
        TestServerConfig::new()
            .with_mesh(3)
            .with_auth(static_subscribe_auth("viewer-secret")),
    )
    .await
    .expect("TestServer::start");

    let url = format!("{}?token=wrong-token", server.signal_url());
    let status = upgrade_status(&url).await;
    assert_eq!(
        status,
        Some(401),
        "expected 401 on /signal with wrong token; got {status:?}"
    );

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn noop_signal_allows_any_upgrade() {
    let server = TestServer::start(TestServerConfig::new().with_mesh(3))
        .await
        .expect("TestServer::start");

    let url = server.signal_url();
    let (ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("noop auth must allow /signal upgrade without a token");
    drop(ws);

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn escape_hatch_disables_signal_auth() {
    let server = TestServer::start(
        TestServerConfig::new()
            .with_mesh(3)
            .with_auth(static_subscribe_auth("viewer-secret"))
            .without_signal_auth(),
    )
    .await
    .expect("TestServer::start");

    let url = server.signal_url();
    let (ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("escape hatch must allow /signal upgrade without a token");
    drop(ws);

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn mesh_coordinator_accessor_reports_peers() {
    // root_peer_count = 1 so peer-1 is the root and peer-2 is
    // a child of peer-1; keeps the integration test assertion
    // surface narrow without having to boot 30 peers.
    let server = TestServer::start(
        TestServerConfig::new()
            .with_mesh(3)
            .with_mesh_root_peer_count(1)
            .with_auth(static_subscribe_auth("viewer-secret")),
    )
    .await
    .expect("TestServer::start");

    let url = format!("{}?token=viewer-secret", server.signal_url());
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url).await.expect("signal upgrade");

    // Send Register so the signal callback calls add_peer on the
    // mesh coordinator. Without this, the coordinator sees no
    // peers even though the WS upgrade succeeded.
    use tokio_tungstenite::tungstenite::Message;
    let register = serde_json::json!({
        "type": "Register",
        "peer_id": "peer-1",
        "track": "live/demo",
    })
    .to_string();
    use futures_util::SinkExt;
    ws.send(Message::Text(register)).await.expect("ws send");

    // Give the server a moment to process the Register.
    for _ in 0..50 {
        if server.mesh_coordinator().map(|m| m.peer_count()).unwrap_or(0) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let count = server
        .mesh_coordinator()
        .expect("mesh coordinator should be present when mesh is enabled")
        .peer_count();
    assert_eq!(
        count, 1,
        "mesh coordinator should have 1 peer after Register; got {count}"
    );

    drop(ws);
    server.shutdown().await.expect("shutdown");
}

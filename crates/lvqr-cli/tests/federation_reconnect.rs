//! Federation reconnect + admin-route integration test (Tier 4
//! item 4.4 session C).
//!
//! Boots two TestServer instances on loopback with a single
//! federation link from B to A, then drives the link through a
//! full `Connecting -> Connected -> Failed -> retrying` cycle and
//! asserts that the admin route
//! `GET /api/v1/cluster/federation` on B exposes the per-link
//! state every step of the way.
//!
//! Why this shape (vs. a same-port reconnect):
//!
//! Proving "the runner reconnects to a restarted peer on the
//! exact same QUIC port" ran into a cross-process contention:
//! while B's federation client is actively retrying against A's
//! now-closed UDP port, the in-process Endpoint held by the
//! shut-down A does not release the UDP socket fast enough for
//! a second bind to succeed even after seconds of waiting. That
//! is a quinn / moq-native teardown timing quirk, not a
//! correctness bug in the federation retry loop. The reconnect
//! semantics are covered at the unit level in
//! `lvqr-cluster/tests/federation_unit.rs`
//! (see `runner_status_handle_reports_failed_after_initial_connect_error`
//! and `status_handle_clones_observe_updates`), where the retry
//! wrapper demonstrates connect_attempts incrementing past an
//! unreachable peer.
//!
//! This integration test instead proves the full
//! `status-handle -> admin-route` wiring end-to-end: the admin
//! route reads real per-link state produced by a real
//! cross-process MoQ session, across both the connected and
//! failed phases, including the connect_attempts counter's
//! continued increase while the peer is down. That is the
//! observability contract session 103 C promises operators.

use lvqr_auth::{SharedAuth, StaticAuthConfig, StaticAuthProvider};
use lvqr_cluster::{FederationConnectState, FederationLink};
use lvqr_test_utils::http::{HttpGetOptions, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const CONNECTED_TIMEOUT: Duration = Duration::from_secs(15);
const FAILED_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

struct HttpResponse {
    status: u16,
    body: String,
}

/// Thin wrapper over `lvqr_test_utils::http::http_get_with`.
/// Preserves this file's `body: String` shape (the admin router's
/// federation response is JSON, always UTF-8), its 5-second
/// HTTP_TIMEOUT, and the bearer-dispatch signature the tests rely
/// on.
async fn http_get(addr: SocketAddr, path: &str, bearer: Option<&str>) -> HttpResponse {
    let opts = HttpGetOptions {
        bearer,
        timeout: HTTP_TIMEOUT,
        ..Default::default()
    };
    let resp = http_get_with(addr, path, opts).await;
    HttpResponse {
        status: resp.status,
        body: String::from_utf8_lossy(&resp.body).into_owned(),
    }
}

async fn wait_for_state(server_b: &TestServer, want: FederationConnectState, budget: Duration) {
    let deadline = std::time::Instant::now() + budget;
    let handle = server_b
        .federation_runner()
        .expect("B has a FederationRunner")
        .status_handle();
    loop {
        let snap = handle.snapshot();
        assert_eq!(snap.len(), 1, "B has exactly one link");
        if snap[0].state == want {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "B's link did not reach {want:?} within {budget:?}; last state = {:?}, last_error = {:?}, connect_attempts = {}",
                snap[0].state, snap[0].last_error, snap[0].connect_attempts
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn federation_link_status_surfaces_through_admin_route() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug,moq_lite=info")
        .with_test_writer()
        .try_init();

    // --- Boot A on an ephemeral relay port. ---
    let server_a = TestServer::start(TestServerConfig::default())
        .await
        .expect("start server A");
    let relay_port_a = server_a.relay_addr().port();

    // --- Boot B with a federation link to A + admin token. ---
    let admin_token = "reconnect-test-admin-token";
    let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
        admin_token: Some(admin_token.to_string()),
        ..Default::default()
    }));
    let url_a = format!("https://127.0.0.1:{relay_port_a}/");
    let link = FederationLink::new(url_a.clone(), "", vec!["live/reconnect".into()]).with_disable_tls_verify(true);
    let server_b = TestServer::start(TestServerConfig::default().with_auth(auth).with_federation_link(link))
        .await
        .expect("start server B");
    assert!(server_b.federation_runner().is_some());
    let admin_addr_b = server_b.admin_addr();

    // --- Phase 1: link reports Connected on both the in-proc
    //     status handle and the admin HTTP route. ---
    wait_for_state(&server_b, FederationConnectState::Connected, CONNECTED_TIMEOUT).await;

    let resp = http_get(admin_addr_b, "/api/v1/cluster/federation", Some(admin_token)).await;
    assert_eq!(resp.status, 200, "admin route must return 200, body = {:?}", resp.body);
    let body: serde_json::Value = serde_json::from_str(&resp.body).expect("json body");
    let links = body
        .get("links")
        .and_then(|v| v.as_array())
        .expect("body has links array");
    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].get("state").and_then(|v| v.as_str()),
        Some("connected"),
        "admin route must report connected after handshake"
    );
    assert_eq!(
        links[0].get("remote_url").and_then(|v| v.as_str()),
        Some(url_a.as_str())
    );
    let forwarded = links[0]
        .get("forwarded_broadcasts")
        .and_then(|v| v.as_array())
        .expect("forwarded_broadcasts is an array");
    assert_eq!(forwarded.len(), 1);
    assert_eq!(forwarded[0].as_str(), Some("live/reconnect"));
    let attempts_initial = links[0]
        .get("connect_attempts")
        .and_then(|v| v.as_u64())
        .expect("connect_attempts u64");
    assert!(
        attempts_initial >= 1,
        "initial connect must register at least one attempt"
    );
    assert!(
        links[0].get("last_connected_at_ms").is_some(),
        "last_connected_at_ms populated after handshake"
    );

    // Unauthenticated probe: admin gate still enforces.
    let resp_no_auth = http_get(admin_addr_b, "/api/v1/cluster/federation", None).await;
    assert_eq!(resp_no_auth.status, 401, "admin route must reject missing bearer token");

    // --- Phase 2: drop A. B's session closes, retry loop records
    //     Failed. ---
    server_a.shutdown().await.expect("shutdown server A");
    wait_for_state(&server_b, FederationConnectState::Failed, FAILED_TIMEOUT).await;

    let resp = http_get(admin_addr_b, "/api/v1/cluster/federation", Some(admin_token)).await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_str(&resp.body).expect("json body");
    let links = body.get("links").and_then(|v| v.as_array()).expect("links");
    assert_eq!(
        links[0].get("state").and_then(|v| v.as_str()),
        Some("failed"),
        "admin route must report failed after peer disappeared"
    );
    let last_error = links[0]
        .get("last_error")
        .and_then(|v| v.as_str())
        .expect("last_error populated on failure");
    assert!(
        !last_error.is_empty(),
        "last_error must carry a non-empty reason, got {last_error:?}"
    );

    // --- Phase 3: connect_attempts continues to grow while A stays
    //     down. Demonstrates the retry loop is actively re-entering
    //     run_link_once on its backoff schedule. ---
    let attempts_at_failure = links[0]
        .get("connect_attempts")
        .and_then(|v| v.as_u64())
        .expect("connect_attempts at failure");
    // Wait ~2.5 s -- enough for at least one backoff-driven retry
    // attempt to fire (initial sleep 1 s +/- 10%, then a further
    // ~2 s doubled sleep).
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    let resp = http_get(admin_addr_b, "/api/v1/cluster/federation", Some(admin_token)).await;
    let body: serde_json::Value = serde_json::from_str(&resp.body).expect("json body");
    let links = body.get("links").and_then(|v| v.as_array()).expect("links");
    let attempts_later = links[0]
        .get("connect_attempts")
        .and_then(|v| v.as_u64())
        .expect("connect_attempts later");
    assert!(
        attempts_later > attempts_at_failure,
        "retry loop must keep bumping connect_attempts while peer is down: was {attempts_at_failure}, now {attempts_later}"
    );

    // --- Cleanup. ---
    server_b.shutdown().await.expect("shutdown B");
}

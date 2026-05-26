//! End-to-end test for the Slice 9b ingest-listener inventory + STOP routes
//! (`GET /api/v1/ingest`, `DELETE /api/v1/ingest/{protocol}`).
//!
//! Each ingest listener (RTMP / WHIP / SRT / RTSP) holds a **child**
//! `CancellationToken` of the shared `shutdown`; cancelling the child via the
//! admin route takes down only that listener task without affecting any
//! other listener or the global shutdown. The strongest assertion is that a
//! TCP connect to the listener's bound port FAILS after the stop -- the
//! socket has actually closed, not just the bookkeeping flipped.
//!
//! This test is feature-agnostic: the default `TestServer` always binds
//! RTMP, so the RTMP path is exercised on every CI lane that compiles
//! lvqr-cli's tests.

use lvqr_test_utils::http::{HttpGetOptions, http_delete, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::time::Duration;
use tokio::net::TcpStream;

/// Helper: snapshot the listener inventory the admin route reports.
async fn ingest_listeners(admin_addr: std::net::SocketAddr) -> serde_json::Value {
    let resp = http_get_with(admin_addr, "/api/v1/ingest", HttpGetOptions::default()).await;
    assert_eq!(resp.status, 200, "GET /ingest must 200; got {}", resp.status);
    serde_json::from_slice(&resp.body).expect("valid JSON")
}

/// Helper: is the named listener present + enabled in the inventory?
fn listener_enabled(body: &serde_json::Value, protocol: &str) -> Option<bool> {
    body["listeners"].as_array()?.iter().find_map(|l| {
        if l["protocol"].as_str()? == protocol {
            l["enabled"].as_bool()
        } else {
            None
        }
    })
}

/// Helper: attempt a brief TCP connect to the listener's bound port. Returns
/// `true` if the listener is still accepting (handshake reachable), `false`
/// if the kernel refuses or the attempt times out.
async fn rtmp_port_accepts(addr: std::net::SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_millis(500), TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ingest_listeners_listed_and_rtmp_stop_closes_the_port() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let admin_addr = server.admin_addr();
    let rtmp_addr = server.rtmp_addr();

    // Initial inventory: RTMP is bound and enabled.
    let inv = ingest_listeners(admin_addr).await;
    assert_eq!(
        listener_enabled(&inv, "rtmp"),
        Some(true),
        "rtmp must be enabled at startup; got inventory={inv}"
    );
    assert!(
        rtmp_port_accepts(rtmp_addr).await,
        "rtmp port {rtmp_addr} must accept TCP before stop",
    );

    // STOP the RTMP listener via the admin route.
    let resp = http_delete(admin_addr, "/api/v1/ingest/rtmp", None).await;
    assert_eq!(
        resp.status,
        200,
        "stop status={} body={}",
        resp.status,
        resp.body_text()
    );
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("valid JSON");
    assert_eq!(body["result"], "stopped");
    assert_eq!(body["protocol"], "rtmp");

    // Inventory now reflects the disabled state (the row stays so the UI can
    // show "rtmp stopped" rather than letting it silently vanish).
    let inv = ingest_listeners(admin_addr).await;
    assert_eq!(
        listener_enabled(&inv, "rtmp"),
        Some(false),
        "rtmp must report enabled=false after stop"
    );

    // STRONGEST assertion: the OS socket is actually closed, not just the
    // bookkeeping flipped. We poll briefly because the listener task needs a
    // moment to observe the cancellation and unbind. macOS occasionally
    // accepts one more SYN after cancel, so we tolerate up to ~500ms.
    let mut closed = false;
    for _ in 0..10 {
        if !rtmp_port_accepts(rtmp_addr).await {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        closed,
        "rtmp port {rtmp_addr} must refuse new TCP connections after stop"
    );

    // Idempotent: a repeat stop is still 200 with result=already_stopped.
    let again = http_delete(admin_addr, "/api/v1/ingest/rtmp", None).await;
    assert_eq!(again.status, 200, "repeat stop must be idempotent (200)");
    let body: serde_json::Value = serde_json::from_slice(&again.body).expect("valid JSON");
    assert_eq!(body["result"], "already_stopped");

    // Unknown protocol -> 404 (no listener of that name bound on this relay).
    let ghost = http_delete(admin_addr, "/api/v1/ingest/ghost", None).await;
    assert_eq!(ghost.status, 404, "unknown protocol must 404; got {}", ghost.status);

    server.shutdown().await.expect("shutdown");
}

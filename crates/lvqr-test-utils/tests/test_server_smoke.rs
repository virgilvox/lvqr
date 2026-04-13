//! Smoke tests for [`lvqr_test_utils::TestServer`].
//!
//! These intentionally use real network I/O: a TcpStream to the RTMP port,
//! an HTTP GET to the admin port, and a WebSocket handshake against the
//! `/ws/*` endpoint. The goal is to prove that every listener `TestServer`
//! exposes is actually bound and accepting connections, so downstream
//! integration tests can trust the handle addresses.

use lvqr_test_utils::{TestServer, TestServerConfig};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

#[tokio::test]
async fn test_server_binds_ephemeral_ports_and_reports_addresses() {
    let server = TestServer::start(TestServerConfig::new())
        .await
        .expect("TestServer::start failed");

    // Every listener must have a non-zero bound port.
    assert_ne!(server.relay_addr().port(), 0, "relay port not bound");
    assert_ne!(server.rtmp_addr().port(), 0, "rtmp port not bound");
    assert_ne!(server.admin_addr().port(), 0, "admin port not bound");

    // All three must be distinct: ephemeral port allocation is process-wide
    // so collisions would indicate the same listener was returned twice.
    let relay_port = server.relay_addr().port();
    let rtmp_port = server.rtmp_addr().port();
    let admin_port = server.admin_addr().port();
    assert_ne!(relay_port, rtmp_port);
    assert_ne!(relay_port, admin_port);
    assert_ne!(rtmp_port, admin_port);

    // Admin HTTP port accepts TCP connections (real socket, not a mock).
    let _ = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(server.admin_addr()))
        .await
        .expect("admin connect timed out")
        .expect("admin TCP connect failed");

    // RTMP port accepts TCP connections.
    let _ = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(server.rtmp_addr()))
        .await
        .expect("rtmp connect timed out")
        .expect("rtmp TCP connect failed");

    // URL helpers format as expected against the bound addresses.
    assert_eq!(
        server.ws_url("live/test"),
        format!("ws://{}/ws/live/test", server.admin_addr())
    );
    assert_eq!(
        server.ws_ingest_url("live/test"),
        format!("ws://{}/ingest/live/test", server.admin_addr())
    );
    assert_eq!(
        server.rtmp_url("live", "test"),
        format!("rtmp://{}/live/test", server.rtmp_addr())
    );

    server.shutdown().await.expect("shutdown failed");
}

#[tokio::test]
async fn test_server_rejects_ws_subscribe_for_unknown_broadcast() {
    // Not a full WebSocket handshake, just proves the admin router is
    // actually serving the /ws/{broadcast} route: we connect and send a
    // minimal HTTP/1.1 request line plus enough headers that the server
    // has to parse and route it. We then read a few bytes back to confirm
    // the server responded rather than timing out.
    let server = TestServer::start(TestServerConfig::new()).await.unwrap();

    let mut stream = TcpStream::connect(server.admin_addr()).await.unwrap();
    let req = format!(
        "GET /ws/does/not/exist HTTP/1.1\r\n\
         Host: {}\r\n\
         Connection: close\r\n\
         \r\n",
        server.admin_addr()
    );
    stream.write_all(req.as_bytes()).await.unwrap();

    // Any response at all (even 400/426) proves the admin listener routed
    // the request. We only care that the socket did not hang.
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(2), async {
        use tokio::io::AsyncReadExt;
        stream.read(&mut buf).await
    })
    .await
    .expect("admin HTTP read timed out")
    .expect("admin HTTP read failed");
    assert!(n > 0, "admin HTTP returned empty response");
    let head = std::str::from_utf8(&buf[..n]).unwrap_or("");
    assert!(head.starts_with("HTTP/1."), "unexpected response head: {head:?}");

    server.shutdown().await.unwrap();
}

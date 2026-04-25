//! End-to-end test for hot config reload (session 147).
//!
//! Boots a `TestServer` with `--config <path>` pointing at a TOML
//! file containing `[auth] publish_key = "v1"`. Verifies:
//!
//! 1. RTMP publish with `"v1"` succeeds (file applied at boot).
//! 2. RTMP publish with `"v2"` denied.
//! 3. Rewrite the file with `publish_key = "v2"`.
//! 4. `POST /api/v1/config-reload`. 200, body confirms `applied_keys: ["auth"]`.
//! 5. RTMP publish with `"v1"` denied (old token invalidated).
//! 6. RTMP publish with `"v2"` succeeds (new provider live).
//! 7. `GET /api/v1/config-reload` reports the most recent reload's
//!    timestamp + kind (`"admin_post"`).
//!
//! No mocks. The test goes through `lvqr_cli::start` exactly as
//! `lvqr serve --config foo.toml` does.

use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const RTMP_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

// =====================================================================
// Raw HTTP/1.1 helpers (inline, no reqwest dev-dep). Same shape as
// `streamkeys_e2e.rs`.
// =====================================================================

struct AdminResponse {
    status: u16,
    body: Vec<u8>,
}

async fn http_request(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> AdminResponse {
    let body_bytes = body.unwrap_or("").as_bytes();
    let host = addr.to_string();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {len}\r\n",
        len = body_bytes.len()
    );
    if body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str("\r\n");
    let mut buf = req.into_bytes();
    buf.extend_from_slice(body_bytes);

    let mut stream = tokio::time::timeout(HTTP_TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("admin connect timed out")
        .expect("admin connect failed");
    stream.write_all(&buf).await.expect("admin write");
    let mut raw = Vec::with_capacity(4096);
    tokio::time::timeout(HTTP_TIMEOUT, stream.read_to_end(&mut raw))
        .await
        .expect("admin read timed out")
        .expect("admin read");

    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("admin response missing header/body separator");
    let header = std::str::from_utf8(&raw[..split]).expect("admin headers not utf-8");
    let status: u16 = header
        .lines()
        .next()
        .expect("missing status line")
        .split_whitespace()
        .nth(1)
        .expect("missing status code")
        .parse()
        .expect("status not numeric");
    AdminResponse {
        status,
        body: raw[split + 4..].to_vec(),
    }
}

async fn admin_get(addr: SocketAddr, path: &str) -> AdminResponse {
    http_request(addr, "GET", path, None).await
}

async fn admin_post(addr: SocketAddr, path: &str, body: Option<&str>) -> AdminResponse {
    http_request(addr, "POST", path, body).await
}

// =====================================================================
// RTMP publish helpers (lifted from streamkeys_e2e.rs / one_token_all_protocols.rs).
// =====================================================================

async fn rtmp_handshake(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut handshake = Handshake::new(PeerType::Client);
    let p0_and_p1 = handshake
        .generate_outbound_p0_and_p1()
        .map_err(|e| std::io::Error::other(format!("p0p1: {e:?}")))?;
    stream.write_all(&p0_and_p1).await?;
    let mut buf = vec![0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "server closed during RTMP handshake",
            ));
        }
        match handshake
            .process_bytes(&buf[..n])
            .map_err(|e| std::io::Error::other(format!("handshake: {e:?}")))?
        {
            HandshakeProcessResult::InProgress { response_bytes } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await?;
                }
            }
            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await?;
                }
                return Ok(remaining_bytes);
            }
        }
    }
}

async fn write_outbound(stream: &mut TcpStream, results: &[ClientSessionResult]) -> std::io::Result<()> {
    for r in results {
        if let ClientSessionResult::OutboundResponse(p) = r {
            stream.write_all(&p.bytes).await?;
        }
    }
    Ok(())
}

async fn try_rtmp_publish(addr: SocketAddr, app: &str, key: &str) -> bool {
    let attempt = async {
        let mut stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        let remaining = rtmp_handshake(&mut stream).await?;

        let (mut session, initial) = ClientSession::new(ClientSessionConfig::new())
            .map_err(|e| std::io::Error::other(format!("client session: {e:?}")))?;
        write_outbound(&mut stream, &initial).await?;
        if !remaining.is_empty() {
            let r = session
                .handle_input(&remaining)
                .map_err(|e| std::io::Error::other(format!("handle_input: {e:?}")))?;
            write_outbound(&mut stream, &r).await?;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;

        let connect = session
            .request_connection(app.to_string())
            .map_err(|e| std::io::Error::other(format!("connect req: {e:?}")))?;
        write_outbound(&mut stream, std::slice::from_ref(&connect)).await?;

        let mut buf = vec![0u8; 65536];
        let mut connected = false;
        while !connected {
            let n = stream.read(&mut buf).await?;
            if n == 0 {
                return Ok::<bool, std::io::Error>(false);
            }
            let results = session
                .handle_input(&buf[..n])
                .map_err(|e| std::io::Error::other(format!("handle_input: {e:?}")))?;
            for r in results {
                match r {
                    ClientSessionResult::OutboundResponse(p) => {
                        stream.write_all(&p.bytes).await?;
                    }
                    ClientSessionResult::RaisedEvent(ClientSessionEvent::ConnectionRequestAccepted) => {
                        connected = true;
                    }
                    _ => {}
                }
            }
        }

        let publish = session
            .request_publishing(key.to_string(), PublishRequestType::Live)
            .map_err(|e| std::io::Error::other(format!("publish req: {e:?}")))?;
        write_outbound(&mut stream, std::slice::from_ref(&publish)).await?;

        loop {
            let n = stream.read(&mut buf).await?;
            if n == 0 {
                return Ok(false);
            }
            let results = session
                .handle_input(&buf[..n])
                .map_err(|e| std::io::Error::other(format!("handle_input: {e:?}")))?;
            for r in results {
                match r {
                    ClientSessionResult::OutboundResponse(p) => {
                        stream.write_all(&p.bytes).await?;
                    }
                    ClientSessionResult::RaisedEvent(ClientSessionEvent::PublishRequestAccepted) => {
                        return Ok(true);
                    }
                    _ => {}
                }
            }
        }
    };

    match tokio::time::timeout(RTMP_TIMEOUT, attempt).await {
        Ok(Ok(accepted)) => accepted,
        Ok(Err(_)) => false,
        Err(_) => false,
    }
}

// =====================================================================
// Tests
// =====================================================================

/// Lifecycle: file applied at boot -> POST reload swaps provider ->
/// old key denied + new key accepted.
#[tokio::test]
async fn config_reload_swaps_publish_key_via_admin_post() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("lvqr.toml");
    std::fs::write(
        &path,
        r#"[auth]
publish_key = "v1""#,
    )
    .expect("write v1");

    // TestServer is wrapped with a streamkey-CRUD-disabled config so
    // the auth chain is only StaticAuthProvider; this isolates the
    // reload behavior to that one provider's state.
    let server = TestServer::start(
        TestServerConfig::new()
            .with_config_file(path.clone())
            .with_no_streamkeys(),
    )
    .await
    .expect("TestServer::start");
    let admin_addr = server.admin_addr();
    let rtmp_addr = server.rtmp_addr();

    // Boot reload applied "v1" from the file. RTMP publish with v1 wins.
    assert!(
        try_rtmp_publish(rtmp_addr, "live", "v1").await,
        "v1 must publish (file applied at boot)"
    );
    assert!(
        !try_rtmp_publish(rtmp_addr, "live", "v2").await,
        "v2 must be denied pre-reload"
    );

    // Rewrite file -> publish_key v2.
    std::fs::write(
        &path,
        r#"[auth]
publish_key = "v2""#,
    )
    .expect("write v2");

    // POST reload.
    let resp = admin_post(admin_addr, "/api/v1/config-reload", None).await;
    assert_eq!(
        resp.status,
        200,
        "POST /api/v1/config-reload must succeed; body: {:?}",
        String::from_utf8_lossy(&resp.body)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&resp.body).expect("config-reload body is JSON");
    assert_eq!(parsed["applied_keys"][0], "auth");
    assert_eq!(parsed["last_reload_kind"], "admin_post");
    assert!(parsed["last_reload_at_ms"].is_number());

    // After reload: v1 denied, v2 accepted.
    assert!(
        !try_rtmp_publish(rtmp_addr, "live", "v1").await,
        "v1 must be denied post-reload"
    );
    assert!(
        try_rtmp_publish(rtmp_addr, "live", "v2").await,
        "v2 must publish post-reload"
    );

    // GET reflects the most-recent reload's metadata.
    let status = admin_get(admin_addr, "/api/v1/config-reload").await;
    assert_eq!(status.status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&status.body).expect("GET body is JSON");
    assert_eq!(parsed["last_reload_kind"], "admin_post");
    assert_eq!(parsed["applied_keys"][0], "auth");
    assert_eq!(parsed["config_path"].as_str().unwrap_or(""), path.display().to_string());

    server.shutdown().await.expect("shutdown");
}

/// Negative path: when the server boots WITHOUT --config, POST
/// returns 503 and GET returns a default-shaped body.
#[tokio::test]
async fn config_reload_is_503_without_config_flag() {
    let server = TestServer::start(TestServerConfig::new().with_no_streamkeys())
        .await
        .expect("TestServer::start");
    let admin_addr = server.admin_addr();

    let post = admin_post(admin_addr, "/api/v1/config-reload", None).await;
    assert_eq!(
        post.status, 503,
        "POST without --config must be 503 (route mounted but disabled)"
    );

    let get = admin_get(admin_addr, "/api/v1/config-reload").await;
    assert_eq!(get.status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&get.body).expect("GET body");
    assert!(parsed["last_reload_at_ms"].is_null(), "no reload has occurred yet");
    assert!(parsed["config_path"].is_null(), "no --config configured");

    server.shutdown().await.expect("shutdown");
}

/// Malformed reload: rewrite the file with garbage TOML, POST reload,
/// observe 500. The prior provider stays live (v1 still publishes).
#[tokio::test]
async fn config_reload_malformed_file_keeps_prior_provider() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("lvqr.toml");
    std::fs::write(
        &path,
        r#"[auth]
publish_key = "v1""#,
    )
    .expect("write v1");

    let server = TestServer::start(
        TestServerConfig::new()
            .with_config_file(path.clone())
            .with_no_streamkeys(),
    )
    .await
    .expect("TestServer::start");
    let admin_addr = server.admin_addr();
    let rtmp_addr = server.rtmp_addr();

    // Baseline.
    assert!(try_rtmp_publish(rtmp_addr, "live", "v1").await);

    // Corrupt the file.
    std::fs::write(&path, "this is = not = valid toml").expect("rewrite garbage");

    // POST reload errors. 500.
    let resp = admin_post(admin_addr, "/api/v1/config-reload", None).await;
    assert_eq!(
        resp.status,
        500,
        "malformed file must surface as 500; body: {:?}",
        String::from_utf8_lossy(&resp.body)
    );

    // Prior provider still live.
    assert!(
        try_rtmp_publish(rtmp_addr, "live", "v1").await,
        "prior provider survives a failed reload"
    );

    server.shutdown().await.expect("shutdown");
}

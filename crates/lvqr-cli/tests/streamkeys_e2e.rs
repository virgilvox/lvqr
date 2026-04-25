//! End-to-end test for the runtime stream-key CRUD admin API
//! (session 146).
//!
//! Boots a real `TestServer` whose fallback auth provider denies
//! every publish (a `StaticAuthProvider` with `publish_key =
//! Some("never-matches")`). Then drives the full CRUD lifecycle
//! through `/api/v1/streamkeys`:
//!
//! 1. Baseline: an arbitrary RTMP key is denied (proves the test
//!    fixture genuinely gates ingest).
//! 2. Mint a stream-key via `POST /api/v1/streamkeys`. Capture the
//!    returned `token`.
//! 3. RTMP publish using the minted token as the stream key:
//!    accepted (`MultiKeyAuthProvider` store-hits the token).
//! 4. `DELETE /api/v1/streamkeys/{id}`. 204.
//! 5. RTMP publish with the same token: now denied (store miss
//!    falls through to the deny-by-default fallback).
//!
//! No mocks. The test goes through `lvqr_cli::start` exactly as
//! `lvqr serve` does, with real TCP sockets against real axum
//! routes and a real `rml_rtmp` client driving the publish
//! handshake.

use lvqr_admin::StreamKeyList;
use lvqr_auth::{SharedAuth, StaticAuthConfig, StaticAuthProvider, StreamKey};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const RTMP_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

// =====================================================================
// Raw HTTP/1.1 helpers (inline to avoid pulling reqwest as a dev-dep).
// One-shot per call: open TCP, write request bytes, read until the
// peer closes or the timeout fires. The admin router emits
// Content-Length on every body and closes after each response, so
// "read until EOF" gives us the full body without HTTP parsing.
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

    parse_admin_response(&raw)
}

fn parse_admin_response(raw: &[u8]) -> AdminResponse {
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

async fn admin_post(addr: SocketAddr, path: &str, body: &str) -> AdminResponse {
    http_request(addr, "POST", path, Some(body)).await
}

async fn admin_delete(addr: SocketAddr, path: &str) -> AdminResponse {
    http_request(addr, "DELETE", path, None).await
}

// =====================================================================
// RTMP publish helpers (lifted verbatim from one_token_all_protocols.rs
// because no shared module owns them; one-shot test files duplicating
// the publish driver is the established repo pattern).
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

/// Drive the RTMP handshake + connect + publish-request flow and
/// wait briefly for `PublishRequestAccepted`. Returns `true` on
/// accept, `false` on deny (the bridge closes the TCP stream when
/// auth denies, which surfaces as an EOF read here).
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

        // Yield briefly so the server's post-handshake control
        // messages arrive before we serialise the connect command.
        // Same wait pattern as `crates/lvqr-cli/tests/rtmp_archive_e2e.rs`.
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
// Test fixture
// =====================================================================

/// Boot a TestServer whose fallback auth denies every publish unless
/// the stream key matches a baseline string nobody actually uses
/// (`"never-matches"`). The empty stream-key store sits in front so
/// every Publish auth check runs the documented order:
/// store.get_by_token -> miss -> StaticAuthProvider -> deny.
async fn server_with_deny_fallback() -> TestServer {
    let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
        publish_key: Some("never-matches".into()),
        admin_token: None,
        subscribe_token: None,
    }));
    TestServer::start(TestServerConfig::new().with_auth(auth))
        .await
        .expect("TestServer::start")
}

// =====================================================================
// Tests
// =====================================================================

/// Primary lifecycle test.
///
/// Order matters: each step builds the assertion the next step
/// asserts against. Splitting the steps into separate `#[test]`s
/// would force four independent server boots and lose the
/// post-revoke deny check (the very assertion the brief calls out
/// as the proof streamkey CRUD is enforcement, not observability).
#[tokio::test]
async fn streamkey_lifecycle_mint_publish_revoke_deny() {
    let server = server_with_deny_fallback().await;
    let admin_addr = server.admin_addr();
    let rtmp_addr = server.rtmp_addr();

    // Step 1. Baseline -- arbitrary RTMP key denied by fallback.
    assert!(
        !try_rtmp_publish(rtmp_addr, "live", "definitely-not-allowed").await,
        "test fixture must deny arbitrary publishes (fallback is StaticAuth with publish_key=never-matches)"
    );

    // Step 2. Mint a stream-key via the admin API.
    let mint = admin_post(admin_addr, "/api/v1/streamkeys", r#"{"label":"e2e-test"}"#).await;
    assert_eq!(
        mint.status,
        201,
        "mint must return 201; body: {:?}",
        String::from_utf8_lossy(&mint.body)
    );
    let key: StreamKey = serde_json::from_slice(&mint.body).expect("mint body is StreamKey JSON");
    assert!(
        key.token.starts_with("lvqr_sk_"),
        "minted token must carry the lvqr_sk_ prefix; got {:?}",
        key.token
    );

    // Step 2b. Confirm the list endpoint surfaces the new entry.
    let list = admin_get(admin_addr, "/api/v1/streamkeys").await;
    assert_eq!(list.status, 200);
    let listed: StreamKeyList = serde_json::from_slice(&list.body).expect("list body is StreamKeyList");
    assert_eq!(listed.keys.len(), 1, "list must surface the just-minted key");
    assert_eq!(listed.keys[0].id, key.id);

    // Step 3. RTMP publish with the minted token must succeed.
    assert!(
        try_rtmp_publish(rtmp_addr, "live", &key.token).await,
        "RTMP publish with minted token must be accepted by MultiKeyAuthProvider"
    );

    // Step 4. Revoke.
    let revoke = admin_delete(admin_addr, &format!("/api/v1/streamkeys/{}", key.id)).await;
    assert_eq!(revoke.status, 204, "revoke must return 204 No Content");

    // Step 5. RTMP publish with the same token must now be denied
    // (store miss falls through to StaticAuthProvider, which denies
    // the token because it doesn't equal "never-matches").
    assert!(
        !try_rtmp_publish(rtmp_addr, "live", &key.token).await,
        "post-revoke RTMP publish with the same token must be denied by the fallback"
    );

    server.shutdown().await.expect("shutdown");
}

/// Rotate is also enforcement: a publisher still using the old
/// token after a rotate gets denied. This is the operator-facing
/// guarantee that a leaked key can be replaced atomically without
/// recreating the entry's id or scope.
#[tokio::test]
async fn streamkey_rotate_invalidates_old_token_on_publish() {
    let server = server_with_deny_fallback().await;
    let admin_addr = server.admin_addr();
    let rtmp_addr = server.rtmp_addr();

    let mint = admin_post(admin_addr, "/api/v1/streamkeys", r#"{}"#).await;
    assert_eq!(mint.status, 201);
    let original: StreamKey = serde_json::from_slice(&mint.body).expect("mint body");

    // Old token works.
    assert!(try_rtmp_publish(rtmp_addr, "live", &original.token).await);

    // Rotate (empty body -- preserve scope, swap token). SDKs
    // sending no override idiomatically POST a zero-length body;
    // the rotate handler parses raw bytes specifically to keep
    // that round-trip clean.
    let rotate = http_request(
        admin_addr,
        "POST",
        &format!("/api/v1/streamkeys/{}/rotate", original.id),
        None,
    )
    .await;
    assert_eq!(rotate.status, 200, "rotate must return 200");
    let rotated: StreamKey = serde_json::from_slice(&rotate.body).expect("rotate body");
    assert_eq!(rotated.id, original.id);
    assert_ne!(rotated.token, original.token);

    // Old token now denied.
    assert!(
        !try_rtmp_publish(rtmp_addr, "live", &original.token).await,
        "post-rotate the OLD token must be invalidated"
    );
    // New token works.
    assert!(
        try_rtmp_publish(rtmp_addr, "live", &rotated.token).await,
        "post-rotate the NEW token must authenticate"
    );

    server.shutdown().await.expect("shutdown");
}

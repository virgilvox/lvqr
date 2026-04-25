//! End-to-end tests for hot config reload (sessions 147 + 148).
//!
//! Session 147 covered the auth-section reload path (RTMP publish key
//! swap via SIGHUP / `POST /api/v1/config-reload`). Session 148 closes
//! the deferred-key gap by hot-reloading two more categories:
//!
//! * **Mesh ICE servers**: the `/signal` callback's `AssignParent`
//!   payload swaps to the new list on the next Register after a
//!   reload. Operators rotating a TURN credential no longer need to
//!   bounce the relay.
//! * **HMAC playback secret**: live HLS / DASH and `/playback/*`
//!   middleware load the secret per request, so a rotated secret
//!   takes effect immediately. Outstanding URLs signed under the
//!   prior secret stop verifying (the documented intent of a secret
//!   rotation).
//!
//! No mocks. Every test goes through `lvqr_cli::start` exactly as
//! `lvqr serve --config foo.toml` does.

use futures_util::SinkExt;
use lvqr_cli::{LiveScheme, sign_live_url};
use lvqr_test_utils::http::http_get_status;
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

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

// =====================================================================
// Session 148: mesh ICE-server hot reload.
// =====================================================================

/// Open `/signal`, send `Register`, return the parsed `AssignParent`
/// JSON. Panics on any protocol deviation. The lvqr-signal server
/// fires the peer callback synchronously on Register and ships the
/// callback's `Some(AssignParent { ... })` reply back over the same
/// WS as a single Text frame.
async fn register_and_read_assign_parent(signal_url: &str, peer_id: &str) -> serde_json::Value {
    use futures_util::StreamExt;
    let (mut ws, _resp) = tokio_tungstenite::connect_async(signal_url)
        .await
        .expect("signal upgrade");
    let register = serde_json::json!({
        "type": "Register",
        "peer_id": peer_id,
        "track": "live/demo",
    })
    .to_string();
    ws.send(Message::Text(register)).await.expect("ws send Register");

    let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("AssignParent frame timed out")
        .expect("ws closed before AssignParent")
        .expect("ws read error");
    let text = match frame {
        Message::Text(t) => t.to_string(),
        other => panic!("expected Text frame after Register; got {other:?}"),
    };
    let value: serde_json::Value = serde_json::from_str(&text).expect("AssignParent frame is not JSON");
    assert_eq!(
        value.get("type").and_then(|v| v.as_str()),
        Some("AssignParent"),
        "expected type=AssignParent; got {value:?}"
    );
    drop(ws);
    value
}

/// Mesh ICE-server hot reload: the operator-configured list flips
/// across `POST /api/v1/config-reload` and the next `Register` on
/// `/signal` sees the new list in its `AssignParent`. Deferred to
/// session 148 because session 147 captured the list by clone in the
/// signal callback's closure; 148 swaps it through an
/// `arc_swap::ArcSwap`.
#[tokio::test]
async fn config_reload_swaps_mesh_ice_servers_via_admin_post() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("lvqr.toml");
    std::fs::write(
        &path,
        r#"
[[mesh_ice_servers]]
urls = ["stun:boot.example:3478"]"#,
    )
    .expect("write boot ice");

    let server = TestServer::start(
        TestServerConfig::new()
            .with_mesh(3)
            .with_config_file(path.clone())
            .with_no_streamkeys(),
    )
    .await
    .expect("TestServer::start");
    let admin_addr = server.admin_addr();
    let signal_url = server.signal_url();

    // Boot reload should have applied the file. Register and verify
    // the boot ICE entry shows up on `AssignParent`.
    let initial = register_and_read_assign_parent(&signal_url, "peer-1").await;
    let initial_urls = initial
        .get("ice_servers")
        .and_then(|v| v.as_array())
        .expect("ice_servers array on AssignParent")
        .iter()
        .flat_map(|e| {
            e.get("urls")
                .and_then(|u| u.as_array())
                .into_iter()
                .flatten()
                .filter_map(|u| u.as_str().map(str::to_string))
        })
        .collect::<Vec<_>>();
    assert!(
        initial_urls.iter().any(|u| u == "stun:boot.example:3478"),
        "boot ICE entry must appear in AssignParent; got {initial_urls:?}"
    );

    // Rewrite the file with a different ICE server, POST reload.
    std::fs::write(
        &path,
        r#"
[[mesh_ice_servers]]
urls = ["turn:rotated.example:3478"]
username = "u"
credential = "p""#,
    )
    .expect("write rotated ice");

    let resp = admin_post(admin_addr, "/api/v1/config-reload", None).await;
    assert_eq!(
        resp.status,
        200,
        "POST /api/v1/config-reload must succeed; body: {:?}",
        String::from_utf8_lossy(&resp.body)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&resp.body).expect("config-reload body is JSON");
    assert!(parsed["applied_keys"].is_array());
    assert!(
        parsed["applied_keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("mesh_ice")),
        "applied_keys must include mesh_ice after a list change; got {parsed:?}"
    );

    // Open another `/signal`, Register, observe the new list.
    let after = register_and_read_assign_parent(&signal_url, "peer-2").await;
    let after_urls = after
        .get("ice_servers")
        .and_then(|v| v.as_array())
        .expect("ice_servers array")
        .iter()
        .flat_map(|e| {
            e.get("urls")
                .and_then(|u| u.as_array())
                .into_iter()
                .flatten()
                .filter_map(|u| u.as_str().map(str::to_string))
        })
        .collect::<Vec<_>>();
    assert!(
        after_urls.iter().any(|u| u == "turn:rotated.example:3478"),
        "rotated ICE entry must appear on subsequent Register; got {after_urls:?}"
    );
    assert!(
        !after_urls.iter().any(|u| u == "stun:boot.example:3478"),
        "boot ICE entry must NOT appear after reload; got {after_urls:?}"
    );

    server.shutdown().await.expect("shutdown");
}

// =====================================================================
// Session 148: HMAC playback secret hot reload.
// =====================================================================

fn unix_exp_in(seconds: u64) -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + seconds
}

/// HMAC playback secret hot reload: an outstanding URL signed under
/// the boot secret stops verifying after `POST /api/v1/config-reload`,
/// while a URL signed under the new secret verifies. Deferred from
/// session 147 because the secret was captured by clone into the
/// live HLS / DASH `LivePlaybackAuthState` and the DVR ArchiveState;
/// session 148 threads a `SwappableHmacSecret` through both surfaces.
#[tokio::test]
async fn config_reload_rotates_hmac_playback_secret_via_admin_post() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("lvqr.toml");
    std::fs::write(&path, r#"hmac_playback_secret = "boot-secret""#).expect("write boot secret");

    let server = TestServer::start(
        TestServerConfig::new()
            .with_config_file(path.clone())
            .with_no_streamkeys(),
    )
    .await
    .expect("TestServer::start");
    let admin_addr = server.admin_addr();
    let hls_addr = server.hls_addr();

    // Boot reload should have applied the file. A URL signed under
    // `boot-secret` must NOT be 401/403 (signed URL short-circuits
    // the noop subscribe gate; the inner HLS handler may 404 if the
    // playlist file is missing, which is fine -- this test is about
    // the auth gate, not the HLS body).
    let exp = unix_exp_in(600);
    let suffix_boot = sign_live_url(b"boot-secret", LiveScheme::Hls, "live/demo", exp);
    let path_boot = format!("/hls/live/demo/playlist.m3u8?{suffix_boot}");
    let status_pre = http_get_status(hls_addr, &path_boot).await;
    assert_ne!(
        status_pre, 401,
        "boot-signed URL must short-circuit subscribe gate pre-reload; got {status_pre}"
    );
    assert_ne!(
        status_pre, 403,
        "boot-signed URL must verify pre-reload; got {status_pre}"
    );

    // Rotate.
    std::fs::write(&path, r#"hmac_playback_secret = "rotated-secret""#).expect("write rotated");
    let resp = admin_post(admin_addr, "/api/v1/config-reload", None).await;
    assert_eq!(
        resp.status,
        200,
        "POST reload must succeed; body: {:?}",
        String::from_utf8_lossy(&resp.body)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&resp.body).expect("config-reload body is JSON");
    assert!(
        parsed["applied_keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("hmac_secret")),
        "applied_keys must include hmac_secret after rotation; got {parsed:?}"
    );

    // Old-signed URL: the rotated secret invalidates it. The HMAC
    // verifier returns 403 ("signed URL signature invalid").
    let status_old = http_get_status(hls_addr, &path_boot).await;
    assert_eq!(
        status_old, 403,
        "boot-signed URL must 403 after rotation; got {status_old}"
    );

    // New-signed URL: verifies under the rotated secret.
    let suffix_new = sign_live_url(b"rotated-secret", LiveScheme::Hls, "live/demo", exp);
    let path_new = format!("/hls/live/demo/playlist.m3u8?{suffix_new}");
    let status_new = http_get_status(hls_addr, &path_new).await;
    assert_ne!(
        status_new, 401,
        "rotated-signed URL must short-circuit subscribe gate; got {status_new}"
    );
    assert_ne!(
        status_new, 403,
        "rotated-signed URL must verify post-reload; got {status_new}"
    );

    server.shutdown().await.expect("shutdown");
}

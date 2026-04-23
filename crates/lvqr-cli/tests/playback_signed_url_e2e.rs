//! Signed playback URL integration test. PLAN v1.1 row 121.
//!
//! Boots a `TestServer` with:
//! * an archive directory so `/playback/*` is mounted
//! * a `StaticAuthProvider` whose `subscribe_token` DENIES
//!   requests that arrive without a bearer
//! * an HMAC signing secret via `TestServerConfig::with_hmac_playback_secret`
//!
//! Then pushes two keyframes via a real `rml_rtmp` client + asserts
//! four scenarios on every `/playback/*` route:
//!
//! 1. **Valid signed URL** -- `?exp=<future>&sig=<correct>` returns
//!    200 even though the caller did NOT present a bearer token.
//!    Proves the HMAC path short-circuits the subscribe-token
//!    gate.
//! 2. **Tampered signature** -- `?exp=<future>&sig=<flipped-byte>`
//!    returns 403 Forbidden (NOT 401 Unauthorized). Clients can
//!    distinguish "no auth" (401) from "wrong auth" (403) on
//!    status code alone.
//! 3. **Expired signature** -- `?exp=<past>&sig=<correct-for-past>`
//!    returns 403. The signature validates crypto-wise but the
//!    expiry check trips first.
//! 4. **No signature, no token** -- plain GET returns 401. The
//!    signed-URL code path does not leak access.
//!
//! Plus a lightweight "signing function round-trips" unit test
//! that re-implements the HMAC input format by hand and asserts
//! the `sign_playback_url` output matches.
//!
//! No mocks: real RTMP handshake, real bridge observer, real
//! on-disk archive writes, real HTTP roundtrips against the
//! admin server. Mirrors the `rtmp_archive_e2e.rs` pattern.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use bytes::Bytes;
use hmac::{Hmac, Mac};
use lvqr_auth::{SharedAuth, StaticAuthConfig, StaticAuthProvider};
use lvqr_cli::sign_playback_url;
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use sha2::Sha256;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);
const HMAC_SECRET: &str = "test-secret-abcdefghijklmnopqrstuvwxyz-1234567890";
const SUBSCRIBE_TOKEN: &str = "cannot-use-without-bearer";

// =====================================================================
// FLV + RTMP helpers (mirror rtmp_archive_e2e.rs).
// =====================================================================

fn flv_video_seq_header() -> Bytes {
    let sps = [0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
    let pps = [0x68, 0xEE, 0x3C, 0x80];
    let mut tag = vec![0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x1F, 0xFF, 0xE1];
    tag.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&sps);
    tag.push(0x01);
    tag.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&pps);
    Bytes::from(tag)
}

fn flv_video_nalu(keyframe: bool, cts: i32, nalu_data: &[u8]) -> Bytes {
    let frame_type = if keyframe { 0x17 } else { 0x27 };
    let mut tag = vec![frame_type, 0x01, (cts >> 16) as u8, (cts >> 8) as u8, cts as u8];
    tag.extend_from_slice(nalu_data);
    Bytes::from(tag)
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    http_get_with_bearer(addr, path, None).await
}

async fn http_get_with_bearer(addr: SocketAddr, path: &str, bearer: Option<&str>) -> HttpResponse {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("connect timed out")
        .expect("connect failed");
    let auth_header = match bearer {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n{auth_header}Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(TIMEOUT, stream.read_to_end(&mut buf))
        .await
        .expect("read timed out")
        .expect("read failed");
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("missing terminator");
    let header_text = std::str::from_utf8(&buf[..split]).expect("utf-8 headers");
    let status: u16 = header_text
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    HttpResponse {
        status,
        body: buf[split + 4..].to_vec(),
    }
}

async fn rtmp_handshake(stream: &mut TcpStream) -> Vec<u8> {
    let mut handshake = Handshake::new(PeerType::Client);
    let p0_p1 = handshake.generate_outbound_p0_and_p1().unwrap();
    stream.write_all(&p0_p1).await.unwrap();
    let mut buf = vec![0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await.unwrap();
        match handshake.process_bytes(&buf[..n]).unwrap() {
            HandshakeProcessResult::InProgress { response_bytes } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await.unwrap();
                }
            }
            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await.unwrap();
                }
                return remaining_bytes;
            }
        }
    }
}

async fn send_results(stream: &mut TcpStream, results: &[ClientSessionResult]) {
    for r in results {
        if let ClientSessionResult::OutboundResponse(packet) = r {
            stream.write_all(&packet.bytes).await.unwrap();
        }
    }
}

async fn send_result(stream: &mut TcpStream, r: &ClientSessionResult) {
    if let ClientSessionResult::OutboundResponse(packet) = r {
        stream.write_all(&packet.bytes).await.unwrap();
    }
}

async fn read_until<F>(stream: &mut TcpStream, session: &mut ClientSession, predicate: F)
where
    F: Fn(&ClientSessionEvent) -> bool,
{
    let mut buf = vec![0u8; 65536];
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        let n = tokio::time::timeout(remaining, stream.read(&mut buf))
            .await
            .expect("timeout")
            .expect("read err");
        let results = session.handle_input(&buf[..n]).unwrap();
        for r in results {
            match r {
                ClientSessionResult::OutboundResponse(packet) => {
                    stream.write_all(&packet.bytes).await.unwrap();
                }
                ClientSessionResult::RaisedEvent(ref e) => {
                    if predicate(e) {
                        return;
                    }
                }
                _ => {}
            }
        }
    }
}

async fn publish_two_keyframes(addr: SocketAddr, app: &str, key: &str) -> (TcpStream, ClientSession) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.set_nodelay(true).unwrap();
    let remaining = rtmp_handshake(&mut stream).await;
    let (mut session, initial) = ClientSession::new(ClientSessionConfig::new()).unwrap();
    send_results(&mut stream, &initial).await;
    if !remaining.is_empty() {
        let r = session.handle_input(&remaining).unwrap();
        send_results(&mut stream, &r).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    let connect = session.request_connection(app.into()).unwrap();
    send_result(&mut stream, &connect).await;
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

    let publish = session
        .request_publishing(key.into(), PublishRequestType::Live)
        .unwrap();
    send_result(&mut stream, &publish).await;
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::PublishRequestAccepted)
    })
    .await;

    let seq = flv_video_seq_header();
    let r = session.publish_video_data(seq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut stream, &r).await;
    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    for ts in [0u32, 2100] {
        let kf = flv_video_nalu(true, 0, &nalu);
        let r = session.publish_video_data(kf, RtmpTimestamp::new(ts), false).unwrap();
        send_result(&mut stream, &r).await;
    }

    (stream, session)
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// Hand-rolled HMAC match of what `sign_playback_url` does
/// internally. Used by the tampered-signature test to build a
/// known-bad sig with a flipped byte.
fn sign_manual(secret: &[u8], request_path: &str, exp: u64) -> Vec<u8> {
    let input = format!("{request_path}?exp={exp}");
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).unwrap();
    mac.update(input.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

// =====================================================================
// Unit test: signing function round-trips against the hand-rolled
// hmac_manual.
// =====================================================================

#[tokio::test]
async fn sign_playback_url_matches_hand_rolled_hmac() {
    let secret = HMAC_SECRET.as_bytes();
    let path = "/playback/live/dvr";
    let exp = 1_760_000_000u64;

    let suffix = sign_playback_url(secret, path, exp);
    // Shape: "exp=<ts>&sig=<base64url>".
    assert!(suffix.starts_with(&format!("exp={exp}&sig=")));
    let sig_b64 = suffix.trim_start_matches(&format!("exp={exp}&sig="));
    let sig_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sig_b64.as_bytes())
        .expect("b64url decodable");

    let expected = sign_manual(secret, path, exp);
    assert_eq!(sig_bytes, expected);
    // HMAC-SHA256 is 32 bytes.
    assert_eq!(sig_bytes.len(), 32);
}

// =====================================================================
// Integration test: four scenarios against a real lvqr.
// =====================================================================

#[tokio::test]
async fn signed_url_grants_access_and_denies_tampering() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let archive_tmp = TempDir::new().expect("tempdir");
    let archive_path = archive_tmp.path().to_path_buf();

    let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
        admin_token: None,
        publish_key: None,
        subscribe_token: Some(SUBSCRIBE_TOKEN.into()),
    }));

    let server = TestServer::start(
        TestServerConfig::default()
            .with_archive_dir(&archive_path)
            .with_auth(auth)
            .with_hmac_playback_secret(HMAC_SECRET),
    )
    .await
    .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    let (_rtmp_stream, _rtmp_session) = publish_two_keyframes(rtmp_addr, "live", "dvr").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // --- Scenario 1: valid signed URL grants access without a
    // bearer token. The route and exp are inside the HMAC input
    // so the server reconstructs the same bytes we signed.
    let exp = now_unix() + 300;
    let path = "/playback/live/dvr";
    let suffix = sign_playback_url(HMAC_SECRET.as_bytes(), path, exp);
    let signed_url = format!("{path}?{suffix}");

    let resp = http_get(admin_addr, &signed_url).await;
    assert_eq!(
        resp.status,
        200,
        "valid signed URL should grant access; got {} body={}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
    // Body must be a JSON array of segments (ground truth the
    // scrub test already verifies).
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&resp.body).expect("JSON array");
    assert!(!rows.is_empty(), "signed-URL access returned zero rows");

    // --- Scenario 2: tampered signature returns 403. Flip one
    // bit of the correct sig + re-base64 it.
    let mut tampered = sign_manual(HMAC_SECRET.as_bytes(), path, exp);
    tampered[0] ^= 0x01;
    let tampered_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&tampered);
    let tampered_url = format!("{path}?exp={exp}&sig={tampered_b64}");

    let resp = http_get(admin_addr, &tampered_url).await;
    assert_eq!(resp.status, 403, "tampered sig must 403 (not 401); got {}", resp.status);

    // --- Scenario 3: expired signature returns 403 even though
    // the sig is crypto-valid for the past timestamp.
    let past_exp = now_unix().saturating_sub(60);
    let past_sig = sign_playback_url(HMAC_SECRET.as_bytes(), path, past_exp);
    let expired_url = format!("{path}?{past_sig}");
    let resp = http_get(admin_addr, &expired_url).await;
    assert_eq!(resp.status, 403, "expired sig must 403 (not 401); got {}", resp.status);

    // --- Scenario 4: no sig + no bearer returns 401 (not 403).
    let resp = http_get(admin_addr, path).await;
    assert_eq!(
        resp.status, 401,
        "no-auth request must 401 (not 403); got {}",
        resp.status
    );

    // --- Bonus: the same signed-URL flow works on
    // /playback/latest/{broadcast}. Route path is different so
    // the HMAC input is different.
    let latest_path = "/playback/latest/live/dvr";
    let latest_suffix = sign_playback_url(HMAC_SECRET.as_bytes(), latest_path, exp);
    let latest_url = format!("{latest_path}?{latest_suffix}");
    let resp = http_get(admin_addr, &latest_url).await;
    assert_eq!(
        resp.status,
        200,
        "latest signed URL status; got {} body={}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );

    // --- Bonus: valid bearer token still works when signed URL
    // is not present. This proves the HMAC path does not
    // replace the existing SubscribeAuth gate, it augments it.
    let resp = http_get_with_bearer(admin_addr, path, Some(SUBSCRIBE_TOKEN)).await;
    assert_eq!(resp.status, 200, "bearer-token path still works");

    // Signing a URL with a DIFFERENT path must fail against the
    // real path (sig is path-bound). Sign /playback/live/other
    // but GET /playback/live/dvr.
    let wrong_path_suffix = sign_playback_url(HMAC_SECRET.as_bytes(), "/playback/live/other", exp);
    let wrong_path_url = format!("{path}?{wrong_path_suffix}");
    let resp = http_get(admin_addr, &wrong_path_url).await;
    assert_eq!(resp.status, 403, "cross-path signature must 403; got {}", resp.status);

    drop(_rtmp_stream);
    server.shutdown().await.expect("shutdown");
}

// =====================================================================
// Integration test: signed URL also short-circuits on
// /playback/file/<rel>, the raw-bytes route.
// =====================================================================

#[tokio::test]
async fn signed_url_works_on_file_route() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let archive_tmp = TempDir::new().expect("tempdir");
    let archive_path = archive_tmp.path().to_path_buf();

    let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
        admin_token: None,
        publish_key: None,
        subscribe_token: Some(SUBSCRIBE_TOKEN.into()),
    }));

    let server = TestServer::start(
        TestServerConfig::default()
            .with_archive_dir(&archive_path)
            .with_auth(auth)
            .with_hmac_playback_secret(HMAC_SECRET),
    )
    .await
    .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    let (_s, _sess) = publish_two_keyframes(rtmp_addr, "live", "dvr").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Find a real archived segment via an authenticated range
    // scan. Everything after this is the signed-URL check on
    // /playback/file/.
    let range = http_get_with_bearer(admin_addr, "/playback/live/dvr", Some(SUBSCRIBE_TOKEN)).await;
    assert_eq!(range.status, 200);
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&range.body).expect("JSON");
    let seq = rows[0]["segment_seq"].as_u64().unwrap();
    let file_path = format!("/playback/file/live/dvr/0.mp4/{seq:08}.m4s");

    let exp = now_unix() + 300;
    let suffix = sign_playback_url(HMAC_SECRET.as_bytes(), &file_path, exp);
    let signed_url = format!("{file_path}?{suffix}");

    let resp = http_get(admin_addr, &signed_url).await;
    assert_eq!(resp.status, 200, "file signed URL status");
    assert!(resp.body.len() >= 8);
    assert_eq!(&resp.body[4..8], b"moof", "signed-URL file body is fMP4");

    server.shutdown().await.expect("shutdown");
}

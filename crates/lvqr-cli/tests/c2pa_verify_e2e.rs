//! RTMP ingest -> C2PA drain-terminated finalize -> admin verify
//! route end-to-end integration test.
//!
//! Tier 4 item 4.3 session B3. Exercises the full session-94 surface:
//!
//! 1. [`TestServer`] is configured with an archive directory AND a
//!    `C2paConfig` whose signer source is
//!    [`c2pa::EphemeralSigner`] wrapped in
//!    [`lvqr_archive::provenance::C2paSignerSource::Custom`] (no PEM
//!    fixtures on disk; the chain is generated in memory via c2pa-rs's
//!    `ephemeral_cert` module at test-start time).
//! 2. A real `rml_rtmp` client handshakes + publishes two keyframes
//!    into the server (pattern copied from `rtmp_archive_e2e.rs`).
//! 3. Dropping the publisher triggers the
//!    `RtmpMoqBridge::on_unpublish` callback, which calls
//!    `FragmentBroadcasterRegistry::remove(..)` for both video +
//!    audio tracks. That drop causes the archive indexer's drain
//!    task to see `next_fragment() -> None`, which in turn fires
//!    the C2PA finalize path inside `spawn_blocking`. The finalize
//!    path concatenates
//!    `<archive>/<broadcast>/<track>/init.mp4` with every indexed
//!    segment in `start_dts` order, signs via the
//!    `C2paSignerSource::Custom` signer, and writes
//!    `finalized.mp4` + `finalized.c2pa` next to the segment files.
//! 4. The test polls for `finalized.c2pa` to appear on disk (the
//!    finalize work runs on a spawn_blocking thread, so there is a
//!    short race window after the publisher drops).
//! 5. `GET /playback/verify/live/dvr` returns the JSON shape documented
//!    on `archive::VerifyResponse`. The assertion confirms the signer
//!    matches the EphemeralSigner's subject CN, validation_state is
//!    `"Valid"` (EphemeralSigner's CA is not in c2pa-rs's default
//!    trust list so we get Valid, not Trusted), `valid` is true, and
//!    `errors` is empty.
//!
//! No mocks: real RTMP handshake, real bridge observer path, real
//! on-disk segment writes, real c2pa-rs sign + verify roundtrip.

#![cfg(feature = "c2pa")]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use lvqr_archive::provenance::{C2paConfig, C2paSignerSource};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);
const FINALIZE_POLL_BUDGET: Duration = Duration::from_secs(10);
const FINALIZE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const EPHEMERAL_SIGNER_NAME: &str = "lvqr-c2pa-e2e.local";

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
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("http GET connect timed out")
        .expect("http GET connect failed");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(TIMEOUT, stream.read_to_end(&mut buf))
        .await
        .expect("http GET read timed out")
        .expect("http GET read failed");
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http response missing header terminator");
    let header_text = std::str::from_utf8(&buf[..split]).expect("http headers are not utf-8");
    let status_line = header_text.lines().next().expect("http response missing status line");
    let mut parts = status_line.splitn(3, ' ');
    let _http_version = parts.next();
    let status: u16 = parts
        .next()
        .expect("status line missing code")
        .parse()
        .expect("status code is not numeric");
    HttpResponse {
        status,
        body: buf[split + 4..].to_vec(),
    }
}

async fn rtmp_client_handshake(stream: &mut TcpStream) -> Vec<u8> {
    let mut handshake = Handshake::new(PeerType::Client);
    let p0_and_p1 = handshake.generate_outbound_p0_and_p1().unwrap();
    stream.write_all(&p0_and_p1).await.unwrap();

    let mut buf = vec![0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0, "server closed during handshake");
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
    for result in results {
        if let ClientSessionResult::OutboundResponse(packet) = result {
            stream.write_all(&packet.bytes).await.unwrap();
        }
    }
}

async fn send_result(stream: &mut TcpStream, result: &ClientSessionResult) {
    if let ClientSessionResult::OutboundResponse(packet) = result {
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
        let n = match tokio::time::timeout(remaining, stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => n,
            Ok(Ok(_)) => panic!("server closed connection unexpectedly"),
            Ok(Err(e)) => panic!("read error: {e}"),
            Err(_) => panic!("timed out waiting for expected RTMP event"),
        };
        let results = session.handle_input(&buf[..n]).unwrap();
        for result in results {
            match result {
                ClientSessionResult::OutboundResponse(packet) => {
                    stream.write_all(&packet.bytes).await.unwrap();
                }
                ClientSessionResult::RaisedEvent(ref event) => {
                    if predicate(event) {
                        return;
                    }
                }
                _ => {}
            }
        }
    }
}

async fn connect_and_publish(addr: SocketAddr, app: &str, stream_key: &str) -> (TcpStream, ClientSession) {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .unwrap()
        .unwrap();
    stream.set_nodelay(true).unwrap();
    let remaining = rtmp_client_handshake(&mut stream).await;

    let config = ClientSessionConfig::new();
    let (mut session, initial_results) = ClientSession::new(config).unwrap();
    send_results(&mut stream, &initial_results).await;
    if !remaining.is_empty() {
        let results = session.handle_input(&remaining).unwrap();
        send_results(&mut stream, &results).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    let connect_result = session.request_connection(app.to_string()).unwrap();
    send_result(&mut stream, &connect_result).await;
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

    let publish_result = session
        .request_publishing(stream_key.to_string(), PublishRequestType::Live)
        .unwrap();
    send_result(&mut stream, &publish_result).await;
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::PublishRequestAccepted)
    })
    .await;

    (stream, session)
}

async fn publish_two_keyframes(addr: SocketAddr, app: &str, key: &str) -> (TcpStream, ClientSession) {
    let (mut rtmp_stream, mut session) = connect_and_publish(addr, app, key).await;

    let seq = flv_video_seq_header();
    let r = session.publish_video_data(seq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf0 = flv_video_nalu(true, 0, &nalu);
    let r = session.publish_video_data(kf0, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    let kf1 = flv_video_nalu(true, 0, &nalu);
    let r = session
        .publish_video_data(kf1, RtmpTimestamp::new(2100), false)
        .unwrap();
    send_result(&mut rtmp_stream, &r).await;

    (rtmp_stream, session)
}

fn ephemeral_c2pa_config() -> C2paConfig {
    let signer = c2pa::EphemeralSigner::new(EPHEMERAL_SIGNER_NAME).expect("generate ephemeral c2pa signer");
    C2paConfig {
        signer_source: C2paSignerSource::Custom(Arc::new(signer)),
        assertion_creator: "LVQR E2E Operator".to_string(),
        // EphemeralSigner's self-signed CA is not in c2pa-rs's default
        // trust list. Leaving `trust_anchor_pem` at None means the
        // verify route reports `validation_state = "Valid"` (crypto
        // integrity) rather than `"Trusted"` (which would require the
        // CA in the trust list). The E2E asserts against "Valid" to
        // match this posture.
        trust_anchor_pem: None,
    }
}

async fn wait_for_finalize(manifest_path: &Path) {
    let deadline = tokio::time::Instant::now() + FINALIZE_POLL_BUDGET;
    loop {
        if manifest_path.exists() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "finalize manifest did not appear at {} within {:?}",
                manifest_path.display(),
                FINALIZE_POLL_BUDGET
            );
        }
        tokio::time::sleep(FINALIZE_POLL_INTERVAL).await;
    }
}

/// Real end-to-end: RTMP publish -> broadcast disconnect -> drain
/// terminates -> C2PA finalize -> `/playback/verify/live/dvr`
/// returns a valid manifest signed by the EphemeralSigner.
#[tokio::test]
async fn rtmp_publish_then_unpublish_yields_verifiable_c2pa_manifest() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let archive_tmp = TempDir::new().expect("tempdir");
    let archive_path = archive_tmp.path().to_path_buf();

    let server = TestServer::start(
        TestServerConfig::default()
            .with_archive_dir(&archive_path)
            .with_c2pa(ephemeral_c2pa_config()),
    )
    .await
    .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    let (rtmp_stream, rtmp_session) = publish_two_keyframes(rtmp_addr, "live", "dvr").await;

    // Let the bridge write every fragment to the archive before we
    // trigger unpublish. Otherwise the drain task may race ahead of
    // the last spawn_blocking segment write and finalize over a
    // partial segment set. 500 ms matches the rtmp_archive_e2e
    // pattern.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Drop the RTMP socket + session. The server-side bridge
    // notices the connection close via the RTMP server's session
    // teardown, which fires `on_unpublish`; that callback calls
    // `FragmentBroadcasterRegistry::remove(..)` for both tracks.
    // Every producer-side clone of the broadcaster then drops, the
    // drain task's `next_fragment().await` returns `None`, and the
    // C2PA finalize branch runs inside spawn_blocking.
    drop(rtmp_stream);
    drop(rtmp_session);

    let manifest_path = archive_path.join("live/dvr/0.mp4/finalized.c2pa");
    let asset_path = archive_path.join("live/dvr/0.mp4/finalized.mp4");
    wait_for_finalize(&manifest_path).await;
    assert!(
        asset_path.exists(),
        "finalize manifest landed but finalized.mp4 is missing at {}",
        asset_path.display()
    );

    // --- GET /playback/verify/live/dvr ---
    let resp = http_get(admin_addr, "/playback/verify/live/dvr").await;
    assert_eq!(
        resp.status,
        200,
        "GET /playback/verify/live/dvr returned {} with body {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
    let body = std::str::from_utf8(&resp.body).expect("verify body utf-8");
    eprintln!("--- /playback/verify/live/dvr ---\n{body}\n--- end ---");

    let v: serde_json::Value = serde_json::from_str(body).expect("verify body is JSON");
    assert_eq!(
        v["valid"].as_bool(),
        Some(true),
        "expected valid=true; manifest failed verification: {body}"
    );
    // Validation state is `Valid` (cryptographic integrity passed)
    // rather than `Trusted` (which requires the EphemeralSigner's CA
    // in c2pa-rs's trust list, which it is not).
    assert_eq!(
        v["validation_state"].as_str(),
        Some("Valid"),
        "expected validation_state=Valid, got {body}"
    );
    // `signer` reports the issuer (CA) subject, not the EE subject,
    // per `c2pa::Manifest::issuer`. c2pa-rs's `EphemeralSigner` uses a
    // stable stock CA subject -- the exact string may shift if
    // c2pa-rs updates the fixture, so assert that it exists and is
    // non-empty rather than hard-coding the string.
    let signer = v["signer"].as_str().expect("signer field is a string");
    assert!(
        !signer.is_empty(),
        "signer string must be non-empty; manifest has no issuer"
    );
    // `errors` must be empty at the "hard failure" level. Non-fatal
    // status codes such as `signingCredential.untrusted` -- which
    // c2pa-rs itself excludes from `validation_state` -- are filtered
    // out by the verify handler, so this assertion is stricter than
    // "no non-trivial errors". If it trips, something actually went
    // wrong during signing / verification.
    let errors = v["errors"].as_array().expect("errors field is array");
    assert!(errors.is_empty(), "expected empty errors array; got {errors:?}");

    // Unknown broadcast must 404 (finalize did not run for it, so
    // no finalized.mp4 / finalized.c2pa exists on disk).
    let resp = http_get(admin_addr, "/playback/verify/live/ghost").await;
    assert_eq!(
        resp.status,
        404,
        "unknown broadcast must 404, got {} ({})",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );

    server.shutdown().await.expect("shutdown");
}

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

use lvqr_archive::provenance::{C2paConfig, C2paSignerSource};
use lvqr_test_utils::flv::{flv_video_nalu, flv_video_seq_header};
use lvqr_test_utils::http::{HttpGetOptions, HttpResponse, http_get_with};
use lvqr_test_utils::rtmp::{read_until, rtmp_client_handshake, send_result, send_results};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::sessions::{ClientSession, ClientSessionConfig, ClientSessionEvent, PublishRequestType};
use rml_rtmp::time::RtmpTimestamp;
use tempfile::TempDir;
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);
const FINALIZE_POLL_BUDGET: Duration = Duration::from_secs(10);
const FINALIZE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const EPHEMERAL_SIGNER_NAME: &str = "lvqr-c2pa-e2e.local";

async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    http_get_with(
        addr,
        path,
        HttpGetOptions {
            timeout: TIMEOUT,
            ..Default::default()
        },
    )
    .await
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
    read_until(&mut stream, &mut session, TIMEOUT, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

    let publish_result = session
        .request_publishing(stream_key.to_string(), PublishRequestType::Live)
        .unwrap();
    send_result(&mut stream, &publish_result).await;
    read_until(&mut stream, &mut session, TIMEOUT, |e| {
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

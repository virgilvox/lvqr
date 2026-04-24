//! PLAN Phase D session 137 end-to-end test: `GET /api/v1/wasm-filter`.
//!
//! Starts a `TestServer` with a two-filter chain
//! (`frame-counter.wasm` + `redact-keyframes.wasm`), publishes RTMP
//! keyframes through it, then issues a real HTTP GET against the
//! admin server's `/api/v1/wasm-filter` route. Asserts the JSON body
//! reports `enabled: true`, `chain_length: 2`, and at least one
//! `(broadcast, track)` entry whose `dropped == seen` (because the
//! chain short-circuits on slot 2's drop).
//!
//! Companion to `wasm_filter_chain.rs` which asserts the same
//! outcome via the in-process `WasmFilterBridgeHandle` accessor.
//! This test proves the admin route's snapshot closure is wired
//! correctly from the bridge into the route's JSON body.

use lvqr_test_utils::flv::{flv_video_nalu, flv_video_seq_header};
use lvqr_test_utils::http::{HttpGetOptions, http_get_with};
use lvqr_test_utils::rtmp::{read_until, rtmp_client_handshake, send_result, send_results};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::sessions::{ClientSession, ClientSessionConfig, ClientSessionEvent, PublishRequestType};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

fn wasm_fixture_dir() -> std::path::PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest)
        .parent()
        .expect("crates/ parent")
        .join("lvqr-wasm/examples")
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
    let result = session.publish_video_data(seq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &result).await;

    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf0 = flv_video_nalu(true, 0, &nalu);
    let result = session.publish_video_data(kf0, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &result).await;

    let kf1 = flv_video_nalu(true, 0, &nalu);
    let result = session
        .publish_video_data(kf1, RtmpTimestamp::new(2100), false)
        .unwrap();
    send_result(&mut rtmp_stream, &result).await;

    (rtmp_stream, session)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_route_reports_chain_length_and_per_broadcast_counters() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug,lvqr_wasm=debug")
        .with_test_writer()
        .try_init();

    let dir = wasm_fixture_dir();
    let noop = dir.join("frame-counter.wasm");
    let drop_all = dir.join("redact-keyframes.wasm");
    assert!(
        noop.exists() && drop_all.exists(),
        "expected committed WASM fixtures at {}; rebuild via `cargo run -p lvqr-wasm --example build_fixtures`",
        dir.display(),
    );

    let server = TestServer::start(
        TestServerConfig::new()
            .with_wasm_filter(&noop)
            .with_wasm_filter(&drop_all),
    )
    .await
    .expect("start TestServer");

    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    let (_rtmp_stream, _session) = publish_two_keyframes(rtmp_addr, "live", "admin-chain").await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let resp = http_get_with(admin_addr, "/api/v1/wasm-filter", HttpGetOptions::default()).await;
    assert_eq!(resp.status, 200, "status={} body={}", resp.status, resp.body_text());

    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("valid JSON");
    assert_eq!(body["enabled"], true, "enabled must be true with filters configured");
    assert_eq!(
        body["chain_length"], 2,
        "chain_length must reflect two --wasm-filter args"
    );

    let broadcasts = body["broadcasts"].as_array().expect("broadcasts is array");
    let entry = broadcasts
        .iter()
        .find(|e| e["broadcast"] == "live/admin-chain")
        .unwrap_or_else(|| panic!("expected live/admin-chain in admin broadcasts; got {broadcasts:?}"));
    let seen = entry["seen"].as_u64().expect("seen is u64");
    let kept = entry["kept"].as_u64().expect("kept is u64");
    let dropped = entry["dropped"].as_u64().expect("dropped is u64");
    assert!(seen > 0, "chain must have observed at least one fragment; got {entry}");
    assert_eq!(kept, 0, "chain with drop at slot 2 must keep zero; got kept={kept}");
    assert_eq!(
        dropped, seen,
        "chain with drop at slot 2 must drop every observed fragment; got dropped={dropped} seen={seen}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_route_reports_disabled_when_no_filter_configured() {
    let server = TestServer::start(TestServerConfig::new())
        .await
        .expect("start TestServer");
    let admin_addr = server.admin_addr();

    let resp = http_get_with(admin_addr, "/api/v1/wasm-filter", HttpGetOptions::default()).await;
    assert_eq!(resp.status, 200);

    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("valid JSON");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["chain_length"], 0);
    assert_eq!(body["broadcasts"].as_array().unwrap().len(), 0);
}

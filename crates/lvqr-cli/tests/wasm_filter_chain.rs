//! PLAN Phase D (session 136) end-to-end test: multi-filter chain.
//!
//! Starts a `TestServer` whose `--wasm-filter` flag was set twice
//! via two `with_wasm_filter` calls -- first
//! `frame-counter.wasm` (no-op: keeps every fragment unchanged),
//! then `redact-keyframes.wasm` (drops every fragment). Publishes
//! a real RTMP broadcast and asserts that the WASM tap observed
//! at least one fragment and that every observed fragment was
//! dropped. That outcome is the chain's composite decision: slot
//! one keeps, slot two drops, short-circuit semantics make the
//! chain's overall verdict "drop".
//!
//! Companion to `wasm_frame_counter.rs` which drives the same
//! RTMP flow through a single-filter chain (just
//! `frame-counter.wasm`) and asserts every fragment was kept. The
//! two tests together assert that the chain wiring is correctly
//! order-preserving and that multi-filter and single-filter
//! `with_wasm_filter` calls produce the outcome their filter
//! composition predicts.

use lvqr_test_utils::flv::{flv_video_nalu, flv_video_seq_header};
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
async fn chain_of_noop_then_drop_denies_every_fragment() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug,lvqr_wasm=debug")
        .with_test_writer()
        .try_init();

    let dir = wasm_fixture_dir();
    let noop = dir.join("frame-counter.wasm");
    let drop_all = dir.join("redact-keyframes.wasm");
    assert!(
        noop.exists() && drop_all.exists(),
        "expected committed WASM fixtures at {}; run `cargo run -p lvqr-wasm --example build_fixtures` to rebuild",
        dir.display(),
    );

    // Two with_wasm_filter calls install an ordered chain of
    // (frame-counter, redact-keyframes). The chain composition is
    // what this test exercises; `wasm_frame_counter.rs` covers the
    // single-filter (all-keep) case, and the unit tests in
    // `lvqr-wasm/src/lib.rs` cover the chain semantics directly.
    let server = TestServer::start(
        TestServerConfig::new()
            .with_wasm_filter(&noop)
            .with_wasm_filter(&drop_all),
    )
    .await
    .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();

    let (_rtmp_stream, _session) = publish_two_keyframes(rtmp_addr, "live", "chain-test").await;

    tokio::time::sleep(Duration::from_millis(400)).await;

    let tap = server
        .wasm_filter()
        .expect("WASM filter handle must be present when with_wasm_filter was called");
    let tracked = tap.tracked();
    assert!(
        !tracked.is_empty(),
        "chain tap must have seen at least one broadcast after a successful RTMP publish; got empty set",
    );

    let key = tracked
        .iter()
        .find(|(b, _)| b == "live/chain-test")
        .cloned()
        .unwrap_or_else(|| panic!("expected live/chain-test broadcast in tap state; saw: {tracked:?}"));
    let (broadcast, track) = key;
    let seen = tap.fragments_seen(&broadcast, &track);
    let kept = tap.fragments_kept(&broadcast, &track);
    let dropped = tap.fragments_dropped(&broadcast, &track);

    assert!(
        seen > 0,
        "chain tap should have observed at least one fragment on {broadcast}/{track}; got seen=0",
    );
    assert_eq!(
        kept, 0,
        "chain with a drop at slot 2 must keep zero fragments; got kept={kept}",
    );
    assert_eq!(
        dropped, seen,
        "chain with a drop at slot 2 must drop every observed fragment; got dropped={dropped} seen={seen}",
    );
}

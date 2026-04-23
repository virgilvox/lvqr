//! Tier 4 item 4.2 session B end-to-end test.
//!
//! Publishes a real RTMP broadcast through a `TestServer` that
//! was constructed with `--wasm-filter` pointed at the
//! committed `frame-counter.wasm` (a no-op filter: passes every
//! fragment through unchanged). After the publish completes,
//! asserts that the WASM filter tap observed at least one
//! fragment on the `live/frame-counter` broadcast and that
//! every observed fragment was kept (zero drops, since the
//! no-op filter never drops).
//!
//! No mocks, no stdout capture, no host-call side channels.
//! The assertion reads straight off the
//! `WasmFilterBridgeHandle` the CLI keeps on its `ServerHandle`;
//! that handle is what an operator sees via the admin API once
//! the surface lands.

use lvqr_test_utils::flv::{flv_video_nalu, flv_video_seq_header};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

fn example_wasm_path() -> std::path::PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest)
        .parent()
        .expect("crates/ parent")
        .join("lvqr-wasm/examples/frame-counter.wasm")
}

// =====================================================================
// RTMP publish helpers (pattern borrowed from rtmp_hls_e2e.rs).
// =====================================================================

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

/// Publish two keyframes spaced past the 2 s segment boundary so
/// the fragment bridge closes at least one full segment and the
/// WASM filter tap gets at least one inbound fragment to count.
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

// =====================================================================
// The test
// =====================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_frame_counter_sees_every_ingested_fragment() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug,lvqr_wasm=debug")
        .with_test_writer()
        .try_init();

    let wasm_path = example_wasm_path();
    assert!(
        wasm_path.exists(),
        "expected committed frame-counter.wasm at {}; assemble via wat2wasm or the build step",
        wasm_path.display(),
    );

    let server = TestServer::start(TestServerConfig::new().with_wasm_filter(&wasm_path))
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();

    let (_rtmp_stream, _session) = publish_two_keyframes(rtmp_addr, "live", "frame-counter").await;

    // Hold the RTMP session open long enough for the bridge's
    // on_fragment taps to drain + the WASM observer's per-
    // broadcast task to increment counters.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let tap = server
        .wasm_filter()
        .expect("WASM filter handle must be present when with_wasm_filter is set");
    let tracked = tap.tracked();
    assert!(
        !tracked.is_empty(),
        "WASM filter must have seen at least one broadcast after a successful RTMP publish; got empty set",
    );

    // The RTMP bridge publishes `live/frame-counter` with the
    // standard video track name `0.mp4`.
    let key = tracked
        .iter()
        .find(|(b, _)| b == "live/frame-counter")
        .cloned()
        .unwrap_or_else(|| panic!("expected live/frame-counter broadcast in tap state; saw: {tracked:?}"));
    let (broadcast, track) = key;
    let seen = tap.fragments_seen(&broadcast, &track);
    let kept = tap.fragments_kept(&broadcast, &track);
    let dropped = tap.fragments_dropped(&broadcast, &track);
    assert!(
        seen > 0,
        "WASM filter should have observed at least one fragment on {broadcast}/{track}; got seen=0",
    );
    assert_eq!(
        dropped, 0,
        "no-op frame-counter filter should never drop; got dropped={dropped}",
    );
    assert_eq!(
        kept, seen,
        "no-op filter's kept count must equal seen count; seen={seen} kept={kept}",
    );

    server.shutdown().await.expect("shutdown");
}

//! Tier 4 item 4.2 session C end-to-end test.
//!
//! Starts a `TestServer` with `--wasm-filter` pointed at a
//! temporary copy of the committed `frame-counter.wasm` (a no-op
//! filter that keeps every fragment). Publishes a real RTMP
//! broadcast and asserts the WASM filter tap observed fragments
//! with `dropped == 0`. Then overwrites the filter path with
//! `redact-keyframes.wasm` (always returns `-1`, so every
//! fragment is dropped), waits for the
//! `lvqr_wasm::WasmFilterReloader` to pick up the change via
//! `notify::RecommendedWatcher`, publishes a second RTMP
//! broadcast, and asserts the subsequent fragments increment
//! the `fragments_dropped` counter.
//!
//! Real network (RTMP over TCP loopback), real filesystem (the
//! temp-copy + overwrite flow exercises the notify path end to
//! end), no mocks. Pairs with `wasm_frame_counter.rs` which
//! already exercises the tap on a static filter; this test
//! covers the swap-at-runtime path the reloader adds.

use lvqr_test_utils::flv::{flv_video_nalu, flv_video_seq_header};
use lvqr_test_utils::rtmp::rtmp_client_handshake;
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

fn example_wasm(name: &str) -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest)
        .parent()
        .expect("crates/ parent")
        .join("lvqr-wasm/examples")
        .join(name)
}

/// Atomically replace `dest` with a copy of `source`. Uses a
/// rename so the update lands in a single filesystem operation
/// (no partial-write window the reloader might observe);
/// notify reports the rename as `EventKind::Modify` which the
/// reloader treats the same as a content change.
fn replace_atomically(source: &Path, dest: &Path) {
    let parent = dest.parent().expect("dest must have parent");
    let tmp = parent.join(format!(
        "swap-{}.tmp",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::copy(source, &tmp).expect("copy new filter into temp file");
    std::fs::rename(&tmp, dest).expect("rename temp file over filter path");
}

// =====================================================================
// RTMP publish helpers (mirror wasm_frame_counter.rs + rtmp_hls_e2e.rs).
// =====================================================================

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
                ClientSessionResult::RaisedEvent(ref event) if predicate(event) => {
                    return;
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
/// WASM filter tap gets at least one inbound fragment to count
/// (same pattern as `wasm_frame_counter.rs`).
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

/// Poll `predicate` every 25 ms up to `budget`, returning as
/// soon as it matches. notify event delivery has platform-
/// specific latency (FSEvents batches on a ~500 ms timer on
/// macOS; inotify is usually sub-10 ms on Linux), so a plain
/// fixed sleep either over-waits on Linux or under-waits on
/// macOS. Polling a condition is both faster in the common
/// case and more resilient to drift.
async fn wait_for<F: FnMut() -> bool>(budget: Duration, mut predicate: F) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    predicate()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_filter_hot_reload_flips_drop_behavior_mid_stream() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug,lvqr_wasm=debug")
        .with_test_writer()
        .try_init();

    let no_op = example_wasm("frame-counter.wasm");
    let drop_all = example_wasm("redact-keyframes.wasm");
    assert!(
        no_op.exists(),
        "expected committed frame-counter.wasm at {}; run cargo run -p lvqr-wasm --example build_fixtures",
        no_op.display(),
    );
    assert!(
        drop_all.exists(),
        "expected committed redact-keyframes.wasm at {}; run cargo run -p lvqr-wasm --example build_fixtures",
        drop_all.display(),
    );

    // Put the live filter path inside a dedicated tempdir so
    // the reloader's parent-directory watcher does not see
    // unrelated churn from the system tempdir root.
    let workdir = tempfile::tempdir().expect("tempdir");
    let filter_path = workdir.path().join("filter.wasm");
    std::fs::copy(&no_op, &filter_path).expect("seed filter.wasm with no-op module");

    let server = TestServer::start(TestServerConfig::new().with_wasm_filter(&filter_path))
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();

    // Phase 1: no-op filter in effect. Publish and assert the
    // tap observed fragments with zero drops.
    let (rtmp_before, session_before) = publish_two_keyframes(rtmp_addr, "live", "hot-reload-before").await;
    let tap = server
        .wasm_filter()
        .expect("WASM filter handle must be present when with_wasm_filter is set");

    let saw_before = wait_for(Duration::from_secs(5), || {
        tap.fragments_seen("live/hot-reload-before", "0.mp4") > 0
    })
    .await;
    assert!(
        saw_before,
        "pre-reload phase: WASM filter should have observed fragments on live/hot-reload-before within 5 s; tracked={:?}",
        tap.tracked(),
    );
    let seen_before = tap.fragments_seen("live/hot-reload-before", "0.mp4");
    let dropped_before = tap.fragments_dropped("live/hot-reload-before", "0.mp4");
    assert_eq!(
        dropped_before, 0,
        "no-op filter must not drop any fragment before the reload; seen={seen_before} dropped={dropped_before}",
    );

    // Close the first broadcast so the next publish is
    // unambiguously after the filter swap. Dropping the client
    // closes the TCP half, which fires the RTMP on_unpublish
    // path.
    drop(rtmp_before);
    drop(session_before);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Swap in the drop-every-fragment filter. The reloader
    // watches the parent dir, so a rename into place (the
    // atomic-replace idiom) fires a Modify event the reloader
    // debounces and acts on.
    replace_atomically(&drop_all, &filter_path);

    // Let the watcher observe the swap before the next
    // publish. notify's FSEvents backend on macOS is
    // configured with latency=0.0 (immediate delivery) but the
    // runloop still needs a beat to deliver the event + the
    // debounce window to elapse + the fresh module to compile.
    // 500 ms is conservative on Linux inotify (sub-ms) and
    // sufficient on macOS in practice; the polling loop below
    // absorbs any residual jitter.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Phase 2: drop filter in effect. Publish a fresh broadcast
    // and assert fragments_dropped increments on it. The new
    // broadcast name is chosen so the assertion cannot be
    // satisfied by stale counters from phase 1.
    let (rtmp_after, session_after) = publish_two_keyframes(rtmp_addr, "live", "hot-reload-after").await;
    let observed_drops = wait_for(Duration::from_secs(10), || {
        tap.fragments_dropped("live/hot-reload-after", "0.mp4") > 0
    })
    .await;

    let seen_after = tap.fragments_seen("live/hot-reload-after", "0.mp4");
    let dropped_after = tap.fragments_dropped("live/hot-reload-after", "0.mp4");
    let kept_after = tap.fragments_kept("live/hot-reload-after", "0.mp4");
    assert!(
        observed_drops,
        "post-reload phase: reloader failed to swap in redact-keyframes within 10 s; seen_after={seen_after} kept_after={kept_after} dropped_after={dropped_after}",
    );
    assert!(
        dropped_after > 0,
        "post-reload phase: drop filter must have dropped at least one fragment on live/hot-reload-after; seen={seen_after} kept={kept_after} dropped={dropped_after}",
    );

    drop(rtmp_after);
    drop(session_after);

    server.shutdown().await.expect("shutdown");
}

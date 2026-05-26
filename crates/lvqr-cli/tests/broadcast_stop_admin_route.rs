//! End-to-end test for Slice 6's `GET /api/v1/broadcasts` +
//! `DELETE /api/v1/broadcasts/{name}` routes -- the truly-disconnect-a-live-
//! publisher mutation.
//!
//! Publishes a real RTMP session through the ingest bridge (same pattern as
//! `stream_detail_admin_route.rs`), waits for the session to surface in the
//! broadcast registry, then issues `DELETE` against it. The strongest
//! assertion: the RTMP TCP stream sees EOF after the kill -- the OS socket
//! has actually closed, not just the bookkeeping flipped -- mirroring the
//! Slice 9b test's "ingest port refuses TCP" guarantee.

use lvqr_test_utils::flv::{flv_video_nalu, flv_video_seq_header};
use lvqr_test_utils::http::{HttpGetOptions, http_delete, http_get_with};
use lvqr_test_utils::rtmp::{read_until, rtmp_client_handshake, send_result, send_results};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::sessions::{ClientSession, ClientSessionConfig, ClientSessionEvent, PublishRequestType};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

/// RTMP client: connect + handshake + connect + publish to the named broadcast.
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

/// Snapshot the broadcast-session inventory and return whether the named
/// broadcast is present.
async fn broadcast_present(admin: SocketAddr, name: &str) -> bool {
    let resp = http_get_with(admin, "/api/v1/broadcasts", HttpGetOptions::default()).await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    body["sessions"]
        .as_array()
        .map(|arr| arr.iter().any(|s| s["broadcast"].as_str() == Some(name)))
        .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broadcast_stop_kicks_a_live_rtmp_publisher() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let admin_addr = server.admin_addr();
    let rtmp_addr = server.rtmp_addr();

    // Publish a real RTMP session: handshake -> connect -> publish -> push a
    // video sequence header + one keyframe so the bridge has an ActiveStream
    // and the session is registered in the broadcast registry.
    let (mut rtmp_stream, mut rtmp_session) = connect_and_publish(rtmp_addr, "live", "demo").await;
    let seq = flv_video_seq_header();
    let result = rtmp_session
        .publish_video_data(seq, RtmpTimestamp::new(0), false)
        .unwrap();
    send_result(&mut rtmp_stream, &result).await;
    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf = flv_video_nalu(true, 0, &nalu);
    let result = rtmp_session
        .publish_video_data(kf, RtmpTimestamp::new(0), false)
        .unwrap();
    send_result(&mut rtmp_stream, &result).await;

    // Poll until the session surfaces (the bridge's on_publish callback fires
    // after PublishRequestAccepted, and the registrar runs synchronously
    // right after that).
    let mut seen = false;
    for _ in 0..20 {
        if broadcast_present(admin_addr, "live/demo").await {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(seen, "broadcast live/demo must surface in /api/v1/broadcasts");

    // Sanity: the inventory row carries protocol=rtmp + a peer address.
    let resp = http_get_with(admin_addr, "/api/v1/broadcasts", HttpGetOptions::default()).await;
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    let row = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["broadcast"].as_str() == Some("live/demo"))
        .unwrap();
    assert_eq!(row["protocol"], "rtmp");
    assert!(row["peer"].as_str().is_some(), "peer addr must be captured for RTMP");

    // KILL the live publisher.
    let resp = http_delete(admin_addr, "/api/v1/broadcasts/live%2Fdemo", None).await;
    assert_eq!(
        resp.status,
        200,
        "kill status={} body={}",
        resp.status,
        resp.body_text()
    );
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(body["result"], "killed");
    assert_eq!(body["broadcast"], "live/demo");
    assert_eq!(body["protocol"], "rtmp");

    // STRONGEST assertion: the publisher's RTMP TCP stream now sees EOF --
    // the OS socket has actually closed, not just bookkeeping flipped.
    let mut buf = [0u8; 4096];
    let mut closed = false;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_millis(100), rtmp_stream.read(&mut buf)).await {
            Ok(Ok(0)) => {
                closed = true;
                break;
            }
            Ok(Err(_)) => {
                closed = true;
                break;
            }
            Ok(Ok(_)) | Err(_) => continue,
        }
    }
    assert!(closed, "RTMP TCP stream must see EOF / error after broadcast stop");

    // Inventory now omits the entry (deregister fires on the cancel path's
    // return, via the DeregisterGuard in handle_rtmp_session).
    let mut gone = false;
    for _ in 0..20 {
        if !broadcast_present(admin_addr, "live/demo").await {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone, "broadcast live/demo must be gone from inventory after stop");

    // Repeat kill -> 404 because no live session remains with that name.
    let repeat = http_delete(admin_addr, "/api/v1/broadcasts/live%2Fdemo", None).await;
    assert_eq!(
        repeat.status, 404,
        "repeat kill must 404 (no live session); got {}",
        repeat.status
    );

    server.shutdown().await.expect("shutdown");
}

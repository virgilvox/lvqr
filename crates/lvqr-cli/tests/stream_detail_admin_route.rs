//! End-to-end test for `GET /api/v1/streams/{name}` (per-broadcast detail).
//!
//! Starts a `TestServer`, publishes a real RTMP video keyframe sequence
//! through the ingest bridge into the shared `FragmentBroadcasterRegistry`,
//! then issues a real HTTP GET against the admin server's per-broadcast
//! detail route. Asserts the JSON body carries the broadcast name, a
//! non-empty `tracks` array with a video track whose `fragments` counter
//! advanced, and that an unknown broadcast returns 404 rather than an
//! empty 200. Companion to `wasm_filter_admin_route.rs`; proves the
//! `with_stream_detail` snapshot closure is wired from the registry into
//! the route's JSON body. No mocks.

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
async fn stream_detail_route_reports_tracks_for_live_broadcast() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::new())
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    let (_rtmp_stream, _session) = publish_two_keyframes(rtmp_addr, "live", "detail").await;
    // The on_fragment path spawns one tokio task per push; give them a
    // tick to register on the shared registry before we read.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // The broadcast name contains a `/`, so it must be URL-encoded into the
    // single `{name}` path segment.
    let resp = http_get_with(admin_addr, "/api/v1/streams/live%2Fdetail", HttpGetOptions::default()).await;
    assert_eq!(resp.status, 200, "status={} body={}", resp.status, resp.body_text());

    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("valid JSON");
    assert_eq!(body["name"], "live/detail", "detail must echo the requested broadcast");

    let tracks = body["tracks"].as_array().expect("tracks is array");
    assert!(
        !tracks.is_empty(),
        "live broadcast must surface at least one track; got {body}"
    );

    let video = tracks
        .iter()
        .find(|t| t["kind"] == "video")
        .unwrap_or_else(|| panic!("expected a video track in {tracks:?}"));
    assert!(
        video["fragments"].as_u64().expect("fragments is u64") > 0,
        "video track must have emitted at least one fragment; got {video}"
    );
    assert!(
        !video["codec"].as_str().unwrap_or("").is_empty(),
        "video track must carry a codec string; got {video}"
    );
    // `subscribers` is the busiest-track count; with no egress subscriber it
    // is 0 but the field must be present and numeric.
    assert!(body["subscribers"].is_u64(), "subscribers must be numeric; got {body}");

    // Unknown broadcast must 404, not fabricate an empty 200.
    let unknown = http_get_with(admin_addr, "/api/v1/streams/live%2Fghost", HttpGetOptions::default()).await;
    assert_eq!(
        unknown.status, 404,
        "unknown broadcast should 404, got {}",
        unknown.status
    );

    drop(_rtmp_stream);
    drop(_session);
    server.shutdown().await.expect("shutdown");
}

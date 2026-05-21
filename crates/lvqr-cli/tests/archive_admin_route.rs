//! End-to-end test for `GET /api/v1/archive` (read-only recording list).
//!
//! Publishes a real RTMP keyframe sequence to a server started with an
//! archive directory, lets the `BroadcasterArchiveIndexer` write segments
//! into the redb index, then issues a real HTTP GET against the admin
//! archive route and asserts the recorded broadcast surfaces with its track
//! and a non-zero segment count. Companion to `rtmp_archive_e2e.rs` (which
//! verifies the `/playback/*` read path); this proves the `with_archive`
//! snapshot closure aggregates the same index. No mocks; archive is not
//! feature-gated, so this runs on every default build.

use lvqr_test_utils::flv::{flv_video_nalu, flv_video_seq_header};
use lvqr_test_utils::http::{HttpGetOptions, http_get_with};
use lvqr_test_utils::rtmp::{read_until, rtmp_client_handshake, send_result, send_results};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::sessions::{ClientSession, ClientSessionConfig, ClientSessionEvent, PublishRequestType};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::Duration;
use tempfile::TempDir;
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
async fn archive_route_lists_recorded_broadcast() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let archive_tmp = TempDir::new().expect("tempdir");
    let server = TestServer::start(TestServerConfig::default().with_archive_dir(archive_tmp.path()))
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    let (_s, _sess) = publish_two_keyframes(rtmp_addr, "live", "dvr").await;
    // The archiving observer spawns one blocking task per fragment; give them
    // time to land on disk and into redb before we query.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = http_get_with(admin_addr, "/api/v1/archive", HttpGetOptions::default()).await;
    assert_eq!(resp.status, 200, "status={} body={}", resp.status, resp.body_text());

    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("valid JSON");
    assert_eq!(body["enabled"], true, "archive-dir configured must report enabled");

    let recordings = body["recordings"].as_array().expect("recordings is array");
    let rec = recordings
        .iter()
        .find(|r| r["broadcast"] == "live/dvr")
        .unwrap_or_else(|| panic!("expected live/dvr in recordings; got {recordings:?}"));

    assert!(
        rec["segment_count"].as_u64().expect("segment_count u64") >= 1,
        "recorded broadcast must have at least one segment; got {rec}"
    );
    let tracks = rec["tracks"].as_array().expect("tracks is array");
    let video = tracks
        .iter()
        .find(|t| t["track"] == "0.mp4")
        .unwrap_or_else(|| panic!("expected 0.mp4 track in {tracks:?}"));
    assert_eq!(video["timescale"], 90_000);
    assert!(
        video["segment_count"].as_u64().expect("track segment_count u64") >= 1,
        "video track must have at least one segment; got {video}"
    );

    // Must read the archive route before shutdown (the server holds the redb
    // lock); shutdown after.
    drop(_s);
    drop(_sess);
    server.shutdown().await.expect("shutdown");
}

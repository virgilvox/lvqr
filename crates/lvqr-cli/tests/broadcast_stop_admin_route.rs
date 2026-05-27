//! End-to-end tests for Slice 6's `GET /api/v1/broadcasts` +
//! `DELETE /api/v1/broadcasts/{name}` routes -- the truly-disconnect-a-live-
//! publisher mutation, across every ingest protocol the relay wires (RTMP,
//! SRT, RTSP). Each test publishes a real session through the relevant
//! ingest crate, polls until the entry surfaces in the broadcast inventory,
//! kills it via DELETE, and -- STRONGEST assertion -- reads from the
//! publisher's transport stream and asserts the disconnect (TCP EOF for
//! RTMP / RTSP, SRT socket close for SRT).
//!
//! All three protocol tests live in this single file so the workspace
//! produces ONE integration-test binary instead of three. That matters: CI
//! Linux compiles every integration-test binary in `--jobs 2` parallel
//! link steps, and the runner has historically crashed with
//! `collect2: fatal error: ld terminated with signal 7 [Bus error]` once
//! the total binary count crosses an mmap-tmp-file ceiling (see ci.yml
//! comment on the test step). Consolidating the three protocol cases here
//! reduces link pressure by two binaries.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use lvqr_test_utils::flv::{flv_video_nalu, flv_video_seq_header};
use lvqr_test_utils::http::{HttpGetOptions, http_delete, http_get_with};
use lvqr_test_utils::rtmp::{read_until, rtmp_client_handshake, send_result, send_results};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::sessions::{ClientSession, ClientSessionConfig, ClientSessionEvent, PublishRequestType};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);
const SYNC_BYTE: u8 = 0x47;

// -----------------------------------------------------------------------
// Shared helpers
// -----------------------------------------------------------------------

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

// -----------------------------------------------------------------------
// RTMP
// -----------------------------------------------------------------------

/// RTMP client: connect + handshake + connect + publish to the named broadcast.
async fn rtmp_connect_and_publish(addr: SocketAddr, app: &str, stream_key: &str) -> (TcpStream, ClientSession) {
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
    let (mut rtmp_stream, mut rtmp_session) = rtmp_connect_and_publish(rtmp_addr, "live", "demo").await;
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
            Ok(Ok(0)) | Ok(Err(_)) => {
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

// -----------------------------------------------------------------------
// SRT
// -----------------------------------------------------------------------

/// Build a minimal MPEG-TS packet for a single PID.
fn ts_packet(pid: u16, pusi: bool, payload: &[u8]) -> Vec<u8> {
    let mut pkt = vec![0xFFu8; 188];
    pkt[0] = SYNC_BYTE;
    pkt[1] = if pusi { 0x40 } else { 0x00 } | ((pid >> 8) as u8 & 0x1F);
    pkt[2] = pid as u8;
    pkt[3] = 0x10;
    let copy = payload.len().min(184);
    pkt[4..4 + copy].copy_from_slice(&payload[..copy]);
    pkt
}

fn minimal_pat(pmt_pid: u16) -> Vec<u8> {
    let mut data = vec![0x00, 0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01];
    data.push(0xE0 | ((pmt_pid >> 8) as u8 & 0x1F));
    data.push(pmt_pid as u8);
    data.extend_from_slice(&[0x00; 4]);
    data
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broadcast_stop_kicks_a_live_srt_publisher() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default().with_srt())
        .await
        .expect("start TestServer with SRT");
    let admin_addr = server.admin_addr();
    let srt_addr = server.srt_addr();

    // Connect as an SRT caller. Without a streamid the server defaults the
    // broadcast name to "srt/default" (see `lvqr_srt::ingest`'s extract
    // fallback). That's the name we kill below.
    let mut srt: srt_tokio::SrtSocket = srt_tokio::SrtSocket::builder()
        .call(srt_addr, None)
        .await
        .expect("SRT connect");

    let pmt_pid = 0x1000u16;
    let mut ts_data = Vec::new();
    ts_data.extend_from_slice(&ts_packet(0, true, &minimal_pat(pmt_pid)));
    srt.send((Instant::now(), Bytes::from(ts_data))).await.unwrap();

    let mut seen = false;
    for _ in 0..40 {
        if broadcast_present(admin_addr, "srt/default").await {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(seen, "broadcast srt/default must surface in /api/v1/broadcasts");

    let resp = http_get_with(admin_addr, "/api/v1/broadcasts", HttpGetOptions::default()).await;
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    let row = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["broadcast"].as_str() == Some("srt/default"))
        .unwrap();
    assert_eq!(row["protocol"], "srt");
    assert!(row["peer"].as_str().is_some(), "peer addr must be captured for SRT");

    let resp = http_delete(admin_addr, "/api/v1/broadcasts/srt%2Fdefault", None).await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(body["result"], "killed");
    assert_eq!(body["protocol"], "srt");

    // SRT teardown takes several poll ticks (`None` or `Err` from
    // `srt.next()` -- either is the disconnect).
    let mut closed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), srt.next()).await {
            Ok(None) | Ok(Some(Err(_))) => {
                closed = true;
                break;
            }
            Ok(Some(Ok(_))) | Err(_) => continue,
        }
    }
    assert!(closed, "SRT socket must close after broadcast stop");

    let mut gone = false;
    for _ in 0..40 {
        if !broadcast_present(admin_addr, "srt/default").await {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone, "broadcast srt/default must be gone from inventory after stop");

    server.shutdown().await.expect("shutdown");
}

// -----------------------------------------------------------------------
// RTSP
// -----------------------------------------------------------------------

async fn rtsp_roundtrip(stream: &mut TcpStream, request: &str) -> String {
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(TIMEOUT, stream.read(&mut buf))
        .await
        .expect("RTSP read timed out")
        .expect("RTSP read failed");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broadcast_stop_kicks_a_live_rtsp_publisher() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default().with_rtsp())
        .await
        .expect("start TestServer with RTSP");
    let admin_addr = server.admin_addr();
    let rtsp_addr = server.rtsp_addr();

    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(rtsp_addr))
        .await
        .expect("RTSP connect timed out")
        .expect("RTSP connect failed");

    let base_uri = format!("rtsp://{rtsp_addr}/publish/rtsp_test");
    let sdp = "v=0\r\n\
         o=- 0 0 IN IP4 127.0.0.1\r\n\
         s=Test\r\n\
         m=video 0 RTP/AVP 96\r\n\
         a=rtpmap:96 H264/90000\r\n\
         a=control:track1\r\n";
    let announce = format!(
        "ANNOUNCE {base_uri} RTSP/1.0\r\n\
         CSeq: 1\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {sdp}",
        sdp.len()
    );
    let resp = rtsp_roundtrip(&mut stream, &announce).await;
    assert!(resp.contains("RTSP/1.0 200"), "ANNOUNCE failed: {resp}");

    let target = "publish/rtsp_test";
    let mut seen = false;
    for _ in 0..40 {
        if broadcast_present(admin_addr, target).await {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(seen, "broadcast {target} must surface in /api/v1/broadcasts");

    let resp = http_get_with(admin_addr, "/api/v1/broadcasts", HttpGetOptions::default()).await;
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    let row = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["broadcast"].as_str() == Some(target))
        .unwrap();
    assert_eq!(row["protocol"], "rtsp");
    assert!(row["peer"].as_str().is_some(), "peer addr must be captured for RTSP");

    let resp = http_delete(admin_addr, "/api/v1/broadcasts/publish%2Frtsp_test", None).await;
    assert_eq!(resp.status, 200, "kill body={}", resp.body_text());
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(body["result"], "killed");
    assert_eq!(body["protocol"], "rtsp");

    let mut buf = [0u8; 4096];
    let mut closed = false;
    for _ in 0..30 {
        match tokio::time::timeout(Duration::from_millis(100), stream.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) => {
                closed = true;
                break;
            }
            Ok(Ok(_)) | Err(_) => continue,
        }
    }
    assert!(closed, "RTSP TCP stream must see EOF after broadcast stop");

    let mut gone = false;
    for _ in 0..40 {
        if !broadcast_present(admin_addr, target).await {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone, "broadcast {target} must be gone from inventory after stop");

    server.shutdown().await.expect("shutdown");
}

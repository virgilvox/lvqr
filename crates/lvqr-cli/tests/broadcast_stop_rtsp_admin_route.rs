//! Slice 6 (RTSP half): `DELETE /api/v1/broadcasts/{name}` truly disconnects
//! a live RTSP publisher. ANNOUNCE registers the session with the
//! per-publisher cancel token; the kill cancels it; the connection's main
//! `select!` falls into the `shutdown.cancelled()` arm and tears the TCP
//! down. Test asserts the publisher's socket sees EOF, the inventory
//! empties, and the broadcast name resolves correctly from the ANNOUNCE
//! URI (`rtsp://addr/publish/<broadcast>`).

use lvqr_test_utils::http::{HttpGetOptions, http_delete, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

async fn rtsp_roundtrip(stream: &mut TcpStream, request: &str) -> String {
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(TIMEOUT, stream.read(&mut buf))
        .await
        .expect("RTSP read timed out")
        .expect("RTSP read failed");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

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

    // ANNOUNCE registers the session. The broadcast name in the URI is
    // `publish/rtsp_test` -- after the lvqr-rtsp `extract_broadcast` strips
    // the leading `rtsp://addr/`, the session.broadcast is `publish/rtsp_test`.
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

    // Poll until the session surfaces in the broadcast inventory.
    let mut seen = false;
    let target = "publish/rtsp_test";
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

    // KILL the live publisher (URL-encoded forward slash).
    let resp = http_delete(admin_addr, "/api/v1/broadcasts/publish%2Frtsp_test", None).await;
    assert_eq!(resp.status, 200, "kill body={}", resp.body_text());
    let body: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(body["result"], "killed");
    assert_eq!(body["protocol"], "rtsp");

    // STRONGEST assertion: the RTSP TCP stream sees EOF -- the OS socket is
    // closed, not just bookkeeping flipped.
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

    // Inventory now omits the entry (deregister fires on the connection's
    // way out, via `registered_broadcasts` in ConnectionState).
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

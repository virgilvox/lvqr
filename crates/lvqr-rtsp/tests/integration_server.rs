//! Integration tests for the RTSP server with real TCP connections.
//!
//! Integration slot of the 5-artifact contract for `lvqr-rtsp`.
//! Tests exercise the server over real TCP sockets, verifying the
//! full RTSP request/response path including connection handling,
//! handshake flows, and interleaved RTP frame routing through the
//! shared `FragmentBroadcasterRegistry`.

use std::net::SocketAddr;
use std::time::Duration;

use lvqr_core::EventBus;
use lvqr_fragment::{FragmentBroadcasterRegistry, FragmentStream};
use lvqr_rtsp::RtspServer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

const TIMEOUT: Duration = Duration::from_secs(5);

async fn start_server() -> (SocketAddr, CancellationToken, FragmentBroadcasterRegistry) {
    let shutdown = CancellationToken::new();
    let events = EventBus::with_capacity(16);
    let registry = FragmentBroadcasterRegistry::new();

    let mut server = RtspServer::with_registry("127.0.0.1:0".parse().unwrap(), registry.clone());
    let addr = server.bind().await.expect("bind RTSP server");
    let ev = events.clone();
    let cancel = shutdown.clone();
    tokio::spawn(async move {
        server.run(ev, cancel).await.ok();
    });
    // Give the server a moment to start accepting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, shutdown, registry)
}

async fn rtsp_send_recv(stream: &mut TcpStream, request: &str) -> String {
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(TIMEOUT, stream.read(&mut buf))
        .await
        .expect("read timed out")
        .expect("read failed");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

#[tokio::test]
async fn options_returns_supported_methods() {
    let (addr, shutdown, _registry) = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();

    let resp = rtsp_send_recv(
        &mut stream,
        "OPTIONS rtsp://localhost/live/test RTSP/1.0\r\nCSeq: 1\r\n\r\n",
    )
    .await;
    assert!(resp.contains("RTSP/1.0 200"));
    assert!(resp.contains("DESCRIBE"));
    assert!(resp.contains("SETUP"));
    assert!(resp.contains("PLAY"));
    assert!(resp.contains("RECORD"));
    assert!(resp.contains("ANNOUNCE"));

    shutdown.cancel();
}

#[tokio::test]
async fn describe_returns_sdp() {
    let (addr, shutdown, _registry) = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();

    let resp = rtsp_send_recv(
        &mut stream,
        "DESCRIBE rtsp://localhost/live/cam1 RTSP/1.0\r\nCSeq: 1\r\n\r\n",
    )
    .await;
    // No broadcaster exists yet, so the SDP carries only the session
    // header with no m= media blocks. The DESCRIBE still returns 200
    // + application/sdp per RFC 2326 so clients can observe the empty
    // stream rather than racing a 404.
    assert!(resp.contains("RTSP/1.0 200"));
    assert!(resp.contains("application/sdp"));
    assert!(resp.contains("v=0"));
    assert!(!resp.contains("m=video"), "no video m= before any publisher");

    shutdown.cancel();
}

/// Full ingest handshake + RTP push: the broadcaster registry exposes
/// the init segment (via `broadcaster.meta().init_segment`) and the
/// keyframe fragment (via `broadcaster.subscribe()`).
#[tokio::test]
async fn full_ingest_handshake_emits_fragments_on_registry() {
    let (addr, shutdown, registry) = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let base = format!("rtsp://{addr}/publish/integration_test");

    // ANNOUNCE
    let sdp = "v=0\r\n\
               o=- 0 0 IN IP4 127.0.0.1\r\n\
               s=Test\r\n\
               m=video 0 RTP/AVP 96\r\n\
               a=rtpmap:96 H264/90000\r\n\
               a=control:track1\r\n";
    let announce = format!(
        "ANNOUNCE {base} RTSP/1.0\r\nCSeq: 1\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{sdp}",
        sdp.len()
    );
    let resp = rtsp_send_recv(&mut stream, &announce).await;
    assert!(resp.contains("200"), "ANNOUNCE: {resp}");
    let session_id = resp
        .lines()
        .find(|l| l.starts_with("Session:"))
        .unwrap()
        .strip_prefix("Session:")
        .unwrap()
        .trim()
        .split(';')
        .next()
        .unwrap()
        .trim()
        .to_string();

    // SETUP
    let setup = format!(
        "SETUP {base}/track1 RTSP/1.0\r\nCSeq: 2\r\nSession: {session_id}\r\nTransport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\r\n"
    );
    let resp = rtsp_send_recv(&mut stream, &setup).await;
    assert!(resp.contains("200"), "SETUP: {resp}");

    // RECORD
    let record = format!("RECORD {base} RTSP/1.0\r\nCSeq: 3\r\nSession: {session_id}\r\n\r\n");
    let resp = rtsp_send_recv(&mut stream, &record).await;
    assert!(resp.contains("200"), "RECORD: {resp}");

    // Push STAP-A (SPS+PPS) so the broadcaster is created with init.
    let sps = [0x67u8, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
    let pps = [0x68u8, 0xEE, 0x3C, 0x80];
    let mut stap = vec![24u8];
    stap.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    stap.extend_from_slice(&sps);
    stap.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    stap.extend_from_slice(&pps);
    stream
        .write_all(&interleave(0, &rtp_packet(96, 1, 90000, false, &stap)))
        .await
        .unwrap();

    // Give the server a moment to process STAP-A so the broadcaster
    // exists before we subscribe. Subscribing before creation would
    // miss the subsequent keyframe since the broadcaster does not
    // history-buffer.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let broadcaster = registry
        .get("publish/integration_test", "0.mp4")
        .expect("broadcaster created after STAP-A + init emit");
    assert!(
        broadcaster.meta().init_segment.is_some(),
        "broadcaster carries init segment set via set_init_segment"
    );
    let mut sub = broadcaster.subscribe();

    // Push IDR keyframe.
    let idr = [0x65u8, 0x88, 0x84, 0x00, 0xDE, 0xAD];
    stream
        .write_all(&interleave(0, &rtp_packet(96, 2, 93000, true, &idr)))
        .await
        .unwrap();

    // Pull the keyframe fragment off the broadcaster side with a timeout.
    let frag = tokio::time::timeout(Duration::from_secs(2), sub.next_fragment())
        .await
        .expect("broadcaster frag timed out")
        .expect("broadcaster stream should emit a fragment");
    assert!(frag.flags.keyframe, "broadcaster keyframe flag preserved");
    assert_eq!(frag.track_id, "0.mp4");

    // TEARDOWN
    let teardown = format!("TEARDOWN {base} RTSP/1.0\r\nCSeq: 4\r\nSession: {session_id}\r\n\r\n");
    let resp = rtsp_send_recv(&mut stream, &teardown).await;
    assert!(resp.contains("200"), "TEARDOWN: {resp}");

    shutdown.cancel();
}

fn rtp_packet(pt: u8, seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
    let mut pkt = vec![0u8; 12 + payload.len()];
    pkt[0] = 0x80;
    pkt[1] = pt | if marker { 0x80 } else { 0x00 };
    pkt[2..4].copy_from_slice(&seq.to_be_bytes());
    pkt[4..8].copy_from_slice(&ts.to_be_bytes());
    pkt[8..12].copy_from_slice(&0x12345678u32.to_be_bytes());
    pkt[12..].copy_from_slice(payload);
    pkt
}

fn interleave(channel: u8, rtp: &[u8]) -> Vec<u8> {
    let len = rtp.len() as u16;
    let mut frame = Vec::with_capacity(4 + rtp.len());
    frame.push(0x24);
    frame.push(channel);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(rtp);
    frame
}

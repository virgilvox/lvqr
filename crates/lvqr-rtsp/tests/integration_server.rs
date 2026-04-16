//! Integration tests for the RTSP server with real TCP connections.
//!
//! Integration slot of the 5-artifact contract for `lvqr-rtsp`.
//! Tests exercise the server over real TCP sockets, verifying the
//! full RTSP request/response path including connection handling,
//! handshake flows, and interleaved RTP frame routing.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use lvqr_core::EventBus;
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentStream};
use lvqr_ingest::{FragmentObserver, SharedFragmentObserver};
use lvqr_rtsp::RtspServer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

const TIMEOUT: Duration = Duration::from_secs(5);

struct SpyObserver {
    init_count: AtomicU32,
    fragment_count: AtomicU32,
    broadcasts: Mutex<Vec<String>>,
}

impl FragmentObserver for SpyObserver {
    fn on_init(&self, broadcast: &str, _track: &str, _timescale: u32, _init: Bytes) {
        self.init_count.fetch_add(1, Ordering::Relaxed);
        self.broadcasts.lock().unwrap().push(broadcast.to_string());
    }
    fn on_fragment(&self, _broadcast: &str, _track: &str, _fragment: &Fragment) {
        self.fragment_count.fetch_add(1, Ordering::Relaxed);
    }
}

async fn start_server() -> (SocketAddr, SharedFragmentObserver, CancellationToken, Arc<SpyObserver>) {
    let spy = Arc::new(SpyObserver {
        init_count: AtomicU32::new(0),
        fragment_count: AtomicU32::new(0),
        broadcasts: Mutex::new(Vec::new()),
    });
    let obs: SharedFragmentObserver = spy.clone();
    let shutdown = CancellationToken::new();
    let events = EventBus::with_capacity(16);

    let mut server = RtspServer::new("127.0.0.1:0".parse().unwrap());
    let addr = server.bind().await.expect("bind RTSP server");
    let obs_clone = Some(obs.clone());
    let ev = events.clone();
    let cancel = shutdown.clone();
    tokio::spawn(async move {
        server.run(obs_clone, ev, cancel).await.ok();
    });
    // Give the server a moment to start accepting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, obs, shutdown, spy)
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
    let (addr, _obs, shutdown, _spy) = start_server().await;
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
    let (addr, _obs, shutdown, _spy) = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();

    let resp = rtsp_send_recv(
        &mut stream,
        "DESCRIBE rtsp://localhost/live/cam1 RTSP/1.0\r\nCSeq: 1\r\n\r\n",
    )
    .await;
    assert!(resp.contains("RTSP/1.0 200"));
    assert!(resp.contains("application/sdp"));
    assert!(resp.contains("H264/90000"));

    shutdown.cancel();
}

#[tokio::test]
async fn full_ingest_handshake_emits_fragments() {
    let (addr, _obs, shutdown, spy) = start_server().await;
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
        .trim();

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

    // Push STAP-A (SPS+PPS) + IDR via interleaved RTP.
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

    let idr = [0x65u8, 0x88, 0x84, 0x00, 0xDE, 0xAD];
    stream
        .write_all(&interleave(0, &rtp_packet(96, 2, 93000, true, &idr)))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(spy.init_count.load(Ordering::Relaxed) >= 1, "init segment not emitted");
    assert!(spy.fragment_count.load(Ordering::Relaxed) >= 1, "no fragments emitted");
    {
        let broadcasts = spy.broadcasts.lock().unwrap();
        assert!(broadcasts.iter().any(|b| b.contains("integration_test")));
    }

    // TEARDOWN
    let teardown = format!("TEARDOWN {base} RTSP/1.0\r\nCSeq: 4\r\nSession: {session_id}\r\n\r\n");
    let resp = rtsp_send_recv(&mut stream, &teardown).await;
    assert!(resp.contains("200"), "TEARDOWN: {resp}");

    shutdown.cancel();
}

/// Session 56 dual-wire regression: after full ingest handshake + RTP
/// push, both the legacy FragmentObserver hook AND a subscription on the
/// server's `FragmentBroadcasterRegistry` must receive the init segment
/// (via `broadcaster.meta().init_segment`) and the keyframe fragment. This
/// pins the migration contract: new broadcaster-native consumers see
/// equivalent data to old observer consumers, so the next session can
/// migrate consumers off the observer side without losing coverage.
#[tokio::test]
async fn dual_wire_broadcaster_matches_observer_for_h264_keyframe() {
    // Custom start path that constructs the server with an external
    // registry so the test can hold a handle.
    let spy = Arc::new(SpyObserver {
        init_count: AtomicU32::new(0),
        fragment_count: AtomicU32::new(0),
        broadcasts: Mutex::new(Vec::new()),
    });
    let obs: SharedFragmentObserver = spy.clone();
    let shutdown = CancellationToken::new();
    let events = EventBus::with_capacity(16);
    let registry = FragmentBroadcasterRegistry::new();

    let mut server = RtspServer::with_registry("127.0.0.1:0".parse().unwrap(), registry.clone());
    let addr = server.bind().await.expect("bind");
    let obs_clone = Some(obs.clone());
    let ev = events.clone();
    let cancel = shutdown.clone();
    tokio::spawn(async move {
        server.run(obs_clone, ev, cancel).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let base = format!("rtsp://{addr}/publish/dual_wire_test");
    let mut stream = TcpStream::connect(addr).await.unwrap();

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

    let setup = format!(
        "SETUP {base}/track1 RTSP/1.0\r\nCSeq: 2\r\nSession: {session_id}\r\nTransport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\r\n"
    );
    let resp = rtsp_send_recv(&mut stream, &setup).await;
    assert!(resp.contains("200"), "SETUP: {resp}");

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
    // exists before we subscribe. (Subscribing before the broadcaster is
    // created would miss the subsequent keyframe because the broadcaster
    // is not history-buffering.)
    tokio::time::sleep(Duration::from_millis(100)).await;

    let broadcaster = registry
        .get("publish/dual_wire_test", "0.mp4")
        .expect("broadcaster created after STAP-A + init emit");
    assert!(
        broadcaster.meta().init_segment.is_some(),
        "broadcaster carries init segment set via set_init_segment"
    );
    let mut sub = broadcaster.subscribe();

    // Push IDR keyframe. Both observer and broadcaster subscription
    // should see exactly one fragment.
    let idr = [0x65u8, 0x88, 0x84, 0x00, 0xDE, 0xAD];
    stream
        .write_all(&interleave(0, &rtp_packet(96, 2, 93000, true, &idr)))
        .await
        .unwrap();

    // Pull a fragment off the broadcaster side with a timeout.
    let frag = tokio::time::timeout(Duration::from_secs(2), sub.next_fragment())
        .await
        .expect("broadcaster frag timed out")
        .expect("broadcaster stream should emit a fragment");

    // Observer side saw at least one fragment (STAP-A alone does not emit,
    // but STAP-A + IDR does: one keyframe fragment).
    let obs_fragment_count = spy.fragment_count.load(Ordering::Relaxed);
    assert!(
        obs_fragment_count >= 1,
        "observer fragment_count >= 1 (saw {obs_fragment_count})"
    );
    assert!(frag.flags.keyframe, "broadcaster keyframe flag preserved");
    assert_eq!(frag.track_id, "0.mp4");

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

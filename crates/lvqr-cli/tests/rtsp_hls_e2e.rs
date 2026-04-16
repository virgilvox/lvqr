//! RTSP ingest -> LL-HLS HTTP egress end-to-end integration test.
//!
//! Connects via raw TCP, performs an RTSP ANNOUNCE/SETUP/RECORD
//! handshake, then pushes interleaved RTP frames containing H.264
//! parameter sets + IDR slices. Verifies the HLS playlist appears.

use lvqr_test_utils::{TestServer, TestServerConfig};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("http connect timed out")
        .expect("http connect failed");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(TIMEOUT, stream.read_to_end(&mut buf))
        .await
        .expect("http read timed out")
        .expect("http read failed");
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("missing header terminator");
    let header_text = std::str::from_utf8(&buf[..split]).unwrap();
    let status: u16 = header_text
        .lines()
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    HttpResponse {
        status,
        body: buf[split + 4..].to_vec(),
    }
}

/// Build a minimal RTP packet with the given payload.
fn make_rtp_packet(pt: u8, seq: u16, ts: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
    let mut pkt = vec![0u8; 12 + payload.len()];
    pkt[0] = 0x80; // version=2
    pkt[1] = pt | if marker { 0x80 } else { 0x00 };
    pkt[2..4].copy_from_slice(&seq.to_be_bytes());
    pkt[4..8].copy_from_slice(&ts.to_be_bytes());
    pkt[8..12].copy_from_slice(&0x12345678u32.to_be_bytes());
    pkt[12..].copy_from_slice(payload);
    pkt
}

/// Wrap an RTP packet in an interleaved TCP frame ($ header).
fn interleave(channel: u8, rtp: &[u8]) -> Vec<u8> {
    let len = rtp.len() as u16;
    let mut frame = Vec::with_capacity(4 + rtp.len());
    frame.push(0x24); // '$'
    frame.push(channel);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(rtp);
    frame
}

/// Send an RTSP request and read the response. Returns the full
/// response text (headers + body).
async fn rtsp_roundtrip(stream: &mut TcpStream, request: &str) -> String {
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(TIMEOUT, stream.read(&mut buf))
        .await
        .expect("RTSP read timed out")
        .expect("RTSP read failed");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

#[tokio::test]
async fn rtsp_push_reaches_hls_playlist() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default().with_rtsp())
        .await
        .expect("start TestServer with RTSP");
    let rtsp_addr = server.rtsp_addr();
    let hls_addr = server.hls_addr();

    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(rtsp_addr))
        .await
        .expect("RTSP connect timed out")
        .expect("RTSP connect failed");

    let base_uri = format!("rtsp://{rtsp_addr}/publish/rtsp_test");

    // ANNOUNCE with SDP describing H.264 video.
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

    // Extract session ID from the response (strip any ;timeout=N suffix).
    let session_line = resp
        .lines()
        .find(|l| l.starts_with("Session:"))
        .expect("no Session header in ANNOUNCE response");
    let session_id = session_line
        .strip_prefix("Session:")
        .unwrap()
        .trim()
        .split(';')
        .next()
        .unwrap()
        .trim();

    // SETUP
    let setup = format!(
        "SETUP {base_uri}/track1 RTSP/1.0\r\n\
         CSeq: 2\r\n\
         Session: {session_id}\r\n\
         Transport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\
         \r\n"
    );
    let resp = rtsp_roundtrip(&mut stream, &setup).await;
    assert!(resp.contains("RTSP/1.0 200"), "SETUP failed: {resp}");

    // RECORD
    let record = format!(
        "RECORD {base_uri} RTSP/1.0\r\n\
         CSeq: 3\r\n\
         Session: {session_id}\r\n\
         \r\n"
    );
    let resp = rtsp_roundtrip(&mut stream, &record).await;
    assert!(resp.contains("RTSP/1.0 200"), "RECORD failed: {resp}");

    // Push interleaved RTP frames.
    // First: STAP-A with SPS + PPS.
    let sps = vec![0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
    let pps = vec![0x68, 0xEE, 0x3C, 0x80];
    let mut stap_payload = vec![24u8]; // STAP-A
    stap_payload.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    stap_payload.extend_from_slice(&sps);
    stap_payload.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    stap_payload.extend_from_slice(&pps);
    let rtp1 = make_rtp_packet(96, 1, 90000, false, &stap_payload);
    stream.write_all(&interleave(0, &rtp1)).await.unwrap();

    // Second: IDR keyframe.
    let idr = vec![0x65, 0x88, 0x84, 0x00, 0xDE, 0xAD, 0xBE, 0xEF];
    let rtp2 = make_rtp_packet(96, 2, 93000, true, &idr);
    stream.write_all(&interleave(0, &rtp2)).await.unwrap();

    // Third: another IDR at a later timestamp (second segment).
    let rtp3 = make_rtp_packet(96, 3, 186000, true, &idr);
    stream.write_all(&interleave(0, &rtp3)).await.unwrap();

    // Wait for processing.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Fetch HLS playlist.
    let resp = http_get(hls_addr, "/hls/publish/rtsp_test/playlist.m3u8").await;
    assert_eq!(resp.status, 200, "HLS playlist must be served for RTSP broadcast");
    let body = std::str::from_utf8(&resp.body).expect("utf-8");
    eprintln!("--- rtsp hls playlist ---\n{body}\n--- end ---");
    assert!(body.starts_with("#EXTM3U"), "playlist must start with #EXTM3U");
    assert!(
        body.contains("#EXT-X-PART:") || body.contains("#EXTINF:"),
        "playlist must contain at least one partial or segment:\n{body}"
    );

    server.shutdown().await.expect("shutdown");
}

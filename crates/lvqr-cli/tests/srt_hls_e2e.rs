//! SRT ingest -> LL-HLS HTTP egress end-to-end integration tests.
//!
//! Pushes synthetic MPEG-TS streams (H.264 and HEVC) over SRT into
//! `lvqr serve`, then verifies the HLS playlist contains segments.
//! Proves the full path: SRT socket -> TsDemuxer -> PES -> Fragment
//! -> shared FragmentBroadcasterRegistry -> BroadcasterHlsBridge
//! drain task -> MultiHlsServer -> axum HTTP.

use bytes::Bytes;
use futures_util::SinkExt;
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);
const SYNC_BYTE: u8 = 0x47;

fn make_ts_packet(pid: u16, pusi: bool, payload: &[u8]) -> Vec<u8> {
    let mut pkt = vec![0xFFu8; 188];
    pkt[0] = SYNC_BYTE;
    pkt[1] = if pusi { 0x40 } else { 0x00 } | ((pid >> 8) as u8 & 0x1F);
    pkt[2] = pid as u8;
    pkt[3] = 0x10;
    let copy_len = payload.len().min(184);
    pkt[4..4 + copy_len].copy_from_slice(&payload[..copy_len]);
    pkt
}

fn minimal_pat(pmt_pid: u16) -> Vec<u8> {
    let mut data = vec![0x00, 0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01];
    data.push(0xE0 | ((pmt_pid >> 8) as u8 & 0x1F));
    data.push(pmt_pid as u8);
    data.extend_from_slice(&[0x00; 4]);
    data
}

fn minimal_pmt(video_pid: u16) -> Vec<u8> {
    vec![
        0x00,
        0x02,
        0xB0,
        0x12,
        0x00,
        0x01,
        0xC1,
        0x00,
        0x00,
        0xE1,
        0x00,
        0xF0,
        0x00,
        0x1B,
        0xE0 | ((video_pid >> 8) as u8 & 0x1F),
        video_pid as u8,
        0xF0,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
    ]
}

fn h264_pes(pts_90k: u64, keyframe: bool) -> Vec<u8> {
    let sps = [0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
    let pps = [0x68, 0xEE, 0x3C, 0x80];
    let nal_type: u8 = if keyframe { 0x65 } else { 0x41 };
    let slice = [nal_type, 0x88, 0x84, 0x00, 0xDE, 0xAD];

    let mut es = Vec::new();
    if keyframe {
        es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        es.extend_from_slice(&sps);
        es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        es.extend_from_slice(&pps);
    }
    es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    es.extend_from_slice(&slice);

    let pes_payload_len = (3 + 5 + es.len()) as u16;
    let mut data = vec![
        0x00,
        0x00,
        0x01,
        0xE0,
        (pes_payload_len >> 8) as u8,
        pes_payload_len as u8,
        0x80,
        0x80,
        0x05,
    ];
    let pts = pts_90k & 0x1_FFFF_FFFF;
    data.push(0x21 | ((pts >> 29) as u8 & 0x0E));
    data.push((pts >> 22) as u8);
    data.push(0x01 | ((pts >> 14) as u8 & 0xFE));
    data.push((pts >> 7) as u8);
    data.push(0x01 | ((pts << 1) as u8 & 0xFE));
    data.extend_from_slice(&es);
    data
}

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

#[tokio::test]
async fn srt_push_reaches_hls_playlist() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default().with_srt())
        .await
        .expect("start TestServer with SRT");
    let srt_addr = server.srt_addr();
    let hls_addr = server.hls_addr();

    // Connect as an SRT caller to the server's listener.
    let mut srt: srt_tokio::SrtSocket = srt_tokio::SrtSocket::builder()
        .call(srt_addr, None)
        .await
        .expect("SRT connect");

    // Build a minimal MPEG-TS stream: PAT + PMT + 2 keyframes.
    let video_pid = 0x100u16;
    let pmt_pid = 0x1000u16;
    let mut ts_data = Vec::new();
    ts_data.extend_from_slice(&make_ts_packet(0, true, &minimal_pat(pmt_pid)));
    ts_data.extend_from_slice(&make_ts_packet(pmt_pid, true, &minimal_pmt(video_pid)));
    ts_data.extend_from_slice(&make_ts_packet(video_pid, true, &h264_pes(0, true)));
    ts_data.extend_from_slice(&make_ts_packet(video_pid, true, &h264_pes(180_000, true)));

    // Push the TS data over SRT.
    let now = Instant::now();
    srt.send((now, Bytes::from(ts_data))).await.unwrap();
    srt.close().await.unwrap();

    // Wait for the fragment observer to process.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Fetch the HLS playlist for the SRT broadcast.
    let resp = http_get(hls_addr, "/hls/srt/default/playlist.m3u8").await;
    assert_eq!(resp.status, 200, "HLS playlist must be served for SRT broadcast");
    let body = std::str::from_utf8(&resp.body).expect("utf-8");
    eprintln!("--- srt hls playlist ---\n{body}\n--- end ---");
    assert!(body.starts_with("#EXTM3U"), "playlist must start with #EXTM3U");
    assert!(
        body.contains("#EXT-X-PART:") || body.contains("#EXTINF:"),
        "playlist must contain at least one partial or segment:\n{body}"
    );

    server.shutdown().await.expect("shutdown");
}

fn minimal_pmt_hevc(video_pid: u16) -> Vec<u8> {
    vec![
        0x00,
        0x02,
        0xB0,
        0x12,
        0x00,
        0x01,
        0xC1,
        0x00,
        0x00,
        0xE1,
        0x00,
        0xF0,
        0x00,
        0x24, // stream_type = 0x24 (HEVC)
        0xE0 | ((video_pid >> 8) as u8 & 0x1F),
        video_pid as u8,
        0xF0,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
    ]
}

// Real x265 parameter sets (320x240, Main profile, level 2.0).
const HEVC_VPS: &[u8] = &[
    0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03,
    0x00, 0x3c, 0x95, 0x94, 0x09,
];
const HEVC_SPS: &[u8] = &[
    0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c, 0xa0,
    0x0a, 0x08, 0x0f, 0x16, 0x59, 0x59, 0x52, 0x93, 0x0b, 0xc0, 0x5a, 0x02, 0x00, 0x00, 0x03, 0x00, 0x02, 0x00, 0x00,
    0x03, 0x00, 0x3c, 0x10,
];
const HEVC_PPS: &[u8] = &[0x44, 0x01, 0xc0, 0x73, 0xc1, 0x89];

fn hevc_pes(pts_90k: u64, keyframe: bool) -> Vec<u8> {
    // IDR_W_RADL: nal_unit_type = 19, encoded as (19 << 1) | 0 = 0x26
    let idr_slice = [0x26, 0x01, 0xAF, 0x09, 0x40, 0xDE, 0xAD];
    // TRAIL_R: nal_unit_type = 1, encoded as (1 << 1) | 0 = 0x02
    let trail_slice = [0x02, 0x01, 0xAF, 0x09, 0x40, 0xBE, 0xEF];

    let mut es = Vec::new();
    if keyframe {
        es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        es.extend_from_slice(HEVC_VPS);
        es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        es.extend_from_slice(HEVC_SPS);
        es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        es.extend_from_slice(HEVC_PPS);
        es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        es.extend_from_slice(&idr_slice);
    } else {
        es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        es.extend_from_slice(&trail_slice);
    }

    let pes_payload_len = (3 + 5 + es.len()) as u16;
    let mut data = vec![
        0x00,
        0x00,
        0x01,
        0xE0,
        (pes_payload_len >> 8) as u8,
        pes_payload_len as u8,
        0x80,
        0x80,
        0x05,
    ];
    let pts = pts_90k & 0x1_FFFF_FFFF;
    data.push(0x21 | ((pts >> 29) as u8 & 0x0E));
    data.push((pts >> 22) as u8);
    data.push(0x01 | ((pts >> 14) as u8 & 0xFE));
    data.push((pts >> 7) as u8);
    data.push(0x01 | ((pts << 1) as u8 & 0xFE));
    data.extend_from_slice(&es);
    data
}

#[tokio::test]
async fn srt_hevc_push_reaches_hls_playlist() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default().with_srt())
        .await
        .expect("start TestServer with SRT");
    let srt_addr = server.srt_addr();
    let hls_addr = server.hls_addr();

    let mut srt: srt_tokio::SrtSocket = srt_tokio::SrtSocket::builder()
        .call(srt_addr, None)
        .await
        .expect("SRT connect");

    let video_pid = 0x100u16;
    let pmt_pid = 0x1000u16;
    let mut ts_data = Vec::new();
    ts_data.extend_from_slice(&make_ts_packet(0, true, &minimal_pat(pmt_pid)));
    ts_data.extend_from_slice(&make_ts_packet(pmt_pid, true, &minimal_pmt_hevc(video_pid)));
    ts_data.extend_from_slice(&make_ts_packet(video_pid, true, &hevc_pes(0, true)));
    ts_data.extend_from_slice(&make_ts_packet(video_pid, true, &hevc_pes(180_000, true)));

    let now = Instant::now();
    srt.send((now, Bytes::from(ts_data))).await.unwrap();
    srt.close().await.unwrap();

    tokio::time::sleep(Duration::from_millis(1000)).await;

    let resp = http_get(hls_addr, "/hls/srt/default/playlist.m3u8").await;
    assert_eq!(resp.status, 200, "HLS playlist must be served for HEVC SRT broadcast");
    let body = std::str::from_utf8(&resp.body).expect("utf-8");
    eprintln!("--- srt hevc hls playlist ---\n{body}\n--- end ---");
    assert!(body.starts_with("#EXTM3U"), "playlist must start with #EXTM3U");
    assert!(
        body.contains("#EXT-X-PART:") || body.contains("#EXTINF:"),
        "playlist must contain at least one partial or segment:\n{body}"
    );

    server.shutdown().await.expect("shutdown");
}

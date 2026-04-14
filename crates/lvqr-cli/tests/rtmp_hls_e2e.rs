//! RTMP ingest -> LL-HLS HTTP egress end-to-end integration test.
//!
//! Sister test to `rtmp_ws_e2e.rs`. Where the WS test verifies the
//! RTMP -> MoQ -> WebSocket fMP4 path, this one verifies the
//! Tier 2.3 RTMP -> Fragment -> CmafChunk -> HlsServer -> axum
//! HTTP path that session 11 wires into `lvqr-cli serve`. There are
//! no mocks: a real `rml_rtmp` client publishes, a real
//! `lvqr_cli::start`-driven server forwards fragments through the
//! HLS bridge, and a real raw-TCP HTTP/1.1 client reads
//! `/playlist.m3u8` plus a referenced media URI off the LL-HLS
//! surface.
//!
//! The test pushes exactly two keyframes spaced 2.1 s apart so the
//! segmenter's default `VIDEO_90KHZ_DEFAULT` policy (2 s segment
//! duration at 90 kHz) closes one full segment after the second
//! keyframe. The closed segment shows up in
//! `manifest.segments` and the second keyframe lives in
//! `preliminary_parts`. From the wire's point of view the playlist
//! references at least one `#EXT-X-PART:` URI; the test fetches one
//! and asserts the body is non-empty.

use bytes::Bytes;
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

// =====================================================================
// FLV tag helpers (mirror crates/lvqr-cli/tests/rtmp_ws_e2e.rs)
// =====================================================================

fn flv_video_seq_header() -> Bytes {
    let sps = [0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
    let pps = [0x68, 0xEE, 0x3C, 0x80];
    let mut tag = vec![0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x1F, 0xFF, 0xE1];
    tag.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&sps);
    tag.push(0x01);
    tag.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&pps);
    Bytes::from(tag)
}

fn flv_video_nalu(keyframe: bool, cts: i32, nalu_data: &[u8]) -> Bytes {
    let frame_type = if keyframe { 0x17 } else { 0x27 };
    let mut tag = vec![frame_type, 0x01, (cts >> 16) as u8, (cts >> 8) as u8, cts as u8];
    tag.extend_from_slice(nalu_data);
    Bytes::from(tag)
}

// =====================================================================
// RTMP publish helpers (copied from rtmp_ws_e2e.rs verbatim)
// =====================================================================

async fn rtmp_client_handshake(stream: &mut TcpStream) -> Vec<u8> {
    let mut handshake = Handshake::new(PeerType::Client);
    let p0_and_p1 = handshake.generate_outbound_p0_and_p1().unwrap();
    stream.write_all(&p0_and_p1).await.unwrap();

    let mut buf = vec![0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0, "server closed during handshake");
        match handshake.process_bytes(&buf[..n]).unwrap() {
            HandshakeProcessResult::InProgress { response_bytes } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await.unwrap();
                }
            }
            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await.unwrap();
                }
                return remaining_bytes;
            }
        }
    }
}

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
                ClientSessionResult::RaisedEvent(ref event) => {
                    if predicate(event) {
                        return;
                    }
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

// =====================================================================
// Minimal raw-TCP HTTP/1.1 GET client.
//
// We deliberately avoid pulling in `reqwest` or `hyper-util` as a
// dev-dep just for two GETs. The HLS server speaks plain HTTP/1.1
// `Connection: close` perfectly well, so a 30-line client is enough.
// =====================================================================

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("http GET connect timed out")
        .expect("http GET connect failed");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(TIMEOUT, stream.read_to_end(&mut buf))
        .await
        .expect("http GET read timed out")
        .expect("http GET read failed");
    parse_http_response(&buf)
}

fn parse_http_response(bytes: &[u8]) -> HttpResponse {
    // Locate the end-of-headers marker (CRLF CRLF).
    let split = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http response missing header terminator");
    let header_block = &bytes[..split];
    let body_block = &bytes[split + 4..];

    let header_text = std::str::from_utf8(header_block).expect("http headers are not utf-8");
    let mut header_lines = header_text.lines();
    let status_line = header_lines.next().expect("http response missing status line");
    let mut status_parts = status_line.splitn(3, ' ');
    let _http_version = status_parts.next();
    let status: u16 = status_parts
        .next()
        .expect("status line missing code")
        .parse()
        .expect("status code is not numeric");

    HttpResponse {
        status,
        body: body_block.to_vec(),
    }
}

// =====================================================================
// Helpers for parsing the LL-HLS playlist body.
// =====================================================================

/// Pull every URI named in an `#EXT-X-PART:` line out of a rendered
/// playlist body. The renderer in `lvqr-hls` emits each part as
/// `#EXT-X-PART:DURATION=...,URI="<uri>"[,INDEPENDENT=YES]`.
fn extract_part_uris(playlist: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in playlist.lines() {
        let Some(rest) = line.strip_prefix("#EXT-X-PART:") else {
            continue;
        };
        let Some(uri_start) = rest.find("URI=\"") else {
            continue;
        };
        let after = &rest[uri_start + 5..];
        let Some(end) = after.find('"') else {
            continue;
        };
        out.push(after[..end].to_string());
    }
    out
}

// =====================================================================
// The test
// =====================================================================

/// Real end-to-end: RTMP publish -> RtmpMoqBridge -> HlsFragmentBridge
/// -> HlsServer -> axum HTTP. Verifies that a real HTTP client reading
/// `/playlist.m3u8` off the bound HLS port sees at least one
/// `#EXT-X-PART:` URI and that fetching that URI returns 200 with a
/// non-empty body.
#[tokio::test]
async fn rtmp_publish_reaches_hls_subscriber_as_playlist_and_segment() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    // --- Spin up the full LVQR stack with HLS enabled (default). ---
    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let hls_addr = server.hls_addr();

    // --- Publish RTMP. ---
    let (mut rtmp_stream, mut session) = connect_and_publish(rtmp_addr, "live", "test").await;

    // Sequence header.
    let seq = flv_video_seq_header();
    let result = session.publish_video_data(seq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &result).await;

    // First keyframe at t=0.
    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf0 = flv_video_nalu(true, 0, &nalu);
    let result = session.publish_video_data(kf0, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &result).await;

    // Second keyframe at t=2100 ms. dts at 90 kHz = 189_000, which is
    // past the default 180_000-tick segment boundary, so this push
    // closes the first segment in the LL-HLS state machine.
    let kf1 = flv_video_nalu(true, 0, &nalu);
    let result = session
        .publish_video_data(kf1, RtmpTimestamp::new(2100), false)
        .unwrap();
    send_result(&mut rtmp_stream, &result).await;

    // The on_fragment path spawns one tokio task per push. Give them a
    // tick to land on the HlsServer state before we read.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // --- Fetch the playlist. ---
    let playlist_resp = http_get(hls_addr, "/playlist.m3u8").await;
    assert_eq!(playlist_resp.status, 200, "playlist GET status");
    let playlist_body = std::str::from_utf8(&playlist_resp.body).expect("playlist body should be utf-8");
    eprintln!("--- playlist body ---\n{playlist_body}\n--- end ---");
    assert!(
        playlist_body.starts_with("#EXTM3U"),
        "playlist missing #EXTM3U header: {playlist_body}"
    );
    assert!(
        playlist_body.contains("#EXT-X-VERSION:9"),
        "playlist missing LL-HLS version tag"
    );
    assert!(
        playlist_body.contains("#EXT-X-MAP:URI=\"init.mp4\""),
        "playlist missing #EXT-X-MAP for init segment"
    );

    // The two-keyframe sequence above must have produced at least one
    // closed segment plus one open partial. Both are referenced
    // through `#EXT-X-PART:` lines in the rendered body.
    let part_uris = extract_part_uris(playlist_body);
    assert!(
        !part_uris.is_empty(),
        "playlist references no #EXT-X-PART URIs:\n{playlist_body}"
    );

    // --- Fetch one of the referenced parts and assert it has bytes. ---
    let first_part_uri = &part_uris[0];
    let part_path = format!("/{first_part_uri}");
    let part_resp = http_get(hls_addr, &part_path).await;
    assert_eq!(part_resp.status, 200, "part GET status for {part_path}");
    assert!(!part_resp.body.is_empty(), "part body for {part_path} was empty");
    // The bridge feeds wire-ready `moof + mdat` bytes into the HLS
    // server, so the first 8 bytes after the box length should spell
    // `moof`.
    assert!(
        part_resp.body.len() >= 8,
        "part body is shorter than a single box header: {} bytes",
        part_resp.body.len()
    );
    assert_eq!(
        &part_resp.body[4..8],
        b"moof",
        "expected the first part to start with a `moof` box"
    );

    // --- Fetch the init segment too: it must be non-empty and start
    // with an `ftyp` box header. ---
    let init_resp = http_get(hls_addr, "/init.mp4").await;
    assert_eq!(init_resp.status, 200, "init GET status");
    assert!(init_resp.body.len() >= 8, "init body too short");
    assert_eq!(&init_resp.body[4..8], b"ftyp", "init segment did not start with `ftyp`");

    // --- Clean shutdown ---
    drop(rtmp_stream);
    server.shutdown().await.expect("shutdown");
}

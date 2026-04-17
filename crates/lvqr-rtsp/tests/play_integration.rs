//! End-to-end RTSP PLAY integration test.
//!
//! Spins up a real `RtspServer` backed by a `FragmentBroadcasterRegistry`,
//! runs an RTSP client through the OPTIONS -> DESCRIBE -> SETUP -> PLAY
//! handshake over real TCP, emits fMP4 fragments through the broadcaster,
//! and verifies the interleaved RTP frames the server writes back
//! round-trip through the depacketizer to recover the original NAL units.
//!
//! This is the PLAY-side counterpart to `integration_server.rs` (which
//! exercises RECORD / ingest). Keeping it in-crate avoids pulling the
//! full `lvqr-cli` TestServer stack in just to verify the PLAY path.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use lvqr_cmaf::{RawSample, VideoInitParams, build_moof_mdat, write_avc_init_segment};
use lvqr_core::EventBus;
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta};
use lvqr_rtsp::RtspServer;
use lvqr_rtsp::rtp::{H264Depacketizer, parse_interleaved_frame, parse_rtp_header};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

const TIMEOUT: Duration = Duration::from_secs(5);

/// Start a bare RtspServer on an ephemeral TCP port with the given
/// registry. Returns the bound address + a cancellation token for
/// clean teardown.
async fn start_rtsp_server(registry: FragmentBroadcasterRegistry) -> (SocketAddr, CancellationToken) {
    let shutdown = CancellationToken::new();
    let events = EventBus::with_capacity(16);
    let mut server = RtspServer::with_registry("127.0.0.1:0".parse().unwrap(), registry);
    let addr = server.bind().await.expect("bind");
    let ev = events.clone();
    let cancel = shutdown.clone();
    tokio::spawn(async move {
        server.run(ev, cancel).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, shutdown)
}

/// Write an RTSP request and read until the header-terminating
/// blank line. Returns the full response headers as a String; the
/// body (if any) is left on the socket for interleaved-frame reads
/// that may follow PLAY responses.
async fn rtsp_request_headers(stream: &mut TcpStream, req: &str, buf: &mut Vec<u8>) -> String {
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut scan_from = 0;
    let headers_end = loop {
        if let Some(pos) = find_crlf_crlf(buf, scan_from) {
            break pos;
        }
        let scratch_start = buf.len();
        buf.resize(scratch_start + 4096, 0);
        let n = tokio::time::timeout(TIMEOUT, stream.read(&mut buf[scratch_start..]))
            .await
            .expect("read timed out")
            .expect("read failed");
        buf.truncate(scratch_start + n);
        assert!(n > 0, "socket closed before headers terminated");
        scan_from = scratch_start.saturating_sub(3);
    };
    let headers = buf[..headers_end].to_vec();
    // Consume the headers (including the terminating CRLF CRLF) from
    // the buffer so later interleaved-frame reads start at the right
    // offset.
    buf.drain(..headers_end + 4);
    String::from_utf8_lossy(&headers).into_owned()
}

fn find_crlf_crlf(haystack: &[u8], from: usize) -> Option<usize> {
    haystack
        .windows(4)
        .skip(from)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| from + p)
}

/// Read one RTSP interleaved frame out of the socket, appending
/// further socket reads into `buf` as needed.
async fn read_interleaved_frame(stream: &mut TcpStream, buf: &mut Vec<u8>) -> (u8, Vec<u8>) {
    loop {
        if !buf.is_empty() && buf[0] != 0x24 {
            // Stray byte before interleaved magic. Not expected on
            // this test path; fail loudly so we don't silently skip
            // real frames.
            panic!("expected interleaved frame magic 0x24, got 0x{:02X}", buf[0]);
        }
        if let Some((frame, consumed)) = parse_interleaved_frame(buf) {
            buf.drain(..consumed);
            return (frame.channel, frame.payload);
        }
        // Need more bytes.
        let scratch_start = buf.len();
        buf.resize(scratch_start + 4096, 0);
        let n = tokio::time::timeout(TIMEOUT, stream.read(&mut buf[scratch_start..]))
            .await
            .expect("interleaved read timed out")
            .expect("interleaved read failed");
        buf.truncate(scratch_start + n);
        assert!(n > 0, "socket closed before interleaved frame completed");
    }
}

/// Build a real AVC init segment + pre-populate a broadcaster with
/// it, then return the broadcaster for fragment emission + the SPS
/// + PPS bytes the test will look for in the re-injection preamble.
fn make_avc_broadcaster(registry: &FragmentBroadcasterRegistry, broadcast: &str) -> (Bytes, Bytes) {
    let sps: Vec<u8> = vec![0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00];
    let pps: Vec<u8> = vec![0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];

    let mut init = BytesMut::new();
    write_avc_init_segment(
        &mut init,
        &VideoInitParams {
            sps: sps.clone(),
            pps: pps.clone(),
            width: 1280,
            height: 720,
            timescale: 90_000,
        },
    )
    .expect("write init");

    let bc = registry.get_or_create(broadcast, "0.mp4", FragmentMeta::new("avc1", 90_000));
    bc.set_init_segment(init.freeze());
    (Bytes::from(sps), Bytes::from(pps))
}

fn avcc_wrap(nal: &[u8]) -> Vec<u8> {
    let mut v = (nal.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(nal);
    v
}

/// Real end-to-end PLAY: DESCRIBE pulls the SDP from the broadcaster
/// meta, SETUP accepts interleaved=0-1, PLAY spawns the drain, and
/// the drain re-injects SPS + PPS followed by the IDR NAL off the
/// emitted fragment. Every frame round-trips through the same
/// H264Depacketizer the ingest path uses.
#[tokio::test]
async fn rtsp_play_handshake_delivers_reinjected_params_and_idr() {
    let registry = FragmentBroadcasterRegistry::new();
    let (expected_sps, expected_pps) = make_avc_broadcaster(&registry, "live/playtest");
    let (addr, shutdown) = start_rtsp_server(registry.clone()).await;

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let base_uri = format!("rtsp://{addr}/live/playtest");
    let mut pending = Vec::<u8>::new();

    // DESCRIBE must include a real H.264 m= block derived from the
    // broadcaster init segment.
    let describe = format!("DESCRIBE {base_uri} RTSP/1.0\r\nCSeq: 1\r\n\r\n");
    let describe_resp = rtsp_request_headers(&mut stream, &describe, &mut pending).await;
    assert!(describe_resp.contains("RTSP/1.0 200"), "DESCRIBE: {describe_resp}");
    assert!(describe_resp.contains("application/sdp"));
    // Content-Length says SDP body follows; drain it from `pending`.
    let content_length = parse_content_length(&describe_resp).expect("Content-Length header");
    let sdp_body = drain_body(&mut stream, &mut pending, content_length).await;
    let sdp = String::from_utf8(sdp_body).expect("SDP utf8");
    assert!(sdp.contains("m=video 0 RTP/AVP 96"), "SDP missing video m=: {sdp}");
    assert!(sdp.contains("a=rtpmap:96 H264/90000"));
    assert!(sdp.contains("sprop-parameter-sets="));

    // SETUP interleaved=0-1 so RTP arrives on channel 0.
    let setup = format!(
        "SETUP {base_uri}/track1 RTSP/1.0\r\nCSeq: 2\r\nTransport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\r\n"
    );
    let setup_resp = rtsp_request_headers(&mut stream, &setup, &mut pending).await;
    assert!(setup_resp.contains("RTSP/1.0 200"), "SETUP: {setup_resp}");
    assert!(setup_resp.contains("interleaved=0-1"));
    let session_id = extract_session_header(&setup_resp).expect("Session header on SETUP");

    // PLAY starts the drain.
    let play_req = format!("PLAY {base_uri} RTSP/1.0\r\nCSeq: 3\r\nSession: {session_id}\r\n\r\n");
    let play_resp = rtsp_request_headers(&mut stream, &play_req, &mut pending).await;
    assert!(play_resp.contains("RTSP/1.0 200"), "PLAY: {play_resp}");

    // The drain re-injects SPS + PPS before any fragments. Read two
    // interleaved frames and run them through the depacketizer.
    let (ch, sps_rtp) = read_interleaved_frame(&mut stream, &mut pending).await;
    assert_eq!(ch, 0, "RTP on the SETUP-negotiated channel");
    let sps_nalus = depack_one(&sps_rtp).expect("SPS packet decodes");
    assert_eq!(sps_nalus, vec![expected_sps.to_vec()], "first RTP packet is SPS");

    let (_ch, pps_rtp) = read_interleaved_frame(&mut stream, &mut pending).await;
    let pps_nalus = depack_one(&pps_rtp).expect("PPS packet decodes");
    assert_eq!(pps_nalus, vec![expected_pps.to_vec()], "second RTP packet is PPS");

    // Emit one IDR fragment through the broadcaster.
    let idr_nal = vec![0x65, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
    let sample = RawSample {
        track_id: 1,
        dts: 3000,
        cts_offset: 0,
        duration: 3000,
        payload: Bytes::from(avcc_wrap(&idr_nal)),
        keyframe: true,
    };
    let fragment_payload = build_moof_mdat(1, 1, 3000, std::slice::from_ref(&sample));
    let bc = registry
        .get("live/playtest", "0.mp4")
        .expect("broadcaster still present");
    bc.emit(Fragment::new(
        "0.mp4",
        1,
        0,
        0,
        3000,
        3000,
        3000,
        FragmentFlags::KEYFRAME,
        fragment_payload,
    ));

    let (_ch, idr_rtp) = read_interleaved_frame(&mut stream, &mut pending).await;
    let idr_result = H264Depacketizer::new()
        .depacketize(
            payload_bytes(&idr_rtp),
            &parse_rtp_header(&idr_rtp).expect("IDR header"),
        )
        .expect("IDR depacks");
    assert!(idr_result.keyframe);
    assert_eq!(idr_result.nalus, vec![idr_nal]);

    shutdown.cancel();
}

fn depack_one(rtp: &[u8]) -> Option<Vec<Vec<u8>>> {
    let hdr = parse_rtp_header(rtp)?;
    let mut depack = H264Depacketizer::new();
    let result = depack.depacketize(payload_bytes(rtp), &hdr)?;
    Some(result.nalus)
}

fn payload_bytes(rtp: &[u8]) -> &[u8] {
    let hdr = parse_rtp_header(rtp).expect("rtp header");
    &rtp[hdr.header_len..]
}

fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        if let Some(v) = line.strip_prefix("Content-Length:") {
            return v.trim().parse().ok();
        }
    }
    None
}

async fn drain_body(stream: &mut TcpStream, pending: &mut Vec<u8>, n: usize) -> Vec<u8> {
    while pending.len() < n {
        let scratch_start = pending.len();
        pending.resize(scratch_start + 4096, 0);
        let got = tokio::time::timeout(TIMEOUT, stream.read(&mut pending[scratch_start..]))
            .await
            .expect("body read timed out")
            .expect("body read failed");
        pending.truncate(scratch_start + got);
        assert!(got > 0, "socket closed before body complete");
    }
    let out = pending[..n].to_vec();
    pending.drain(..n);
    out
}

fn extract_session_header(headers: &str) -> Option<String> {
    for line in headers.lines() {
        if let Some(v) = line.strip_prefix("Session:") {
            return Some(v.trim().split(';').next()?.trim().to_string());
        }
    }
    None
}

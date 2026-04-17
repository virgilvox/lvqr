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
use lvqr_cmaf::{
    AudioInitParams, HevcInitParams, OpusInitParams, RawSample, VideoInitParams, build_moof_mdat,
    write_aac_init_segment, write_avc_init_segment, write_hevc_init_segment, write_opus_init_segment,
};
use lvqr_core::EventBus;
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta};
use lvqr_rtsp::RtspServer;
use lvqr_rtsp::rtp::{
    AacDepacketizer, H264Depacketizer, HevcDepacketizer, OpusDepacketizer, parse_interleaved_frame, parse_rtp_header,
};
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

// ---------- HEVC PLAY end-to-end ----------

/// Real HEVC NAL units captured from x265. Same bytes the lvqr-cmaf
/// init tests use so the writer/extractor/depacketizer triplet
/// round-trips over the network path.
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

fn hevc_sps_info() -> lvqr_codec::hevc::HevcSps {
    lvqr_codec::hevc::HevcSps {
        general_profile_space: 0,
        general_tier_flag: false,
        general_profile_idc: 1,
        general_profile_compatibility_flags: 0x60000000,
        general_level_idc: 60,
        chroma_format_idc: 1,
        pic_width_in_luma_samples: 320,
        pic_height_in_luma_samples: 240,
    }
}

fn make_hevc_broadcaster(registry: &FragmentBroadcasterRegistry, broadcast: &str) {
    let mut init = BytesMut::new();
    write_hevc_init_segment(
        &mut init,
        &HevcInitParams {
            vps: HEVC_VPS.to_vec(),
            sps: HEVC_SPS.to_vec(),
            pps: HEVC_PPS.to_vec(),
            sps_info: hevc_sps_info(),
            timescale: 90_000,
        },
    )
    .expect("write hevc init");
    let bc = registry.get_or_create(broadcast, "0.mp4", FragmentMeta::new("hev1", 90_000));
    bc.set_init_segment(init.freeze());
}

#[tokio::test]
async fn rtsp_play_hevc_handshake_delivers_vps_sps_pps_and_idr() {
    let registry = FragmentBroadcasterRegistry::new();
    make_hevc_broadcaster(&registry, "live/hevctest");
    let (addr, shutdown) = start_rtsp_server(registry.clone()).await;

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let base_uri = format!("rtsp://{addr}/live/hevctest");
    let mut pending = Vec::<u8>::new();

    let describe = format!("DESCRIBE {base_uri} RTSP/1.0\r\nCSeq: 1\r\n\r\n");
    let describe_resp = rtsp_request_headers(&mut stream, &describe, &mut pending).await;
    assert!(describe_resp.contains("RTSP/1.0 200"));
    let content_length = parse_content_length(&describe_resp).expect("Content-Length");
    let sdp = String::from_utf8(drain_body(&mut stream, &mut pending, content_length).await).expect("utf8");
    assert!(sdp.contains("a=rtpmap:96 H265/90000"), "HEVC SDP: {sdp}");
    assert!(sdp.contains("sprop-vps="));
    assert!(sdp.contains("sprop-sps="));
    assert!(sdp.contains("sprop-pps="));
    assert!(sdp.contains("profile-id=1"));

    let setup = format!(
        "SETUP {base_uri}/track1 RTSP/1.0\r\nCSeq: 2\r\nTransport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\r\n"
    );
    let setup_resp = rtsp_request_headers(&mut stream, &setup, &mut pending).await;
    let session_id = extract_session_header(&setup_resp).expect("Session");
    let play_req = format!("PLAY {base_uri} RTSP/1.0\r\nCSeq: 3\r\nSession: {session_id}\r\n\r\n");
    let play_resp = rtsp_request_headers(&mut stream, &play_req, &mut pending).await;
    assert!(play_resp.contains("RTSP/1.0 200"));

    // Three preamble packets (VPS, SPS, PPS) followed by IDR. Each
    // fits in the default MTU so they come through as single-NAL
    // packets; depacketize into Vec<u8> and compare byte-for-byte
    // against the original NALs.
    let mut depack = HevcDepacketizer::new();
    let mut recovered: Vec<Vec<u8>> = Vec::new();
    for _ in 0..3 {
        let (_ch, rtp) = read_interleaved_frame(&mut stream, &mut pending).await;
        let hdr = parse_rtp_header(&rtp).expect("rtp header");
        let result = depack
            .depacketize(&rtp[hdr.header_len..], &hdr)
            .expect("hevc preamble depacks");
        recovered.push(result.nalus.into_iter().next().expect("one nal"));
    }
    assert_eq!(recovered[0], HEVC_VPS);
    assert_eq!(recovered[1], HEVC_SPS);
    assert_eq!(recovered[2], HEVC_PPS);

    // Emit one HEVC IDR fragment. NAL type 19 = IDR_W_RADL, the
    // HEVC depacketizer treats that as a keyframe.
    let mut idr_nal = vec![0x26, 0x01]; // nal_header for type 19
    idr_nal.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
    let sample = RawSample {
        track_id: 1,
        dts: 3000,
        cts_offset: 0,
        duration: 3000,
        payload: Bytes::from({
            let mut v = (idr_nal.len() as u32).to_be_bytes().to_vec();
            v.extend_from_slice(&idr_nal);
            v
        }),
        keyframe: true,
    };
    let fragment_payload = build_moof_mdat(1, 1, 3000, std::slice::from_ref(&sample));
    registry
        .get("live/hevctest", "0.mp4")
        .expect("hevc broadcaster")
        .emit(Fragment::new(
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
    let hdr = parse_rtp_header(&idr_rtp).expect("idr header");
    let result = depack
        .depacketize(&idr_rtp[hdr.header_len..], &hdr)
        .expect("idr depacks");
    assert!(result.keyframe, "IDR flagged on depack");
    assert_eq!(result.nalus.into_iter().next().unwrap(), idr_nal);

    shutdown.cancel();
}

// ---------- AAC PLAY end-to-end ----------

fn make_aac_broadcaster(registry: &FragmentBroadcasterRegistry, broadcast: &str) {
    let mut init = BytesMut::new();
    write_aac_init_segment(
        &mut init,
        &AudioInitParams {
            asc: vec![0x12, 0x10],
            timescale: 44_100,
        },
    )
    .expect("write aac init");
    let bc = registry.get_or_create(broadcast, "1.mp4", FragmentMeta::new("mp4a.40.2", 44_100));
    bc.set_init_segment(init.freeze());
}

#[tokio::test]
async fn rtsp_play_aac_audio_track_delivers_access_unit() {
    let registry = FragmentBroadcasterRegistry::new();
    make_aac_broadcaster(&registry, "live/aactest");
    let (addr, shutdown) = start_rtsp_server(registry.clone()).await;

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let base_uri = format!("rtsp://{addr}/live/aactest");
    let mut pending = Vec::<u8>::new();

    // DESCRIBE: the SDP carries an audio block (track2 control,
    // mpeg4-generic, config=1210) even without a video broadcaster.
    let describe = format!("DESCRIBE {base_uri} RTSP/1.0\r\nCSeq: 1\r\n\r\n");
    let describe_resp = rtsp_request_headers(&mut stream, &describe, &mut pending).await;
    let content_length = parse_content_length(&describe_resp).expect("Content-Length");
    let sdp = String::from_utf8(drain_body(&mut stream, &mut pending, content_length).await).expect("utf8");
    assert!(sdp.contains("m=audio 0 RTP/AVP 97"), "AAC SDP: {sdp}");
    assert!(sdp.contains("a=rtpmap:97 mpeg4-generic/44100/2"));
    assert!(sdp.contains("config=1210"));
    assert!(sdp.contains("a=control:track2"));

    // SETUP the audio track on interleaved 2-3 (the conventional
    // second pair after video's 0-1 even when video is absent).
    let setup = format!(
        "SETUP {base_uri}/track2 RTSP/1.0\r\nCSeq: 2\r\nTransport: RTP/AVP/TCP;unicast;interleaved=2-3\r\n\r\n"
    );
    let setup_resp = rtsp_request_headers(&mut stream, &setup, &mut pending).await;
    let session_id = extract_session_header(&setup_resp).expect("Session");
    let play_req = format!("PLAY {base_uri} RTSP/1.0\r\nCSeq: 3\r\nSession: {session_id}\r\n\r\n");
    let play_resp = rtsp_request_headers(&mut stream, &play_req, &mut pending).await;
    assert!(play_resp.contains("RTSP/1.0 200"));

    // Wait for the drain to subscribe before emitting. Parallels the
    // in-crate unit test: AAC has no pre-roll so a race-free emit
    // needs the subscriber_count() check.
    let bc = registry.get("live/aactest", "1.mp4").expect("aac broadcaster");
    for _ in 0..100 {
        if bc.subscriber_count() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(bc.subscriber_count() > 0, "aac drain subscribed");

    let au = vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
    let sample = RawSample {
        track_id: 2,
        dts: 1024,
        cts_offset: 0,
        duration: 1024,
        payload: Bytes::from(au.clone()),
        keyframe: true,
    };
    let fragment_payload = build_moof_mdat(1, 2, 1024, std::slice::from_ref(&sample));
    bc.emit(Fragment::new(
        "1.mp4",
        1,
        0,
        0,
        1024,
        1024,
        1024,
        FragmentFlags::KEYFRAME,
        fragment_payload,
    ));

    let (ch, rtp) = read_interleaved_frame(&mut stream, &mut pending).await;
    assert_eq!(ch, 2, "AAC on the SETUP-negotiated channel");
    let hdr = parse_rtp_header(&rtp).expect("rtp header");
    assert_eq!(hdr.payload_type, 97);
    assert_eq!(hdr.timestamp, 1024);

    let depack = AacDepacketizer::new();
    let result = depack.depacketize(&rtp[hdr.header_len..], &hdr).expect("aac depack");
    assert_eq!(result.frames, vec![au]);

    shutdown.cancel();
}

// ---------- RTCP Sender Report end-to-end ----------

/// Every PLAY drain spawns a Sender Report timer on top of the RTP
/// stream. Drive the H.264 handshake end-to-end, emit one IDR so the
/// drain accumulates packet+octet counters, wait past the default
/// 5 s SR interval, and assert an SR arrives on the odd RTCP
/// interleaved channel with plausible counts.
///
/// Runs under real wall-clock time rather than `start_paused`: the
/// shared `rtsp_request_headers` helper wraps each read in a
/// `tokio::time::timeout`, and under `start_paused=true` tokio's
/// auto-advance fires those timeouts as the runtime finds nothing
/// else to run while waiting on real I/O. The resulting
/// "read timed out" panic flakes ~80% of runs. Real-time makes the
/// test take ~6 s but removes the race entirely (audited in session
/// 80). A future session can thread an `sr_interval` setting
/// through `RtspServer` so tests override it to 100 ms and go back
/// to sub-second.
#[tokio::test]
async fn rtsp_play_emits_rtcp_sender_report_after_interval() {
    // Inline server setup: the shared `start_rtsp_server` helper uses
    // `tokio::time::sleep(50ms)` which we avoid here on principle;
    // pre-bind is enough on its own since the OS queues incoming
    // connections until accept() runs.
    let registry = FragmentBroadcasterRegistry::new();
    let (expected_sps, expected_pps) = make_avc_broadcaster(&registry, "live/srtest");
    let shutdown = CancellationToken::new();
    let events = EventBus::with_capacity(16);
    let mut server = RtspServer::with_registry("127.0.0.1:0".parse().unwrap(), registry.clone());
    let addr = server.bind().await.expect("bind");
    let cancel = shutdown.clone();
    tokio::spawn(async move {
        server.run(events, cancel).await.ok();
    });

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let base_uri = format!("rtsp://{addr}/live/srtest");
    let mut pending = Vec::<u8>::new();

    // DESCRIBE / SETUP / PLAY handshake identical to the other
    // integration tests. Assertions here focus on the SR so we
    // compress the SDP / SETUP checks.
    let describe = format!("DESCRIBE {base_uri} RTSP/1.0\r\nCSeq: 1\r\n\r\n");
    let describe_resp = rtsp_request_headers(&mut stream, &describe, &mut pending).await;
    let content_length = parse_content_length(&describe_resp).expect("Content-Length");
    let _sdp = drain_body(&mut stream, &mut pending, content_length).await;

    let setup = format!(
        "SETUP {base_uri}/track1 RTSP/1.0\r\nCSeq: 2\r\nTransport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n\r\n"
    );
    let setup_resp = rtsp_request_headers(&mut stream, &setup, &mut pending).await;
    let session_id = extract_session_header(&setup_resp).expect("Session");
    let play_req = format!("PLAY {base_uri} RTSP/1.0\r\nCSeq: 3\r\nSession: {session_id}\r\n\r\n");
    let play_resp = rtsp_request_headers(&mut stream, &play_req, &mut pending).await;
    assert!(play_resp.contains("RTSP/1.0 200"));

    // Drain the SPS + PPS re-injection packets so they are not
    // mistaken for the SR when we advance below.
    let (_, sps_rtp) = read_interleaved_frame(&mut stream, &mut pending).await;
    assert_eq!(
        parse_rtp_header(&sps_rtp).expect("sps header").payload_type,
        96,
        "SPS is RTP video PT"
    );
    let _ = expected_sps; // silence unused hints
    let (_, pps_rtp) = read_interleaved_frame(&mut stream, &mut pending).await;
    assert_eq!(
        parse_rtp_header(&pps_rtp).expect("pps header").payload_type,
        96,
        "PPS is RTP video PT"
    );
    let _ = expected_pps;

    // One IDR fragment -> one more RTP packet on channel 0.
    let idr_nal = vec![0x65, 0xAA, 0xBB, 0xCC];
    let sample = RawSample {
        track_id: 1,
        dts: 9000,
        cts_offset: 0,
        duration: 3000,
        payload: Bytes::from({
            let mut v = (idr_nal.len() as u32).to_be_bytes().to_vec();
            v.extend_from_slice(&idr_nal);
            v
        }),
        keyframe: true,
    };
    let fragment_payload = build_moof_mdat(1, 1, 9000, std::slice::from_ref(&sample));
    registry
        .get("live/srtest", "0.mp4")
        .expect("broadcaster")
        .emit(Fragment::new(
            "0.mp4",
            1,
            0,
            0,
            9000,
            9000,
            3000,
            FragmentFlags::KEYFRAME,
            fragment_payload,
        ));

    let (_, idr_rtp) = read_interleaved_frame(&mut stream, &mut pending).await;
    let idr_hdr = parse_rtp_header(&idr_rtp).expect("idr hdr");
    assert_eq!(idr_hdr.timestamp, 9000, "last RTP timestamp carried in SR");

    // Real-time wait past the default 5 s SR interval so the
    // ticker fires. Slow (takes ~6 s) but deterministic; see the
    // fn doc for the start_paused tradeoff.
    tokio::time::sleep(Duration::from_secs(6)).await;

    // Next interleaved frame on the socket is the SR on channel 1.
    let (ch, sr) = read_interleaved_frame(&mut stream, &mut pending).await;
    assert_eq!(ch, 1, "SR on the odd RTCP channel");
    assert_eq!(sr.len(), 28, "bare SR packet: 8-byte header + 20-byte sender info");
    // Version=2, Padding=0, RC=0.
    assert_eq!(sr[0], 0x80);
    // PT=SR=200.
    assert_eq!(sr[1], 200);
    // Length = (28 / 4) - 1 = 6 32-bit words.
    assert_eq!(u16::from_be_bytes([sr[2], sr[3]]), 6);
    // SSRC matches the video drain's DEFAULT_SSRC.
    assert_eq!(&sr[4..8], &0xDEAD_BEEFu32.to_be_bytes());
    // RTP timestamp = most recent emitted (the IDR's 9000).
    assert_eq!(
        u32::from_be_bytes([sr[16], sr[17], sr[18], sr[19]]),
        9000,
        "SR carries latest RTP timestamp",
    );
    // Packet count >= 3 (SPS + PPS + IDR).
    let packet_count = u32::from_be_bytes([sr[20], sr[21], sr[22], sr[23]]);
    assert!(
        packet_count >= 3,
        "packet count covers SPS + PPS + IDR (got {packet_count})"
    );
    // Octet count > 0 (payload bytes of the three packets).
    let octet_count = u32::from_be_bytes([sr[24], sr[25], sr[26], sr[27]]);
    assert!(octet_count > 0, "octet count positive after real emits");

    shutdown.cancel();
}

// ---------- Opus PLAY end-to-end ----------

fn make_opus_broadcaster(registry: &FragmentBroadcasterRegistry, broadcast: &str) {
    let mut init = BytesMut::new();
    write_opus_init_segment(
        &mut init,
        &OpusInitParams {
            channel_count: 2,
            pre_skip: 312,
            input_sample_rate: 48_000,
            timescale: 48_000,
        },
    )
    .expect("write opus init");
    let bc = registry.get_or_create(broadcast, "1.mp4", FragmentMeta::new("opus", 48_000));
    bc.set_init_segment(init.freeze());
}

/// Real end-to-end Opus PLAY. DESCRIBE pulls an RFC 7587 audio block
/// off the dOps-bearing init segment, SETUP negotiates interleaved
/// 2-3, PLAY spawns the drain, and a single synthetic Opus frame
/// round-trips through OpusDepacketizer.
#[tokio::test]
async fn rtsp_play_opus_audio_track_delivers_frame() {
    let registry = FragmentBroadcasterRegistry::new();
    make_opus_broadcaster(&registry, "live/opustest");
    let (addr, shutdown) = start_rtsp_server(registry.clone()).await;

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let base_uri = format!("rtsp://{addr}/live/opustest");
    let mut pending = Vec::<u8>::new();

    // DESCRIBE must carry an Opus audio block keyed at PT 98.
    let describe = format!("DESCRIBE {base_uri} RTSP/1.0\r\nCSeq: 1\r\n\r\n");
    let describe_resp = rtsp_request_headers(&mut stream, &describe, &mut pending).await;
    let content_length = parse_content_length(&describe_resp).expect("Content-Length");
    let sdp = String::from_utf8(drain_body(&mut stream, &mut pending, content_length).await).expect("utf8");
    assert!(sdp.contains("m=audio 0 RTP/AVP 98"), "Opus SDP: {sdp}");
    assert!(sdp.contains("a=rtpmap:98 opus/48000/2"));
    assert!(sdp.contains("sprop-stereo=1"));
    assert!(sdp.contains("useinbandfec=1"));
    assert!(sdp.contains("a=control:track2"));
    // Must not leak AAC-only keys into the Opus block.
    assert!(!sdp.contains("mode=AAC-hbr"));

    let setup = format!(
        "SETUP {base_uri}/track2 RTSP/1.0\r\nCSeq: 2\r\nTransport: RTP/AVP/TCP;unicast;interleaved=2-3\r\n\r\n"
    );
    let setup_resp = rtsp_request_headers(&mut stream, &setup, &mut pending).await;
    let session_id = extract_session_header(&setup_resp).expect("Session");
    let play_req = format!("PLAY {base_uri} RTSP/1.0\r\nCSeq: 3\r\nSession: {session_id}\r\n\r\n");
    let play_resp = rtsp_request_headers(&mut stream, &play_req, &mut pending).await;
    assert!(play_resp.contains("RTSP/1.0 200"));

    // Wait for the drain to subscribe: Opus has no pre-roll.
    let bc = registry.get("live/opustest", "1.mp4").expect("opus broadcaster");
    for _ in 0..100 {
        if bc.subscriber_count() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(bc.subscriber_count() > 0, "opus drain subscribed");

    // Synthetic Opus frame -- bytes are not a real decodable frame
    // (the depacketizer is byte-transparent per RFC 7587, so any
    // non-empty payload round-trips). The 48 kHz RTP timestamp
    // matches a standard 20 ms Opus frame (960 samples).
    let frame_bytes = vec![0xFC, 0xE1, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE];
    let sample = RawSample {
        track_id: 2,
        dts: 960,
        cts_offset: 0,
        duration: 960,
        payload: Bytes::from(frame_bytes.clone()),
        keyframe: true,
    };
    let fragment_payload = build_moof_mdat(1, 2, 960, std::slice::from_ref(&sample));
    bc.emit(Fragment::new(
        "1.mp4",
        1,
        0,
        0,
        960,
        960,
        960,
        FragmentFlags::KEYFRAME,
        fragment_payload,
    ));

    let (ch, rtp) = read_interleaved_frame(&mut stream, &mut pending).await;
    assert_eq!(ch, 2, "Opus on the SETUP-negotiated channel");
    let hdr = parse_rtp_header(&rtp).expect("rtp header");
    assert_eq!(hdr.payload_type, 98, "Opus PT distinct from AAC PT 97");
    assert_eq!(hdr.timestamp, 960);
    assert!(hdr.marker, "RFC 7587: marker set on every packet");

    let result = OpusDepacketizer::new()
        .depacketize(&rtp[hdr.header_len..], &hdr)
        .expect("opus depack");
    assert_eq!(result.frame, frame_bytes);

    shutdown.cancel();
}

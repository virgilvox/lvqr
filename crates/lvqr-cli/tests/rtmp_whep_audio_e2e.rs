//! RTMP ingest -> WHEP Opus-audio egress end-to-end integration test.
//!
//! Session 115 closes the last cross-ingress E2E gap from the
//! session 114 triage: OBS-to-browser audio. An `rml_rtmp`
//! publisher pushes a minimal video init plus real AAC-LC access
//! units generated in-test via
//! `audiotestsrc ! avenc_aac ! aacparse`; the session 113 WHEP
//! pipeline (AAC sequence header cached on the broadcast ->
//! per-session `AacToOpusEncoder` -> Opus RTP out) transcodes to
//! Opus on the fly; a real `str0m::Rtc` client negotiates
//! Opus-only against `POST /whep/live/test` and asserts at least
//! one `Event::MediaData` lands on the negotiated Opus Pt.
//!
//! The test is feature-gated on `transcode` so the default CI gate
//! (GStreamer-absent hosts) does not even compile the target. On a
//! host with the `transcode` feature enabled but the runtime
//! plugins missing, the test prints a skip reason and returns
//! clean rather than failing.

#![cfg(feature = "transcode")]

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bytes::Bytes;
use lvqr_test_utils::{TestServer, TestServerConfig};
use lvqr_transcode::AacToOpusEncoderFactory;
use lvqr_transcode::test_support::generate_aac_access_units;
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use str0m::change::SdpAnswer;
use str0m::media::{Direction, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, RtcConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket as TokioUdp};

const RTMP_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const OVERALL_DEADLINE: Duration = Duration::from_secs(20);
const MAX_POLL_SLEEP: Duration = Duration::from_millis(50);

// =====================================================================
// FLV wrappers (aligned with rtmp_hls_e2e.rs + aac_opus_roundtrip.rs)
// =====================================================================

/// FLV video sequence header for a minimal AVC config (SPS + PPS).
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

fn flv_video_keyframe(cts: i32, nalu_data: &[u8]) -> Bytes {
    let mut tag = vec![0x17, 0x01, (cts >> 16) as u8, (cts >> 8) as u8, cts as u8];
    tag.extend_from_slice(nalu_data);
    Bytes::from(tag)
}

/// FLV AAC sequence header. AAC-LC 48 kHz stereo:
/// object_type=2, freq_idx=3, channel_config=2 -> ASC [0x11, 0x90].
/// Same bytes the AacToOpusEncoder unit tests expect in `aac_opus_roundtrip.rs`.
fn flv_audio_seq_header_48k_stereo() -> Bytes {
    Bytes::from(vec![0xAF, 0x00, 0x11, 0x90])
}

fn flv_audio_raw(aac_access_unit: &[u8]) -> Bytes {
    let mut tag = vec![0xAF, 0x01];
    tag.extend_from_slice(aac_access_unit);
    Bytes::from(tag)
}

// =====================================================================
// RTMP publish helpers (copied from rtmp_dash_e2e.rs verbatim)
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
    let deadline = tokio::time::Instant::now() + RTMP_TIMEOUT;
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
    let mut stream = tokio::time::timeout(RTMP_TIMEOUT, TcpStream::connect(addr))
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
// HTTP helpers (mirrors crates/lvqr-cli/tests/whip_hls_e2e.rs)
// =====================================================================

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn find_header<'a>(resp: &'a HttpResponse, name: &str) -> Option<&'a str> {
    resp.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

async fn http_post_sdp(addr: SocketAddr, path: &str, body: &[u8]) -> HttpResponse {
    let mut stream = tokio::time::timeout(HTTP_TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("http connect timed out")
        .expect("http connect failed");
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n",
        len = body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(HTTP_TIMEOUT, stream.read_to_end(&mut buf))
        .await
        .expect("http read timed out")
        .expect("http read failed");
    parse_http_response(&buf)
}

fn parse_http_response(buf: &[u8]) -> HttpResponse {
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("missing header terminator");
    let header_text = std::str::from_utf8(&buf[..split]).unwrap();
    let mut lines = header_text.lines();
    let status_line = lines.next().unwrap();
    let status: u16 = status_line.split(' ').nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    HttpResponse {
        status,
        headers,
        body: buf[split + 4..].to_vec(),
    }
}

fn extract_first_host_candidate(sdp: &str) -> Option<SocketAddr> {
    for line in sdp.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("a=candidate:") else {
            continue;
        };
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        if tokens.len() < 8 || !tokens[7].eq_ignore_ascii_case("host") {
            continue;
        }
        let ip = tokens[4];
        let port = tokens[5];
        if let Ok(addr) = format!("{ip}:{port}").parse::<SocketAddr>() {
            return Some(addr);
        }
    }
    None
}

/// RTMP publisher task body: publish a minimal video init + keyframe
/// pair (so the broadcast registers on the relay) plus the AAC
/// sequence header and the pre-generated access units spaced at
/// ~21.33 ms per frame (1024 samples / 48 kHz = 21 1/3 ms).
async fn run_rtmp_publisher(rtmp_addr: SocketAddr, aac_access_units: Vec<Vec<u8>>) -> (TcpStream, ClientSession) {
    let (mut stream, mut session) = connect_and_publish(rtmp_addr, "live", "test").await;

    // Video init + one keyframe so the bridge registers the broadcast.
    let vseq = flv_video_seq_header();
    let r = session.publish_video_data(vseq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut stream, &r).await;

    // AAC sequence header BEFORE the first keyframe so the WhepServer
    // caches the AudioSpecificConfig before any WHEP subscriber POSTs
    // an offer. `handle_offer` replays the cached config onto the new
    // session so the AacToOpusEncoder has it by the time the first
    // real AAC sample arrives.
    let aseq = flv_audio_seq_header_48k_stereo();
    let r = session.publish_audio_data(aseq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut stream, &r).await;

    let idr_nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf0 = flv_video_keyframe(0, &idr_nalu);
    let r = session.publish_video_data(kf0, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut stream, &r).await;

    // Spin the AAC access units out at real-time cadence (~21.33 ms
    // per frame at 1024 samples / 48 kHz). The `u32` FLV timestamp
    // field is millisecond granular, so we push a steady 21 ms
    // cadence and let opusenc absorb the jitter. Real-time cadence
    // is load-bearing for this test: bursting all 38 samples in a
    // tight loop would finish in ~5 ms, well before the subscriber
    // side's ICE + DTLS completes (loopback-typical ~200-500 ms),
    // which would route every resulting Opus packet through the
    // pre-Connected drop branch and produce zero Event::MediaData.
    // Keeping the publisher alive for the full ~800 ms span gives
    // ICE + DTLS plenty of time to settle while samples continue to
    // arrive at the WHEP session poll loop.
    let frame_interval = Duration::from_millis(21);
    let mut next_tick = tokio::time::Instant::now();
    for (i, au) in aac_access_units.iter().enumerate() {
        let ts = (i as u32).saturating_mul(21);
        let tag = flv_audio_raw(au);
        let r = session.publish_audio_data(tag, RtmpTimestamp::new(ts), false).unwrap();
        send_result(&mut stream, &r).await;
        next_tick += frame_interval;
        tokio::time::sleep_until(next_tick).await;
    }

    (stream, session)
}

/// Real end-to-end: RTMP publisher pushes AAC audio -> WhepServer
/// fans out to an Opus-only str0m WHEP subscriber -> AacToOpusEncoder
/// emits Opus RTP -> client's poll loop raises `Event::MediaData` on
/// the negotiated Opus Pt.
#[tokio::test(flavor = "current_thread")]
async fn rtmp_aac_publish_reaches_whep_opus_subscriber() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    // Probe the AAC-to-Opus factory before booting the server. If the
    // host lacks any of `aacparse`, `avdec_aac`, `audioconvert`,
    // `audioresample`, or `opusenc`, print a skip reason and return.
    let factory = AacToOpusEncoderFactory::new();
    if !factory.is_available() {
        eprintln!(
            "skipping rtmp_whep_audio_e2e: AacToOpusEncoderFactory unavailable, missing {:?}",
            factory.missing_elements()
        );
        return;
    }

    // Generate ~1600 ms of real AAC before starting the server so a
    // generator-plugin shortfall skips cleanly before the network
    // listeners get bound. 1600 ms is load-bearing: the publisher
    // spends this span pushing at real-time cadence, which keeps
    // fresh AAC arriving at the WHEP session for the full 500-900 ms
    // ICE + DTLS warm-up plus a healthy tail so the first post-
    // Connected Opus frame has time to land.
    let Some(aac_access_units) = generate_aac_access_units(1600) else {
        eprintln!("skipping rtmp_whep_audio_e2e: failed to generate AAC source samples via GStreamer");
        return;
    };
    assert!(
        aac_access_units.len() >= 16,
        "AAC generator produced too few access units: {}",
        aac_access_units.len(),
    );

    str0m::crypto::from_feature_flags().install_process_default();

    let server = TestServer::start(TestServerConfig::default().with_whep())
        .await
        .expect("start TestServer with WHEP + transcode");
    let rtmp_addr = server.rtmp_addr();
    let whep_addr = server.whep_addr();
    eprintln!("[rtmp-whep-e2e] rtmp addr = {rtmp_addr}; whep addr = {whep_addr}");

    // Kick off the RTMP publisher in the background. The returned
    // stream + session are held alive until the test completes so
    // the bridge does not emit `BroadcastStopped` mid-read.
    let publisher = tokio::spawn(run_rtmp_publisher(rtmp_addr, aac_access_units));

    // Give the publisher enough time to complete the RTMP handshake
    // and publish the AAC sequence header so the `WhepServer` caches
    // the ASC before the offer arrives. The `handle_offer` replay
    // then delivers the cached config to the new session handle.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // --- Client side: Opus-only recvonly offer. ---
    let mut client = RtcConfig::new().enable_opus(true).build(Instant::now());

    let client_std = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind client udp");
    client_std.set_nonblocking(true).expect("nonblocking");
    let client_local_addr = client_std.local_addr().expect("local addr");
    let client_candidate = Candidate::host(client_local_addr, Protocol::Udp).expect("host candidate");
    client.add_local_candidate(client_candidate);

    let mut changes = client.sdp_api();
    let _client_mid = changes.add_media(MediaKind::Audio, Direction::RecvOnly, None, None, None);
    let (offer, pending) = changes.apply().expect("client sdp_api().apply() produced an offer");
    let offer_sdp = offer.to_sdp_string();

    // --- POST the offer to WHEP. ---
    let resp = http_post_sdp(whep_addr, "/whep/live/test", offer_sdp.as_bytes()).await;
    assert_eq!(
        resp.status,
        201,
        "POST /whep/live/test expected 201 Created, got {} (body: {:?})",
        resp.status,
        std::str::from_utf8(&resp.body).unwrap_or("<binary>"),
    );
    let location = find_header(&resp, "location").expect("WHEP answer must include Location header");
    assert!(
        location.starts_with("/whep/live/test/"),
        "Location header must point at the new session resource: {location}",
    );

    let answer_text = std::str::from_utf8(&resp.body).expect("answer body is utf-8");
    assert!(
        answer_text.to_ascii_lowercase().contains("opus"),
        "answer must negotiate Opus: {answer_text}",
    );
    let answer = SdpAnswer::from_sdp_string(answer_text).expect("answer re-parses");
    client
        .sdp_api()
        .accept_answer(pending, answer)
        .expect("client accept_answer");

    let server_candidate = extract_first_host_candidate(answer_text)
        .expect("WHEP answer carries a host candidate so ICE has somewhere to connect");
    eprintln!("[rtmp-whep-e2e] server host candidate = {server_candidate}");

    // --- Client poll loop: drive Rtc forward until the client raises
    //     `Event::MediaData` (i.e. a real Opus RTP packet landed). ---
    let client_socket = TokioUdp::from_std(client_std).expect("tokio udp from std");
    let mut buf = vec![0u8; 2048];
    let deadline = Instant::now() + OVERALL_DEADLINE;
    let mut connected = false;
    let mut media_frames = 0usize;

    while Instant::now() < deadline && (!connected || media_frames == 0) {
        let wait_until = loop {
            match client.poll_output().expect("client.poll_output") {
                Output::Timeout(when) => break when,
                Output::Transmit(t) => {
                    if let Err(e) = client_socket.send_to(&t.contents, t.destination).await {
                        panic!("client udp send_to {} failed: {e}", t.destination);
                    }
                }
                Output::Event(event) => match event {
                    Event::Connected => {
                        connected = true;
                        eprintln!("[rtmp-whep-e2e] client: Connected");
                    }
                    Event::IceConnectionStateChange(state) => {
                        eprintln!("[rtmp-whep-e2e] client: ice state {state:?}");
                        if state == IceConnectionState::Disconnected {
                            panic!("client ICE disconnected unexpectedly");
                        }
                    }
                    Event::MediaAdded(added) => {
                        eprintln!(
                            "[rtmp-whep-e2e] client: MediaAdded mid={:?} kind={:?}",
                            added.mid, added.kind
                        );
                    }
                    Event::MediaData(data) => {
                        media_frames += 1;
                        eprintln!(
                            "[rtmp-whep-e2e] client: MediaData mid={:?} pt={:?} len={}",
                            data.mid,
                            data.pt,
                            data.data.len(),
                        );
                    }
                    _ => {}
                },
            }
        };

        if connected && media_frames >= 1 {
            break;
        }

        let sleep_dur = wait_until
            .saturating_duration_since(Instant::now())
            .min(MAX_POLL_SLEEP)
            .max(Duration::from_millis(1));

        tokio::select! {
            biased;
            recv = client_socket.recv_from(&mut buf) => {
                let (n, source) = recv.expect("client recv_from");
                let contents = match (&buf[..n]).try_into() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("[rtmp-whep-e2e] client: skipping unparseable datagram: {e:?}");
                        continue;
                    }
                };
                let input = Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: client_local_addr,
                        contents,
                    },
                );
                client.handle_input(input).expect("client handle_input(receive)");
            }
            _ = tokio::time::sleep(sleep_dur) => {
                client.handle_input(Input::Timeout(Instant::now()))
                    .expect("client handle_input(timeout)");
            }
        }
    }

    assert!(
        connected,
        "WHEP client ICE/DTLS never completed within {OVERALL_DEADLINE:?}",
    );
    assert!(
        media_frames >= 1,
        "WHEP client never received any Opus audio frames within {OVERALL_DEADLINE:?}",
    );
    eprintln!("[rtmp-whep-e2e] got {media_frames} Opus media frames");

    // Hold the RTMP publisher open until we have asserted on the
    // subscriber, then tear the test down cleanly. Aborting the task
    // closes the TCP stream which fires `BroadcastStopped`.
    publisher.abort();
    let _ = publisher.await;
    server.shutdown().await.expect("shutdown");
}

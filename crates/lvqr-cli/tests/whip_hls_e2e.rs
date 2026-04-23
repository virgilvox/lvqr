//! WHIP ingest -> LL-HLS HTTP egress end-to-end integration test.
//!
//! Session 114 (phase B row 1) closes the biggest cross-ingress
//! test gap from the 2026-04-13 audit: WHIP (WebRTC HTTP Ingest
//! Protocol) had no coverage against any non-WebRTC egress. This
//! test drives a real `str0m::Rtc` client against the WHIP HTTP
//! surface, completes ICE + DTLS + SRTP in-process over loopback
//! UDP, writes synthetic H.264 samples through the client's
//! `Writer::write`, then reads the HLS playlist off the bound
//! MultiHlsServer to prove the fragment path all the way from
//! WHIP -> ingest bridge -> FragmentBroadcasterRegistry ->
//! BroadcasterHlsBridge -> axum HTTP.
//!
//! The publisher side mirrors `lvqr-whip/tests/e2e_str0m_loopback.rs`
//! (the harness that already pins WHIP offer / answer / ICE) and
//! the subscriber side mirrors `rtmp_hls_e2e.rs` (the HLS playlist
//! read path). Real ingest + real egress; no mocks.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use lvqr_test_utils::http::{HttpGetOptions, HttpResponse, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};
use str0m::change::SdpAnswer;
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, RtcConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket as TokioUdp};

const OVERALL_DEADLINE: Duration = Duration::from_secs(20);
const MAX_POLL_SLEEP: Duration = Duration::from_millis(50);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

// =====================================================================
// HTTP helpers. `http_get` forwards to the shared
// `lvqr_test_utils::http::http_get_with`; `http_post_sdp` stays
// local because the shared module is GET-only, but it returns the
// shared `HttpResponse` shape via a local `parse_http_response`
// helper kept narrowly scoped to the POST path.
// =====================================================================

async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    http_get_with(
        addr,
        path,
        HttpGetOptions {
            timeout: HTTP_TIMEOUT,
            ..Default::default()
        },
    )
    .await
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

/// Build a synthetic H.264 Annex B sample carrying SPS + PPS + IDR.
/// Identical to the `lvqr-whip/tests/e2e_str0m_loopback.rs` helper.
fn build_fake_annex_b_sample(dts: u64) -> Vec<u8> {
    let tag = (dts & 0xff) as u8;
    let sps: Vec<u8> = vec![0x67, 0x42, 0xC0, 0x1E, 0x9A, 0x66, 0x0A, tag];
    let pps: Vec<u8> = vec![0x68, 0xCE, 0x3C, 0x80, tag];
    let idr: Vec<u8> = vec![0x65, 0x88, 0x84, 0x40, 0x00, 0x00, 0x03, 0x00, tag];
    let mut out = Vec::new();
    for nal in [&sps, &pps, &idr] {
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        out.extend_from_slice(nal);
    }
    out
}

/// Real end-to-end: str0m client POSTs a WHIP offer via HTTP, the
/// lvqr-cli-hosted WHIP server responds with an answer + host
/// candidate, client completes ICE + DTLS + SRTP + starts pushing
/// synthetic H.264 samples, the WHIP bridge remuxes into CMAF
/// fragments on the shared FragmentBroadcasterRegistry, and the
/// LL-HLS drain publishes the per-broadcast playlist at
/// `/hls/live/test/playlist.m3u8`.
///
/// Test-timescale caveat: the LL-HLS segmenter needs a couple of
/// keyframes spaced beyond the default 2 s / 90 kHz segment budget
/// to close a full segment. The client writes at ~50 Hz for up to
/// `OVERALL_DEADLINE`; the assertion accepts `#EXT-X-PART:` (LL-HLS
/// partial) OR `#EXTINF:` (full segment) so a short test run does
/// not have to wait for a closed segment.
#[tokio::test(flavor = "current_thread")]
async fn whip_publish_reaches_hls_playlist() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    // Install crypto provider for the standalone client Rtc. The
    // server-side Str0mIngestAnswerer also installs it behind a
    // OnceLock, so redundant installs are safe.
    str0m::crypto::from_feature_flags().install_process_default();

    let server = TestServer::start(TestServerConfig::default().with_whip())
        .await
        .expect("start TestServer with WHIP + HLS");
    let whip_addr = server.whip_addr();
    let hls_addr = server.hls_addr();
    eprintln!("[whip-hls-e2e] whip addr = {whip_addr}; hls addr = {hls_addr}");

    // --- Client side: build a sendonly-video offer. ---
    let mut client = RtcConfig::new().enable_h264(true).build(Instant::now());

    let client_std = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind client udp");
    client_std.set_nonblocking(true).expect("nonblocking");
    let client_local_addr = client_std.local_addr().expect("local addr");
    let client_candidate = Candidate::host(client_local_addr, Protocol::Udp).expect("host candidate");
    client.add_local_candidate(client_candidate);

    let mut changes = client.sdp_api();
    let client_mid = changes.add_media(MediaKind::Video, Direction::SendOnly, None, None, None);
    let (offer, pending) = changes.apply().expect("client sdp_api().apply() produced an offer");
    let offer_sdp = offer.to_sdp_string();

    // --- POST the offer to the WHIP HTTP surface. ---
    let resp = http_post_sdp(whip_addr, "/whip/live/test", offer_sdp.as_bytes()).await;
    assert_eq!(
        resp.status,
        201,
        "POST /whip/live/test expected 201 Created, got {} (body: {:?})",
        resp.status,
        std::str::from_utf8(&resp.body).unwrap_or("<binary>"),
    );
    let location = resp
        .header("location")
        .expect("WHIP answer must include Location header");
    assert!(
        location.starts_with("/whip/live/test/"),
        "Location header must point at the new session resource: {location}",
    );

    let answer_text = std::str::from_utf8(&resp.body).expect("answer body is utf-8");
    let answer = SdpAnswer::from_sdp_string(answer_text).expect("answer re-parses");
    client
        .sdp_api()
        .accept_answer(pending, answer)
        .expect("client accept_answer");

    let server_candidate = extract_first_host_candidate(answer_text)
        .expect("WHIP answer carries a host candidate so ICE has somewhere to connect");
    eprintln!("[whip-hls-e2e] server host candidate = {server_candidate}");

    // --- Client poll loop: drive Rtc forward and write synthetic
    //     H.264 samples until the HLS playlist carries at least
    //     one part or full segment. ---
    let client_socket = TokioUdp::from_std(client_std).expect("tokio udp from std");
    let mut buf = vec![0u8; 2048];
    let deadline = Instant::now() + OVERALL_DEADLINE;
    let mut connected = false;
    let video_mid: Option<Mid> = Some(client_mid);
    let mut video_pt: Option<Pt> = None;
    let mut dts: u64 = 0;
    let mut last_write_at = Instant::now();
    let mut last_playlist_check = Instant::now();
    let mut playlist_ok = false;
    let mut playlist_body: Vec<u8> = Vec::new();

    while Instant::now() < deadline && !playlist_ok {
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
                        eprintln!("[whip-hls-e2e] client: Connected");
                        connected = true;
                    }
                    Event::IceConnectionStateChange(state) => {
                        eprintln!("[whip-hls-e2e] client: ice state {state:?}");
                        if state == IceConnectionState::Disconnected {
                            panic!("client ICE disconnected unexpectedly");
                        }
                    }
                    _ => {}
                },
            }
        };

        // Resolve the H.264 Pt once we know the video mid.
        if video_pt.is_none()
            && let Some(mid) = video_mid
            && let Some(writer) = client.writer(mid)
        {
            for params in writer.payload_params() {
                if params.spec().codec == Codec::H264 {
                    video_pt = Some(params.pt());
                    eprintln!("[whip-hls-e2e] client: resolved h264 pt {:?}", params.pt());
                    break;
                }
            }
        }

        // Write synthetic H.264 samples at ~50 Hz once connected.
        // Each sample is an SPS + PPS + IDR stack so the WHIP bridge
        // stamps `keyframe: true` on every sample and the HLS
        // segmenter closes a segment on each one.
        if connected && let (Some(mid), Some(pt)) = (video_mid, video_pt) {
            let now = Instant::now();
            if now.duration_since(last_write_at) >= Duration::from_millis(20) {
                last_write_at = now;
                let annex_b = build_fake_annex_b_sample(dts);
                if let Some(writer) = client.writer(mid) {
                    let rtp_time = MediaTime::new(dts, str0m::media::Frequency::NINETY_KHZ);
                    if let Err(e) = writer.write(pt, Instant::now(), rtp_time, annex_b) {
                        eprintln!("[whip-hls-e2e] client writer.write failed: {e:?}");
                    }
                }
                dts += 3000; // 33 ms at 90 kHz -> ~30 fps
            }
        }

        // Poll the HLS playlist at most every 200 ms so we do not
        // drown the test in HTTP traffic while ICE is still forming.
        if connected && Instant::now().duration_since(last_playlist_check) >= Duration::from_millis(200) {
            last_playlist_check = Instant::now();
            let resp = http_get(hls_addr, "/hls/live/test/playlist.m3u8").await;
            if resp.status == 200 {
                let body = std::str::from_utf8(&resp.body).unwrap_or("");
                if body.starts_with("#EXTM3U") && (body.contains("#EXT-X-PART:") || body.contains("#EXTINF:")) {
                    playlist_ok = true;
                    playlist_body = resp.body;
                    break;
                }
            }
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
                        eprintln!("[whip-hls-e2e] client: skipping unparseable datagram: {e:?}");
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
        "WHIP client ICE/DTLS never completed within {OVERALL_DEADLINE:?}",
    );
    assert!(
        playlist_ok,
        "HLS playlist never reported a part or segment within {OVERALL_DEADLINE:?}",
    );

    let body = std::str::from_utf8(&playlist_body).expect("playlist body utf-8");
    eprintln!("--- whip hls playlist ---\n{body}\n--- end ---");
    assert!(body.starts_with("#EXTM3U"), "playlist must start with #EXTM3U:\n{body}");
    assert!(
        body.contains("#EXT-X-PART:") || body.contains("#EXTINF:"),
        "playlist must contain at least one partial or segment:\n{body}",
    );

    server.shutdown().await.expect("shutdown");
}

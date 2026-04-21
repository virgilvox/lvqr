//! WHEP Opus-audio egress E2E.
//!
//! Session 30 closes item 1 of the session-29 recommended
//! entry-point list on the WHEP side: an Opus publisher lands
//! Opus audio on a WHEP subscriber that negotiated Opus,
//! without any transcode. This test pins that flow.
//!
//! Shape: build a client `Rtc` with `enable_opus(true)` only
//! (no video) and a recvonly audio mid, hand its offer to the
//! server `Str0mAnswerer`, pump synthetic Opus samples into the
//! session handle via `on_raw_sample` with track `"1.mp4"` and
//! `MediaCodec::Opus`, and wait on the client poll loop for
//! `Event::MediaData` frames routed through the negotiated
//! Opus Pt. Completes in under a second on loopback.
//!
//! This is the audio-side counterpart to
//! `e2e_str0m_loopback.rs` (H.264) and
//! `e2e_str0m_loopback_hevc.rs` (H.265). The same poll-loop
//! harness is copied verbatim because each test pins a
//! different slice of WHEP's state machine and keeping them
//! standalone makes regressions obvious in CI output.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use lvqr_cmaf::RawSample;
use lvqr_ingest::MediaCodec;
use lvqr_whep::{SdpAnswerer, SessionHandle, Str0mAnswerer, Str0mConfig};
use str0m::change::SdpAnswer;
use str0m::media::{Direction, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, RtcConfig};
use tokio::net::UdpSocket as TokioUdp;

const OVERALL_DEADLINE: Duration = Duration::from_secs(15);
const MAX_POLL_SLEEP: Duration = Duration::from_millis(50);

#[tokio::test(flavor = "current_thread")]
async fn client_receives_opus_audio_from_str0m_answerer() {
    str0m::crypto::from_feature_flags().install_process_default();

    // Client enables ONLY Opus so the negotiated m=audio
    // section has a single payload type. The server answerer
    // already enables h264 + h265 + opus after session 28; the
    // overlap here is opus only, so every audio sample routes
    // through the Opus Pt unambiguously.
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

    let answerer = Str0mAnswerer::new(Str0mConfig::default());
    let (handle, answer_bytes) = answerer
        .create_session("test/e2e-opus", offer_sdp.as_bytes())
        .expect("Str0mAnswerer accepted the Opus-only offer");

    let answer_text = std::str::from_utf8(&answer_bytes).expect("answer is utf8");
    assert!(
        answer_text.to_ascii_lowercase().contains("opus"),
        "answer should negotiate Opus: {answer_text}"
    );
    let answer = SdpAnswer::from_sdp_string(answer_text).expect("answer re-parses");
    client
        .sdp_api()
        .accept_answer(pending, answer)
        .expect("client accept_answer");

    let _server_addr = extract_first_host_candidate(answer_text).expect("answer carries a host candidate");

    // Pump synthetic Opus samples into the session handle once
    // ICE has a chance to complete. str0m's Opus packetizer is
    // opaque to the payload content, so a 10-byte "Opus packet"
    // round-trips through the RTP pipeline intact.
    let handle_arc: Arc<dyn SessionHandle> = Arc::from(handle);
    let sample_task = tokio::spawn(spam_opus_samples(handle_arc.clone()));

    let client_socket = TokioUdp::from_std(client_std).expect("tokio udp from std");
    let mut buf = vec![0u8; 2048];
    let deadline = Instant::now() + OVERALL_DEADLINE;
    let mut connected = false;
    let mut got_media = false;
    let mut media_frames = 0usize;

    while Instant::now() < deadline && (!connected || !got_media) {
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
                        eprintln!("[whep-e2e-opus] client: Connected");
                    }
                    Event::IceConnectionStateChange(state) => {
                        eprintln!("[whep-e2e-opus] client: ice state {state:?}");
                        if state == IceConnectionState::Disconnected {
                            panic!("client ICE disconnected unexpectedly");
                        }
                    }
                    Event::MediaAdded(added) => {
                        eprintln!(
                            "[whep-e2e-opus] client: MediaAdded mid={:?} kind={:?}",
                            added.mid, added.kind
                        );
                    }
                    Event::MediaData(data) => {
                        got_media = true;
                        media_frames += 1;
                        eprintln!(
                            "[whep-e2e-opus] client: MediaData mid={:?} pt={:?} len={}",
                            data.mid,
                            data.pt,
                            data.data.len()
                        );
                    }
                    _ => {}
                },
            }
        };

        if got_media && connected {
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
                        eprintln!("[whep-e2e-opus] client: skipping unparseable datagram: {e:?}");
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

    sample_task.abort();

    assert!(
        connected,
        "ICE/DTLS never completed on the client side within {OVERALL_DEADLINE:?}"
    );
    assert!(
        got_media,
        "client never received any Opus audio frames within {OVERALL_DEADLINE:?}; connected={connected}"
    );
    eprintln!("[whep-e2e-opus] got {media_frames} media frames");
    assert!(media_frames >= 1, "expected at least one Opus media frame");
}

async fn spam_opus_samples(handle: Arc<dyn SessionHandle>) {
    let frame_ticks: u64 = 960; // 20 ms at 48 kHz.
    let mut dts: u64 = 0;
    // Give the handshake a head start before pushing samples.
    tokio::time::sleep(Duration::from_millis(100)).await;
    loop {
        let tag = (dts & 0xff) as u8;
        let opus = vec![0x78, 0x01, 0x02, tag, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09];
        let sample = RawSample {
            track_id: 2,
            dts,
            cts_offset: 0,
            duration: frame_ticks as u32,
            payload: Bytes::from(opus),
            keyframe: true,
        };
        handle.on_raw_sample("1.mp4", MediaCodec::Opus, &sample, 0);
        dts += frame_ticks;
        tokio::time::sleep(Duration::from_millis(20)).await;
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

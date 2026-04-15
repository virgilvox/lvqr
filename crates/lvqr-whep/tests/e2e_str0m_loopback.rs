//! In-process str0m loopback end-to-end test for the WHEP media
//! write path.
//!
//! What this test proves:
//!
//! * A client `str0m::Rtc` builds a recvonly-video SDP offer.
//! * `Str0mAnswerer::create_session` accepts the offer and spins
//!   up its own `Rtc` + poll task on a loopback UDP socket.
//! * Both poll loops exchange UDP datagrams in-process and complete
//!   ICE + DTLS + SRTP.
//! * The test writer pushes real H.264 AVCC samples (SPS + PPS +
//!   IDR concatenation) into the session handle via
//!   `on_raw_sample`.
//! * The server task's media-write path converts AVCC to Annex B,
//!   calls `Writer::write` with the negotiated `Pt`, and the
//!   server's poll loop packetizes and encrypts RTP onto the wire.
//! * The client's poll loop receives, decrypts, depacketizes, and
//!   yields an `Event::MediaData` for at least one frame.
//!
//! This is a real end-to-end test, not a mock: every layer between
//! "observer accepts a sample" and "client Rtc yields a frame"
//! runs exactly the code path a real browser hits, except that the
//! packets travel over loopback UDP instead of the public
//! internet. The only thing a real-browser test would cover beyond
//! this one is the browser's H.264 decoder, which is not a WHEP
//! concern.
//!
//! This is the test that would have caught the session-21 design
//! note's "AVCC passthrough works" bug in fifteen seconds instead
//! of at the next review. It is slot 4 (E2E) in the 5-artifact
//! test contract for `lvqr-whep`.

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

/// Hard timeout for the whole flow. DTLS over loopback should
/// finish in well under a second; any failure mode that does not
/// cancel explicitly will hit this deadline and fail the test with
/// a precise assert rather than timing out silently in CI.
const OVERALL_DEADLINE: Duration = Duration::from_secs(15);

/// Poll-loop sleep cap: never block on the timeout arm for more
/// than 50ms at a time. str0m asks for longer sleeps when it has
/// nothing urgent to do, but inside a test we want to make
/// frequent progress on the sample-pumping task as well.
const MAX_POLL_SLEEP: Duration = Duration::from_millis(50);

#[tokio::test(flavor = "current_thread")]
async fn client_receives_video_from_str0m_answerer() {
    // Install the crypto provider once. `Str0mAnswerer::new` already
    // does this internally via a `OnceLock`, but we also build a
    // standalone client `Rtc` outside the answerer and that path
    // needs the provider installed too.
    str0m::crypto::from_feature_flags().install_process_default();

    // --- Client side: build a recvonly-video offer. ---
    let mut client = RtcConfig::new()
        .enable_h264(true)
        .enable_opus(true)
        .build(Instant::now());

    let client_std = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind client udp");
    client_std.set_nonblocking(true).expect("nonblocking");
    let client_local_addr = client_std.local_addr().expect("local addr");
    let client_candidate = Candidate::host(client_local_addr, Protocol::Udp).expect("host candidate");
    client.add_local_candidate(client_candidate);

    let mut changes = client.sdp_api();
    let _client_mid = changes.add_media(MediaKind::Video, Direction::RecvOnly, None, None, None);
    let (offer, pending) = changes.apply().expect("client sdp_api().apply() produced an offer");
    let offer_sdp = offer.to_sdp_string();

    // --- Server side: hand the offer to Str0mAnswerer. ---
    let answerer = Str0mAnswerer::new(Str0mConfig::default());
    let (handle, answer_bytes) = answerer
        .create_session("test/e2e", offer_sdp.as_bytes())
        .expect("Str0mAnswerer accepted the offer");

    // --- Client side: apply the server's answer. ---
    let answer_text = std::str::from_utf8(&answer_bytes).expect("answer is utf8");
    let answer = SdpAnswer::from_sdp_string(answer_text).expect("answer re-parses");
    client
        .sdp_api()
        .accept_answer(pending, answer)
        .expect("client accept_answer");

    // Extract the server's first host candidate from the answer so
    // the client knows where to send its first STUN binding
    // request. str0m adds the candidate internally from the SDP,
    // but we still assert the presence here to keep the failure
    // mode explicit when an answer accidentally ships without
    // candidates.
    let server_addr = extract_first_host_candidate(answer_text).expect("answer carries a host candidate");
    eprintln!("[e2e] server host candidate = {server_addr}; client addr = {client_local_addr}");

    // --- Sample pump: drive video samples into the session handle. ---
    //
    // str0m's H264 packetizer buffers SPS (type 7) and PPS (type 8)
    // until the next non-parameter-set NAL arrives, then wraps them
    // into a STAP-A. So every test sample must be SPS + PPS + IDR
    // to cause any RTP output. The payloads here are synthetic
    // byte sequences, not a real encoder output — str0m does not
    // decode, it only packetizes by NAL type, and the client does
    // not decode either, it only depacketizes and emits a
    // `MediaData` event. That is enough to prove the chain end-
    // to-end without carrying a real H.264 encoder into the test.
    let handle_arc: Arc<dyn SessionHandle> = Arc::from(handle);
    let sample_task = tokio::spawn(spam_video_samples(handle_arc.clone()));

    // --- Client loop: pump poll_output / recv_from until Connected
    //     and MediaData fire, or the deadline hits. ---
    let client_socket = TokioUdp::from_std(client_std).expect("tokio udp from std");
    let mut buf = vec![0u8; 2048];
    let deadline = Instant::now() + OVERALL_DEADLINE;
    let mut connected = false;
    let mut got_media = false;
    let mut media_frames = 0usize;

    while Instant::now() < deadline && (!connected || !got_media) {
        // Drain client outputs until we hit a timeout.
        let wait_until = loop {
            match client.poll_output().expect("client.poll_output") {
                Output::Timeout(when) => break when,
                Output::Transmit(t) => {
                    if let Err(e) = client_socket.send_to(&t.contents, t.destination).await {
                        panic!("client udp send_to {} failed: {e}", t.destination);
                    }
                }
                Output::Event(event) => {
                    absorb_client_event(event, &mut connected, &mut got_media, &mut media_frames);
                }
            }
        };

        if got_media && connected {
            break;
        }

        let now = Instant::now();
        let dur = wait_until
            .saturating_duration_since(now)
            .min(MAX_POLL_SLEEP)
            .max(Duration::from_millis(1));

        tokio::select! {
            biased;
            recv = client_socket.recv_from(&mut buf) => {
                let (n, source) = recv.expect("client recv_from");
                let contents = match (&buf[..n]).try_into() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("[e2e] client: skipping unparseable datagram: {e:?}");
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
            _ = tokio::time::sleep(dur) => {
                client.handle_input(Input::Timeout(Instant::now()))
                    .expect("client handle_input(timeout)");
            }
        }
    }

    sample_task.abort();
    // Dropping handle_arc here (implicitly at the function end)
    // tears down the server task cleanly.

    assert!(
        connected,
        "ICE/DTLS never completed on the client side within {OVERALL_DEADLINE:?}"
    );
    assert!(
        got_media,
        "client never received any video frames within {OVERALL_DEADLINE:?}; connected={connected}"
    );
    eprintln!("[e2e] got {media_frames} media frames");
    assert!(media_frames >= 1, "expected at least one media frame");
}

fn absorb_client_event(event: Event, connected: &mut bool, got_media: &mut bool, frames: &mut usize) {
    match event {
        Event::Connected => {
            *connected = true;
            eprintln!("[e2e] client: Connected");
        }
        Event::IceConnectionStateChange(state) => {
            eprintln!("[e2e] client: ice state {state:?}");
            if state == IceConnectionState::Disconnected {
                panic!("client ICE disconnected unexpectedly");
            }
        }
        Event::MediaAdded(added) => {
            eprintln!("[e2e] client: MediaAdded mid={:?} kind={:?}", added.mid, added.kind);
        }
        Event::MediaData(data) => {
            *got_media = true;
            *frames += 1;
            eprintln!(
                "[e2e] client: MediaData mid={:?} pt={:?} len={} keyframe?",
                data.mid,
                data.pt,
                data.data.len()
            );
        }
        _ => {}
    }
}

async fn spam_video_samples(handle: Arc<dyn SessionHandle>) {
    // 30 fps at a 90 kHz timescale -> 3000 ticks per frame.
    let frame_ticks: u64 = 3000;
    let mut dts: u64 = 0;
    // Give the handshake a head start. Writes before
    // `Event::Connected` are dropped on the server side anyway,
    // but sending them early is wasteful and pollutes the log.
    tokio::time::sleep(Duration::from_millis(100)).await;
    loop {
        let sample = build_fake_h264_sample(dts);
        handle.on_raw_sample("0.mp4", MediaCodec::H264, &sample);
        dts += frame_ticks;
        // Tight cadence: 20ms instead of 33ms so the test does not
        // sit idle while DTLS finishes. The server's Writer absorbs
        // them at the negotiated clock rate regardless.
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Build a synthetic H.264 AVCC sample carrying SPS + PPS + IDR.
///
/// NAL header bytes:
///
/// * 0x67 = `nal_ref_idc=3, type=7` (SPS)
/// * 0x68 = `nal_ref_idc=3, type=8` (PPS)
/// * 0x65 = `nal_ref_idc=3, type=5` (IDR slice)
///
/// str0m's packetizer inspects the NAL type and cares about this
/// structure; the body bytes are arbitrary and no decoder ever
/// runs on them in this test. See
/// `str0m::packet::h264::H264Packetizer::emit` for the behavior we
/// rely on (SPS + PPS buffered and emitted as STAP-A with the next
/// non-parameter-set NAL).
fn build_fake_h264_sample(dts: u64) -> RawSample {
    // Payload bodies are short, deterministic, and include the DTS
    // low byte so every sample is unique — helps spot duplicate
    // delivery in the logs if the loop ever loses bookkeeping.
    let tag = (dts & 0xff) as u8;
    let sps: Vec<u8> = vec![0x67, 0x42, 0xC0, 0x1E, 0x9A, 0x66, 0x0A, tag];
    let pps: Vec<u8> = vec![0x68, 0xCE, 0x3C, 0x80, tag];
    let idr: Vec<u8> = vec![0x65, 0x88, 0x84, 0x40, 0x00, 0x00, 0x03, 0x00, tag];
    let avcc = avcc_concat(&[&sps, &pps, &idr]);
    RawSample {
        track_id: 1,
        dts,
        cts_offset: 0,
        duration: 3000,
        payload: Bytes::from(avcc),
        keyframe: true,
    }
}

/// Encode a sequence of NAL bodies as one AVCC-framed buffer: each
/// entry is a 4-byte big-endian length followed by the body.
fn avcc_concat(nals: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nals {
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// Parse the first `a=candidate:` line from an SDP and return its
/// connection address. Simple hand-rolled parser: we only need the
/// 5th token (host) and the 6th token (port) of the candidate
/// attribute. Returns `None` if no host candidate is present.
fn extract_first_host_candidate(sdp: &str) -> Option<SocketAddr> {
    for line in sdp.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("a=candidate:") else {
            continue;
        };
        // candidate-attribute tokens: foundation component protocol
        // priority ip port typ candidate-type ...
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        if tokens.len() < 8 {
            continue;
        }
        if !tokens[7].eq_ignore_ascii_case("host") {
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

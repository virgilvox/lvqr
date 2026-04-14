//! In-process str0m loopback end-to-end test for the WHIP ingest
//! path.
//!
//! What this test proves:
//!
//! * A client `str0m::Rtc` builds a sendonly-video SDP offer.
//! * `Str0mIngestAnswerer::create_session` accepts the offer,
//!   spawns a server `Rtc` + poll task on a loopback UDP socket,
//!   and returns a parseable SDP answer.
//! * Both poll loops exchange UDP datagrams in-process and complete
//!   ICE + DTLS + SRTP.
//! * The client writer pushes synthetic H.264 AVCC samples (SPS +
//!   PPS + IDR concatenation) into its own `Rtc` via
//!   `Writer::write`. str0m converts Annex B boundaries on the
//!   client side and packetizes into RTP.
//! * The server task's poll loop receives, decrypts, depacketizes,
//!   and yields `Event::MediaData`. The backend converts the frame
//!   into an [`IngestSample`] and pumps it through an
//!   [`IngestSampleSink`] we install here.
//! * The capture sink records at least one keyframe sample whose
//!   Annex B payload parses cleanly back into a sequence of NAL
//!   units.
//!
//! This is the 4th artifact slot (E2E) in the 5-artifact test
//! contract for `lvqr-whip`. It is real end-to-end: every layer
//! between "client calls writer.write" and "capture sink sees a
//! sample" runs the exact code path a real browser publisher
//! would hit, except that the packets travel over loopback UDP
//! instead of the public internet.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bytes::Bytes;
use lvqr_whip::{IngestSample, IngestSampleSink, SdpAnswerer, Str0mIngestAnswerer, Str0mIngestConfig};
use str0m::change::SdpAnswer;
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, RtcConfig};
use tokio::net::UdpSocket as TokioUdp;

const OVERALL_DEADLINE: Duration = Duration::from_secs(15);
const MAX_POLL_SLEEP: Duration = Duration::from_millis(50);

/// Capture sink that records every [`IngestSample`] the server
/// backend forwards. Safe to clone (internally `Arc<Mutex<..>>`).
#[derive(Clone, Default)]
struct CaptureSink {
    samples: Arc<Mutex<Vec<IngestSample>>>,
}

impl CaptureSink {
    fn len(&self) -> usize {
        self.samples.lock().unwrap().len()
    }
    fn snapshot(&self) -> Vec<IngestSample> {
        self.samples.lock().unwrap().clone()
    }
}

impl IngestSampleSink for CaptureSink {
    fn on_sample(&self, _broadcast: &str, sample: IngestSample) {
        self.samples.lock().unwrap().push(sample);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn server_receives_video_from_client_publisher() {
    // Crypto provider for the standalone client `Rtc` we build
    // outside the answerer. The answerer's `new` also installs it
    // but does so behind a `OnceLock`, so redundant installs are
    // fine.
    str0m::crypto::from_feature_flags().install_process_default();

    // --- Server side: a Str0mIngestAnswerer with a capture sink. ---
    let capture = CaptureSink::default();
    let answerer = Str0mIngestAnswerer::new(Str0mIngestConfig::default(), Arc::new(capture.clone()));

    // --- Client side: build a sendonly-video offer. ---
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
    let client_mid = changes.add_media(MediaKind::Video, Direction::SendOnly, None, None, None);
    let (offer, pending) = changes.apply().expect("client sdp_api().apply() produced an offer");
    let offer_sdp = offer.to_sdp_string();

    // --- Hand the offer to the server answerer. ---
    let (handle, answer_bytes) = answerer
        .create_session("test/e2e-whip", offer_sdp.as_bytes())
        .expect("Str0mIngestAnswerer accepted the offer");

    // Apply the answer on the client side.
    let answer_text = std::str::from_utf8(&answer_bytes).expect("answer is utf8");
    let answer = SdpAnswer::from_sdp_string(answer_text).expect("answer re-parses");
    client
        .sdp_api()
        .accept_answer(pending, answer)
        .expect("client accept_answer");

    // Assert the server answer carries at least one host candidate
    // so the ICE agents can find each other.
    let server_addr = extract_first_host_candidate(answer_text).expect("answer carries a host candidate");
    eprintln!("[whip-e2e] server host candidate = {server_addr}; client addr = {client_local_addr}");

    // --- Client poll loop: runs inline in this task. Drives
    //     poll_output, pumps UDP, periodically writes synthetic
    //     H.264 samples through `Writer::write`. ---
    let client_socket = TokioUdp::from_std(client_std).expect("tokio udp from std");
    let mut buf = vec![0u8; 2048];
    let deadline = Instant::now() + OVERALL_DEADLINE;
    let mut connected = false;
    // On the sender side we already know our mid from `add_media`,
    // so do not wait for `MediaAdded` (which fires when a remote
    // media section is observed, not for local ones).
    let video_mid: Option<Mid> = Some(client_mid);
    let mut video_pt: Option<Pt> = None;
    let mut dts: u64 = 0;
    let mut last_write_at = Instant::now();

    while Instant::now() < deadline && capture.len() == 0 {
        // Drain client outputs.
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
                        eprintln!("[whip-e2e] client: Connected");
                        connected = true;
                    }
                    Event::IceConnectionStateChange(state) => {
                        eprintln!("[whip-e2e] client: ice state {state:?}");
                        if state == IceConnectionState::Disconnected {
                            panic!("client ICE disconnected unexpectedly");
                        }
                    }
                    Event::MediaAdded(added) => {
                        eprintln!(
                            "[whip-e2e] client: MediaAdded mid={:?} kind={:?}",
                            added.mid, added.kind
                        );
                    }
                    _ => {}
                },
            }
        };

        // Resolve the H.264 payload type once we know the video mid.
        if video_pt.is_none()
            && let Some(mid) = video_mid
            && let Some(writer) = client.writer(mid)
        {
            for params in writer.payload_params() {
                if params.spec().codec == Codec::H264 {
                    video_pt = Some(params.pt());
                    eprintln!("[whip-e2e] client: resolved h264 pt {:?}", params.pt());
                    break;
                }
            }
        }

        // Once connected, write synthetic samples at ~50 Hz so DTLS
        // finishes first. Writes before Connected are dropped by
        // str0m on the client side anyway.
        if connected && let (Some(mid), Some(pt)) = (video_mid, video_pt) {
            let now = Instant::now();
            if now.duration_since(last_write_at) >= Duration::from_millis(20) {
                last_write_at = now;
                let annex_b = build_fake_annex_b_sample(dts);
                if let Some(writer) = client.writer(mid) {
                    let rtp_time = MediaTime::new(dts, str0m::media::Frequency::NINETY_KHZ);
                    if let Err(e) = writer.write(pt, Instant::now(), rtp_time, annex_b) {
                        eprintln!("[whip-e2e] client writer.write failed: {e:?}");
                    }
                }
                dts += 3000;
            }
        }

        if capture.len() > 0 {
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
                        eprintln!("[whip-e2e] client: skipping unparseable datagram: {e:?}");
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

    // Tear down: dropping the handle sends the shutdown oneshot
    // which exits the server poll loop.
    drop(handle);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let samples = capture.snapshot();
    assert!(
        !samples.is_empty(),
        "capture sink never received a sample within {OVERALL_DEADLINE:?}; connected={connected}"
    );
    eprintln!("[whip-e2e] captured {} samples", samples.len());

    // At least one captured sample must be a keyframe whose
    // payload parses back into at least one NAL unit via the
    // bridge's Annex B splitter. This is the assertion that would
    // have caught a silent "everything empty" regression in the
    // poll loop.
    let kf = samples
        .iter()
        .find(|s| s.keyframe)
        .expect("expected at least one keyframe sample");
    let nals = lvqr_whip::split_annex_b(&kf.annex_b);
    assert!(
        !nals.is_empty(),
        "keyframe payload did not parse as Annex B NAL units: {:?}",
        &kf.annex_b
    );
}

/// Build a synthetic H.264 Annex B sample carrying SPS + PPS +
/// IDR, ready to hand to `str0m::Writer::write`. str0m's
/// `H264Packetizer` scans for Annex B start codes and splits into
/// NAL units, buffers SPS / PPS until the next non-parameter-set
/// NAL, and emits STAP-A + IDR over the wire.
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

/// Drop implementation marker: if the test panics before
/// `drop(handle)` runs, the server task will still exit cleanly
/// because the oneshot is dropped when the handle leaves scope.
/// This marker exists only to document the invariant.
#[allow(dead_code)]
fn _drop_invariant() {}

/// Hold Bytes import to satisfy the unused-import check without
/// actually needing bytes at the top level.
#[allow(dead_code)]
fn _import_bytes() -> Bytes {
    Bytes::new()
}

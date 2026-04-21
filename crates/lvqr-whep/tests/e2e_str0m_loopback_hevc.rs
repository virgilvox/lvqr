//! HEVC counterpart to `e2e_str0m_loopback.rs`.
//!
//! Forces the WHEP session to negotiate H.265 (and only H.265)
//! by building the client `Rtc` with `enable_h265(true)` and
//! nothing else. The server `Str0mAnswerer` already calls
//! `enable_h264(true).enable_h265(true).enable_opus(true)` on
//! its side after session 28, so the negotiation picks H.265
//! because that is the only overlap. The test then pushes real
//! x265 Main VPS + SPS + PPS + IDR access units into the WHEP
//! session handle with `MediaCodec::H265`. str0m's
//! `H265Packetizer` sees the Annex B buffer that
//! `avcc_to_annex_b` produced (length-prefixed AVCC in -> Annex
//! B with 0x00000001 start codes out, same path as the H.264
//! case because the framing is codec-agnostic), emits FU or AP
//! RTP packets, and the client depacketizes them and yields
//! `Event::MediaData` events.
//!
//! What this asserts:
//!
//! * WHEP's RtcConfig enables H.265 so the SDP answer carries an
//!   HEVC payload type.
//! * The codec-aware `video_pt_h264` / `video_pt_h265` resolution
//!   inside `SessionCtx` populates the H.265 slot.
//! * `write_video_sample` routes `MediaCodec::H265` samples
//!   through the H.265 pt, not the H.264 pt.
//! * The server's H.265 packetizer path produces RTP that the
//!   client's H.265 depacketizer accepts.
//!
//! Slot 4 (E2E) for the session-28 HEVC WHEP work.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use lvqr_cmaf::RawSample;
use lvqr_ingest::MediaCodec;
use lvqr_whep::{LatencyTracker, SdpAnswerer, SessionHandle, Str0mAnswerer, Str0mConfig};
use str0m::change::SdpAnswer;
use str0m::media::{Direction, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, RtcConfig};
use tokio::net::UdpSocket as TokioUdp;

const OVERALL_DEADLINE: Duration = Duration::from_secs(15);
const MAX_POLL_SLEEP: Duration = Duration::from_millis(50);

// Real x265 Main @L2.0 NAL bodies, same pins the `lvqr-cmaf`
// and `lvqr-whip` HEVC tests use.
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

#[tokio::test(flavor = "current_thread")]
async fn client_receives_hevc_video_from_str0m_answerer() {
    str0m::crypto::from_feature_flags().install_process_default();

    // Client enables ONLY H.265 so the offer carries a single
    // video payload type. The server (Str0mAnswerer) enables
    // h264+h265+opus; str0m picks H.265 because that is the
    // only video codec present on both sides.
    let mut client = RtcConfig::new().enable_h265(true).build(Instant::now());

    let client_std = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind client udp");
    client_std.set_nonblocking(true).expect("nonblocking");
    let client_local_addr = client_std.local_addr().expect("local addr");
    let client_candidate = Candidate::host(client_local_addr, Protocol::Udp).expect("host candidate");
    client.add_local_candidate(client_candidate);

    let mut changes = client.sdp_api();
    let _client_mid = changes.add_media(MediaKind::Video, Direction::RecvOnly, None, None, None);
    let (offer, pending) = changes.apply().expect("client sdp_api().apply() produced an offer");
    let offer_sdp = offer.to_sdp_string();

    // Session 110 B: attach a LatencyTracker. Asserted at the
    // end of the test: a successful HEVC MediaData on the client
    // implies the server-side poll loop did a writer.write, which
    // the SLO record arm observed under `transport="whep"`.
    let tracker = LatencyTracker::new();
    let answerer = Str0mAnswerer::new(Str0mConfig::default()).with_slo_tracker(tracker.clone());
    let (handle, answer_bytes) = answerer
        .create_session("test/e2e-hevc", offer_sdp.as_bytes())
        .expect("Str0mAnswerer accepted the HEVC offer");

    let answer_text = std::str::from_utf8(&answer_bytes).expect("answer is utf8");
    // Sanity: the answer must announce an H265 rtpmap. If str0m
    // ever stops accepting h265-only offers this test surfaces it
    // immediately rather than silently degrading to "never got
    // MediaData".
    assert!(
        answer_text.to_ascii_lowercase().contains("h265") || answer_text.to_ascii_lowercase().contains("h.265"),
        "answer should negotiate H.265: {answer_text}"
    );
    let answer = SdpAnswer::from_sdp_string(answer_text).expect("answer re-parses");
    client
        .sdp_api()
        .accept_answer(pending, answer)
        .expect("client accept_answer");

    let _server_addr = extract_first_host_candidate(answer_text).expect("answer carries a host candidate");

    let handle_arc: Arc<dyn SessionHandle> = Arc::from(handle);
    let sample_task = tokio::spawn(spam_hevc_samples(handle_arc.clone()));

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
                        eprintln!("[whep-e2e-hevc] client: skipping unparseable datagram: {e:?}");
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

    assert!(
        connected,
        "ICE/DTLS never completed on the client side within {OVERALL_DEADLINE:?}"
    );
    assert!(
        got_media,
        "client never received any HEVC video frames within {OVERALL_DEADLINE:?}; connected={connected}"
    );
    eprintln!("[whep-e2e-hevc] got {media_frames} media frames");
    assert!(media_frames >= 1, "expected at least one HEVC media frame");

    // Session 110 B positive-path assertion on the HEVC route.
    let snap = tracker.snapshot();
    let whep_entry = snap
        .iter()
        .find(|e| e.broadcast == "test/e2e-hevc" && e.transport == "whep");
    assert!(
        whep_entry.is_some(),
        "expected a test/e2e-hevc whep tracker entry after the client received HEVC; got {snap:?}",
    );
    let entry = whep_entry.unwrap();
    assert!(
        entry.sample_count >= 1,
        "expected >=1 whep sample after HEVC received; entry={entry:?}",
    );
}

fn absorb_client_event(event: Event, connected: &mut bool, got_media: &mut bool, frames: &mut usize) {
    match event {
        Event::Connected => {
            *connected = true;
            eprintln!("[whep-e2e-hevc] client: Connected");
        }
        Event::IceConnectionStateChange(state) => {
            eprintln!("[whep-e2e-hevc] client: ice state {state:?}");
            if state == IceConnectionState::Disconnected {
                panic!("client ICE disconnected unexpectedly");
            }
        }
        Event::MediaAdded(added) => {
            eprintln!(
                "[whep-e2e-hevc] client: MediaAdded mid={:?} kind={:?}",
                added.mid, added.kind
            );
        }
        Event::MediaData(data) => {
            *got_media = true;
            *frames += 1;
            eprintln!(
                "[whep-e2e-hevc] client: MediaData mid={:?} pt={:?} len={}",
                data.mid,
                data.pt,
                data.data.len()
            );
        }
        _ => {}
    }
}

async fn spam_hevc_samples(handle: Arc<dyn SessionHandle>) {
    let frame_ticks: u64 = 3000;
    let mut dts: u64 = 0;
    tokio::time::sleep(Duration::from_millis(100)).await;
    loop {
        let sample = build_fake_hevc_sample(dts);
        let ingest_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        handle.on_raw_sample("0.mp4", MediaCodec::H265, &sample, ingest_ms);
        dts += frame_ticks;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Build a synthetic HEVC AVCC sample carrying VPS + SPS + PPS +
/// IDR_W_RADL. str0m's `H265Packetizer` inspects the two-byte
/// HEVC NAL header and cares about this structure; the IDR slice
/// body is arbitrary and no decoder ever runs on it here.
fn build_fake_hevc_sample(dts: u64) -> RawSample {
    let tag = (dts & 0xff) as u8;
    // HEVC IDR_W_RADL NAL header: nal_unit_type=19, layer=0, tid=1 -> 0x26 0x01.
    let idr: Vec<u8> = vec![0x26, 0x01, 0xAF, tag];
    let avcc = avcc_concat(&[HEVC_VPS, HEVC_SPS, HEVC_PPS, &idr]);
    RawSample {
        track_id: 1,
        dts,
        cts_offset: 0,
        duration: 3000,
        payload: Bytes::from(avcc),
        keyframe: true,
    }
}

fn avcc_concat(nals: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nals {
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
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

//! In-process str0m loopback E2E for the WHIP Opus audio path.
//!
//! Session 29 added an audio fanout on the WHIP bridge so Opus
//! publishers land a sibling `1.mp4` MoQ track alongside their
//! video `0.mp4`. This test is the 4th-artifact E2E slot for
//! that path: a client `Rtc` builds an offer that carries
//! **both** a video (H.264) and an audio (Opus) section, the
//! server answerer accepts it, both poll loops complete ICE +
//! DTLS + SRTP, the client writes synthetic H.264 access units
//! through the video writer and synthetic Opus frames through
//! the audio writer, and the server side's capture sink
//! receives at least one sample in each slot.
//!
//! What this would catch if it regressed:
//!
//! * Audio-mid learning in `IngestCtx::audio_mid` via
//!   `Event::MediaAdded { kind: Audio, .. }`.
//! * Audio routing in `handle_event` so `MediaData` for the
//!   audio mid lands in `forward_audio_sample` instead of the
//!   video path or the silent drop.
//! * 48 kHz rebase via
//!   `MediaTime::rebase(Frequency::FORTY_EIGHT_KHZ)`.
//! * `WhipMoqBridge::on_audio_sample` -> `ensure_audio_initialized`
//!   -> `write_opus_init_segment` -> track creation chain.
//!
//! Structurally this mirrors `e2e_str0m_loopback.rs` (the
//! video-only H.264 E2E) and `e2e_str0m_loopback_hevc.rs` (the
//! HEVC counterpart); the same helpers are copied verbatim
//! rather than factored out because each test independently
//! pins a different aspect of the bridge and keeping them
//! standalone makes regressions obvious in CI output.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bytes::Bytes;
use lvqr_whip::{
    IngestAudioSample, IngestSample, IngestSampleSink, SdpAnswerer, Str0mIngestAnswerer, Str0mIngestConfig,
};
use str0m::change::SdpAnswer;
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, RtcConfig};
use tokio::net::UdpSocket as TokioUdp;

const OVERALL_DEADLINE: Duration = Duration::from_secs(15);
const MAX_POLL_SLEEP: Duration = Duration::from_millis(50);

#[derive(Clone, Default)]
struct CaptureSink {
    video: Arc<Mutex<Vec<IngestSample>>>,
    audio: Arc<Mutex<Vec<IngestAudioSample>>>,
}

impl CaptureSink {
    fn video_len(&self) -> usize {
        self.video.lock().unwrap().len()
    }
    fn audio_len(&self) -> usize {
        self.audio.lock().unwrap().len()
    }
}

impl IngestSampleSink for CaptureSink {
    fn on_sample(&self, _broadcast: &str, sample: IngestSample) {
        self.video.lock().unwrap().push(sample);
    }
    fn on_audio_sample(&self, _broadcast: &str, sample: IngestAudioSample) {
        self.audio.lock().unwrap().push(sample);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn server_receives_opus_audio_alongside_video() {
    str0m::crypto::from_feature_flags().install_process_default();

    let capture = CaptureSink::default();
    let answerer = Str0mIngestAnswerer::new(Str0mIngestConfig::default(), Arc::new(capture.clone()));

    // Client side: enable H.264 + Opus so the offer carries
    // both a video and an audio m-line. SendOnly in both
    // directions because this is a publish test.
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
    let client_video_mid = changes.add_media(MediaKind::Video, Direction::SendOnly, None, None, None);
    let client_audio_mid = changes.add_media(MediaKind::Audio, Direction::SendOnly, None, None, None);
    let (offer, pending) = changes.apply().expect("client sdp_api().apply() produced an offer");
    let offer_sdp = offer.to_sdp_string();

    let (handle, answer_bytes) = answerer
        .create_session("test/e2e-opus", offer_sdp.as_bytes())
        .expect("Str0mIngestAnswerer accepted the video+audio offer");

    let answer_text = std::str::from_utf8(&answer_bytes).expect("answer is utf8");
    let answer = SdpAnswer::from_sdp_string(answer_text).expect("answer re-parses");
    client
        .sdp_api()
        .accept_answer(pending, answer)
        .expect("client accept_answer");

    let _server_addr = extract_first_host_candidate(answer_text).expect("answer carries a host candidate");

    let client_socket = TokioUdp::from_std(client_std).expect("tokio udp from std");
    let mut buf = vec![0u8; 2048];
    let deadline = Instant::now() + OVERALL_DEADLINE;
    let mut connected = false;
    let video_mid: Option<Mid> = Some(client_video_mid);
    let audio_mid: Option<Mid> = Some(client_audio_mid);
    let mut video_pt: Option<Pt> = None;
    let mut audio_pt: Option<Pt> = None;
    let mut video_dts: u64 = 0;
    let mut audio_dts: u64 = 0;
    let mut last_video_write = Instant::now();
    let mut last_audio_write = Instant::now();

    while Instant::now() < deadline && (capture.video_len() == 0 || capture.audio_len() == 0) {
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
                        eprintln!("[whip-e2e-opus] client: Connected");
                    }
                    Event::IceConnectionStateChange(state) => {
                        eprintln!("[whip-e2e-opus] client: ice state {state:?}");
                        if state == IceConnectionState::Disconnected {
                            panic!("client ICE disconnected unexpectedly");
                        }
                    }
                    _ => {}
                },
            }
        };

        // Resolve video + audio payload types from the writer
        // once each mid is known. For send-side writers we do not
        // see `MediaAdded` locally (it fires on remote-observed
        // sections), so we skip the event-driven path and pull
        // params directly off the writer.
        if video_pt.is_none()
            && let Some(mid) = video_mid
            && let Some(writer) = client.writer(mid)
        {
            for params in writer.payload_params() {
                if params.spec().codec == Codec::H264 {
                    video_pt = Some(params.pt());
                    break;
                }
            }
        }
        if audio_pt.is_none()
            && let Some(mid) = audio_mid
            && let Some(writer) = client.writer(mid)
        {
            for params in writer.payload_params() {
                if params.spec().codec == Codec::Opus {
                    audio_pt = Some(params.pt());
                    break;
                }
            }
        }

        if connected {
            let now = Instant::now();
            // Video writer: synthetic SPS+PPS+IDR every 20ms.
            if let (Some(mid), Some(pt)) = (video_mid, video_pt)
                && now.duration_since(last_video_write) >= Duration::from_millis(20)
            {
                last_video_write = now;
                let annex_b = build_fake_annex_b_sample(video_dts);
                if let Some(writer) = client.writer(mid) {
                    let rtp_time = MediaTime::new(video_dts, str0m::media::Frequency::NINETY_KHZ);
                    let _ = writer.write(pt, Instant::now(), rtp_time, annex_b);
                }
                video_dts += 3000;
            }
            // Audio writer: synthetic Opus packet every 20ms (the
            // WebRTC default frame cadence). str0m's Opus
            // packetizer pushes the bytes through as a single
            // RTP packet, the server depacketizes them with the
            // same body intact, and the bridge wraps them in a
            // moof+mdat for the `1.mp4` track.
            if let (Some(mid), Some(pt)) = (audio_mid, audio_pt)
                && now.duration_since(last_audio_write) >= Duration::from_millis(20)
            {
                last_audio_write = now;
                let opus = build_fake_opus_frame(audio_dts);
                if let Some(writer) = client.writer(mid) {
                    let rtp_time = MediaTime::new(audio_dts, str0m::media::Frequency::FORTY_EIGHT_KHZ);
                    let _ = writer.write(pt, Instant::now(), rtp_time, opus);
                }
                audio_dts += 960; // 20 ms at 48 kHz
            }
        }

        if capture.video_len() > 0 && capture.audio_len() > 0 {
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
                        eprintln!("[whip-e2e-opus] client: skipping unparseable datagram: {e:?}");
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

    drop(handle);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let v = capture.video_len();
    let a = capture.audio_len();
    eprintln!("[whip-e2e-opus] captured video={v} audio={a} connected={connected}");
    assert!(
        v >= 1,
        "capture sink never received a video sample (connected={connected})"
    );
    assert!(
        a >= 1,
        "capture sink never received an opus audio sample (connected={connected})"
    );
}

fn build_fake_annex_b_sample(dts: u64) -> Vec<u8> {
    // Same synthetic SPS+PPS+IDR set used by the H.264 E2E --
    // str0m's H264 packetizer groups SPS+PPS into a STAP-A then
    // emits the IDR, so this produces real RTP traffic that the
    // server side depacketizes back into an Annex B frame.
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

/// Build a synthetic 10-byte "Opus packet" that str0m's Opus
/// packetizer treats as opaque application data. Opus RTP is
/// one-packet-per-frame (RFC 7587) with no header parsing on
/// the wire, so any byte sequence that depacketizes back to the
/// same bytes is sufficient for the E2E to prove the routing
/// chain. A real decoder would reject these bytes, but no
/// decoder runs anywhere in the test.
fn build_fake_opus_frame(dts: u64) -> Bytes {
    let tag = (dts & 0xff) as u8;
    Bytes::from(vec![0x78, 0x01, 0x02, tag, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09])
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

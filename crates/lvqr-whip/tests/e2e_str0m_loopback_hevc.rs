//! HEVC counterpart to `e2e_str0m_loopback.rs`.
//!
//! Builds a `str0m::Rtc` on the client side with **only**
//! `enable_h265(true)` so the SDP offer carries a single H.265
//! payload type and the server answerer is forced to negotiate
//! HEVC (rather than falling back to H.264). The client then
//! writes a real x265 IRAP access unit (VPS + SPS + PPS + IDR_W_RADL)
//! through `Writer::write` on every tick. str0m's `H265Packetizer`
//! scans for Annex B start codes and emits FU or AP RTP packets,
//! the server's `Str0mIngestAnswerer` depacketizes them through
//! its own poll loop, and `forward_video_sample` stamps the
//! incoming sample with `VideoCodec::H265` based on
//! `data.params.spec().codec`. The capture sink we install asserts
//! that at least one keyframe sample arrived and carries the H265
//! tag.
//!
//! This is session 26's delta on the 5-artifact contract: the
//! existing E2E covered H.264 end-to-end, the new one covers HEVC
//! end-to-end through the same bridge.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lvqr_whip::{IngestSample, IngestSampleSink, SdpAnswerer, Str0mIngestAnswerer, Str0mIngestConfig, VideoCodec};
use str0m::change::SdpAnswer;
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, RtcConfig};
use tokio::net::UdpSocket as TokioUdp;

const OVERALL_DEADLINE: Duration = Duration::from_secs(15);
const MAX_POLL_SLEEP: Duration = Duration::from_millis(50);

// Real x265 HEVC Main 3.0 NAL units, pinned to the same capture
// used by `lvqr-cmaf::init::tests` and
// `lvqr-whip::bridge::tests`. The slice body of the IDR is a
// stand-in — the bridge only cares about SPS-derived dimensions
// plus whether str0m's depacketizer flags the frame as keyframe,
// and IDR_W_RADL (nal_unit_type = 19) is self-identifying via its
// NAL header byte.
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
async fn server_receives_hevc_video_from_client_publisher() {
    str0m::crypto::from_feature_flags().install_process_default();

    let capture = CaptureSink::default();
    let answerer = Str0mIngestAnswerer::new(Str0mIngestConfig::default(), Arc::new(capture.clone()));

    // Client enables **only** H.265 so the negotiated m=video
    // section has a single HEVC payload type. If we also enabled
    // H.264 here, str0m would advertise both and the answerer
    // could pick H.264 first, defeating the purpose of this
    // test.
    let mut client = RtcConfig::new().enable_h265(true).build(Instant::now());

    let client_std = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind client udp");
    client_std.set_nonblocking(true).expect("nonblocking");
    let client_local_addr = client_std.local_addr().expect("local addr");
    let client_candidate = Candidate::host(client_local_addr, Protocol::Udp).expect("host candidate");
    client.add_local_candidate(client_candidate);

    let mut changes = client.sdp_api();
    let client_mid = changes.add_media(MediaKind::Video, Direction::SendOnly, None, None, None);
    let (offer, pending) = changes.apply().expect("client sdp_api().apply() produced an offer");
    let offer_sdp = offer.to_sdp_string();

    let (handle, answer_bytes) = answerer
        .create_session("test/e2e-whip-hevc", offer_sdp.as_bytes())
        .expect("Str0mIngestAnswerer accepted the HEVC offer");

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
    let video_mid: Option<Mid> = Some(client_mid);
    let mut video_pt: Option<Pt> = None;
    let mut dts: u64 = 0;
    let mut last_write_at = Instant::now();

    while Instant::now() < deadline && capture.len() == 0 {
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
                        eprintln!("[whip-e2e-hevc] client: Connected");
                        connected = true;
                    }
                    Event::IceConnectionStateChange(state) => {
                        eprintln!("[whip-e2e-hevc] client: ice state {state:?}");
                        if state == IceConnectionState::Disconnected {
                            panic!("client ICE disconnected unexpectedly");
                        }
                    }
                    Event::MediaAdded(added) => {
                        eprintln!(
                            "[whip-e2e-hevc] client: MediaAdded mid={:?} kind={:?}",
                            added.mid, added.kind
                        );
                    }
                    _ => {}
                },
            }
        };

        // Resolve the H.265 payload type once the writer's
        // payload params list is populated.
        if video_pt.is_none()
            && let Some(mid) = video_mid
            && let Some(writer) = client.writer(mid)
        {
            for params in writer.payload_params() {
                if params.spec().codec == Codec::H265 {
                    video_pt = Some(params.pt());
                    eprintln!("[whip-e2e-hevc] client: resolved h265 pt {:?}", params.pt());
                    break;
                }
            }
        }

        if connected && let (Some(mid), Some(pt)) = (video_mid, video_pt) {
            let now = Instant::now();
            if now.duration_since(last_write_at) >= Duration::from_millis(20) {
                last_write_at = now;
                let annex_b = build_hevc_irap(dts);
                if let Some(writer) = client.writer(mid) {
                    let rtp_time = MediaTime::new(dts, str0m::media::Frequency::NINETY_KHZ);
                    if let Err(e) = writer.write(pt, Instant::now(), rtp_time, annex_b) {
                        eprintln!("[whip-e2e-hevc] client writer.write failed: {e:?}");
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
                        eprintln!("[whip-e2e-hevc] client: skipping unparseable datagram: {e:?}");
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

    let samples = capture.snapshot();
    assert!(
        !samples.is_empty(),
        "capture sink never received an HEVC sample within {OVERALL_DEADLINE:?}; connected={connected}"
    );
    eprintln!("[whip-e2e-hevc] captured {} samples", samples.len());

    let kf = samples
        .iter()
        .find(|s| s.keyframe)
        .expect("expected at least one HEVC keyframe sample");
    assert_eq!(
        kf.codec,
        VideoCodec::H265,
        "keyframe should be tagged with the negotiated HEVC codec; got {:?}",
        kf.codec
    );
    let nals = lvqr_whip::split_annex_b(&kf.annex_b);
    assert!(
        !nals.is_empty(),
        "HEVC keyframe payload did not parse as Annex B NAL units: {:?}",
        &kf.annex_b
    );
    // At least one of the recovered NALs must be an HEVC VCL
    // (types 0..=31) or parameter set so we know the depack path
    // reached the slice data. `hevc_nal_type` lives in the crate
    // public API.
    let has_vcl_or_ps = nals
        .iter()
        .any(|n| matches!(lvqr_whip::hevc_nal_type(n), Some(t) if t <= 34));
    assert!(has_vcl_or_ps, "no HEVC VCL or parameter-set NALs recovered: {nals:?}");
}

/// Build a full HEVC IRAP access unit (VPS + SPS + PPS + IDR)
/// wrapped in 4-byte start codes, ready to hand to
/// `str0m::Writer::write`. The IDR NAL body is a stand-in — the
/// test only needs str0m's H265 packetizer to accept the access
/// unit and the depacketizer on the far side to re-emit it as one
/// `MediaData` event flagged as a keyframe.
fn build_hevc_irap(dts: u64) -> Vec<u8> {
    // Two-byte HEVC NAL header for nal_unit_type = 19
    // (IDR_W_RADL), layer 0, TID 1. Append a small dts-derived
    // byte so every frame has unique content (so str0m does not
    // treat back-to-back writes as a replay / dedupe target).
    let tag = (dts & 0xff) as u8;
    let idr = vec![0x26, 0x01, 0xAF, tag];
    let mut out = Vec::with_capacity(HEVC_VPS.len() + HEVC_SPS.len() + HEVC_PPS.len() + idr.len() + 16);
    for nal in [HEVC_VPS, HEVC_SPS, HEVC_PPS, &idr] {
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

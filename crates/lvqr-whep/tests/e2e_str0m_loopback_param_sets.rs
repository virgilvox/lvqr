//! In-process str0m loopback test for in-band SPS/PPS injection on
//! the WHEP write path (audit C-2 follow-up: RTMP-origin keyframes are
//! IDR-only).
//!
//! What this test proves end-to-end, over real loopback UDP with real
//! ICE / DTLS / SRTP and the real packetizer / depacketizer:
//!
//! * The publisher's parameter sets arrive out-of-band via
//!   `SessionHandle::on_video_config` (as the RTMP bridge delivers
//!   them from the AVC sequence header).
//! * The video samples that follow are IDR-only keyframes -- exactly
//!   the FLV/RTMP shape, where SPS/PPS live in the sequence header and
//!   never in the per-keyframe NALU payload.
//! * The WHEP write path prepends the SPS/PPS ahead of each such
//!   keyframe, so the client receives a keyframe whose depacketized
//!   Annex B contains the SPS (NAL type 7) and PPS (type 8) in
//!   addition to the IDR (type 5).
//!
//! Without the injection the client would receive an IDR with no
//! in-band parameter sets, which a real browser decoder (RFC 6184)
//! cannot initialize from -- the latent bug this fix closes.

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
const MAX_POLL_SLEEP: Duration = Duration::from_millis(20);

const START_CODE: [u8; 4] = [0x00, 0x00, 0x00, 0x01];

#[tokio::test(flavor = "current_thread")]
async fn out_of_band_param_sets_are_injected_into_idr_only_keyframes() {
    str0m::crypto::from_feature_flags().install_process_default();

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

    let answerer = Str0mAnswerer::new(Str0mConfig::default());
    let (handle, answer_bytes) = answerer
        .create_session("test/paramsets", offer_sdp.as_bytes())
        .expect("Str0mAnswerer accepted the offer");

    let answer_text = std::str::from_utf8(&answer_bytes).expect("answer is utf8");
    let answer = SdpAnswer::from_sdp_string(answer_text).expect("answer re-parses");
    client
        .sdp_api()
        .accept_answer(pending, answer)
        .expect("client accept_answer");

    let server_addr = extract_first_host_candidate(answer_text).expect("answer carries a host candidate");
    eprintln!("[ps] server host candidate = {server_addr}; client addr = {client_local_addr}");

    let handle_arc: Arc<dyn SessionHandle> = Arc::from(handle);
    let sample_task = tokio::spawn(feed_out_of_band_then_idr_only(handle_arc.clone()));

    let client_socket = TokioUdp::from_std(client_std).expect("tokio udp from std");
    let mut buf = vec![0u8; 2048];
    let deadline = Instant::now() + OVERALL_DEADLINE;

    let mut connected = false;
    let mut keyframe_with_sps = false;

    while Instant::now() < deadline && !keyframe_with_sps {
        let wait_until = loop {
            match client.poll_output().expect("client.poll_output") {
                Output::Timeout(when) => break when,
                Output::Transmit(t) => {
                    if let Err(e) = client_socket.send_to(&t.contents, t.destination).await {
                        panic!("client udp send_to {} failed: {e}", t.destination);
                    }
                }
                Output::Event(event) => {
                    absorb(event, &mut connected, &mut keyframe_with_sps);
                }
            }
        };

        if keyframe_with_sps {
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
                    Err(_) => continue,
                };
                let input = Input::Receive(
                    Instant::now(),
                    Receive { proto: Protocol::Udp, source, destination: client_local_addr, contents },
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

    assert!(connected, "ICE/DTLS never completed within {OVERALL_DEADLINE:?}");
    assert!(
        keyframe_with_sps,
        "expected a keyframe carrying an in-band SPS (NAL type 7); the IDR-only samples \
         must have had their out-of-band parameter sets injected",
    );
    eprintln!("[ps] success: received a keyframe with injected SPS/PPS");
}

fn absorb(event: Event, connected: &mut bool, keyframe_with_sps: &mut bool) {
    match event {
        Event::Connected => *connected = true,
        Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
            panic!("client ICE disconnected unexpectedly");
        }
        // A keyframe whose depacketized Annex B contains an SPS NAL
        // (type 7) proves the parameter sets were injected: the server
        // was fed IDR-only keyframes.
        Event::MediaData(data) if data.is_keyframe() && annexb_has_nal_type(&data.data, 7) => {
            *keyframe_with_sps = true;
        }
        _ => {}
    }
}

/// Deliver SPS/PPS out-of-band via on_video_config, then feed IDR-only
/// keyframes (no in-band parameter sets) at 20ms cadence.
async fn feed_out_of_band_then_idr_only(handle: Arc<dyn SessionHandle>) {
    tokio::time::sleep(Duration::from_millis(80)).await;

    // SPS (type 7) + PPS (type 8), Annex B framed, as the RTMP bridge
    // builds from the AVC sequence header.
    let mut param_sets = Vec::new();
    param_sets.extend_from_slice(&START_CODE);
    param_sets.extend_from_slice(&[0x67, 0x42, 0xC0, 0x1E, 0x9A, 0x66, 0x0A]);
    param_sets.extend_from_slice(&START_CODE);
    param_sets.extend_from_slice(&[0x68, 0xCE, 0x3C, 0x80]);
    handle.on_video_config("0.mp4", MediaCodec::H264, &param_sets);

    let frame_ticks: u64 = 3000;
    let mut dts: u64 = 0;
    loop {
        handle.on_raw_sample("0.mp4", MediaCodec::H264, &build_idr_only(dts), 0);
        dts += frame_ticks;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// IDR-only AVCC keyframe (NAL type 5), no SPS/PPS -- the FLV/RTMP shape.
fn build_idr_only(dts: u64) -> RawSample {
    let tag = (dts & 0xff) as u8;
    let idr: Vec<u8> = vec![0x65, 0x88, 0x84, 0x40, 0x00, 0x00, 0x03, 0x00, tag];
    let mut avcc = Vec::new();
    avcc.extend_from_slice(&(idr.len() as u32).to_be_bytes());
    avcc.extend_from_slice(&idr);
    RawSample {
        track_id: 1,
        dts,
        cts_offset: 0,
        duration: 3000,
        payload: Bytes::from(avcc),
        keyframe: true,
    }
}

/// Scan an Annex B byte stream for a NAL of the given H.264 type.
fn annexb_has_nal_type(data: &[u8], nal_type: u8) -> bool {
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if (data[i + 3] & 0x1F) == nal_type {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

fn extract_first_host_candidate(sdp: &str) -> Option<SocketAddr> {
    for line in sdp.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("a=candidate:") else {
            continue;
        };
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        if tokens.len() < 8 {
            continue;
        }
        if !tokens[7].eq_ignore_ascii_case("host") {
            continue;
        }
        if let Ok(addr) = format!("{}:{}", tokens[4], tokens[5]).parse::<SocketAddr>() {
            return Some(addr);
        }
    }
    None
}

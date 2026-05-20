//! In-process str0m loopback test for WHEP PLI / FIR keyframe-request
//! handling (audit C-2 / I-6).
//!
//! What this test proves end-to-end, over real loopback UDP with real
//! ICE / DTLS / SRTP and the real packetizer / depacketizer:
//!
//! * A recvonly-video `str0m::Rtc` client connects to a
//!   `Str0mAnswerer` session.
//! * The server is fed exactly ONE keyframe (SPS + PPS + IDR) followed
//!   by a continuous run of inter (P) frames -- it never produces a
//!   second keyframe sample.
//! * Once the client is connected and has received the initial
//!   keyframe plus several P-frames, it sends a PLI by calling
//!   `Writer::request_keyframe`.
//! * The server's poll loop surfaces `Event::KeyframeRequest` and
//!   replays the cached keyframe.
//! * The client therefore receives a SECOND keyframe even though the
//!   sample feed only ever delivered one. Without the C-2 / I-6 fix the
//!   client would sit on its single keyframe until the (never-arriving)
//!   next publisher keyframe.
//!
//! The replayed keyframe carries the original keyframe's dts, which is
//! older than the P-frames already sent. str0m's receive buffer dedupes
//! by RTP sequence number (always fresh and monotonic per write), not
//! by timestamp, so the client emits the replay as a new frame. This is
//! the exact behaviour `VideoKeyframeSnapshot` documents and relies on.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use lvqr_cmaf::RawSample;
use lvqr_ingest::MediaCodec;
use lvqr_whep::{SdpAnswerer, SessionHandle, Str0mAnswerer, Str0mConfig};
use str0m::change::SdpAnswer;
use str0m::media::{Direction, KeyframeRequestKind, MediaKind};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, RtcConfig};
use tokio::net::UdpSocket as TokioUdp;

const OVERALL_DEADLINE: Duration = Duration::from_secs(15);
const MAX_POLL_SLEEP: Duration = Duration::from_millis(20);

#[tokio::test(flavor = "current_thread")]
async fn pli_triggers_keyframe_replay() {
    str0m::crypto::from_feature_flags().install_process_default();

    // --- Client side: recvonly-video offer. ---
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
    let client_mid = changes.add_media(MediaKind::Video, Direction::RecvOnly, None, None, None);
    let (offer, pending) = changes.apply().expect("client sdp_api().apply() produced an offer");
    let offer_sdp = offer.to_sdp_string();

    // --- Server side. ---
    let answerer = Str0mAnswerer::new(Str0mConfig::default());
    let (handle, answer_bytes) = answerer
        .create_session("test/pli", offer_sdp.as_bytes())
        .expect("Str0mAnswerer accepted the offer");

    let answer_text = std::str::from_utf8(&answer_bytes).expect("answer is utf8");
    let answer = SdpAnswer::from_sdp_string(answer_text).expect("answer re-parses");
    client
        .sdp_api()
        .accept_answer(pending, answer)
        .expect("client accept_answer");

    let server_addr = extract_first_host_candidate(answer_text).expect("answer carries a host candidate");
    eprintln!("[pli] server host candidate = {server_addr}; client addr = {client_local_addr}");

    // --- Sample pump: ONE keyframe, then P-frames forever. ---
    // The shared counter lets the test observe how many keyframe
    // samples the server was actually fed, proving the second client
    // keyframe came from a replay rather than a fresh sample.
    let handle_arc: Arc<dyn SessionHandle> = Arc::from(handle);
    let keyframes_fed = Arc::new(AtomicU64::new(0));
    let sample_task = tokio::spawn(spam_one_keyframe_then_pframes(
        handle_arc.clone(),
        keyframes_fed.clone(),
    ));

    let client_socket = TokioUdp::from_std(client_std).expect("tokio udp from std");
    let mut buf = vec![0u8; 2048];
    let deadline = Instant::now() + OVERALL_DEADLINE;

    let mut connected = false;
    let mut keyframes_seen = 0usize;
    let mut deltas_seen = 0usize;
    let mut pli_sent = false;
    let mut keyframes_at_pli = 0usize;

    while Instant::now() < deadline {
        let wait_until = loop {
            match client.poll_output().expect("client.poll_output") {
                Output::Timeout(when) => break when,
                Output::Transmit(t) => {
                    if let Err(e) = client_socket.send_to(&t.contents, t.destination).await {
                        panic!("client udp send_to {} failed: {e}", t.destination);
                    }
                }
                Output::Event(event) => {
                    absorb_client_event(event, &mut connected, &mut keyframes_seen, &mut deltas_seen);
                }
            }
        };

        // Once connected and mid-stream (we hold the initial keyframe
        // and have seen a few deltas), send a single PLI and remember
        // the keyframe count at that moment.
        if connected && !pli_sent && keyframes_seen >= 1 && deltas_seen >= 3 {
            let mut writer = client.writer(client_mid).expect("client writer for recvonly mid");
            writer
                .request_keyframe(None, KeyframeRequestKind::Pli)
                .expect("client request_keyframe(Pli)");
            pli_sent = true;
            keyframes_at_pli = keyframes_seen;
            eprintln!("[pli] sent PLI; keyframes_seen so far = {keyframes_at_pli}");
        }

        // Success: a keyframe arrived strictly after the PLI was sent.
        if pli_sent && keyframes_seen > keyframes_at_pli {
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
                        eprintln!("[pli] client: skipping unparseable datagram: {e:?}");
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

    assert!(connected, "ICE/DTLS never completed within {OVERALL_DEADLINE:?}");
    assert!(pli_sent, "never reached a state where the client could send a PLI");

    // The server was fed exactly one keyframe sample for the whole
    // run, so any keyframe the client received after the PLI must be
    // the replay the C-2 / I-6 handler produced.
    let fed = keyframes_fed.load(Ordering::Relaxed);
    assert_eq!(
        fed, 1,
        "test feed should deliver exactly one keyframe sample; fed={fed}"
    );
    assert!(
        keyframes_seen > keyframes_at_pli,
        "expected a replayed keyframe after the PLI: keyframes_at_pli={keyframes_at_pli}, \
         keyframes_seen={keyframes_seen}, deltas_seen={deltas_seen}",
    );
    eprintln!(
        "[pli] success: keyframes_seen={keyframes_seen} (was {keyframes_at_pli} at PLI), deltas_seen={deltas_seen}",
    );
}

fn absorb_client_event(event: Event, connected: &mut bool, keyframes: &mut usize, deltas: &mut usize) {
    match event {
        Event::Connected => {
            *connected = true;
            eprintln!("[pli] client: Connected");
        }
        Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
            panic!("client ICE disconnected unexpectedly");
        }
        Event::MediaData(data) => {
            if data.is_keyframe() {
                *keyframes += 1;
                eprintln!("[pli] client: keyframe MediaData len={}", data.data.len());
            } else {
                *deltas += 1;
            }
        }
        _ => {}
    }
}

/// Feed one SPS+PPS+IDR keyframe, then inter (P) frames at 20ms
/// cadence forever. Increments `keyframes_fed` for each keyframe
/// sample so the test can assert exactly one was delivered.
async fn spam_one_keyframe_then_pframes(handle: Arc<dyn SessionHandle>, keyframes_fed: Arc<AtomicU64>) {
    let frame_ticks: u64 = 3000; // 30fps @ 90kHz
    let mut dts: u64 = 0;

    // Let the handshake make progress before the first write.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The single keyframe.
    handle.on_raw_sample("0.mp4", MediaCodec::H264, &build_keyframe(dts), 0);
    keyframes_fed.fetch_add(1, Ordering::Relaxed);
    dts += frame_ticks;

    // P-frames only from here on.
    loop {
        handle.on_raw_sample("0.mp4", MediaCodec::H264, &build_pframe(dts), 0);
        dts += frame_ticks;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Synthetic AVCC keyframe: SPS (type 7) + PPS (type 8) + IDR (type 5).
/// str0m's packetizer buffers SPS/PPS and emits a STAP-A with the IDR.
fn build_keyframe(dts: u64) -> RawSample {
    let tag = (dts & 0xff) as u8;
    let sps: Vec<u8> = vec![0x67, 0x42, 0xC0, 0x1E, 0x9A, 0x66, 0x0A, tag];
    let pps: Vec<u8> = vec![0x68, 0xCE, 0x3C, 0x80, tag];
    let idr: Vec<u8> = vec![0x65, 0x88, 0x84, 0x40, 0x00, 0x00, 0x03, 0x00, tag];
    RawSample {
        track_id: 1,
        dts,
        cts_offset: 0,
        duration: 3000,
        payload: Bytes::from(avcc_concat(&[&sps, &pps, &idr])),
        keyframe: true,
    }
}

/// Synthetic AVCC inter frame: a single non-IDR slice (type 1). str0m
/// depacketizes it as a non-keyframe.
fn build_pframe(dts: u64) -> RawSample {
    let tag = (dts & 0xff) as u8;
    let slice: Vec<u8> = vec![0x41, 0x9A, 0x00, 0x12, 0x34, tag];
    RawSample {
        track_id: 1,
        dts,
        cts_offset: 0,
        duration: 3000,
        payload: Bytes::from(avcc_concat(&[&slice])),
        keyframe: false,
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

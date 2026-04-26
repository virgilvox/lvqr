//! `scte35-rtmp-push` -- session 155 test bin.
//!
//! Drives a real RTMP publisher session against the relay's RTMP
//! listener. Sends synthetic H.264 NALUs (a parseable AVC sequence
//! header followed by 1 IDR + N P-slices per GOP) plus one or more
//! `onCuePoint scte35-bin64` AMF0 Data messages at caller-chosen
//! offsets. Used by:
//!
//! 1. `crates/lvqr-test-utils/tests/scte35_rtmp_push_smoke.rs` --
//!    Rust integration test that spawns the bin against a TestServer
//!    and asserts `#EXT-X-DATERANGE` shows up in the served HLS
//!    variant playlist.
//! 2. `bindings/js/tests/e2e/dvr-player/markers.spec.ts` -- new
//!    `LVQR_LIVE_RTMP_TESTS=1`-gated Playwright e2e that mounts the
//!    `@lvqr/dvr-player` web component against a real publishing
//!    relay and asserts the marker tick + span render at the
//!    expected fractions.
//!
//! Why a custom bin instead of ffmpeg: ffmpeg cannot natively emit
//! AMF0 `onCuePoint` Data messages. Session 152 vendored
//! `rml_rtmp` v0.8.0 with a server-side patch surfacing
//! non-`@setDataFrame` AMF0 Data as a typed event; session 155 adds
//! the symmetric client-side `publish_amf0_data` method this bin
//! depends on.
//!
//! On exit, prints a single JSON line on stdout so the spawning
//! Playwright spec / smoke test can capture the publish summary:
//!
//! ```text
//! {"events_sent":1,"frames_sent":120,"duration_secs":8.0}
//! ```
//!
//! stderr carries `tracing` info-level logs for diagnostics.

use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use clap::Parser;
use rml_amf0::Amf0Value;
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use tokio::net::TcpStream;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use lvqr_test_utils::h264::{
    PPS_HIGH, SPS_HIGH_64X64, flv_avc_nalu, flv_avc_sequence_header, synthetic_idr_nal, synthetic_p_slice_nal,
};
use lvqr_test_utils::rtmp::{read_until, rtmp_client_handshake, send_result, send_results};
use lvqr_test_utils::scte35::splice_insert_section_bytes;

/// CLI: drives a deterministic synthetic-H.264 + onCuePoint publish
/// session against the relay's RTMP listener.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Session 155 test bin: synthetic RTMP publish + onCuePoint scte35-bin64 injection."
)]
struct Cli {
    /// Full RTMP URL incl. app + stream key, e.g.
    /// `rtmp://127.0.0.1:11936/live/dvr-test`.
    #[arg(long)]
    rtmp_url: String,

    /// Total publish runtime in seconds (after the first IDR).
    #[arg(long, default_value_t = 8.0)]
    duration_secs: f64,

    /// Comma-separated list of offsets (seconds) at which to send an
    /// `onCuePoint scte35-bin64` AMF0 Data message. Each offset gets
    /// a fresh `event_id` (incremented from the base) so the relay
    /// renders a unique `#EXT-X-DATERANGE` ID per emission.
    #[arg(long, value_delimiter = ',', default_value = "3.0")]
    inject_at_secs: Vec<f64>,

    /// Hex-encoded splice_info_section. Optional `0x` prefix. When
    /// omitted, falls back to the canonical fixture
    /// `splice_insert_section_bytes(0xCAFEBABE, 8_100_000, 2_700_000)`
    /// also used by `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs`.
    #[arg(long)]
    scte35_hex: Option<String>,

    /// Frame rate in Hz.
    #[arg(long, default_value_t = 30)]
    video_fps: u32,

    /// Keyframe interval (frames). Default 60 = 2 s segments at 30 fps.
    #[arg(long, default_value_t = 60)]
    keyframe_interval_frames: u32,
}

#[derive(Debug)]
struct ParsedRtmpUrl {
    host: String,
    port: u16,
    app: String,
    stream_key: String,
}

fn parse_rtmp_url(raw: &str) -> Result<ParsedRtmpUrl> {
    let rest = raw
        .strip_prefix("rtmp://")
        .ok_or_else(|| anyhow!("rtmp_url must start with rtmp://"))?;
    let (authority, path) = rest
        .split_once('/')
        .ok_or_else(|| anyhow!("rtmp_url must contain /<app>/<stream_key>"))?;
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().context("rtmp_url port not u16")?),
        None => (authority.to_string(), 1935u16),
    };
    let mut path_parts = path.splitn(2, '/');
    let app = path_parts
        .next()
        .ok_or_else(|| anyhow!("rtmp_url missing app"))?
        .to_string();
    let stream_key = path_parts
        .next()
        .ok_or_else(|| anyhow!("rtmp_url missing stream_key"))?
        .to_string();
    if app.is_empty() || stream_key.is_empty() {
        bail!("rtmp_url app + stream_key must be non-empty");
    }
    Ok(ParsedRtmpUrl {
        host,
        port,
        app,
        stream_key,
    })
}

fn decode_hex(input: &str) -> Result<Vec<u8>> {
    let s = input.trim();
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    if s.len() % 2 != 0 {
        bail!("scte35_hex must have an even number of hex digits");
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for i in (0..s.len()).step_by(2) {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(10 + (b - b'a')),
        b'A'..=b'F' => Ok(10 + (b - b'A')),
        other => Err(anyhow!("invalid hex digit: 0x{:02x}", other)),
    }
}

/// Build the AMF0 wire shape the relay's `parse_oncuepoint_scte35`
/// at `crates/lvqr-ingest/src/rtmp.rs:470` consumes:
///
/// ```text
/// amf0_string("onCuePoint")
/// amf0_object {
///     "name" => "scte35-bin64",
///     "data" => "<base64-encoded splice_info_section>",
///     "time" => Number(<offset_secs>),
///     "type" => "event",
/// }
/// ```
fn build_oncuepoint_amf0(section: &[u8], time_secs: f64) -> Vec<Amf0Value> {
    let mut props = HashMap::new();
    props.insert("name".to_string(), Amf0Value::Utf8String("scte35-bin64".to_string()));
    props.insert(
        "data".to_string(),
        Amf0Value::Utf8String(BASE64_STANDARD.encode(section)),
    );
    props.insert("time".to_string(), Amf0Value::Number(time_secs));
    props.insert("type".to_string(), Amf0Value::Utf8String("event".to_string()));

    vec![
        Amf0Value::Utf8String("onCuePoint".to_string()),
        Amf0Value::Object(props),
    ]
}

/// Patch the splice_event_id field of a CRC-valid splice_info_section
/// in place + recompute CRC-32/MPEG-2 over the new bytes. Lets the bin
/// emit multiple onCuePoints per run with a unique daterange ID per
/// emission (the relay derives the DATERANGE ID from `splice_event_id`).
fn rewrite_event_id(mut bytes: Vec<u8>, new_event_id: u32) -> Vec<u8> {
    // Layout per ANSI/SCTE 35-2024 section 8.1: 14-byte prefix (table
    // header) then the splice_command. For splice_insert the first 4
    // bytes of the command body are the event_id (BE u32). With the
    // 14-byte prefix that's offsets 14..18.
    if bytes.len() < 22 {
        return bytes; // too short to contain event_id + CRC; bail.
    }
    bytes[14] = (new_event_id >> 24) as u8;
    bytes[15] = (new_event_id >> 16) as u8;
    bytes[16] = (new_event_id >> 8) as u8;
    bytes[17] = new_event_id as u8;

    // CRC-32/MPEG-2 over [0..len-4]; replace trailing 4 bytes.
    let crc_end = bytes.len() - 4;
    let mut c: u32 = 0xFFFF_FFFF;
    for &b in &bytes[..crc_end] {
        c ^= (b as u32) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 {
                (c << 1) ^ 0x04C1_1DB7
            } else {
                c << 1
            };
        }
    }
    bytes[crc_end] = (c >> 24) as u8;
    bytes[crc_end + 1] = (c >> 16) as u8;
    bytes[crc_end + 2] = (c >> 8) as u8;
    bytes[crc_end + 3] = c as u8;
    bytes
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .try_init();

    let cli = Cli::parse();
    match run(cli).await {
        Ok(summary) => {
            // Single-line JSON on stdout for the spawning harness to parse.
            println!(
                "{{\"events_sent\":{},\"frames_sent\":{},\"duration_secs\":{:.3}}}",
                summary.events_sent, summary.frames_sent, summary.duration_secs
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            warn!("scte35-rtmp-push failed: {e:#}");
            // Still print a JSON line on stdout for parsing parity; the
            // exit code communicates failure independent of stdout shape.
            println!("{{\"events_sent\":0,\"frames_sent\":0,\"duration_secs\":0.000,\"error\":\"{e}\"}}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Default)]
struct PublishSummary {
    events_sent: u32,
    frames_sent: u32,
    duration_secs: f64,
}

async fn run(cli: Cli) -> Result<PublishSummary> {
    let parsed = parse_rtmp_url(&cli.rtmp_url)?;
    info!(
        host = %parsed.host,
        port = parsed.port,
        app = %parsed.app,
        stream_key = %parsed.stream_key,
        duration_secs = cli.duration_secs,
        inject_at_secs = ?cli.inject_at_secs,
        "scte35-rtmp-push starting"
    );

    let scte35_section = match cli.scte35_hex.as_ref() {
        Some(hex) => decode_hex(hex).context("decoding --scte35-hex")?,
        // Default fixture mirrors `scte35_hls_dash_e2e.rs`: event_id
        // 0xCAFEBABE renders as DATERANGE ID `splice-3405691582`.
        None => splice_insert_section_bytes(0xCAFE_BABE, 8_100_000, 2_700_000),
    };

    // TCP connect + RTMP handshake.
    let mut stream = TcpStream::connect((parsed.host.as_str(), parsed.port))
        .await
        .with_context(|| format!("TCP connect rtmp://{}:{}", parsed.host, parsed.port))?;
    let trailing = rtmp_client_handshake(&mut stream).await;
    debug!(trailing_len = trailing.len(), "RTMP handshake completed");

    // Bootstrap the client session at the rml_rtmp default
    // chunk_size (4096). The relay's deserializer adopts that size
    // once it receives the post-connect `SetChunkSize` packet from
    // our session; before session 155's read_until fix that packet
    // was silently dropped because the helper short-circuited on
    // `ConnectionRequestAccepted`, so the relay stayed at 128-byte
    // chunks while our serializer ramped to 4096 -- breaking the
    // 132-byte AMF0 onCuePoint send mid-payload.
    let config = ClientSessionConfig::new();
    let (mut session, initial) = ClientSession::new(config).map_err(|e| anyhow!("ClientSession::new: {e:?}"))?;
    send_results(&mut stream, &initial).await;

    // Feed any post-handshake bytes the server emitted before the
    // first session.handle_input call so the chunk stream stays in
    // sync.
    if !trailing.is_empty() {
        let pre_results = session
            .handle_input(&trailing)
            .map_err(|e| anyhow!("session.handle_input on trailing handshake bytes: {e:?}"))?;
        send_results(&mut stream, &pre_results).await;
    }

    // Connect to the app.
    let connect = session
        .request_connection(parsed.app.clone())
        .map_err(|e| anyhow!("request_connection: {e:?}"))?;
    send_result(&mut stream, &connect).await;
    read_until(&mut stream, &mut session, Duration::from_secs(15), |evt| {
        matches!(evt, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;
    info!(app = %parsed.app, "RTMP connection accepted");

    // Request publish.
    let publish = session
        .request_publishing(parsed.stream_key.clone(), PublishRequestType::Live)
        .map_err(|e| anyhow!("request_publishing: {e:?}"))?;
    send_result(&mut stream, &publish).await;
    read_until(&mut stream, &mut session, Duration::from_secs(15), |evt| {
        matches!(evt, ClientSessionEvent::PublishRequestAccepted)
    })
    .await;
    info!(stream_key = %parsed.stream_key, "RTMP publish accepted");

    // Send the AVC sequence header so the relay's bridge populates
    // video_config + video_init; subsequent NALU tags are dropped
    // until that happens.
    let seq_header = flv_avc_sequence_header(SPS_HIGH_64X64, PPS_HIGH);
    let result = session
        .publish_video_data(seq_header, RtmpTimestamp::new(0), false)
        .map_err(|e| anyhow!("publish_video_data (sequence header): {e:?}"))?;
    send_result(&mut stream, &result).await;
    debug!("AVC sequence header sent");

    // Frame loop: 1 IDR every keyframe_interval_frames, P-slice
    // otherwise. Pace via tokio::time::sleep at fps Hz.
    let fps = cli.video_fps.max(1);
    let inter_frame_ms = (1000.0 / fps as f64).max(1.0) as u64;
    let total_frames = (cli.duration_secs * fps as f64).max(1.0) as u32;
    let keyframe_interval = cli.keyframe_interval_frames.max(1);

    let mut summary = PublishSummary::default();
    let start = Instant::now();
    let mut next_inject_idx = 0usize;

    for frame in 0..total_frames {
        let elapsed = start.elapsed().as_secs_f64();
        let timestamp_ms = (frame as u64 * 1000) / fps as u64;
        let keyframe = frame % keyframe_interval == 0;
        let nalu_payload = if keyframe {
            synthetic_idr_nal()
        } else {
            synthetic_p_slice_nal()
        };
        let tag = flv_avc_nalu(keyframe, 0, &nalu_payload);
        let result = session
            .publish_video_data(tag, RtmpTimestamp::new(timestamp_ms as u32), !keyframe)
            .map_err(|e| anyhow!("publish_video_data (frame {frame}): {e:?}"))?;
        send_result(&mut stream, &result).await;
        summary.frames_sent += 1;

        // Inject onCuePoint events at their offsets. Each emission
        // patches the section's event_id so the relay renders a fresh
        // DATERANGE ID; the base event_id is read from the section
        // bytes themselves so callers who supply a custom --scte35-hex
        // get their own base.
        while next_inject_idx < cli.inject_at_secs.len() && elapsed >= cli.inject_at_secs[next_inject_idx] {
            let offset = cli.inject_at_secs[next_inject_idx];
            let base_event_id = u32::from_be_bytes([
                scte35_section.get(14).copied().unwrap_or(0),
                scte35_section.get(15).copied().unwrap_or(0),
                scte35_section.get(16).copied().unwrap_or(0),
                scte35_section.get(17).copied().unwrap_or(0),
            ]);
            let emission_event_id = base_event_id.wrapping_add(next_inject_idx as u32);
            let section = if next_inject_idx == 0 {
                scte35_section.clone()
            } else {
                rewrite_event_id(scte35_section.clone(), emission_event_id)
            };
            let amf0 = build_oncuepoint_amf0(&section, offset);
            let result = session
                .publish_amf0_data(amf0)
                .map_err(|e| anyhow!("publish_amf0_data (offset {offset}): {e:?}"))?;
            send_result(&mut stream, &result).await;
            summary.events_sent += 1;
            info!(
                offset_secs = offset,
                event_id = format!("{:#010x}", emission_event_id),
                "onCuePoint scte35-bin64 sent"
            );
            next_inject_idx += 1;
        }

        tokio::time::sleep(Duration::from_millis(inter_frame_ms)).await;
    }

    summary.duration_secs = start.elapsed().as_secs_f64();

    // Tear down the publish cleanly.
    let stop = session
        .stop_publishing()
        .map_err(|e| anyhow!("stop_publishing: {e:?}"))?;
    send_results(&mut stream, &stop).await;

    // Drain any final events (e.g. server-side ack of the
    // deleteStream); ignore errors -- the publish was successful by
    // this point.
    let _ = tokio::time::timeout(Duration::from_millis(200), drain_remaining(&mut stream, &mut session)).await;

    Ok(summary)
}

async fn drain_remaining(stream: &mut TcpStream, session: &mut ClientSession) {
    let mut buf = vec![0u8; 8192];
    use tokio::io::AsyncReadExt as _;
    loop {
        match stream.read(&mut buf).await {
            Ok(0) => return,
            Ok(n) => {
                let results = match session.handle_input(&buf[..n]) {
                    Ok(r) => r,
                    Err(_) => return,
                };
                let _ = handle_drain_results(stream, results).await;
            }
            Err(_) => return,
        }
    }
}

async fn handle_drain_results(stream: &mut TcpStream, results: Vec<ClientSessionResult>) -> Result<()> {
    use tokio::io::AsyncWriteExt as _;
    for r in results {
        if let ClientSessionResult::OutboundResponse(packet) = r {
            stream.write_all(&packet.bytes).await?;
        }
    }
    Ok(())
}

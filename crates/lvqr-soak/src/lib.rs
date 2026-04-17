//! Long-duration soak harness for LVQR's RTSP PLAY egress.
//!
//! Spins up a real [`lvqr_rtsp::RtspServer`] on an ephemeral TCP
//! port with a shared [`FragmentBroadcasterRegistry`], drives a
//! synthetic H.264 publisher at a configurable fragment rate, and
//! spawns `N` concurrent RTSP PLAY subscribers over the loopback
//! interface. Every subscriber counts received RTP + RTCP frames
//! so the harness can detect:
//!
//! * Fragment delivery drops (per-subscriber RTP packet count drifts
//!   below the expected floor).
//! * RTCP Sender Report starvation (per-subscriber RTCP packet count
//!   stays at 0 when the configured SR interval predicts >=1 SR).
//! * Resource leaks (RSS + open-FD samples over the soak window;
//!   Linux-only today, None on other platforms).
//!
//! Scope is intentionally narrow: the harness does **not** fix
//! things it finds. It runs the stack as written for the configured
//! duration, collects deltas, and reports them. Pass/fail is a
//! simple threshold check you can tune from the CLI. The goal is
//! that a nightly 24 h run against `lvqr-soak` with the default
//! thresholds is what unblocks the M4 readiness claim.
//!
//! ## Not covered
//!
//! * HEVC / AAC / Opus drain coverage. The harness exercises H.264
//!   only; the other codecs share the same drain skeleton so
//!   duplicating the publisher + SDP plumbing here is redundant.
//! * CPU sampling. `/proc/self/stat` exposes it on Linux but the
//!   multi-threaded tokio worker noise drowns the signal; wire
//!   that through if and when a specific regression needs it.
//! * True client-side jitter / latency. A single-host loopback
//!   measurement is meaningless for jitter; that check belongs in
//!   a multi-host harness (Tier 1 follow-up).

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use bytes::{Bytes, BytesMut};
use lvqr_cmaf::{
    AudioInitParams, HevcInitParams, OpusInitParams, RawSample, VideoInitParams, build_moof_mdat,
    write_aac_init_segment, write_avc_init_segment, write_hevc_init_segment, write_opus_init_segment,
};
use lvqr_codec::hevc::HevcSps;
use lvqr_core::EventBus;
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta};
use lvqr_rtsp::RtspServer;
use lvqr_rtsp::rtp::parse_interleaved_frame;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Which codec the synthetic publisher emits and the PLAY
/// subscribers negotiate.
///
/// Every codec shares the same soak plumbing (publisher + N
/// subscribers + RTP/RTCP counters); what differs is the init
/// segment shape, the fragment body, the broadcaster track key,
/// and the RTSP control URI + interleaved channel pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Codec {
    /// H.264 video on `0.mp4` / track1 / channels 0-1.
    H264,
    /// HEVC video on `0.mp4` / track1 / channels 0-1.
    Hevc,
    /// AAC-LC audio on `1.mp4` / track2 / channels 2-3.
    Aac,
    /// Opus audio on `1.mp4` / track2 / channels 2-3.
    Opus,
}

impl Codec {
    /// Broadcaster track key the publisher emits to and the drain
    /// subscribes on. Matches the convention used everywhere else
    /// in LVQR: `0.mp4` for video, `1.mp4` for audio.
    pub fn track_key(self) -> &'static str {
        match self {
            Self::H264 | Self::Hevc => "0.mp4",
            Self::Aac | Self::Opus => "1.mp4",
        }
    }

    /// RTSP control URI suffix the subscriber SETUPs against. The
    /// DESCRIBE-rendered SDP carries these `a=control:` lines for
    /// the matching media block.
    pub fn setup_control(self) -> &'static str {
        match self {
            Self::H264 | Self::Hevc => "track1",
            Self::Aac | Self::Opus => "track2",
        }
    }

    /// Interleaved RTP / RTCP channel pair. RFC 2326 convention is
    /// RTP on an even channel and RTCP on the immediately-following
    /// odd channel.
    pub fn interleaved(self) -> (u8, u8) {
        match self {
            Self::H264 | Self::Hevc => (0, 1),
            Self::Aac | Self::Opus => (2, 3),
        }
    }

    /// Media timescale in Hz. RTP timestamps and the fMP4 `mdhd`
    /// field both tick at this rate.
    pub fn timescale(self) -> u32 {
        match self {
            Self::H264 | Self::Hevc => 90_000,
            Self::Aac => 44_100,
            Self::Opus => 48_000,
        }
    }

    /// `RawSample::track_id` the publisher stamps on samples.
    pub fn track_id(self) -> u32 {
        match self {
            Self::H264 | Self::Hevc => 1,
            Self::Aac | Self::Opus => 2,
        }
    }

    /// Codec string stamped on [`FragmentMeta::new`] and surfaced
    /// in the broadcaster metadata.
    pub fn meta_codec(self) -> &'static str {
        match self {
            Self::H264 => "avc1",
            Self::Hevc => "hev1",
            Self::Aac => "mp4a.40.2",
            Self::Opus => "opus",
        }
    }
}

/// Soak configuration. Populate with [`SoakConfig::default`] and
/// override the fields you care about; defaults target a 60-second
/// smoke that any developer laptop can run without flaking.
#[derive(Debug, Clone)]
pub struct SoakConfig {
    /// Total soak duration. Publisher stops emitting after this
    /// elapses; subscribers then shut down.
    pub duration: Duration,
    /// Concurrent RTSP PLAY subscriber count.
    pub subscribers: usize,
    /// Target fragment emission rate in fragments per second.
    /// 30 Hz mirrors a typical live broadcast cadence.
    pub fragment_hz: u32,
    /// Minimum RTP packet count per subscriber for the run to pass.
    /// A value of `None` means derive from `duration * fragment_hz`
    /// with a 20 % slack.
    pub rtp_packets_per_subscriber_min: Option<u64>,
    /// Minimum RTCP Sender Report count per subscriber for the run
    /// to pass. A value of `None` means derive from `duration /
    /// sr_interval` minus one (the first tick is skipped until the
    /// drain has recorded a packet).
    pub rtcp_packets_per_subscriber_min: Option<u64>,
    /// How often to sample resident-set-size / open-FD counts.
    pub metrics_interval: Duration,
    /// Broadcast identifier the synthetic publisher writes to.
    pub broadcast: String,
    /// Codec the synthetic publisher emits. Every codec shares the
    /// soak scaffold but swaps the init segment writer, the
    /// fragment body shape, and the RTSP transport pair.
    pub codec: Codec,
    /// Video init segment width reported in the AVC / HEVC init.
    /// Ignored for audio codecs.
    pub video_width: u16,
    /// Video init segment height reported in the AVC / HEVC init.
    /// Ignored for audio codecs.
    pub video_height: u16,
}

impl Default for SoakConfig {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(60),
            subscribers: 10,
            fragment_hz: 30,
            rtp_packets_per_subscriber_min: None,
            rtcp_packets_per_subscriber_min: None,
            metrics_interval: Duration::from_secs(5),
            broadcast: "live/soak".to_string(),
            codec: Codec::H264,
            video_width: 1280,
            video_height: 720,
        }
    }
}

/// Per-subscriber metrics collected over the soak window.
#[derive(Debug, Clone)]
pub struct SubscriberStats {
    /// Zero-based subscriber index.
    pub id: usize,
    /// Interleaved frames seen on the RTP (even) channel.
    pub rtp_packets: u64,
    /// Interleaved frames seen on the RTCP (odd) channel.
    pub rtcp_packets: u64,
    /// Total bytes of RTP + RTCP payload observed (excluding the
    /// 4-byte interleaved frame header).
    pub bytes_received: u64,
    /// Time from the PLAY response to the first RTP frame.
    pub first_rtp_after: Option<Duration>,
    /// Non-fatal error string if the subscriber terminated early.
    pub error: Option<String>,
}

/// One resource snapshot. Values are `None` on platforms where the
/// harness does not know how to read them (everything except Linux
/// today).
#[derive(Debug, Clone, Copy)]
pub struct MetricsSample {
    /// Elapsed time from the soak start.
    pub elapsed: Duration,
    /// Resident set size in bytes.
    pub rss_bytes: Option<u64>,
    /// Open file descriptor count.
    pub fd_count: Option<usize>,
    /// Cumulative user + system CPU ticks since process start,
    /// from `/proc/self/stat` (fields 14 + 15). Ticks per second
    /// are the POSIX `_SC_CLK_TCK` value -- 100 on every Linux
    /// kernel LVQR is expected to run on. `None` outside Linux or
    /// on a parse failure.
    pub cpu_ticks: Option<u64>,
}

/// Complete soak outcome. `passed` is true iff every subscriber met
/// both the RTP and RTCP thresholds AND no subscriber recorded a
/// fatal error during its PLAY handshake.
#[derive(Debug, Clone)]
pub struct SoakReport {
    /// The config the run was driven with.
    pub config: SoakConfig,
    /// Wall-clock duration of the run (may be slightly longer than
    /// `config.duration` because subscriber teardown is awaited).
    pub wall_duration: Duration,
    /// Count of fragments the synthetic publisher emitted.
    pub fragments_emitted: u64,
    /// Sorted by `id`.
    pub subscribers: Vec<SubscriberStats>,
    /// Resource samples taken at `config.metrics_interval`.
    pub metrics: Vec<MetricsSample>,
    /// True when every subscriber met the pass thresholds.
    pub passed: bool,
    /// Human-readable reason on failure.
    pub failure_reason: Option<String>,
}

impl SoakReport {
    /// Render a terminal-friendly summary.
    pub fn render_summary(&self) -> String {
        let mut out = String::new();
        out.push_str("=== LVQR soak report ===\n");
        out.push_str(&format!(
            "duration_target : {:?}\nwall_duration   : {:?}\nfragments_emit  : {}\nsubscribers     : {}\n",
            self.config.duration, self.wall_duration, self.fragments_emitted, self.config.subscribers,
        ));
        let rtp_counts: Vec<u64> = self.subscribers.iter().map(|s| s.rtp_packets).collect();
        let rtcp_counts: Vec<u64> = self.subscribers.iter().map(|s| s.rtcp_packets).collect();
        out.push_str(&format!(
            "rtp per sub     : min={} max={}\nrtcp per sub    : min={} max={}\n",
            rtp_counts.iter().min().copied().unwrap_or(0),
            rtp_counts.iter().max().copied().unwrap_or(0),
            rtcp_counts.iter().min().copied().unwrap_or(0),
            rtcp_counts.iter().max().copied().unwrap_or(0),
        ));
        if let (Some(first), Some(last)) = (self.metrics.first(), self.metrics.last()) {
            if let (Some(a), Some(b)) = (first.rss_bytes, last.rss_bytes) {
                out.push_str(&format!(
                    "rss delta       : {} -> {} bytes ({:+} kB)\n",
                    a,
                    b,
                    (b as i128 - a as i128) / 1024,
                ));
            }
            if let (Some(a), Some(b)) = (first.fd_count, last.fd_count) {
                out.push_str(&format!(
                    "fd delta        : {} -> {} ({:+})\n",
                    a,
                    b,
                    b as i128 - a as i128,
                ));
            }
            if let (Some(a), Some(b)) = (first.cpu_ticks, last.cpu_ticks) {
                let ticks_consumed = b.saturating_sub(a);
                let cpu_seconds = ticks_consumed as f64 / CLK_TCK_PER_SEC as f64;
                let wall_seconds = self.wall_duration.as_secs_f64().max(f64::EPSILON);
                out.push_str(&format!(
                    "cpu over window : {:.3} s cpu / {:.3} s wall ({:.1}% busy)\n",
                    cpu_seconds,
                    wall_seconds,
                    100.0 * cpu_seconds / wall_seconds,
                ));
            }
        }
        out.push_str(&format!(
            "passed          : {}\n",
            if self.passed { "yes" } else { "NO" }
        ));
        if let Some(r) = &self.failure_reason {
            out.push_str(&format!("failure_reason  : {r}\n"));
        }
        out
    }
}

// ---- internal helpers ----

// AVC parameter-set NALs from the lvqr-cmaf x264 corpus. A valid
// SPS + PPS pair is required so DESCRIBE's SDP renders a complete
// H.264 fmtp line.
const AVC_SPS: &[u8] = &[0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00];
const AVC_PPS: &[u8] = &[0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];

// HEVC parameter sets from the lvqr-cmaf x265 corpus (320x240
// Main 6.0 Level L60). Same bytes the play_integration.rs test
// uses; keeping a local copy avoids pulling a test-utils dep
// into the soak crate.
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

// 2-byte AAC-LC AudioSpecificConfig for 44.1 kHz stereo.
const AAC_ASC_44100_STEREO: &[u8] = &[0x12, 0x10];

fn hevc_sps_info() -> HevcSps {
    HevcSps {
        general_profile_space: 0,
        general_tier_flag: false,
        general_profile_idc: 1,
        general_profile_compatibility_flags: 0x60000000,
        general_level_idc: 60,
        chroma_format_idc: 1,
        pic_width_in_luma_samples: 320,
        pic_height_in_luma_samples: 240,
    }
}

/// Build and register the correct init segment on the registry
/// for the configured codec. The broadcaster metadata carries the
/// codec-appropriate track key + fourcc + timescale so the RTSP
/// server's DESCRIBE handler can synthesize a valid SDP even
/// before the first fragment lands.
fn setup_broadcaster(registry: &FragmentBroadcasterRegistry, config: &SoakConfig) -> Result<()> {
    let codec = config.codec;
    let mut init = BytesMut::new();
    match codec {
        Codec::H264 => {
            write_avc_init_segment(
                &mut init,
                &VideoInitParams {
                    sps: AVC_SPS.to_vec(),
                    pps: AVC_PPS.to_vec(),
                    width: config.video_width,
                    height: config.video_height,
                    timescale: codec.timescale(),
                },
            )
            .context("write avc init")?;
        }
        Codec::Hevc => {
            write_hevc_init_segment(
                &mut init,
                &HevcInitParams {
                    vps: HEVC_VPS.to_vec(),
                    sps: HEVC_SPS.to_vec(),
                    pps: HEVC_PPS.to_vec(),
                    sps_info: hevc_sps_info(),
                    timescale: codec.timescale(),
                },
            )
            .context("write hevc init")?;
        }
        Codec::Aac => {
            write_aac_init_segment(
                &mut init,
                &AudioInitParams {
                    asc: AAC_ASC_44100_STEREO.to_vec(),
                    timescale: codec.timescale(),
                },
            )
            .context("write aac init")?;
        }
        Codec::Opus => {
            write_opus_init_segment(
                &mut init,
                &OpusInitParams {
                    channel_count: 2,
                    pre_skip: 312,
                    input_sample_rate: 48_000,
                    timescale: codec.timescale(),
                },
            )
            .context("write opus init")?;
        }
    }
    let bc = registry.get_or_create(
        &config.broadcast,
        codec.track_key(),
        FragmentMeta::new(codec.meta_codec(), codec.timescale()),
    );
    bc.set_init_segment(init.freeze());
    Ok(())
}

/// Wrap a NAL unit in AVCC (4-byte length prefix + body).
fn avcc_wrap(nal: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + nal.len());
    v.extend_from_slice(&(nal.len() as u32).to_be_bytes());
    v.extend_from_slice(nal);
    v
}

/// Produce the codec-specific fMP4 sample payload for one
/// synthetic fragment.
///
/// Video codecs go into `mdat` as a length-prefixed AVCC NAL unit;
/// audio codecs go in as the raw access-unit / Opus frame bytes.
/// The contents are deliberately not decodable — the soak harness
/// only counts bytes that flow through RTP, so scrambling the
/// payload per sequence is enough to defeat any downstream dedup.
fn make_synthetic_fragment(codec: Codec, seq: u64) -> Bytes {
    match codec {
        Codec::H264 => {
            // IDR slice NAL: header 0x65 (nal_ref_idc=3, type=5) + 31 bytes.
            let mut nal = Vec::with_capacity(32);
            nal.push(0x65u8);
            nal.extend_from_slice(&seq.to_be_bytes());
            nal.extend(std::iter::repeat_n((seq & 0xFF) as u8, 23));
            Bytes::from(avcc_wrap(&nal))
        }
        Codec::Hevc => {
            // IDR_W_RADL NAL: header 0x26, 0x01 (type=19) + 30 bytes.
            let mut nal = Vec::with_capacity(32);
            nal.push(0x26u8);
            nal.push(0x01u8);
            nal.extend_from_slice(&seq.to_be_bytes());
            nal.extend(std::iter::repeat_n((seq & 0xFF) as u8, 22));
            Bytes::from(avcc_wrap(&nal))
        }
        Codec::Aac => {
            // Raw AAC access unit body: 32 synthetic bytes.
            let mut au = Vec::with_capacity(32);
            au.extend_from_slice(&seq.to_be_bytes());
            au.extend(std::iter::repeat_n((seq & 0xFF) as u8, 24));
            Bytes::from(au)
        }
        Codec::Opus => {
            // Raw Opus frame: 16 synthetic bytes. RFC 7587 treats
            // the body as opaque on the wire.
            let mut frame = Vec::with_capacity(16);
            frame.extend_from_slice(&seq.to_be_bytes());
            frame.extend(std::iter::repeat_n((seq & 0xFF) as u8, 8));
            Bytes::from(frame)
        }
    }
}

/// Fire fragments into the broadcaster at `fragment_hz` for the
/// soak duration. DTS steps are derived from the codec's timescale
/// so audio and video RTP timestamps tick at the encoder clock the
/// downstream drain expects.
///
/// Returns the total fragment count the publisher emitted.
async fn publisher_task(
    registry: FragmentBroadcasterRegistry,
    broadcast: String,
    codec: Codec,
    fragment_hz: u32,
    cancel: CancellationToken,
) -> u64 {
    let Some(bc) = registry.get(&broadcast, codec.track_key()) else {
        warn!(%broadcast, track = codec.track_key(), "publisher: broadcaster missing at start");
        return 0;
    };
    let hz = fragment_hz.max(1);
    let period = Duration::from_secs_f64(1.0 / f64::from(hz));
    let timescale = u64::from(codec.timescale());
    let dts_step = (timescale / u64::from(hz)).max(1);
    let track_id = codec.track_id();
    let track_key = codec.track_key();
    let mut seq: u64 = 0;
    let mut dts: u64 = 0;
    let mut next_deadline = tokio::time::Instant::now();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep_until(next_deadline) => {}
        }
        next_deadline += period;

        let payload = make_synthetic_fragment(codec, seq);
        let sample = RawSample {
            track_id,
            dts,
            cts_offset: 0,
            duration: dts_step as u32,
            payload,
            keyframe: true,
        };
        seq += 1;
        let moof = build_moof_mdat(seq as u32, track_id, dts, std::slice::from_ref(&sample));
        bc.emit(Fragment::new(
            track_key,
            seq,
            0,
            0,
            dts,
            dts,
            dts_step,
            FragmentFlags::KEYFRAME,
            moof,
        ));
        dts += dts_step;
    }
    seq
}

/// Run one RTSP PLAY subscriber. Performs the full DESCRIBE / SETUP
/// / PLAY handshake, then streams interleaved frames into the per-
/// subscriber counters until `cancel` fires. On any protocol error
/// the subscriber records an error string and exits.
async fn subscriber_task(
    id: usize,
    server_addr: SocketAddr,
    broadcast: String,
    codec: Codec,
    cancel: CancellationToken,
) -> SubscriberStats {
    let mut stats = SubscriberStats {
        id,
        rtp_packets: 0,
        rtcp_packets: 0,
        bytes_received: 0,
        first_rtp_after: None,
        error: None,
    };
    match subscribe_and_read(id, server_addr, &broadcast, codec, &mut stats, cancel).await {
        Ok(()) => {}
        Err(e) => {
            stats.error = Some(e.to_string());
            warn!(subscriber = id, error = %e, "subscriber terminated with error");
        }
    }
    stats
}

async fn subscribe_and_read(
    id: usize,
    server_addr: SocketAddr,
    broadcast: &str,
    codec: Codec,
    stats: &mut SubscriberStats,
    cancel: CancellationToken,
) -> Result<()> {
    let mut stream = TcpStream::connect(server_addr).await.context("connect")?;
    let base_uri = format!("rtsp://{server_addr}/{broadcast}");
    let mut pending = Vec::<u8>::new();

    // DESCRIBE -> pulls the SDP; the body is drained so later
    // interleaved-frame parsing starts at a clean offset.
    let describe = format!("DESCRIBE {base_uri} RTSP/1.0\r\nCSeq: 1\r\n\r\n");
    let describe_resp = read_response_headers(&mut stream, describe.as_bytes(), &mut pending)
        .await
        .context("DESCRIBE")?;
    if !describe_resp.contains("RTSP/1.0 200") {
        return Err(anyhow!("DESCRIBE not 200: {describe_resp}"));
    }
    let content_length = parse_content_length(&describe_resp).unwrap_or(0);
    if content_length > 0 {
        drain_body(&mut stream, &mut pending, content_length).await?;
    }

    // SETUP on the codec's canonical control URI + interleaved pair
    // (0-1 for video on track1, 2-3 for audio on track2).
    let (rtp_ch, rtcp_ch) = codec.interleaved();
    let setup = format!(
        "SETUP {base_uri}/{control} RTSP/1.0\r\nCSeq: 2\r\nTransport: RTP/AVP/TCP;unicast;interleaved={rtp_ch}-{rtcp_ch}\r\n\r\n",
        control = codec.setup_control(),
    );
    let setup_resp = read_response_headers(&mut stream, setup.as_bytes(), &mut pending)
        .await
        .context("SETUP")?;
    let session_id = extract_session_header(&setup_resp).context("SETUP missing Session header")?;

    // PLAY kicks off the server's drain and SR timer.
    let play = format!("PLAY {base_uri} RTSP/1.0\r\nCSeq: 3\r\nSession: {session_id}\r\n\r\n");
    let play_resp = read_response_headers(&mut stream, play.as_bytes(), &mut pending)
        .await
        .context("PLAY")?;
    if !play_resp.contains("RTSP/1.0 200") {
        return Err(anyhow!("PLAY not 200"));
    }
    let play_start = Instant::now();

    // Interleaved frame loop.
    let mut read_buf = [0u8; 8192];
    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Consume any complete frames already sitting in `pending`.
        while let Some((frame, consumed)) = parse_interleaved_frame(&pending) {
            stats.bytes_received += frame.payload.len() as u64;
            if frame.channel % 2 == 0 {
                stats.rtp_packets += 1;
                if stats.first_rtp_after.is_none() {
                    stats.first_rtp_after = Some(play_start.elapsed());
                }
            } else {
                stats.rtcp_packets += 1;
            }
            pending.drain(..consumed);
        }

        // Read more bytes. Wrap in a select so cancel unblocks us.
        let read = tokio::select! {
            _ = cancel.cancelled() => break,
            r = stream.read(&mut read_buf) => r,
        };
        match read {
            Ok(0) => {
                debug!(subscriber = id, "peer closed socket");
                break;
            }
            Ok(n) => {
                pending.extend_from_slice(&read_buf[..n]);
            }
            Err(e) => return Err(anyhow!("read: {e}")),
        }
    }
    Ok(())
}

async fn read_response_headers(stream: &mut TcpStream, req: &[u8], buf: &mut Vec<u8>) -> Result<String> {
    stream.write_all(req).await.context("write request")?;
    let mut scan_from = 0;
    let headers_end = loop {
        if let Some(pos) = find_crlf_crlf(buf, scan_from) {
            break pos;
        }
        let scratch_start = buf.len();
        buf.resize(scratch_start + 4096, 0);
        let n = stream.read(&mut buf[scratch_start..]).await.context("header read")?;
        buf.truncate(scratch_start + n);
        if n == 0 {
            return Err(anyhow!("socket closed before response headers terminated"));
        }
        scan_from = scratch_start.saturating_sub(3);
    };
    let headers = String::from_utf8_lossy(&buf[..headers_end]).into_owned();
    buf.drain(..headers_end + 4);
    Ok(headers)
}

fn find_crlf_crlf(haystack: &[u8], from: usize) -> Option<usize> {
    haystack
        .windows(4)
        .skip(from)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| from + p)
}

fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        if let Some(v) = line.strip_prefix("Content-Length:") {
            return v.trim().parse().ok();
        }
    }
    None
}

async fn drain_body(stream: &mut TcpStream, pending: &mut Vec<u8>, n: usize) -> Result<()> {
    while pending.len() < n {
        let scratch_start = pending.len();
        pending.resize(scratch_start + 4096, 0);
        let got = stream.read(&mut pending[scratch_start..]).await.context("body read")?;
        pending.truncate(scratch_start + got);
        if got == 0 {
            return Err(anyhow!("socket closed before body complete"));
        }
    }
    pending.drain(..n);
    Ok(())
}

fn extract_session_header(headers: &str) -> Option<String> {
    for line in headers.lines() {
        if let Some(v) = line.strip_prefix("Session:") {
            return Some(v.trim().split(';').next()?.trim().to_string());
        }
    }
    None
}

/// Read `/proc/self/statm` resident-set-size in bytes on Linux.
/// Returns `None` on other platforms or if the file is missing.
fn read_rss_bytes() -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let raw = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = raw.split_whitespace().nth(1)?.parse().ok()?;
    let page_size = page_size();
    Some(resident_pages * page_size)
}

/// Count entries in `/proc/self/fd` on Linux. `None` elsewhere.
fn read_fd_count() -> Option<usize> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let entries = std::fs::read_dir(Path::new("/proc/self/fd")).ok()?;
    Some(entries.count())
}

fn page_size() -> u64 {
    // Conservative: 4 KiB is the de-facto page size on Linux x86_64
    // and aarch64. Reading the real value via sysconf requires libc
    // which this crate otherwise avoids; 4096 is good enough for a
    // soak-level RSS sample (the signal is growth, not absolute
    // value).
    4096
}

/// Cumulative user + system CPU ticks for the current process.
///
/// Reads `/proc/self/stat`, locates the last `)` to skip past the
/// `comm` field (which may itself contain spaces or parens), then
/// pulls `utime` (field 14) and `stime` (field 15) per `proc(5)`.
///
/// Returns `None` on non-Linux targets or on any parse failure. A
/// delta between two samples divided by [`CLK_TCK_PER_SEC`] yields
/// the CPU seconds consumed across the window.
fn read_cpu_ticks() -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let raw = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rparen = raw.rfind(')')?;
    let tail = &raw[rparen + 1..];
    let fields: Vec<&str> = tail.split_whitespace().collect();
    // `tail` starts with the state char; index 11 = utime, 12 = stime.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

/// POSIX `_SC_CLK_TCK` for the Linux kernel. Hard-coded 100 so the
/// soak crate avoids a `libc` dependency. Kernels with
/// `CONFIG_HZ_250` or higher still expose 100 at the sysconf layer
/// (userspace-visible clock ticks are decoupled from jiffies).
pub const CLK_TCK_PER_SEC: u64 = 100;

// ---- public entry point ----

/// Run the soak as described in `config`. Returns a [`SoakReport`]
/// whose `passed` field captures the verdict; the function itself
/// only returns `Err` for setup failures (bind, init writer), not
/// for threshold failures.
pub async fn run_soak(config: SoakConfig) -> Result<SoakReport> {
    let start_wall = Instant::now();
    let registry = FragmentBroadcasterRegistry::new();
    setup_broadcaster(&registry, &config)?;

    let mut server = RtspServer::with_registry("127.0.0.1:0".parse().unwrap(), registry.clone());
    let server_addr = server.bind().await.context("bind RTSP server")?;
    let server_cancel = CancellationToken::new();
    let server_shutdown = server_cancel.clone();
    let events = EventBus::with_capacity(64);
    let server_handle = tokio::spawn(async move {
        server.run(events, server_shutdown).await.ok();
    });

    // Per-subsystem cancel token shared by publisher + subscribers +
    // metrics. Cancelling this ends the soak gracefully; server
    // shutdown is deferred until subscribers detach so their reads
    // see peer close rather than connection reset.
    let cancel = CancellationToken::new();
    let publisher_handle = tokio::spawn(publisher_task(
        registry.clone(),
        config.broadcast.clone(),
        config.codec,
        config.fragment_hz,
        cancel.clone(),
    ));

    let mut subscriber_handles = Vec::with_capacity(config.subscribers);
    for id in 0..config.subscribers {
        let handle = tokio::spawn(subscriber_task(
            id,
            server_addr,
            config.broadcast.clone(),
            config.codec,
            cancel.clone(),
        ));
        subscriber_handles.push(handle);
    }

    let metrics = Arc::new(tokio::sync::Mutex::new(Vec::<MetricsSample>::new()));
    let metrics_cancel = cancel.clone();
    let metrics_interval = config.metrics_interval;
    let metrics_collector = metrics.clone();
    let metrics_stop = Arc::new(AtomicBool::new(false));
    let metrics_stop_flag = metrics_stop.clone();
    let metrics_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(metrics_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let run_start = Instant::now();
        // Initial sample at t=0.
        metrics_collector.lock().await.push(MetricsSample {
            elapsed: Duration::ZERO,
            rss_bytes: read_rss_bytes(),
            fd_count: read_fd_count(),
            cpu_ticks: read_cpu_ticks(),
        });
        ticker.tick().await; // drop the immediate tick; sample again at t+interval
        loop {
            if metrics_stop_flag.load(Ordering::SeqCst) {
                break;
            }
            tokio::select! {
                _ = metrics_cancel.cancelled() => break,
                _ = ticker.tick() => {
                    metrics_collector.lock().await.push(MetricsSample {
                        elapsed: run_start.elapsed(),
                        rss_bytes: read_rss_bytes(),
                        fd_count: read_fd_count(),
                        cpu_ticks: read_cpu_ticks(),
                    });
                }
            }
        }
    });

    info!(
        duration = ?config.duration,
        subscribers = config.subscribers,
        fragment_hz = config.fragment_hz,
        %server_addr,
        "soak started"
    );

    // Run the soak.
    tokio::time::sleep(config.duration).await;

    // Cancel publisher + subscribers; join everything.
    cancel.cancel();
    let fragments_emitted = publisher_handle.await.unwrap_or(0);
    let mut subscribers: Vec<SubscriberStats> = Vec::with_capacity(subscriber_handles.len());
    for handle in subscriber_handles {
        subscribers.push(handle.await.unwrap_or_else(|e| SubscriberStats {
            id: usize::MAX,
            rtp_packets: 0,
            rtcp_packets: 0,
            bytes_received: 0,
            first_rtp_after: None,
            error: Some(format!("join: {e}")),
        }));
    }
    subscribers.sort_by_key(|s| s.id);

    // Final metrics sample + stop the collector.
    metrics.lock().await.push(MetricsSample {
        elapsed: start_wall.elapsed(),
        rss_bytes: read_rss_bytes(),
        fd_count: read_fd_count(),
        cpu_ticks: read_cpu_ticks(),
    });
    metrics_stop.store(true, Ordering::SeqCst);
    let _ = metrics_handle.await;

    // Shutdown server last so subscribers finished their reads first.
    server_cancel.cancel();
    let _ = server_handle.await;

    let wall_duration = start_wall.elapsed();
    let metrics_out = {
        let guard = metrics.lock().await;
        guard.clone()
    };

    let rtp_floor = config.rtp_packets_per_subscriber_min.unwrap_or_else(|| {
        // At 30 Hz for 60 s we expect ~1800 RTP packets per sub. Allow
        // a 20 % slack for startup delay + drain warmup.
        let secs = config.duration.as_secs_f64();
        let expected = secs * f64::from(config.fragment_hz);
        (expected * 0.8).max(1.0) as u64
    });
    let rtcp_floor = config.rtcp_packets_per_subscriber_min.unwrap_or_else(|| {
        // SR cadence is 5 s (DEFAULT_SR_INTERVAL). First tick is at
        // start + 5 s; minus one for safety on short soaks.
        let window = config.duration.as_secs_f64();
        let expected = (window / 5.0).floor() as u64;
        expected.saturating_sub(1)
    });

    let mut failures: Vec<String> = Vec::new();
    for s in &subscribers {
        if let Some(err) = &s.error {
            failures.push(format!("sub {}: {err}", s.id));
        } else {
            if s.rtp_packets < rtp_floor {
                failures.push(format!("sub {}: rtp {} < floor {}", s.id, s.rtp_packets, rtp_floor));
            }
            if s.rtcp_packets < rtcp_floor {
                failures.push(format!("sub {}: rtcp {} < floor {}", s.id, s.rtcp_packets, rtcp_floor));
            }
        }
    }

    let passed = failures.is_empty();
    let failure_reason = if failures.is_empty() {
        None
    } else {
        Some(failures.join("; "))
    };

    Ok(SoakReport {
        config,
        wall_duration,
        fragments_emitted,
        subscribers,
        metrics: metrics_out,
        passed,
        failure_reason,
    })
}

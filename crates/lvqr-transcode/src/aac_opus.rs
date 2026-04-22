//! [`AacToOpusEncoder`] + [`AacToOpusEncoderFactory`].
//!
//! Real GStreamer AAC-to-Opus audio transcoder landed in session 113
//! behind the `transcode` Cargo feature. Sibling to
//! [`crate::SoftwareTranscoder`]: same worker-thread pattern, same
//! element-probe opt-out, same `sync_channel(64)` back-pressure.
//!
//! Pipeline shape:
//!
//! ```text
//! appsrc name=src is-live=true format=time do-timestamp=true
//!         caps=audio/mpeg,mpegversion=4,stream-format=adts
//!   ! aacparse
//!   ! avdec_aac
//!   ! audioconvert
//!   ! audioresample
//!   ! audio/x-raw,format=S16LE,rate=48000,channels=2,layout=interleaved
//!   ! opusenc bitrate=64000 frame-size=20
//!   ! appsink name=sink emit-signals=true sync=false
//! ```
//!
//! Input framing: raw AAC access units (one access unit per
//! [`AacToOpusEncoder::push`] call) wrapped with an ADTS header
//! derived from the [`AacAudioConfig`] handed in at build time. ADTS
//! is used rather than the `stream-format=raw,codec_data=<asc>`
//! caps variant because `aacparse` is more forgiving about the ADTS
//! path when individual input buffers are single access units; the
//! raw-codec_data path can require the caller to provide aligned
//! Access-Unit groups to trigger output.
//!
//! Output: `opusenc` emits one Opus packet per 20 ms frame (960
//! samples at 48 kHz). Every packet is pushed through the caller-
//! supplied `opus_tx` channel as an [`OpusFrame`]; the caller's
//! tokio task (the WHEP session poll loop, typically) drains the
//! receiver arm and writes each frame into the negotiated Opus
//! `str0m::Pt`.
//!
//! Why `avdec_aac` and not `faad`: `faad` lives in
//! `gst-plugins-bad` and is LGPL-tainted; `avdec_aac` lives in
//! `gst-libav` which is already a session-105 dependency. Host
//! installs that provisioned GStreamer for the 105 video pipeline
//! inherit the AAC decoder for free.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread;

use bytes::Bytes;
use glib::object::Cast;
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use tracing::{debug, info, warn};

/// GStreamer elements required by the AAC-to-Opus pipeline. Probed
/// at factory construction; missing elements cause the factory to
/// opt out of every `build()` call rather than panicking, so
/// deployments without `gst-libav` or `gst-plugins-base` stay
/// functional for the rest of WHEP.
const REQUIRED_ELEMENTS: &[&str] = &[
    "appsrc",
    "aacparse",
    "avdec_aac",
    "audioconvert",
    "audioresample",
    "opusenc",
    "appsink",
];

/// Opus wire clock. WebRTC Opus is always negotiated at 48 kHz.
const OPUS_CLOCK_RATE: u32 = 48_000;

/// Opus wire channel count. Stereo is the WebRTC baseline.
const OPUS_CHANNELS: u32 = 2;

/// Opus frame duration in milliseconds. 20 ms is the WebRTC default
/// and matches `opusenc`'s default frame-size property.
const OPUS_FRAME_MS: u32 = 20;

/// Opus frame duration in 48 kHz samples (ticks the downstream WHEP
/// writer uses as the `MediaTime` numerator).
const OPUS_FRAME_TICKS: u32 = OPUS_CLOCK_RATE / 1000 * OPUS_FRAME_MS;

/// Bounded mpsc depth between `push` and the worker. Matches the
/// session 105 value; full-channel pushes drop the AAC access unit
/// with a warn log.
const WORKER_QUEUE_DEPTH: usize = 64;

/// Shutdown grace for the worker after EOS. Same 5 s window the
/// video pipeline uses.
const SHUTDOWN_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(5);

/// ADTS sampling-frequency table index -> Hz. Used in reverse to
/// derive the sampling_frequency_index field from a known sample
/// rate. Matches ISO/IEC 14496-3 Table 1.16.
const ADTS_SAMPLE_RATES: &[u32] = &[
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000, 7_350,
];

/// Parsed AAC `AudioSpecificConfig` fields needed to synthesise ADTS
/// headers and to document the input stream format. Produced by the
/// RTMP bridge on each FLV audio sequence header and forwarded to
/// the WHEP session via the `on_audio_config` observer hook.
///
/// The raw `asc` bytes are preserved so the caller can re-serialise
/// the config verbatim into an MP4 `esds` box or a GStreamer caps
/// `codec_data` buffer; the explicit `sample_rate` / `channels` /
/// `object_type` fields avoid a second parse step inside the
/// transcoder.
#[derive(Debug, Clone)]
pub struct AacAudioConfig {
    /// Raw AudioSpecificConfig bytes (typically 2 bytes for AAC-LC).
    pub asc: Bytes,
    /// Audio sample rate in Hz, e.g. 44_100 or 48_000.
    pub sample_rate: u32,
    /// Channel count, e.g. 2 for stereo.
    pub channels: u8,
    /// AAC audio object type (2 = AAC-LC, 5 = HE-AAC v1, 29 = HE-AAC v2).
    pub object_type: u8,
}

/// One Opus packet out of `opusenc`. Carries the wire payload plus
/// the decode timestamp + duration in 48 kHz ticks so the caller can
/// build a `str0m::MediaTime` without having to track an internal
/// clock.
#[derive(Debug, Clone)]
pub struct OpusFrame {
    /// Opaque Opus packet bytes, ready to hand to
    /// `str0m::Writer::write` under the negotiated Opus `Pt`.
    pub payload: Bytes,
    /// Decode timestamp in 48 kHz ticks, monotonic per encoder
    /// instance. Starts at the first input sample's dts (converted
    /// to 48 kHz).
    pub dts: u64,
    /// Frame duration in 48 kHz ticks. 960 for the standard 20 ms
    /// Opus frame.
    pub duration_ticks: u32,
}

/// Factory that probes the required GStreamer elements once and
/// builds [`AacToOpusEncoder`] instances per WHEP session.
///
/// A single factory is constructed at the CLI composition root and
/// shared across every `Str0mAnswerer` session. Element probing is
/// idempotent via `gst::init()`; missing elements are logged once
/// at factory construction and every `build()` call returns `None`.
pub struct AacToOpusEncoderFactory {
    missing_elements: Vec<&'static str>,
}

impl Default for AacToOpusEncoderFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl AacToOpusEncoderFactory {
    /// Construct a factory. `gst::init()` is called (idempotent); if
    /// it fails or any required element is absent, the factory
    /// records the shortfall and every `build()` returns `None`.
    pub fn new() -> Self {
        let missing_elements = match gst::init() {
            Ok(()) => missing_required_elements(),
            Err(err) => {
                warn!(error = %err, "AacToOpusEncoderFactory: gst::init() failed");
                REQUIRED_ELEMENTS.to_vec()
            }
        };
        if !missing_elements.is_empty() {
            warn!(
                missing = ?missing_elements,
                "AacToOpusEncoderFactory: required GStreamer elements absent; factory will opt out of every build()",
            );
        }
        Self { missing_elements }
    }

    /// `true` when every required GStreamer element was found.
    pub fn is_available(&self) -> bool {
        self.missing_elements.is_empty()
    }

    /// Snapshot of missing elements. Exposed for integration tests
    /// that want to skip cleanly when the host lacks the plugin set.
    pub fn missing_elements(&self) -> &[&'static str] {
        &self.missing_elements
    }

    /// Build a fresh encoder for one WHEP session.
    ///
    /// `config` carries the AAC AudioSpecificConfig fields needed to
    /// synthesise ADTS headers on every input sample. `opus_tx` is
    /// the channel the worker pushes Opus frames into; the caller's
    /// poll loop drains the receiver and writes each frame through
    /// `str0m::Writer::write`.
    ///
    /// Returns `None` when the factory is unavailable (missing
    /// GStreamer elements) or when the pipeline fails to parse /
    /// downcast (logged at `warn`).
    pub fn build(
        &self,
        config: AacAudioConfig,
        opus_tx: tokio::sync::mpsc::UnboundedSender<OpusFrame>,
    ) -> Option<AacToOpusEncoder> {
        if !self.is_available() {
            return None;
        }
        match AacToOpusEncoder::spawn(config, opus_tx) {
            Ok(enc) => Some(enc),
            Err(err) => {
                warn!(error = %err, "AacToOpusEncoderFactory: encoder spawn failed");
                None
            }
        }
    }
}

/// Per-WHEP-session AAC-to-Opus encoder. Owns a worker thread + a
/// GStreamer pipeline; the public surface is `push` (drop AAC bytes
/// in) + `Drop` (tear the worker down).
///
/// Lifecycle mirrors [`crate::SoftwareTranscoder`]'s worker: one
/// `std::thread` that owns the whole `gst::Pipeline`; one
/// `sync_channel` between the push site and the worker; EOS on the
/// sender drop side; bounded drain with a 5 s timeout on shutdown.
pub struct AacToOpusEncoder {
    tx: Option<SyncSender<AacInput>>,
    join: Option<thread::JoinHandle<()>>,
    config: AacAudioConfig,
    dropped: Arc<AtomicU64>,
}

/// One AAC access unit on its way to the encoder worker.
struct AacInput {
    payload: Bytes,
    dts_48k: u64,
}

impl AacToOpusEncoder {
    fn spawn(
        config: AacAudioConfig,
        opus_tx: tokio::sync::mpsc::UnboundedSender<OpusFrame>,
    ) -> Result<Self, EncoderSpawnError> {
        let pipeline = build_pipeline()?;
        attach_output_callback(&pipeline.appsink, opus_tx);

        let (tx, rx) = sync_channel::<AacInput>(WORKER_QUEUE_DEPTH);
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_for_thread = Arc::clone(&dropped);
        let config_for_thread = config.clone();

        let join = thread::Builder::new()
            .name("lvqr-transcode:aac-opus".into())
            .spawn(move || {
                run_worker(pipeline, rx, dropped_for_thread, config_for_thread);
            })
            .map_err(|e| EncoderSpawnError::ThreadSpawn(e.to_string()))?;

        Ok(Self {
            tx: Some(tx),
            join: Some(join),
            config,
            dropped,
        })
    }

    /// Push one raw AAC access unit into the encoder. `dts_ticks` is
    /// the sample's decode timestamp in the AAC source timescale
    /// (the sample rate carried on [`AacAudioConfig`]); the worker
    /// converts to 48 kHz for the Opus output.
    ///
    /// Non-blocking: a full worker channel drops the sample and
    /// increments the dropped counter. The session poll loop that
    /// owns this encoder sees no back-pressure, and the subscribed
    /// WHEP client hears a momentary gap instead of the RTP stream
    /// stalling.
    pub fn push(&self, aac_sample: &[u8], dts_ticks: u64) {
        let Some(tx) = self.tx.as_ref() else {
            return;
        };
        let dts_48k = scale_to_48k(dts_ticks, self.config.sample_rate);
        let input = AacInput {
            payload: Bytes::copy_from_slice(aac_sample),
            dts_48k,
        };
        match tx.try_send(input) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                metrics::counter!(
                    "lvqr_transcode_dropped_fragments_total",
                    "transcoder" => "aac_opus",
                    "rendition" => "audio",
                )
                .increment(1);
                warn!("AacToOpusEncoder: worker channel full; dropping AAC access unit");
            }
            Err(TrySendError::Disconnected(_)) => {
                debug!("AacToOpusEncoder: worker shut down; AAC access unit discarded");
            }
        }
    }

    /// Fragments dropped on a full worker channel. Test-facing.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// AAC input audio config the encoder was built with.
    pub fn config(&self) -> &AacAudioConfig {
        &self.config
    }
}

impl Drop for AacToOpusEncoder {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(join) = self.join.take()
            && let Err(p) = join.join()
        {
            warn!(payload = ?p, "AacToOpusEncoder: worker thread panicked during shutdown");
        }
    }
}

/// Errors that can surface when spawning the worker thread.
#[derive(Debug, thiserror::Error)]
enum EncoderSpawnError {
    #[error("gstreamer pipeline parse or downcast failed: {0}")]
    Pipeline(String),

    #[error("worker thread spawn failed: {0}")]
    ThreadSpawn(String),
}

/// Built pipeline handle: gst::Pipeline plus downcast appsrc /
/// appsink ends so the worker thread does not re-query by name.
struct BuiltPipeline {
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    appsink: gst_app::AppSink,
}

fn build_pipeline() -> Result<BuiltPipeline, EncoderSpawnError> {
    // ADTS-framed input drops the need for an explicit codec_data
    // on appsrc caps; aacparse strips the ADTS header and hands
    // raw AAC access units to avdec_aac. opusenc's default
    // frame-size (20 ms) + default bitrate (inputs-matched) is
    // fine for WebRTC-grade Opus.
    let pipeline_str = format!(
        "appsrc name=src is-live=true format=time do-timestamp=true \
         caps=audio/mpeg,mpegversion=(int)4,stream-format=(string)adts \
         ! aacparse \
         ! avdec_aac \
         ! audioconvert \
         ! audioresample \
         ! audio/x-raw,format=(string)S16LE,rate=(int){rate},channels=(int){channels},layout=(string)interleaved \
         ! opusenc bitrate=64000 frame-size={frame_ms} \
         ! appsink name=sink emit-signals=true sync=false",
        rate = OPUS_CLOCK_RATE,
        channels = OPUS_CHANNELS,
        frame_ms = OPUS_FRAME_MS,
    );
    let element = gst::parse::launch(&pipeline_str).map_err(|e| EncoderSpawnError::Pipeline(e.to_string()))?;
    let pipeline = element
        .downcast::<gst::Pipeline>()
        .map_err(|_| EncoderSpawnError::Pipeline("parse_launch result is not a pipeline".into()))?;

    let appsrc_elem = pipeline
        .by_name("src")
        .ok_or_else(|| EncoderSpawnError::Pipeline("appsrc element 'src' not found".into()))?;
    let appsrc = appsrc_elem
        .downcast::<gst_app::AppSrc>()
        .map_err(|_| EncoderSpawnError::Pipeline("'src' is not an AppSrc".into()))?;
    appsrc.set_max_bytes(256 * 1024);
    appsrc.set_property("block", false);

    let appsink_elem = pipeline
        .by_name("sink")
        .ok_or_else(|| EncoderSpawnError::Pipeline("appsink element 'sink' not found".into()))?;
    let appsink = appsink_elem
        .downcast::<gst_app::AppSink>()
        .map_err(|_| EncoderSpawnError::Pipeline("'sink' is not an AppSink".into()))?;
    appsink.set_sync(false);

    Ok(BuiltPipeline {
        pipeline,
        appsrc,
        appsink,
    })
}

fn attach_output_callback(appsink: &gst_app::AppSink, opus_tx: tokio::sync::mpsc::UnboundedSender<OpusFrame>) {
    let running_dts = Arc::new(AtomicU64::new(0));
    let tx_for_cb = opus_tx;
    let dts_cursor = Arc::clone(&running_dts);

    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = match sink.pull_sample() {
                    Ok(s) => s,
                    Err(_) => return Err(gst::FlowError::Eos),
                };
                let Some(buffer) = sample.buffer() else {
                    return Err(gst::FlowError::Error);
                };
                // Skip header buffers (the Opus stream header set
                // `opusenc` emits before the first payload). str0m
                // expects opaque Opus packets on the wire, not the
                // out-of-band OpusHead / OpusTags headers.
                if buffer.flags().contains(gst::BufferFlags::HEADER) {
                    return Ok(gst::FlowSuccess::Ok);
                }
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let payload = Bytes::copy_from_slice(map.as_slice());
                drop(map);
                if payload.is_empty() {
                    return Ok(gst::FlowSuccess::Ok);
                }
                // opusenc's pts is in the pipeline's clock domain
                // (running-time). Prefer it when available; fall
                // back to an internally-accumulated cursor so a
                // pipeline that somehow omits pts still produces
                // monotonic dts stamps.
                let dts = buffer
                    .pts()
                    .map(|t| ns_to_ticks(t.nseconds(), OPUS_CLOCK_RATE))
                    .unwrap_or_else(|| dts_cursor.fetch_add(OPUS_FRAME_TICKS as u64, Ordering::Relaxed));
                let frame = OpusFrame {
                    payload: payload.clone(),
                    dts,
                    duration_ticks: OPUS_FRAME_TICKS,
                };
                if tx_for_cb.send(frame).is_err() {
                    // Receiver dropped (session ended); tear down
                    // the pipeline on the next pull_sample.
                    return Err(gst::FlowError::Eos);
                }
                metrics::counter!(
                    "lvqr_transcode_output_fragments_total",
                    "transcoder" => "aac_opus",
                    "rendition" => "audio",
                )
                .increment(1);
                metrics::counter!(
                    "lvqr_transcode_output_bytes_total",
                    "transcoder" => "aac_opus",
                    "rendition" => "audio",
                )
                .increment(payload.len() as u64);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
}

fn run_worker(built: BuiltPipeline, rx: Receiver<AacInput>, dropped: Arc<AtomicU64>, config: AacAudioConfig) {
    let BuiltPipeline {
        pipeline,
        appsrc,
        appsink: _,
    } = built;

    if let Err(err) = pipeline.set_state(gst::State::Playing) {
        warn!(error = %err, "AacToOpusEncoder: failed to set pipeline to Playing");
        let _ = pipeline.set_state(gst::State::Null);
        return;
    }

    while let Ok(input) = rx.recv() {
        let adts = wrap_adts(&input.payload, &config);
        let pts_ns = ticks_to_ns(input.dts_48k, OPUS_CLOCK_RATE);
        let mut buffer = gst::Buffer::from_slice(adts);
        if let Some(buf_ref) = buffer.get_mut() {
            buf_ref.set_pts(gst::ClockTime::from_nseconds(pts_ns));
            buf_ref.set_dts(gst::ClockTime::from_nseconds(pts_ns));
        }
        match appsrc.push_buffer(buffer) {
            Ok(_) => {}
            Err(gst::FlowError::Flushing) | Err(gst::FlowError::Eos) => break,
            Err(err) => {
                warn!(error = ?err, "AacToOpusEncoder: appsrc.push_buffer failed");
                break;
            }
        }
    }

    if let Err(err) = appsrc.end_of_stream() {
        warn!(error = %err, "AacToOpusEncoder: end_of_stream signal failed");
    }

    wait_for_drain(&pipeline);

    if let Err(err) = pipeline.set_state(gst::State::Null) {
        warn!(error = %err, "AacToOpusEncoder: failed to set pipeline to Null");
    }

    info!(
        dropped = dropped.load(Ordering::Relaxed),
        "AacToOpusEncoder: worker exited",
    );
}

fn wait_for_drain(pipeline: &gst::Pipeline) {
    let Some(bus) = pipeline.bus() else {
        return;
    };
    let types = [gst::MessageType::Eos, gst::MessageType::Error];
    let Some(msg) = bus.timed_pop_filtered(Some(SHUTDOWN_TIMEOUT), &types) else {
        warn!(
            timeout_s = SHUTDOWN_TIMEOUT.seconds(),
            "AacToOpusEncoder: pipeline did not drain within timeout; forcing Null",
        );
        return;
    };
    if let gst::MessageView::Error(err) = msg.view() {
        warn!(
            error = %err.error(),
            debug = ?err.debug(),
            "AacToOpusEncoder: pipeline reported error during drain",
        );
    }
}

fn missing_required_elements() -> Vec<&'static str> {
    REQUIRED_ELEMENTS
        .iter()
        .copied()
        .filter(|name| gst::ElementFactory::find(name).is_none())
        .collect()
}

/// Wrap a raw AAC access unit in a 7-byte ADTS header derived from
/// the source [`AacAudioConfig`]. ADTS layout per ISO/IEC 13818-7
/// section 6.2.
///
/// The header is MPEG-4 (`ID=0`), with `protection_absent=1` (no
/// CRC), `profile=object_type-1` (AAC-LC -> profile 1), and
/// `home=original=private=0`. `aac_frame_length` counts the 7-byte
/// header plus the access unit payload.
fn wrap_adts(aac: &[u8], config: &AacAudioConfig) -> Bytes {
    let freq_idx = sample_rate_to_adts_index(config.sample_rate);
    let channel_config = config.channels.min(7);
    let profile = config.object_type.saturating_sub(1).min(3);
    let total_len = aac.len() + 7;

    let mut header = [0u8; 7];
    header[0] = 0xFF;
    header[1] = 0xF1; // syncword[3..0]=1111 | mpeg4(0) | layer(00) | protection_absent(1)
    header[2] = ((profile & 0x03) << 6) | ((freq_idx & 0x0F) << 2) | ((channel_config & 0x04) >> 2);
    header[3] = ((channel_config & 0x03) << 6) | (((total_len >> 11) & 0x03) as u8);
    header[4] = ((total_len >> 3) & 0xFF) as u8;
    header[5] = (((total_len & 0x07) << 5) as u8) | 0x1F; // buffer_fullness bits 10..7 = 11111
    header[6] = 0xFC; // buffer_fullness bits 6..0 = 1111100 | number_of_raw_data_blocks(0)

    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(&header);
    out.extend_from_slice(aac);
    Bytes::from(out)
}

fn sample_rate_to_adts_index(rate: u32) -> u8 {
    ADTS_SAMPLE_RATES
        .iter()
        .position(|&r| r == rate)
        .map(|i| i as u8)
        .unwrap_or(4) // default to 44.1 kHz if the rate is outside the ADTS table
}

fn scale_to_48k(ticks: u64, from_rate: u32) -> u64 {
    if from_rate == OPUS_CLOCK_RATE {
        return ticks;
    }
    ticks.saturating_mul(OPUS_CLOCK_RATE as u64) / from_rate.max(1) as u64
}

fn ns_to_ticks(ns: u64, rate: u32) -> u64 {
    ns.saturating_mul(rate as u64) / 1_000_000_000u64
}

fn ticks_to_ns(ticks: u64, rate: u32) -> u64 {
    ticks.saturating_mul(1_000_000_000u64) / rate.max(1) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adts_header_matches_layout_for_aac_lc_44100_stereo() {
        // AAC-LC (object_type=2, profile=1 in ADTS), 44.1 kHz
        // (freq_idx=4), stereo (channel_config=2). Known-shape
        // assertion: the 7-byte header bytes read back as the
        // documented fields, and the aac_frame_length field
        // matches the wrapped-length math (7 + payload).
        let config = AacAudioConfig {
            asc: Bytes::from_static(&[0x12, 0x10]),
            sample_rate: 44_100,
            channels: 2,
            object_type: 2,
        };
        let payload = vec![0xAA; 200];
        let wrapped = wrap_adts(&payload, &config);
        assert_eq!(wrapped.len(), payload.len() + 7);
        assert_eq!(wrapped[0], 0xFF);
        assert_eq!(
            wrapped[1] & 0xF6,
            0xF0,
            "syncword top-nibble + id/layer/protection bits: got {:08b}",
            wrapped[1]
        );
        // profile=1 -> top 2 bits of byte 2 = 01
        assert_eq!((wrapped[2] >> 6) & 0x03, 1, "ADTS profile for AAC-LC");
        // freq_idx=4 in bits 5..2 of byte 2
        assert_eq!(
            (wrapped[2] >> 2) & 0x0F,
            4,
            "ADTS sampling_frequency_index for 44.1 kHz"
        );
        // channel_config=2 spread across byte 2 bit 0 (MSB of 3-bit field) and byte 3 bits 7..6 (bottom 2)
        let chan_hi = wrapped[2] & 0x01;
        let chan_lo = (wrapped[3] >> 6) & 0x03;
        let chan = (chan_hi << 2) | chan_lo;
        assert_eq!(chan, 2, "ADTS channel_configuration for stereo");
        // aac_frame_length is 13 bits across bytes 3..5
        let frame_len = (((wrapped[3] & 0x03) as u32) << 11) | ((wrapped[5] as u32) >> 5) | ((wrapped[4] as u32) << 3);
        assert_eq!(frame_len, (payload.len() + 7) as u32);
    }

    #[test]
    fn sample_rate_to_adts_index_known_values() {
        assert_eq!(sample_rate_to_adts_index(44_100), 4);
        assert_eq!(sample_rate_to_adts_index(48_000), 3);
        assert_eq!(sample_rate_to_adts_index(22_050), 7);
        // Out-of-table rates fall back to 44.1 kHz (index 4) so
        // the pipeline does not refuse unknown rates; aacparse
        // re-derives the rate from the AAC payload anyway.
        assert_eq!(sample_rate_to_adts_index(17_000), 4);
    }

    #[test]
    fn scale_to_48k_noop_when_source_is_already_48k() {
        assert_eq!(scale_to_48k(12_345, 48_000), 12_345);
    }

    #[test]
    fn scale_to_48k_upscales_44100_correctly() {
        // 1 AAC access unit = 1024 samples at 44.1 kHz = ~23.2 ms
        // = ~1114 ticks at 48 kHz.
        let ticks = scale_to_48k(1024, 44_100);
        assert!((1100..1120).contains(&ticks), "got {ticks}");
    }

    #[test]
    fn factory_opt_out_when_elements_missing() {
        // Test-host-dependent: on a host without GStreamer
        // installed `is_available()` is false and every build()
        // returns None. On a host with GStreamer we do not push
        // any samples through; this is an API-shape test.
        let factory = AacToOpusEncoderFactory::new();
        if factory.is_available() {
            return;
        }
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let config = AacAudioConfig {
            asc: Bytes::from_static(&[0x12, 0x10]),
            sample_rate: 44_100,
            channels: 2,
            object_type: 2,
        };
        assert!(factory.build(config, tx).is_none());
    }
}

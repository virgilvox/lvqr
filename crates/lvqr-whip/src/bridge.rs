//! Bridge between WHIP inbound samples and the LVQR Unified
//! Fragment Model.
//!
//! Sibling of `lvqr_ingest::RtmpMoqBridge`. Unlike the RTMP bridge,
//! which is driven by the `rml_rtmp` callback chain and parses FLV
//! tags into samples, this bridge is driven by the `str0m` poll
//! loop in [`crate::str0m_backend`] and consumes already-depacketized
//! H.264 access units (Annex B framed) produced by `Event::MediaData`.
//!
//! The composition pattern mirrors the session-24 archive tap:
//! observers (`FragmentObserver`, `RawSampleObserver`) are crate-
//! public types in `lvqr-ingest` and are passed by clone into every
//! bridge, so every existing egress (MoQ, LL-HLS, WHEP, disk
//! record, DVR archive) picks up WHIP ingest with zero changes to
//! the egress side.
//!
//! Scope of session 25 (H.264): video-only, one MoQ track per
//! broadcast (`0.mp4`). Session 26 added HEVC publishers through
//! the same track slot, distinguished via the [`MediaCodec`] tag
//! carried on every [`IngestSample`]. Session 27 made LL-HLS
//! codec-aware via `lvqr_cmaf::detect_video_codec_string` so the
//! fragment observer (HLS + archive) fans HEVC fragments without
//! advertising the wrong `CODECS` attribute. Session 28 widened
//! `RawSampleObserver::on_raw_sample` to carry the codec tag so
//! the WHEP backend can route per sample through the matching
//! `str0m::Pt`. HEVC now reaches every egress end-to-end.
//!
//! Audio rejection is still explicit: Opus samples are dropped
//! with a one-shot warn rather than silently lost. Opus-native
//! audio egress is session 29's recommended entry point.

use bytes::{Bytes, BytesMut};
use dashmap::DashMap;
use lvqr_cmaf::{
    HevcInitParams, OpusInitParams, RawSample, VideoInitParams, build_moof_mdat, write_avc_init_segment,
    write_hevc_init_segment, write_opus_init_segment,
};
use lvqr_codec::hevc::{self as hevc_codec, HevcSps};
use lvqr_core::{EventBus, RelayEvent};
use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta, MoqTrackSink};
use lvqr_ingest::{MediaCodec, SharedFragmentObserver, SharedRawSampleObserver};
use lvqr_moq::{OriginProducer, Track};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

use crate::depack::{
    AVC_NAL_TYPE_PPS, AVC_NAL_TYPE_SPS, HEVC_NAL_TYPE_PPS, HEVC_NAL_TYPE_SPS, HEVC_NAL_TYPE_VPS, annex_b_to_avcc,
    hevc_nal_type, split_annex_b,
};

/// One inbound sample pumped from the `str0m` poll loop into the
/// bridge.
///
/// The payload is Annex B framed bytes straight from
/// `Event::MediaData`. The bridge converts it to AVCC before
/// building the downstream fragment; see `depack::annex_b_to_avcc`
/// for the load-bearing conversion.
#[derive(Debug, Clone)]
pub struct IngestSample {
    /// Decode timestamp in 90 kHz ticks, rebased so the first
    /// sample of a session reads zero. Rebasing is the poll
    /// loop's responsibility, not the bridge's.
    pub dts_90k: u64,
    /// True iff this sample can start a new independent decoder
    /// state (IDR for H.264 / HEVC keyframe NAL type).
    pub keyframe: bool,
    /// Which video codec this sample carries.
    pub codec: MediaCodec,
    /// Annex B framed NAL payload.
    pub annex_b: Bytes,
}

/// One inbound audio sample pumped from the `str0m` poll loop
/// into the bridge.
///
/// WebRTC audio is Opus today. The payload is the opaque Opus
/// frame bytes straight out of `Event::MediaData` -- str0m has
/// already depacketized the RTP, there is no further framing to
/// strip. The bridge wraps the bytes as a single mdat sample per
/// Opus frame and fans them through the audio MoQ track.
#[derive(Debug, Clone)]
pub struct IngestAudioSample {
    /// Decode timestamp in 48 kHz ticks (the Opus wire rate),
    /// rebased so the first audio sample of a session reads zero.
    /// Rebasing is the poll loop's responsibility.
    pub dts_48k: u64,
    /// Duration of the frame in 48 kHz ticks. WebRTC Opus
    /// defaults to 20ms frames = 960 ticks; the poll loop passes
    /// the real duration when str0m knows it.
    pub duration_48k: u32,
    /// Raw Opus packet bytes. Session 29 writes these into an
    /// mdat sample verbatim; a future session may want to
    /// re-frame long RTP bursts but today Opus is one-packet
    /// per `MediaData` event.
    pub payload: Bytes,
}

/// Contract between the WebRTC poll loop and any downstream
/// consumer that wants to receive ingest samples. Implemented by
/// [`WhipMoqBridge`] in production and by test stubs that only
/// want to capture the flow for assertions.
pub trait IngestSampleSink: Send + Sync + 'static {
    /// Called once per depacketized video access unit. The
    /// bridge lazily constructs MoQ state on the first sample
    /// that carries parameter sets for a fresh broadcast;
    /// samples that arrive before the first keyframe are dropped.
    fn on_sample(&self, broadcast: &str, sample: IngestSample);

    /// Called once per depacketized audio (Opus) frame. Default
    /// impl is a no-op so existing test sinks do not need to
    /// grow a method; [`WhipMoqBridge`] overrides this to
    /// lazily create an audio MoQ track on the first audio
    /// frame after the video broadcast has been initialised.
    ///
    /// Audio samples that arrive before the first video
    /// keyframe are dropped silently: the broadcast slot does
    /// not exist yet and holding them back would just grow an
    /// unbounded queue.
    fn on_audio_sample(&self, _broadcast: &str, _sample: IngestAudioSample) {}

    /// Called when the WebRTC session ends (ICE disconnect, poll
    /// error, or shutdown signal). The bridge uses this to clean
    /// up per-broadcast state and emit `BroadcastStopped` so the
    /// HLS finalize subscriber can append `#EXT-X-ENDLIST`.
    fn on_disconnect(&self, _broadcast: &str) {}
}

/// Drop-in [`IngestSampleSink`] that throws everything away.
/// Useful as a default when a test only cares about the signaling
/// path.
pub struct NoopIngestSampleSink;

impl IngestSampleSink for NoopIngestSampleSink {
    fn on_sample(&self, _broadcast: &str, _sample: IngestSample) {}
}

/// Per-broadcast state kept by the bridge.
///
/// Constructed lazily on the first keyframe that carries SPS +
/// PPS, torn down implicitly when the bridge itself is dropped.
struct BroadcastState {
    broadcast: lvqr_moq::BroadcastProducer,
    video_sink: MoqTrackSink,
    video_seq: u32,
    init_emitted: bool,
    /// Optional audio sink. Lazily created on the first Opus
    /// frame that arrives after the broadcast has a video track.
    /// Kept as `Option` so broadcasts without audio (video-only
    /// publishers) pay zero cost.
    audio_sink: Option<MoqTrackSink>,
    /// Per-broadcast audio fragment sequence, bumped on every
    /// audio sample pushed through the sink. Starts at zero.
    audio_seq: u32,
    /// `true` once the bridge has called `on_init` for the audio
    /// track; kept alongside `init_emitted` so the two tracks
    /// have independent lifecycles and a late audio arrival does
    /// not disturb the video path.
    audio_init_emitted: bool,
}

/// Bridges WHIP inbound samples to a MoQ [`OriginProducer`] and
/// the shared fragment + raw-sample observer taps.
pub struct WhipMoqBridge {
    origin: OriginProducer,
    streams: DashMap<String, BroadcastState>,
    observer: Option<SharedFragmentObserver>,
    raw_observer: Option<SharedRawSampleObserver>,
    events: Option<EventBus>,
    audio_warn: AtomicBool,
}

impl WhipMoqBridge {
    pub fn new(origin: OriginProducer) -> Self {
        Self {
            origin,
            streams: DashMap::new(),
            observer: None,
            raw_observer: None,
            events: None,
            audio_warn: AtomicBool::new(false),
        }
    }

    pub fn with_events(mut self, events: EventBus) -> Self {
        self.events = Some(events);
        self
    }

    pub fn with_observer(mut self, observer: SharedFragmentObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    pub fn with_raw_sample_observer(mut self, observer: SharedRawSampleObserver) -> Self {
        self.raw_observer = Some(observer);
        self
    }

    pub fn active_stream_count(&self) -> usize {
        self.streams.len()
    }

    pub fn stream_names(&self) -> Vec<String> {
        self.streams.iter().map(|e| e.key().clone()).collect()
    }

    /// Ensure a broadcast + video track exist and an init segment
    /// has been emitted. Returns `true` iff the caller may push
    /// the sample payload through the video sink.
    ///
    /// Split out from [`Self::push_sample`] because DashMap's
    /// `get_mut` borrow cannot be upgraded into a `&mut`
    /// simultaneously with a fresh `insert`; doing the init work
    /// in two hops keeps the borrow scopes clean.
    fn ensure_initialized(&self, broadcast: &str, codec: MediaCodec, annex_b: &[u8], keyframe: bool) -> bool {
        if self.streams.contains_key(broadcast) {
            return true;
        }
        if !keyframe {
            // Drop non-keyframes until the first IDR: downstream
            // decoders can't do anything with them without an init
            // segment anyway.
            return false;
        }

        let built = match codec {
            MediaCodec::H264 => build_avc_init(broadcast, annex_b),
            MediaCodec::H265 => build_hevc_init(broadcast, annex_b),
            MediaCodec::Aac | MediaCodec::Opus => {
                // Audio codecs never reach the video init path.
                // `IngestSample` only carries video samples; an
                // audio codec here is a caller bug, not a
                // runtime condition, but dropping the sample is
                // strictly correct and avoids a panic.
                debug!(broadcast, ?codec, "whip: audio codec on video sample path; dropping");
                return false;
            }
        };
        let Some((codec_fourcc, init, width, height)) = built else {
            return false;
        };

        let Some(mut producer) = self.origin.create_broadcast(broadcast) else {
            warn!(broadcast, "whip: origin refused to create broadcast (duplicate?)");
            return false;
        };
        let video_track = match producer.create_track(Track::new("0.mp4")) {
            Ok(t) => t,
            Err(e) => {
                warn!(broadcast, error = ?e, "whip: failed to create MoQ video track");
                return false;
            }
        };
        let mut video_sink = MoqTrackSink::new(video_track, FragmentMeta::new(codec_fourcc, 90_000));
        video_sink.set_init_segment(init.clone());
        info!(broadcast, width, height, ?codec, "whip: broadcast initialized");

        // Fragment observer (LL-HLS + archive tee) fires for both
        // codecs. Session 27 lifted the original AVC-only guard
        // once `lvqr-hls::HlsServer::push_init` grew a codec
        // detector via `lvqr_cmaf::detect_video_codec_string`;
        // the archive indexer is and always has been codec-
        // indifferent. Session 28 then lifted the raw-sample
        // observer's guard (below) so WHEP egress gets the same
        // codec-aware routing.
        if let Some(obs) = self.observer.as_ref() {
            obs.on_init(broadcast, "0.mp4", 90_000, init);
        }

        self.streams.insert(
            broadcast.to_string(),
            BroadcastState {
                broadcast: producer,
                video_sink,
                video_seq: 0,
                init_emitted: true,
                audio_sink: None,
                audio_seq: 0,
                audio_init_emitted: false,
            },
        );
        true
    }

    /// Ensure the broadcast has an audio MoQ track + Opus init
    /// segment. Returns `true` iff the caller may push the audio
    /// payload through the audio sink.
    ///
    /// Audio is lazy and driven by the first Opus frame: the
    /// broadcast must already exist (video-first model), and
    /// audio frames before that arrive get dropped by
    /// [`Self::on_audio_sample`] with a one-shot debug log.
    fn ensure_audio_initialized(&self, broadcast: &str) -> bool {
        // Look up the existing broadcast. If there isn't one yet
        // (audio arrived before video), drop the frame: the MoQ
        // track requires a BroadcastProducer that only
        // `ensure_initialized` (video path) creates.
        let mut entry = match self.streams.get_mut(broadcast) {
            Some(e) => e,
            None => return false,
        };
        let state = entry.value_mut();
        if state.audio_sink.is_some() {
            return true;
        }

        // Create the Opus sibling track.
        let audio_track = match state.broadcast.create_track(Track::new("1.mp4")) {
            Ok(t) => t,
            Err(e) => {
                warn!(broadcast, error = ?e, "whip: failed to create MoQ audio track");
                return false;
            }
        };

        // Build the Opus init segment. 48 kHz / 2 channels is the
        // WebRTC default; multi-stream layouts need a different
        // dOps box and are out of scope for session 29.
        let params = OpusInitParams {
            channel_count: 2,
            pre_skip: 0,
            input_sample_rate: 48_000,
            timescale: 48_000,
        };
        let mut buf = BytesMut::new();
        if let Err(e) = write_opus_init_segment(&mut buf, &params) {
            warn!(broadcast, error = ?e, "whip: failed to build Opus init segment; dropping audio");
            return false;
        }
        let init = buf.freeze();

        let mut audio_sink = MoqTrackSink::new(audio_track, FragmentMeta::new("Opus", 48_000));
        audio_sink.set_init_segment(init.clone());
        state.audio_sink = Some(audio_sink);
        state.audio_init_emitted = true;
        info!(broadcast, "whip: opus audio track initialized");

        // Session 31: fire the fragment observer's `on_init` for
        // the Opus track so `HlsFragmentBridge::ensure_audio` wires
        // up a 48 kHz audio rendition whose init segment carries
        // the `dOps` box, and so the archive indexer picks up the
        // audio track alongside video. `detect_audio_codec_string`
        // on the HLS side (session 30) picks "Opus" out of the init
        // bytes so the master playlist's `CODECS` attribute is
        // correct without any extra signalling here. Release the
        // DashMap entry before the observer call to avoid the same
        // reentrancy footgun the video path already guards against.
        drop(entry);
        if let Some(obs) = self.observer.as_ref() {
            obs.on_init(broadcast, "1.mp4", 48_000, init);
        }
        true
    }

    fn push_sample(&self, broadcast: &str, sample: IngestSample) {
        let avcc = annex_b_to_avcc(&sample.annex_b);
        if avcc.is_empty() {
            debug!(broadcast, "whip: annex_b produced no NAL units; dropping");
            return;
        }
        let avcc_bytes = Bytes::from(avcc);

        let raw = RawSample {
            track_id: 1,
            dts: sample.dts_90k,
            cts_offset: 0,
            duration: 3000, // default 30 fps @ 90 kHz; updated per-frame below
            payload: avcc_bytes,
            keyframe: sample.keyframe,
        };

        // Raw-sample observer (WHEP). Session 28 lifted the
        // AVC-only guard: the observer signature now carries a
        // codec tag so the WHEP session backend can pick the
        // matching `str0m::Pt` for the negotiated codec.
        if let Some(obs) = self.raw_observer.as_ref() {
            obs.on_raw_sample(broadcast, "0.mp4", sample.codec, &raw);
        }

        let Some(mut entry) = self.streams.get_mut(broadcast) else {
            return;
        };
        let state = entry.value_mut();
        if !state.init_emitted {
            return;
        }
        state.video_seq += 1;
        let seq = state.video_seq;
        let dts = raw.dts;
        let seg = build_moof_mdat(seq, 1, dts, std::slice::from_ref(&raw));

        let flags = if raw.keyframe {
            FragmentFlags::KEYFRAME
        } else {
            FragmentFlags::DELTA
        };
        let frag = Fragment::new("0.mp4", seq as u64, 0, 0, dts, dts, raw.duration as u64, flags, seg);
        if let Err(e) = state.video_sink.push(&frag) {
            debug!(broadcast, error = ?e, "whip: moq sink push failed");
        }

        // Release the dashmap entry before invoking the observer:
        // observers may themselves walk the bridge, and holding a
        // value lock across a potentially reentrant call is a
        // deadlock footgun.
        drop(entry);
        // Fragment observer (HLS + archive) fires for both codecs
        // after session 27. See the matching note in
        // `ensure_initialized` for the rationale.
        if let Some(obs) = self.observer.as_ref() {
            obs.on_fragment(broadcast, "0.mp4", &frag);
        }
    }
}

impl IngestSampleSink for WhipMoqBridge {
    fn on_sample(&self, broadcast: &str, sample: IngestSample) {
        // Defensive: empty payloads are either a str0m bug or a
        // misbehaving caller; either way we can't do anything
        // useful with a zero-length NAL buffer. Warn once so the
        // cause is obvious without spamming logs.
        if sample.annex_b.is_empty() {
            if !self.audio_warn.swap(true, Ordering::Relaxed) {
                warn!("whip: empty payload pumped through bridge (logging once)");
            }
            return;
        }
        if !self.ensure_initialized(broadcast, sample.codec, &sample.annex_b, sample.keyframe) {
            return;
        }
        self.push_sample(broadcast, sample);
    }

    fn on_audio_sample(&self, broadcast: &str, sample: IngestAudioSample) {
        if sample.payload.is_empty() {
            debug!(broadcast, "whip: empty opus payload; dropping");
            return;
        }
        if !self.ensure_audio_initialized(broadcast) {
            return;
        }
        self.push_audio_sample(broadcast, sample);
    }

    fn on_disconnect(&self, broadcast: &str) {
        if let Some((_, mut state)) = self.streams.remove(broadcast) {
            state.video_sink.finish_current_group();
            if let Some(ref bus) = self.events {
                bus.emit(RelayEvent::BroadcastStopped {
                    name: broadcast.to_string(),
                });
            }
            info!(broadcast, "whip: removed broadcast on disconnect");
        }
    }
}

impl WhipMoqBridge {
    fn push_audio_sample(&self, broadcast: &str, sample: IngestAudioSample) {
        let Some(mut entry) = self.streams.get_mut(broadcast) else {
            return;
        };
        let state = entry.value_mut();
        let Some(audio_sink) = state.audio_sink.as_mut() else {
            return;
        };
        if !state.audio_init_emitted {
            return;
        }

        // Build one `RawSample` for the Opus frame. track_id=2
        // so the `moof` `traf` distinguishes it from the video
        // track (track_id=1). DTS is in the track's own
        // timescale (48 kHz); the downstream fragment model
        // carries it verbatim.
        let raw = RawSample {
            track_id: 2,
            dts: sample.dts_48k,
            cts_offset: 0,
            duration: sample.duration_48k,
            payload: sample.payload,
            keyframe: true,
        };
        state.audio_seq += 1;
        let seq = state.audio_seq;
        let dts = raw.dts;
        let dur = raw.duration as u64;
        let seg = build_moof_mdat(seq, 2, dts, std::slice::from_ref(&raw));

        let frag = Fragment::new(
            "1.mp4",
            seq as u64,
            0,
            0,
            dts,
            dts,
            dur,
            FragmentFlags::KEYFRAME, // every opus frame is independently decodable
            seg,
        );
        if let Err(e) = audio_sink.push(&frag) {
            debug!(broadcast, error = ?e, "whip: moq audio sink push failed");
        }

        // Release the DashMap entry before invoking observers:
        // they may themselves reach back into the bridge, and
        // holding a value lock across a reentrant call is a
        // deadlock footgun. The video path already guards against
        // this.
        drop(entry);
        // Raw-sample observer (WHEP): forwards Opus frames to
        // subscribers that negotiated Opus on their side (same-
        // codec passthrough, no transcode). Session 30.
        if let Some(obs) = self.raw_observer.as_ref() {
            obs.on_raw_sample(broadcast, "1.mp4", MediaCodec::Opus, &raw);
        }
        // Fragment observer (LL-HLS + archive): session 31 closed
        // the last loose thread in the WHIP audio story. The HLS
        // audio rendition is codec-agnostic above the init
        // segment, so Opus fragments fan out through the same
        // `HlsFragmentBridge` path AAC already uses.
        if let Some(obs) = self.observer.as_ref() {
            obs.on_fragment(broadcast, "1.mp4", &frag);
        }
    }
}

/// Build the AVC init segment bytes from the first SPS+PPS-bearing
/// IDR access unit. Returns `None` if the expected parameter sets
/// are missing or the init writer rejects them.
fn build_avc_init(broadcast: &str, annex_b: &[u8]) -> Option<(&'static str, Bytes, u16, u16)> {
    let (sps, pps) = extract_sps_pps(annex_b);
    let (Some(sps), Some(pps)) = (sps, pps) else {
        debug!(
            broadcast,
            "whip: first keyframe missing SPS/PPS; waiting for a parameter-set-bearing IDR"
        );
        return None;
    };
    let (width, height) = parse_avc_sps_dims(&sps).unwrap_or((0, 0));

    let params = VideoInitParams {
        sps,
        pps,
        width,
        height,
        timescale: 90_000,
    };
    let mut buf = BytesMut::new();
    if let Err(e) = write_avc_init_segment(&mut buf, &params) {
        warn!(broadcast, error = ?e, "whip: failed to build AVC init segment; dropping sample");
        return None;
    }
    Some(("avc1", buf.freeze(), width, height))
}

/// Build the HEVC init segment bytes from the first VPS+SPS+PPS-
/// bearing IRAP access unit. Returns `None` if any parameter set
/// is missing, the SPS parser rejects it, or the init writer
/// rejects the params.
fn build_hevc_init(broadcast: &str, annex_b: &[u8]) -> Option<(&'static str, Bytes, u16, u16)> {
    let HevcParamSets { vps, sps, pps } = extract_hevc_params(annex_b);
    let (Some(vps), Some(sps), Some(pps)) = (vps, sps, pps) else {
        debug!(
            broadcast,
            "whip: first HEVC keyframe missing VPS/SPS/PPS; waiting for a complete parameter-set IRAP"
        );
        return None;
    };
    let sps_info: HevcSps = match hevc_codec::parse_sps(&sps) {
        Ok(v) => v,
        Err(e) => {
            debug!(broadcast, error = ?e, "whip: HEVC SPS parse failed; dropping sample");
            return None;
        }
    };
    let width = sps_info.pic_width_in_luma_samples as u16;
    let height = sps_info.pic_height_in_luma_samples as u16;
    let params = HevcInitParams {
        vps,
        sps,
        pps,
        sps_info,
        timescale: 90_000,
    };
    let mut buf = BytesMut::new();
    if let Err(e) = write_hevc_init_segment(&mut buf, &params) {
        warn!(broadcast, error = ?e, "whip: failed to build HEVC init segment; dropping sample");
        return None;
    }
    Some(("hvc1", buf.freeze(), width, height))
}

/// Pull SPS + PPS NAL units out of an Annex B access unit.
///
/// Returns the NAL bodies (with their NAL header byte intact),
/// ready to hand to `mp4-atom::Avcc::new` via
/// [`VideoInitParams::sps`] / [`VideoInitParams::pps`].
fn extract_sps_pps(annex_b: &[u8]) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let mut sps = None;
    let mut pps = None;
    for nal in split_annex_b(annex_b) {
        if nal.is_empty() {
            continue;
        }
        let nal_type = nal[0] & 0x1f;
        match nal_type {
            t if t == AVC_NAL_TYPE_SPS => {
                sps.get_or_insert_with(|| nal.to_vec());
            }
            t if t == AVC_NAL_TYPE_PPS => {
                pps.get_or_insert_with(|| nal.to_vec());
            }
            _ => {}
        }
    }
    (sps, pps)
}

/// Triple of HEVC parameter-set NAL bodies recovered from a
/// single IRAP access unit. Each slot is `None` until the matching
/// NAL unit type is observed.
#[derive(Default)]
struct HevcParamSets {
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

/// Pull HEVC VPS + SPS + PPS NAL units out of an Annex B access
/// unit. HEVC NAL unit types live in bits 6..=1 of the first byte
/// (see [`crate::depack::hevc_nal_type`]).
fn extract_hevc_params(annex_b: &[u8]) -> HevcParamSets {
    let mut out = HevcParamSets::default();
    for nal in split_annex_b(annex_b) {
        let Some(t) = hevc_nal_type(nal) else {
            continue;
        };
        match t {
            HEVC_NAL_TYPE_VPS => {
                out.vps.get_or_insert_with(|| nal.to_vec());
            }
            HEVC_NAL_TYPE_SPS => {
                out.sps.get_or_insert_with(|| nal.to_vec());
            }
            HEVC_NAL_TYPE_PPS => {
                out.pps.get_or_insert_with(|| nal.to_vec());
            }
            _ => {}
        }
    }
    out
}

/// Decode AVC SPS NAL bytes to pixel dimensions using
/// `h264-reader`. Mirrors `lvqr_ingest::remux::flv::extract_resolution`;
/// copied here rather than re-exported to keep `lvqr-whip`
/// decoupled from the ingest crate's FLV parser.
fn parse_avc_sps_dims(sps: &[u8]) -> Option<(u16, u16)> {
    if sps.len() < 2 {
        return None;
    }
    let rbsp_data;
    let rbsp: &[u8] = match h264_reader::rbsp::decode_nal(sps) {
        Ok(cow) => {
            rbsp_data = cow;
            &rbsp_data
        }
        Err(_) => &sps[1..],
    };
    let parsed = h264_reader::nal::sps::SeqParameterSet::from_bits(h264_reader::rbsp::BitReader::new(rbsp)).ok()?;
    let (w, h) = parsed.pixel_dimensions().ok()?;
    Some((w as u16, h as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_annex_b(nals: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for nal in nals {
            out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            out.extend_from_slice(nal);
        }
        out
    }

    #[test]
    fn extracts_sps_pps_from_access_unit() {
        let sps: &[u8] = &[0x67, 0x42, 0xC0, 0x1E];
        let pps: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];
        let idr: &[u8] = &[0x65, 0x88, 0x84];
        let buf = build_annex_b(&[sps, pps, idr]);
        let (got_sps, got_pps) = extract_sps_pps(&buf);
        assert_eq!(got_sps.as_deref(), Some(sps));
        assert_eq!(got_pps.as_deref(), Some(pps));
    }

    #[test]
    fn missing_parameter_sets_returns_none() {
        let idr: &[u8] = &[0x65, 0x88, 0x84];
        let buf = build_annex_b(&[idr]);
        let (sps, pps) = extract_sps_pps(&buf);
        assert!(sps.is_none());
        assert!(pps.is_none());
    }

    #[tokio::test]
    async fn non_keyframe_is_dropped_until_first_idr() {
        let origin = OriginProducer::new();
        let bridge = WhipMoqBridge::new(origin);
        let delta = IngestSample {
            dts_90k: 0,
            keyframe: false,
            codec: MediaCodec::H264,
            annex_b: Bytes::from(build_annex_b(&[&[0x41, 0x00][..]])),
        };
        bridge.on_sample("live/test", delta);
        assert_eq!(bridge.active_stream_count(), 0);
    }

    #[tokio::test]
    async fn keyframe_without_parameter_sets_is_dropped() {
        let origin = OriginProducer::new();
        let bridge = WhipMoqBridge::new(origin);
        let idr_only = IngestSample {
            dts_90k: 0,
            keyframe: true,
            codec: MediaCodec::H264,
            annex_b: Bytes::from(build_annex_b(&[&[0x65, 0x88, 0x84][..]])),
        };
        bridge.on_sample("live/test", idr_only);
        assert_eq!(bridge.active_stream_count(), 0);
    }

    // --- HEVC-specific coverage (session 26) -----------------------

    /// Real x265 HEVC Main 3.0 VPS/SPS/PPS bytes (NAL body, no
    /// start code). Pinned to the same capture used by
    /// `lvqr-cmaf::init::tests` so the two writers agree on what a
    /// valid IRAP looks like. If x265 is updated and these bytes
    /// drift, the cmaf conformance test catches it first.
    const HEVC_VPS: &[u8] = &[
        0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x03, 0x00, 0x3c, 0x95, 0x94, 0x09,
    ];
    const HEVC_SPS: &[u8] = &[
        0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c,
        0xa0, 0x0a, 0x08, 0x0f, 0x16, 0x59, 0x59, 0x52, 0x93, 0x0b, 0xc0, 0x5a, 0x02, 0x00, 0x00, 0x03, 0x00, 0x02,
        0x00, 0x00, 0x03, 0x00, 0x3c, 0x10,
    ];
    const HEVC_PPS: &[u8] = &[0x44, 0x01, 0xc0, 0x73, 0xc1, 0x89];
    /// Synthetic HEVC IDR_W_RADL NAL (type 19). Two-byte NAL
    /// header followed by a one-byte slice body stand-in; the
    /// bridge only needs to find the SPS-derived dimensions, not
    /// decode the slice.
    const HEVC_IDR: &[u8] = &[0x26, 0x01, 0xAF];

    #[test]
    fn extracts_hevc_params_from_access_unit() {
        let buf = build_annex_b(&[HEVC_VPS, HEVC_SPS, HEVC_PPS, HEVC_IDR]);
        let out = extract_hevc_params(&buf);
        assert_eq!(out.vps.as_deref(), Some(HEVC_VPS));
        assert_eq!(out.sps.as_deref(), Some(HEVC_SPS));
        assert_eq!(out.pps.as_deref(), Some(HEVC_PPS));
    }

    #[test]
    fn hevc_missing_vps_returns_none() {
        let buf = build_annex_b(&[HEVC_SPS, HEVC_PPS, HEVC_IDR]);
        let out = extract_hevc_params(&buf);
        assert!(out.vps.is_none());
    }

    #[tokio::test]
    async fn hevc_keyframe_with_full_parameter_sets_initializes_broadcast() {
        let origin = OriginProducer::new();
        let bridge = WhipMoqBridge::new(origin);
        let keyframe = IngestSample {
            dts_90k: 0,
            keyframe: true,
            codec: MediaCodec::H265,
            annex_b: Bytes::from(build_annex_b(&[HEVC_VPS, HEVC_SPS, HEVC_PPS, HEVC_IDR])),
        };
        bridge.on_sample("live/hevc", keyframe);
        assert_eq!(bridge.active_stream_count(), 1);
        assert_eq!(bridge.stream_names(), vec!["live/hevc".to_string()]);
    }

    #[tokio::test]
    async fn hevc_keyframe_missing_parameter_sets_is_dropped() {
        let origin = OriginProducer::new();
        let bridge = WhipMoqBridge::new(origin);
        let idr_only = IngestSample {
            dts_90k: 0,
            keyframe: true,
            codec: MediaCodec::H265,
            annex_b: Bytes::from(build_annex_b(&[HEVC_IDR])),
        };
        bridge.on_sample("live/hevc", idr_only);
        assert_eq!(bridge.active_stream_count(), 0);
    }

    // --- Audio (Opus) coverage (session 29) ------------------------

    fn avc_keyframe_sample() -> IngestSample {
        // Same minimal fixture the happy-path H.264 AVC tests rely
        // on: SPS + PPS + IDR, enough for the bridge to finish
        // `build_avc_init` and create the broadcast.
        let sps: &[u8] = &[
            0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00,
            0x03, 0x03, 0xC0, 0xF1, 0x83, 0x2A,
        ];
        let pps: &[u8] = &[0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];
        let idr: &[u8] = &[0x65, 0x88, 0x84, 0x40];
        IngestSample {
            dts_90k: 0,
            keyframe: true,
            codec: MediaCodec::H264,
            annex_b: Bytes::from(build_annex_b(&[sps, pps, idr])),
        }
    }

    #[tokio::test]
    async fn opus_audio_sample_before_video_is_dropped() {
        let origin = OriginProducer::new();
        let bridge = WhipMoqBridge::new(origin);
        // No video yet -> ensure_audio_initialized returns false
        // because the broadcast has not been created. The bridge
        // must not spontaneously create a video-less broadcast.
        bridge.on_audio_sample(
            "live/audio-first",
            IngestAudioSample {
                dts_48k: 0,
                duration_48k: 960,
                payload: Bytes::from_static(&[0x78, 0x01, 0x02]),
            },
        );
        assert_eq!(bridge.active_stream_count(), 0);
    }

    #[tokio::test]
    async fn opus_audio_sample_after_video_initializes_audio_track() {
        let origin = OriginProducer::new();
        let bridge = WhipMoqBridge::new(origin);
        // First land a video keyframe so the broadcast exists.
        bridge.on_sample("live/audio-after-video", avc_keyframe_sample());
        assert_eq!(bridge.active_stream_count(), 1);

        // Now push an Opus frame. The bridge should lazily create
        // the `1.mp4` audio track and build the Opus init segment.
        bridge.on_audio_sample(
            "live/audio-after-video",
            IngestAudioSample {
                dts_48k: 0,
                duration_48k: 960,
                payload: Bytes::from_static(&[0x78, 0x01, 0x02, 0x03]),
            },
        );

        // The broadcast is still counted once -- audio does not
        // create new broadcasts, only new tracks.
        assert_eq!(bridge.active_stream_count(), 1);

        // And a follow-up audio frame works through the already-
        // initialized sink (idempotent ensure_audio_initialized).
        bridge.on_audio_sample(
            "live/audio-after-video",
            IngestAudioSample {
                dts_48k: 960,
                duration_48k: 960,
                payload: Bytes::from_static(&[0x79, 0x04, 0x05, 0x06]),
            },
        );
        assert_eq!(bridge.active_stream_count(), 1);
    }

    /// Recording fragment observer used to prove that session-31's
    /// Opus fanout fires `on_init` + `on_fragment` for the `1.mp4`
    /// audio track with the right init bytes and timescale.
    #[derive(Default)]
    struct RecordingObserver {
        init_calls: std::sync::Mutex<Vec<(String, String, u32, Bytes)>>,
        frag_calls: std::sync::Mutex<Vec<(String, String, u64, u64)>>,
    }

    impl lvqr_ingest::FragmentObserver for RecordingObserver {
        fn on_init(&self, broadcast: &str, track: &str, timescale: u32, init: Bytes) {
            self.init_calls
                .lock()
                .unwrap()
                .push((broadcast.to_string(), track.to_string(), timescale, init));
        }
        fn on_fragment(&self, broadcast: &str, track: &str, fragment: &Fragment) {
            self.frag_calls.lock().unwrap().push((
                broadcast.to_string(),
                track.to_string(),
                fragment.dts,
                fragment.duration,
            ));
        }
    }

    #[tokio::test]
    async fn opus_audio_fires_fragment_observer_on_init_and_fragment() {
        let origin = OriginProducer::new();
        let obs = std::sync::Arc::new(RecordingObserver::default());
        let shared: lvqr_ingest::SharedFragmentObserver = obs.clone();
        let bridge = WhipMoqBridge::new(origin).with_observer(shared);

        bridge.on_sample("live/opus-obs", avc_keyframe_sample());
        bridge.on_audio_sample(
            "live/opus-obs",
            IngestAudioSample {
                dts_48k: 0,
                duration_48k: 960,
                payload: Bytes::from_static(&[0x78, 0x01, 0x02, 0x03]),
            },
        );
        bridge.on_audio_sample(
            "live/opus-obs",
            IngestAudioSample {
                dts_48k: 960,
                duration_48k: 960,
                payload: Bytes::from_static(&[0x78, 0x04, 0x05, 0x06]),
            },
        );

        let inits = obs.init_calls.lock().unwrap().clone();
        // One video init (`0.mp4`, 90 kHz) + one audio init
        // (`1.mp4`, 48 kHz). The audio init must be non-empty and
        // start with an `ftyp` box -- the Opus init segment writer
        // emits a standard CMAF init layout that
        // `detect_audio_codec_string` can then parse.
        assert_eq!(inits.len(), 2, "expected one video + one audio init");
        let audio_init = inits
            .iter()
            .find(|(_, track, _, _)| track == "1.mp4")
            .expect("audio init must fire");
        assert_eq!(audio_init.0, "live/opus-obs");
        assert_eq!(audio_init.2, 48_000, "opus track timescale is 48 kHz");
        assert!(!audio_init.3.is_empty(), "opus init bytes must be non-empty");
        // The mp4 `ftyp` box is at offset 4; the 4-byte box header
        // length precedes it. This is a cheap shape assertion that
        // the bridge handed real CMAF init bytes to the observer,
        // not a placeholder.
        assert_eq!(&audio_init.3[4..8], b"ftyp", "opus init must be a CMAF ftyp box");
        // Route the same bytes through `detect_audio_codec_string`
        // to prove the HLS master playlist will resolve "Opus".
        assert_eq!(
            lvqr_cmaf::detect_audio_codec_string(&audio_init.3).as_deref(),
            Some("opus"),
            "opus init must be recognised by detect_audio_codec_string"
        );

        let frags = obs.frag_calls.lock().unwrap().clone();
        // One video fragment (from the H.264 keyframe) + two audio
        // fragments (one per Opus frame). The audio fragment DTS
        // values must monotonically advance in 48 kHz ticks.
        let audio_frags: Vec<_> = frags.iter().filter(|(_, t, _, _)| t == "1.mp4").collect();
        assert_eq!(audio_frags.len(), 2, "expected two opus fragments");
        assert_eq!(audio_frags[0].2, 0);
        assert_eq!(audio_frags[0].3, 960);
        assert_eq!(audio_frags[1].2, 960);
        assert_eq!(audio_frags[1].3, 960);
    }

    #[tokio::test]
    async fn opus_empty_payload_is_dropped_silently() {
        let origin = OriginProducer::new();
        let bridge = WhipMoqBridge::new(origin);
        bridge.on_sample("live/empty-audio", avc_keyframe_sample());
        bridge.on_audio_sample(
            "live/empty-audio",
            IngestAudioSample {
                dts_48k: 0,
                duration_48k: 960,
                payload: Bytes::new(),
            },
        );
        assert_eq!(bridge.active_stream_count(), 1);
    }
}

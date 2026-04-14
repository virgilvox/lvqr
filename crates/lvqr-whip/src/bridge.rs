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
//! Scope of session 25: video-only, H.264 only, one MoQ track per
//! broadcast (`0.mp4`). Audio will land in a follow-up session
//! alongside an Opus path; HEVC follows once `Rtc::enable_h265` is
//! stable in str0m. Rejection is explicit: audio samples are
//! dropped with a one-shot warn rather than silently lost.

use bytes::{Bytes, BytesMut};
use dashmap::DashMap;
use lvqr_cmaf::{RawSample, VideoInitParams, build_moof_mdat, write_avc_init_segment};
use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta, MoqTrackSink};
use lvqr_ingest::{SharedFragmentObserver, SharedRawSampleObserver};
use lvqr_moq::{OriginProducer, Track};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

use crate::depack::{AVC_NAL_TYPE_PPS, AVC_NAL_TYPE_SPS, annex_b_to_avcc, split_annex_b};

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
    /// state (IDR for H.264).
    pub keyframe: bool,
    /// Annex B framed NAL payload.
    pub annex_b: Bytes,
}

/// Contract between the WebRTC poll loop and any downstream
/// consumer that wants to receive ingest samples. Implemented by
/// [`WhipMoqBridge`] in production and by test stubs that only
/// want to capture the flow for assertions.
pub trait IngestSampleSink: Send + Sync + 'static {
    /// Called once per depacketized access unit. The bridge
    /// lazily constructs MoQ state on the first sample that
    /// carries SPS + PPS for a fresh broadcast; samples that
    /// arrive before the first keyframe are dropped.
    fn on_sample(&self, broadcast: &str, sample: IngestSample);
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
    _broadcast: lvqr_moq::BroadcastProducer,
    video_sink: MoqTrackSink,
    video_seq: u32,
    init_emitted: bool,
}

/// Bridges WHIP inbound samples to a MoQ [`OriginProducer`] and
/// the shared fragment + raw-sample observer taps.
pub struct WhipMoqBridge {
    origin: OriginProducer,
    streams: DashMap<String, BroadcastState>,
    observer: Option<SharedFragmentObserver>,
    raw_observer: Option<SharedRawSampleObserver>,
    audio_warn: AtomicBool,
}

impl WhipMoqBridge {
    pub fn new(origin: OriginProducer) -> Self {
        Self {
            origin,
            streams: DashMap::new(),
            observer: None,
            raw_observer: None,
            audio_warn: AtomicBool::new(false),
        }
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
    fn ensure_initialized(&self, broadcast: &str, annex_b: &[u8], keyframe: bool) -> bool {
        if self.streams.contains_key(broadcast) {
            return true;
        }
        if !keyframe {
            // Drop non-keyframes until the first IDR: downstream
            // decoders can't do anything with them without an init
            // segment anyway.
            return false;
        }

        // Extract SPS + PPS from the keyframe. WebRTC clients
        // typically prepend them to every IDR access unit, so we
        // expect both to be present on the first keyframe we see.
        let (sps, pps) = extract_sps_pps(annex_b);
        let (Some(sps), Some(pps)) = (sps, pps) else {
            debug!(
                broadcast,
                "whip: first keyframe missing SPS/PPS; waiting for a parameter-set-bearing IDR"
            );
            return false;
        };
        let (width, height) = parse_sps_dims(&sps).unwrap_or((0, 0));

        let params = VideoInitParams {
            sps: sps.clone(),
            pps: pps.clone(),
            width,
            height,
            timescale: 90_000,
        };
        let mut buf = BytesMut::new();
        match write_avc_init_segment(&mut buf, &params) {
            Ok(_) => {}
            Err(e) => {
                warn!(broadcast, error = ?e, "whip: failed to build AVC init segment; dropping sample");
                return false;
            }
        }
        let init = buf.freeze();

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
        let mut video_sink = MoqTrackSink::new(video_track, FragmentMeta::new("avc1", 90_000));
        video_sink.set_init_segment(init.clone());
        info!(broadcast, width, height, "whip: broadcast initialized");

        if let Some(obs) = self.observer.as_ref() {
            obs.on_init(broadcast, "0.mp4", 90_000, init);
        }

        self.streams.insert(
            broadcast.to_string(),
            BroadcastState {
                _broadcast: producer,
                video_sink,
                video_seq: 0,
                init_emitted: true,
            },
        );
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

        if let Some(obs) = self.raw_observer.as_ref() {
            obs.on_raw_sample(broadcast, "0.mp4", &raw);
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
        if let Some(obs) = self.observer.as_ref() {
            obs.on_fragment(broadcast, "0.mp4", &frag);
        }
    }
}

impl IngestSampleSink for WhipMoqBridge {
    fn on_sample(&self, broadcast: &str, sample: IngestSample) {
        // Defensive: the poll loop only routes video via the
        // `0.mp4` track convention, and audio is dropped upstream.
        // Keep the warn slot so a misbehaving caller is obvious in
        // logs without spamming.
        if sample.annex_b.is_empty() {
            if !self.audio_warn.swap(true, Ordering::Relaxed) {
                warn!("whip: empty payload pumped through bridge (logging once)");
            }
            return;
        }
        if !self.ensure_initialized(broadcast, &sample.annex_b, sample.keyframe) {
            return;
        }
        self.push_sample(broadcast, sample);
    }
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

/// Decode SPS NAL bytes to pixel dimensions using `h264-reader`.
/// Mirrors `lvqr_ingest::remux::flv::extract_resolution`; copied
/// here rather than re-exported to keep `lvqr-whip` decoupled from
/// the ingest crate's FLV parser.
fn parse_sps_dims(sps: &[u8]) -> Option<(u16, u16)> {
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
            annex_b: Bytes::from(build_annex_b(&[&[0x65, 0x88, 0x84][..]])),
        };
        bridge.on_sample("live/test", idr_only);
        assert_eq!(bridge.active_stream_count(), 0);
    }
}

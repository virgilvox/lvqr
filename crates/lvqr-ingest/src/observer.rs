//! Raw-sample observer hook for the RTMP / WHIP / RTSP / SRT bridges.
//!
//! Session 60 completed the Tier 2.1 consumer-side switchover: every
//! downstream fragment consumer (archive, LL-HLS, DASH) now subscribes
//! to a `lvqr_fragment::FragmentBroadcaster` owned by a shared
//! `FragmentBroadcasterRegistry`, so the `FragmentObserver` trait and
//! its transitive callback wiring are gone.
//!
//! The raw-sample surface remains: consumers that need unmuxed NAL /
//! AAC / Opus bytes (notably the WHEP egress, which packetizes per-NAL
//! into RTP) subscribe here instead of reparsing `CmafChunk` `mdat`
//! bodies. Raw samples are a read-only tap on the ingest side; they
//! never replace the fragment path.
//!
//! Implementations must be cheap and non-blocking. The observer is
//! invoked from inside the `rml_rtmp` callback chain / WHIP str0m
//! runloop / SRT reader loop, which all run on the ingest task;
//! long work there stalls ingest. Spawn a tokio task per notification
//! if downstream work is expensive.

use lvqr_cmaf::RawSample;
use std::sync::Arc;

/// Which audio or video codec a [`RawSample`] carries.
///
/// Stamped on every [`RawSampleObserver::on_raw_sample`] call by
/// the producing bridge so the downstream consumer (notably the
/// WHEP egress and the LL-HLS audio rendition router) can pick
/// the matching output path without sniffing payload bytes. The
/// track name distinguishes kinds (`0.mp4` = video, `1.mp4` =
/// audio) and this enum distinguishes codec within the kind.
///
/// Session-level guarantees from each producer:
///
/// * `lvqr_ingest::RtmpMoqBridge` emits `H264` for video and
///   `Aac` for audio (FLV-over-RTMP is AVC + AAC only in LVQR
///   today; enhanced-RTMP HEVC is deferred).
/// * `lvqr_whip::WhipMoqBridge` emits whichever codec the
///   `str0m` answerer negotiated: `H264` / `H265` for video,
///   `Opus` for audio.
///
/// The default is `H264` so code paths that existed before
/// session 28 and never passed an explicit codec still behave
/// as they did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaCodec {
    /// H.264 / AVC video.
    #[default]
    H264,
    /// H.265 / HEVC video.
    H265,
    /// AAC audio. Either AAC-LC or HE-AAC; the downstream
    /// consumer does not distinguish sub-profiles via this tag.
    Aac,
    /// Opus audio.
    Opus,
}

/// Shared, dynamically-dispatched raw-sample observer handle.
pub type SharedRawSampleObserver = Arc<dyn RawSampleObserver>;

/// Observer hook called by [`crate::RtmpMoqBridge`] (and every
/// other ingest bridge) for every per-sample unit it sees, *before*
/// the sample is muxed into an fMP4 fragment.
///
/// Consumers that need the raw NAL / AAC / Opus access-unit bytes
/// subscribe here instead of re-parsing `CmafChunk` mdat bodies
/// downstream. The bridge still produces fragments in parallel for
/// the broadcaster path; raw-sample observers are a read-only tap.
///
/// Implementations must be cheap and non-blocking: the observer is
/// fired from inside the ingest crate's receive loop, which runs on
/// the ingest task. Long work or blocking I/O will stall ingest;
/// spawn a tokio task per notification if downstream work is
/// expensive.
pub trait RawSampleObserver: Send + Sync {
    /// Called for every video / audio sample the bridge receives,
    /// in DTS order per track. `track` follows the same `0.mp4` /
    /// `1.mp4` convention the fragment path uses. `codec`
    /// identifies the codec carried on the sample payload so
    /// codec-aware consumers (notably `lvqr-whep`) can pick the
    /// matching `str0m::Pt` without having to sniff NAL headers.
    /// Audio samples carry the default value and the audio path
    /// must not branch on it.
    fn on_raw_sample(&self, broadcast: &str, track: &str, codec: MediaCodec, sample: &RawSample);
}

/// Drop-in raw-sample observer that does nothing.
pub struct NoopRawSampleObserver;

impl RawSampleObserver for NoopRawSampleObserver {
    fn on_raw_sample(&self, _broadcast: &str, _track: &str, _codec: MediaCodec, _sample: &RawSample) {}
}

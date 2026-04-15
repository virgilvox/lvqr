//! Fragment observer hook for the RTMP -> MoQ bridge.
//!
//! The bridge already feeds every outgoing media unit through a
//! [`lvqr_fragment::MoqTrackSink`] so MoQ subscribers see the data. A
//! second class of consumers (LL-HLS, future DASH, future archive index)
//! needs the same fragments without mutating the MoQ path. The hook
//! here is the smallest contract that lets `lvqr-cli` wire HLS into
//! the bridge without baking HLS-specific code into `lvqr-ingest`.
//!
//! Implementations must be cheap and non-blocking. The observer is
//! invoked from inside the `rml_rtmp` callback chain, which runs on
//! the RTMP server's tokio task; long work or blocking I/O there will
//! stall ingest. The HLS implementation in `lvqr-cli` spawns a task
//! per push for that reason.

use bytes::Bytes;
use lvqr_cmaf::RawSample;
use lvqr_fragment::Fragment;
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
///
/// Session 29 renamed this from `VideoCodec` and added the
/// audio variants. Audio callers must not pass a video variant
/// (and vice versa); consumers may panic or drop in those cases.
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


/// Shared, dynamically-dispatched fragment observer handle.
pub type SharedFragmentObserver = Arc<dyn FragmentObserver>;

/// Shared, dynamically-dispatched raw-sample observer handle.
pub type SharedRawSampleObserver = Arc<dyn RawSampleObserver>;

/// Observer hook called by [`crate::RtmpMoqBridge`] for every fragment it
/// emits. Implementations stay HLS- / DASH- / archive-agnostic; the
/// bridge only knows it has a list of observers to notify.
pub trait FragmentObserver: Send + Sync {
    /// Called when an init segment becomes available for `(broadcast,
    /// track)`. Fired again on every codec re-config (e.g. mid-stream
    /// resolution change), so implementations should treat repeat
    /// calls as overwrites rather than errors.
    ///
    /// `timescale` is the track's native timescale in Hz (e.g. 90_000
    /// for video, 44_100 / 48_000 for audio) so downstream consumers
    /// that need to render a wall-clock duration from tick counts can
    /// do the division with the right denominator. The LL-HLS bridge
    /// uses this to build a [`lvqr_cmaf::CmafPolicy`] that matches
    /// the real track timescale rather than assuming a hardcoded
    /// default.
    fn on_init(&self, broadcast: &str, track: &str, timescale: u32, init: Bytes);

    /// Called for every video / audio [`Fragment`] the bridge emits,
    /// in DTS order per track.
    fn on_fragment(&self, broadcast: &str, track: &str, fragment: &Fragment);
}

/// Observer hook called by [`crate::RtmpMoqBridge`] for every per-sample
/// unit it sees, *before* the sample is muxed into an fMP4 fragment.
///
/// Sibling to [`FragmentObserver`]. Consumers that need the raw NAL /
/// AAC access unit bytes (notably a future WHEP egress path that
/// packetizes per-NAL into RTP) subscribe here instead of re-parsing
/// `CmafChunk` mdat bodies downstream. The bridge still produces
/// fragments in parallel for the MoQ / HLS path; raw-sample observers
/// are a read-only tap, never replace the fragment path.
///
/// Implementations must be cheap and non-blocking for the same reason
/// `FragmentObserver` must: the observer is fired from inside the
/// `rml_rtmp` callback chain, which runs on the RTMP server's tokio
/// task. Long work or blocking I/O will stall ingest; spawn a tokio
/// task per notification if downstream work is expensive.
pub trait RawSampleObserver: Send + Sync {
    /// Called for every video / audio sample the bridge receives,
    /// in DTS order per track. `track` follows the same `0.mp4` /
    /// `1.mp4` convention [`FragmentObserver`] uses. `codec`
    /// identifies the video codec carried on the sample payload so
    /// codec-aware consumers (notably the `lvqr-whep` WHEP
    /// egress) can pick the matching `str0m::Pt` without having
    /// to sniff NAL headers. Audio samples carry the default
    /// value and the audio path must not branch on it.
    fn on_raw_sample(&self, broadcast: &str, track: &str, codec: MediaCodec, sample: &RawSample);
}

/// Drop-in fragment observer that does nothing. Useful as a default
/// when the caller does not want any side channel.
pub struct NoopFragmentObserver;

impl FragmentObserver for NoopFragmentObserver {
    fn on_init(&self, _broadcast: &str, _track: &str, _timescale: u32, _init: Bytes) {}
    fn on_fragment(&self, _broadcast: &str, _track: &str, _fragment: &Fragment) {}
}

/// Drop-in raw-sample observer that does nothing.
pub struct NoopRawSampleObserver;

impl RawSampleObserver for NoopRawSampleObserver {
    fn on_raw_sample(&self, _broadcast: &str, _track: &str, _codec: MediaCodec, _sample: &RawSample) {}
}

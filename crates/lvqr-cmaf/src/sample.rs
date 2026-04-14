//! Raw-sample input types for the segmenter.
//!
//! Producers that want to drive the Tier 2.3 coalescer emit
//! [`RawSample`] values rather than pre-muxed `moof + mdat`
//! fragments. The coalescer batches samples into partial / segment
//! boundaries according to a [`crate::CmafPolicy`] and builds the
//! `moof + mdat` wire bytes itself via `mp4-atom`, which keeps the
//! producer side free of MP4 box knowledge.
//!
//! The design note lives in [`crate::segmenter`] and explains the
//! producer contract in full.

use bytes::Bytes;

/// One decoded or encoded sample on its way into the coalescer.
///
/// The payload layout depends on the codec:
///
/// * **AVC / HEVC**: AVCC length-prefixed NAL units. The first four
///   bytes of each NAL unit are the 32-bit big-endian length (the
///   Annex-B start code is NOT used).
/// * **AAC**: one raw Access Unit, no ADTS header.
///
/// The producer is authoritative for every field: the coalescer
/// never re-parses the payload to infer keyframe status, never
/// re-derives DTS from PTS, and never changes the sample order. If
/// the producer needs composition reordering (e.g. B-frame DTS/CTS
/// split), it encodes that in [`RawSample::cts_offset`].
#[derive(Debug, Clone)]
pub struct RawSample {
    /// Logical track identifier. Usually the MP4 `track_id` the
    /// init segment published via `mvex.trex`. Callers can use any
    /// `u32` they like as long as every sample on the same
    /// coalescer carries the same value.
    pub track_id: u32,
    /// Decode timestamp in the track's own timescale.
    pub dts: u64,
    /// Signed composition-time offset, `PTS - DTS`, in the track's
    /// timescale. Zero for audio; zero or positive for AVC / HEVC
    /// without B-frames; can be negative for streams with B-frame
    /// reordering. mp4-atom encodes `Trun` as version 1 by default
    /// so negative offsets round-trip correctly.
    pub cts_offset: i32,
    /// Sample duration in the track's timescale.
    pub duration: u32,
    /// Codec payload (see type-level docs for layout).
    pub payload: Bytes,
    /// True iff a decoder can start decoding at this sample with no
    /// prior samples. For AVC this corresponds to an IDR slice; for
    /// HEVC, an IDR / CRA / BLA; for AAC, every sample is a
    /// keyframe so set this to `true` on every AAC sample.
    pub keyframe: bool,
}

impl RawSample {
    /// Build a keyframe sample with a zero composition offset. The
    /// common case for AVC Baseline and every audio track; the
    /// explicit struct literal is always available for unusual
    /// producers.
    pub fn keyframe(track_id: u32, dts: u64, duration: u32, payload: Bytes) -> Self {
        Self {
            track_id,
            dts,
            cts_offset: 0,
            duration,
            payload,
            keyframe: true,
        }
    }

    /// Build a non-keyframe (P / B) sample with a zero composition
    /// offset. The common case for AVC P-frames without reordering.
    pub fn delta(track_id: u32, dts: u64, duration: u32, payload: Bytes) -> Self {
        Self {
            track_id,
            dts,
            cts_offset: 0,
            duration,
            payload,
            keyframe: false,
        }
    }
}

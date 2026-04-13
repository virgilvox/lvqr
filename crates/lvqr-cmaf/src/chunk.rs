//! [`CmafChunk`]: one CMAF chunk aligned for HLS, DASH, and MoQ.

use bytes::Bytes;

/// One chunk emitted by the segmenter.
///
/// A chunk is a complete `moof + mdat` pair in wire-ready bytes plus
/// the segmenter's opinion on whether the chunk starts a new HLS
/// partial boundary, a new DASH segment, and a new MoQ group. Each of
/// those three decisions is independent so the egress crates never
/// need to walk the payload to figure out which boundaries they are on.
///
/// The timing fields are in the track's own timescale (90 kHz for
/// typical video, the sample rate for audio). `dts`/`duration` come
/// directly from the source `Fragment` values the chunk was built from.
#[derive(Debug, Clone)]
pub struct CmafChunk {
    /// Logical track name, matching the MoQ track convention used by
    /// `lvqr_fragment::Fragment::track_id`.
    pub track_id: String,

    /// Wire-ready `moof + mdat` bytes. `Bytes` is reference-counted so
    /// cloning a chunk for fan-out is cheap.
    pub payload: Bytes,

    /// Decode timestamp of the first sample in this chunk.
    pub dts: u64,

    /// Total duration of every sample packed into this chunk.
    pub duration: u64,

    /// Classification of the chunk's boundary role. See [`CmafChunkKind`].
    pub kind: CmafChunkKind,
}

/// Boundary role of a chunk. Split out so the egress crates can match
/// on a single enum value instead of three booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmafChunkKind {
    /// Non-keyframe continuation chunk. HLS will emit an `EXT-X-PART`
    /// without `INDEPENDENT=YES`; DASH will extend the current segment;
    /// MoQ will append to the current group.
    Partial,
    /// First chunk after a keyframe, not yet at a DASH segment boundary.
    /// HLS partial with `INDEPENDENT=YES`; DASH extends; MoQ starts a
    /// new group.
    PartialIndependent,
    /// Segment boundary: first chunk of a new DASH segment. Always
    /// independent. HLS emits both an `EXT-X-PART` and a new
    /// `EXT-X-MEDIA-SEQUENCE` entry; MoQ starts a new group.
    Segment,
}

impl CmafChunkKind {
    /// True if a decoder can start decoding at this chunk without
    /// needing any prior chunk.
    pub fn is_independent(self) -> bool {
        matches!(self, Self::PartialIndependent | Self::Segment)
    }

    /// True if this chunk is the first chunk of a DASH segment.
    pub fn is_segment_start(self) -> bool {
        matches!(self, Self::Segment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_flags_are_consistent() {
        assert!(!CmafChunkKind::Partial.is_independent());
        assert!(!CmafChunkKind::Partial.is_segment_start());

        assert!(CmafChunkKind::PartialIndependent.is_independent());
        assert!(!CmafChunkKind::PartialIndependent.is_segment_start());

        assert!(CmafChunkKind::Segment.is_independent());
        assert!(CmafChunkKind::Segment.is_segment_start());
    }
}

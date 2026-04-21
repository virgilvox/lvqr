//! Core [`Fragment`] type plus companion metadata.
//!
//! A `Fragment` is one media unit: a CMAF chunk, an fMP4 segment, or one
//! MoQ object. The semantics are deliberately CMAF-flavored because that is
//! the container every LVQR egress will eventually speak, but nothing in this
//! file actually parses or writes CMAF bytes. `payload` is opaque `Bytes`
//! from the Fragment model's point of view.

use bytes::Bytes;

/// One unit of media in the LVQR unified model.
///
/// The triple `(track_id, group_id, object_id)` is the addressing scheme
/// shared with MoQ: `track_id` names the logical track (video / audio /
/// catalog / captions), `group_id` bumps at every keyframe (so every group
/// is independently decodable), and `object_id` counts fragments within a
/// group. Projections that do not need this addressing may ignore the
/// fields -- a recorder only cares about `track_id` and monotonic ordering.
///
/// Timestamps are in the track's own timescale. The bridge currently uses
/// 90 kHz for video and `sample_rate` for audio; those conventions are
/// carried on [`FragmentMeta::timescale`], not inside `Fragment` itself,
/// because a single `Fragment` is cheap to clone and should not grow a
/// pointer to its own metadata.
///
/// `payload` is the wire-ready bytes (e.g. an fMP4 `moof + mdat`). Cloning
/// a `Fragment` is cheap because `Bytes` is reference-counted.
#[derive(Debug, Clone)]
pub struct Fragment {
    /// Logical track name, e.g. `"0.mp4"` for video or `"1.mp4"` for audio.
    /// Matches the MoQ track-name convention used by the moq-js catalog.
    pub track_id: String,

    /// Group identifier. Bumps at every keyframe for video tracks. Audio
    /// tracks may use a per-sample group (new group each frame) since every
    /// AAC frame is independently decodable.
    pub group_id: u64,

    /// Monotonic index within the group. Starts at 0 for the init segment
    /// (or the first payload frame when no init segment is carried).
    pub object_id: u64,

    /// MoQ priority hint. 0 is default; higher values preempt lower.
    pub priority: u8,

    /// Decode timestamp in the track's timescale.
    pub dts: u64,

    /// Presentation timestamp in the track's timescale. Equal to `dts` plus
    /// any `cts` (composition) offset from the codec.
    pub pts: u64,

    /// Duration of this fragment in the track's timescale.
    pub duration: u64,

    /// Keyframe / independent / discardable flags.
    pub flags: FragmentFlags,

    /// Wire-ready payload bytes.
    pub payload: Bytes,

    /// Server-side wall-clock (UNIX milliseconds) at which the fragment
    /// was first observed by the ingest path. `0` means the field is
    /// unset -- typical for fragments constructed in tests or in
    /// backfill paths that have no real ingest wall-clock. Used by the
    /// Tier 4 item 4.7 latency SLO tracker to compute server-side
    /// glass-to-glass delta on each subscriber-side delivery; zero
    /// values are skipped by the histogram recorder. Never negative,
    /// so `u64` is a fine fit.
    pub ingest_time_ms: u64,
}

impl Fragment {
    /// Construct a new fragment. All fields are required; prefer this over
    /// `Fragment { .. }` so future field additions force an explicit choice
    /// at every construction site.
    ///
    /// `ingest_time_ms` defaults to `0` (unset); callers that want the Tier
    /// 4 item 4.7 latency SLO tracker to observe this fragment should chain
    /// [`Fragment::with_ingest_time_ms`] directly after `new()`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        track_id: impl Into<String>,
        group_id: u64,
        object_id: u64,
        priority: u8,
        dts: u64,
        pts: u64,
        duration: u64,
        flags: FragmentFlags,
        payload: Bytes,
    ) -> Self {
        Self {
            track_id: track_id.into(),
            group_id,
            object_id,
            priority,
            dts,
            pts,
            duration,
            flags,
            payload,
            ingest_time_ms: 0,
        }
    }

    /// Stamp the fragment with a UNIX-wall-clock milliseconds ingest time.
    /// Typical call site: `Fragment::new(...).with_ingest_time_ms(now_ms)`
    /// inside an ingest protocol's fragment dispatch path. Tier 4 item 4.7
    /// session A.
    pub fn with_ingest_time_ms(mut self, ms: u64) -> Self {
        self.ingest_time_ms = ms;
        self
    }

    /// Size of the payload in bytes. Convenience for metrics reporting.
    pub fn payload_len(&self) -> usize {
        self.payload.len()
    }
}

/// Per-fragment flags.
///
/// Split out so the common `keyframe + independent` case can be matched with
/// a single boolean read, and so future flags (e.g. `end_of_group`,
/// `contains_init_segment`) can be added without churning every call site.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FragmentFlags {
    /// True for I-frames and anything else the decoder can resume on.
    pub keyframe: bool,
    /// True if this fragment does not reference any previous fragment. For
    /// video this is a keyframe; for audio this is every frame (AAC / Opus).
    pub independent: bool,
    /// Hint to the relay/egress that dropping this fragment is acceptable
    /// under backpressure (B-frames, non-reference P-frames).
    pub discardable: bool,
}

impl FragmentFlags {
    /// Common case for a keyframe: also independent, not discardable.
    pub const KEYFRAME: Self = Self {
        keyframe: true,
        independent: true,
        discardable: false,
    };

    /// Common case for an audio frame: independent, not keyframe semantically
    /// (no MoQ group break), not discardable.
    pub const AUDIO: Self = Self {
        keyframe: false,
        independent: true,
        discardable: false,
    };

    /// Common case for a non-reference delta (B-frame): neither keyframe
    /// nor independent, discardable under pressure.
    pub const DELTA_DISCARDABLE: Self = Self {
        keyframe: false,
        independent: false,
        discardable: true,
    };

    /// Plain delta frame: neither keyframe nor discardable.
    pub const DELTA: Self = Self {
        keyframe: false,
        independent: false,
        discardable: false,
    };
}

/// Static metadata describing a fragment stream as a whole.
///
/// A producer emits one `FragmentMeta` before (or alongside) its first
/// `Fragment`. Consumers that need the init segment (disk recorder, MoQ
/// relay) read it from here.
#[derive(Debug, Clone)]
pub struct FragmentMeta {
    /// Codec string in RFC 6381 form, e.g. `"avc1.640028"` or
    /// `"mp4a.40.2"`.
    pub codec: String,
    /// Timescale for `dts`, `pts`, and `duration`. 90000 for typical video;
    /// the sample rate for audio.
    pub timescale: u32,
    /// Optional init segment bytes (e.g. an fMP4 `ftyp+moov`). The recorder
    /// and MoQ adapter both prepend this to the first fragment written so
    /// late-joining subscribers can decode from the next keyframe.
    pub init_segment: Option<Bytes>,
}

impl FragmentMeta {
    pub fn new(codec: impl Into<String>, timescale: u32) -> Self {
        Self {
            codec: codec.into(),
            timescale,
            init_segment: None,
        }
    }

    pub fn with_init_segment(mut self, init: Bytes) -> Self {
        self.init_segment = Some(init);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_new_preserves_fields() {
        let payload = Bytes::from_static(b"hello");
        let f = Fragment::new("0.mp4", 7, 3, 0, 100, 110, 33, FragmentFlags::KEYFRAME, payload.clone());
        assert_eq!(f.track_id, "0.mp4");
        assert_eq!(f.group_id, 7);
        assert_eq!(f.object_id, 3);
        assert_eq!(f.dts, 100);
        assert_eq!(f.pts, 110);
        assert_eq!(f.duration, 33);
        assert_eq!(f.payload, payload);
        assert_eq!(f.payload_len(), 5);
        assert!(f.flags.keyframe);
        assert!(f.flags.independent);
        assert!(!f.flags.discardable);
    }

    #[test]
    fn flag_presets_are_consistent() {
        // Each preset is compared field-by-field against a hand-built
        // `FragmentFlags` so the test checks struct-equality semantics,
        // not just `const` field reads (which clippy rightly flags as
        // assertions on constants).
        assert_eq!(
            FragmentFlags::KEYFRAME,
            FragmentFlags {
                keyframe: true,
                independent: true,
                discardable: false,
            }
        );
        assert_eq!(
            FragmentFlags::AUDIO,
            FragmentFlags {
                keyframe: false,
                independent: true,
                discardable: false,
            }
        );
        assert_eq!(
            FragmentFlags::DELTA_DISCARDABLE,
            FragmentFlags {
                keyframe: false,
                independent: false,
                discardable: true,
            }
        );
        assert_eq!(
            FragmentFlags::DELTA,
            FragmentFlags {
                keyframe: false,
                independent: false,
                discardable: false,
            }
        );
    }

    #[test]
    fn fragment_meta_builder() {
        let m = FragmentMeta::new("avc1.640028", 90000).with_init_segment(Bytes::from_static(b"ftyp-stub"));
        assert_eq!(m.codec, "avc1.640028");
        assert_eq!(m.timescale, 90000);
        assert_eq!(m.init_segment.as_ref().unwrap().as_ref(), b"ftyp-stub");
    }
}

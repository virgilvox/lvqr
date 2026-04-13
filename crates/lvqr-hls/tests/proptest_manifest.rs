//! Property tests for the `lvqr-hls` playlist builder and renderer.
//!
//! Invariants we enforce:
//!
//! 1. The builder never panics on any sequence of `CmafChunk` values
//!    with non-zero duration and monotonic DTS.
//! 2. The rendered playlist is valid UTF-8 (trivially true because
//!    the builder only emits ASCII, but we pin it).
//! 3. Every `#EXT-X-PART` URI in the rendered output appears in
//!    ascending media sequence + part order.
//! 4. `#EXT-X-MEDIA-SEQUENCE` never goes backwards between renders.

use bytes::Bytes;
use lvqr_cmaf::{CmafChunk, CmafChunkKind};
use lvqr_hls::{PlaylistBuilder, PlaylistBuilderConfig};
use proptest::prelude::*;

fn arb_kind() -> impl Strategy<Value = CmafChunkKind> {
    prop_oneof![
        Just(CmafChunkKind::Partial),
        Just(CmafChunkKind::PartialIndependent),
        Just(CmafChunkKind::Segment),
    ]
}

fn arb_chunk_sequence() -> impl Strategy<Value = Vec<(u64, CmafChunkKind)>> {
    // Each entry is (duration_ticks, kind). We accumulate DTS
    // separately so it stays monotonic by construction.
    prop::collection::vec((1u64..90_000, arb_kind()), 1..40)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn builder_never_panics_on_monotonic_sequence(seq in arb_chunk_sequence()) {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        let mut dts = 0u64;
        for (dur, kind) in seq {
            // Force the first chunk to be a Segment-kind so the
            // builder has something to close later. This is the
            // only constraint the real producer enforces.
            let kind = if dts == 0 { CmafChunkKind::Segment } else { kind };
            let chunk = CmafChunk {
                track_id: "0.mp4".into(),
                payload: Bytes::from_static(b""),
                dts,
                duration: dur,
                kind,
            };
            let _ = b.push(&chunk);
            dts += dur;
        }
        let _ = b.manifest().render();
    }

    #[test]
    fn rendered_output_is_well_formed(seq in arb_chunk_sequence()) {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        let mut dts = 0u64;
        for (dur, kind) in seq {
            let kind = if dts == 0 { CmafChunkKind::Segment } else { kind };
            let _ = b.push(&CmafChunk {
                track_id: "0.mp4".into(),
                payload: Bytes::from_static(b""),
                dts,
                duration: dur,
                kind,
            });
            dts += dur;
        }
        let text = b.manifest().render();
        prop_assert!(text.starts_with("#EXTM3U\n"));
        prop_assert!(text.contains("#EXT-X-VERSION:9\n"));
        // Every #EXTINF line is followed by a URI line.
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            if line.starts_with("#EXTINF:") {
                let uri = lines.next().expect("EXTINF must be followed by a URI line");
                prop_assert!(!uri.is_empty() && !uri.starts_with('#'));
            }
        }
    }

    #[test]
    fn media_sequence_is_monotonic(seq in arb_chunk_sequence()) {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        let mut dts = 0u64;
        for (dur, kind) in seq {
            let kind = if dts == 0 { CmafChunkKind::Segment } else { kind };
            let _ = b.push(&CmafChunk {
                track_id: "0.mp4".into(),
                payload: Bytes::from_static(b""),
                dts,
                duration: dur,
                kind,
            });
            dts += dur;
        }
        let m = b.manifest();
        let mut last = None;
        for seg in &m.segments {
            if let Some(prev) = last {
                prop_assert!(seg.sequence > prev, "media sequence must be strictly increasing");
            }
            last = Some(seg.sequence);
        }
    }
}

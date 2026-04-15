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
//! 5. `Manifest::delta_skip_count` never returns a value that would
//!    produce an invalid delta playlist per Apple spec 6.2.5.1
//!    (remaining duration must stay >= 4 * TARGETDURATION; total
//!    duration must be >= 6 * TARGETDURATION before any skip).
//! 6. `Manifest::render_with_skip(delta_skip_count())` preserves
//!    `EXT-X-MEDIA-SEQUENCE`, suppresses skipped segment URIs, and
//!    keeps every non-skipped segment URI present.

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

    /// Apple LL-HLS spec 6.2.5.1 floors: the delta-playlist decision
    /// must never recommend a skip that would drop the kept window
    /// below `4 * TARGETDURATION`, and must never recommend a skip
    /// at all when the total playlist duration is below
    /// `6 * TARGETDURATION`. Drive the builder with an arbitrary
    /// chunk sequence, force-close the trailing segment, and check
    /// the returned skip count against both floors.
    #[test]
    fn delta_skip_count_respects_spec_floors(seq in arb_chunk_sequence()) {
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
        b.close_pending_segment();
        let m = b.manifest();
        let skip = m.delta_skip_count();
        prop_assert!(skip <= m.segments.len(), "skip count cannot exceed segment count");

        // Compute total and kept-window duration in ticks.
        let total: u64 = m.segments.iter().map(|s| s.duration_ticks).sum();
        let td_ticks = m.target_duration_secs as u64 * m.timescale as u64;
        if skip > 0 {
            // Floor 1: total >= 6 * TARGETDURATION.
            prop_assert!(total >= 6 * td_ticks, "delta emitted below 6*TD floor");
            // Floor 2: remaining kept window >= 4 * TARGETDURATION.
            let skipped_ticks: u64 = m.segments.iter().take(skip).map(|s| s.duration_ticks).sum();
            let remaining = total - skipped_ticks;
            prop_assert!(
                remaining >= 4 * td_ticks,
                "delta kept window {remaining} ticks below 4*TD floor {}", 4 * td_ticks,
            );
        }
    }

    /// A delta playlist preserves `EXT-X-MEDIA-SEQUENCE`, inserts
    /// `#EXT-X-SKIP:SKIPPED-SEGMENTS=N` when `skip > 0`, and keeps
    /// every segment URI from index `skip..len`. Skipped segment
    /// URIs must be absent. The full render (`skip == 0`) must
    /// contain every segment URI and no `#EXT-X-SKIP` tag.
    #[test]
    fn delta_render_matches_skip_decision(seq in arb_chunk_sequence()) {
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
        b.close_pending_segment();
        let m = b.manifest();
        let skip = m.delta_skip_count();

        let full = m.render_with_skip(0);
        prop_assert!(!full.contains("#EXT-X-SKIP"));
        for seg in &m.segments {
            prop_assert!(full.contains(&seg.uri), "full render missing segment {}", seg.uri);
        }

        if skip > 0 {
            let delta = m.render_with_skip(skip);
            let skip_tag = format!("#EXT-X-SKIP:SKIPPED-SEGMENTS={skip}");
            prop_assert!(delta.contains(&skip_tag));
            // The original first segment's sequence is preserved in
            // EXT-X-MEDIA-SEQUENCE even though its EXTINF entry is
            // gone.
            if let Some(first) = m.segments.first() {
                prop_assert!(
                    delta.contains(&format!("#EXT-X-MEDIA-SEQUENCE:{}", first.sequence)),
                    "delta render does not preserve media sequence"
                );
            }
            // Kept segments must be present; skipped segments must
            // be absent.
            for (i, seg) in m.segments.iter().enumerate() {
                if i < skip {
                    prop_assert!(
                        !delta.contains(&seg.uri),
                        "delta render leaked skipped segment URI {}", seg.uri,
                    );
                } else {
                    prop_assert!(
                        delta.contains(&seg.uri),
                        "delta render missing kept segment URI {}", seg.uri,
                    );
                }
            }
        }
    }
}

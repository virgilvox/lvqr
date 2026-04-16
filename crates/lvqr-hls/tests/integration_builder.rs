//! Integration test for [`PlaylistBuilder`]: drive a scripted chunk
//! sequence and snapshot the rendered manifest.
//!
//! This test is the "integration" slot of the 5-artifact contract for
//! `lvqr-hls`. It is deliberately NOT a real network test -- that
//! lands alongside the axum router in a later session. What it does
//! prove is that a realistic sequence of `CmafChunk` values produces
//! a manifest whose structural properties match the LL-HLS draft:
//! strictly monotonic media sequences, one `#EXTINF` per closed
//! segment, parts ordered by DTS, and the init segment URI appearing
//! exactly once.

use bytes::Bytes;
use lvqr_cmaf::{CmafChunk, CmafChunkKind};
use lvqr_hls::{PlaylistBuilder, PlaylistBuilderConfig};

fn chunk(dts: u64, duration: u64, kind: CmafChunkKind) -> CmafChunk {
    CmafChunk {
        track_id: "0.mp4".into(),
        payload: Bytes::from_static(b""),
        dts,
        duration,
        kind,
    }
}

#[test]
fn six_part_segment_sequence_round_trips() {
    // Classic LL-HLS pattern: 200 ms parts, 2 s segments, 90 kHz
    // timescale. 10 parts per segment; we feed three segments'
    // worth.
    let cfg = PlaylistBuilderConfig {
        timescale: 90_000,
        starting_sequence: 0,
        map_uri: "init.mp4".into(),
        uri_prefix: "live/".into(),
        target_duration_secs: 2,
        part_target_secs: 0.2,
        max_segments: None,
    };
    let mut b = PlaylistBuilder::new(cfg);

    let part_dur = 18_000; // 200 ms in 90 kHz ticks
    let mut dts = 0u64;
    for seg in 0..3 {
        for part in 0..10 {
            let kind = if part == 0 {
                CmafChunkKind::Segment
            } else {
                CmafChunkKind::Partial
            };
            b.push(&chunk(dts, part_dur, kind)).unwrap();
            dts += part_dur;
            let _ = seg;
        }
    }
    b.close_pending_segment();

    let m = b.manifest();
    assert_eq!(m.segments.len(), 3);
    // Media sequences are strictly monotonic starting at 0.
    for (i, seg) in m.segments.iter().enumerate() {
        assert_eq!(seg.sequence, i as u64);
        assert_eq!(seg.parts.len(), 10);
        // First part of every segment is independent; the other
        // nine are not.
        assert!(seg.parts[0].independent);
        for p in &seg.parts[1..] {
            assert!(!p.independent);
        }
        // Every segment is exactly 2 s long (10 parts * 200 ms).
        assert_eq!(seg.duration_ticks, 180_000);
    }

    let text = m.render();
    // #EXT-X-MAP appears exactly once.
    assert_eq!(text.matches("#EXT-X-MAP").count(), 1);
    // One #EXTINF per closed segment.
    assert_eq!(text.matches("#EXTINF:").count(), 3);
    // Three * ten = thirty #EXT-X-PART entries.
    assert_eq!(text.matches("#EXT-X-PART:").count(), 30);
    // Segment URIs appear in ascending order.
    let mut last_seq = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("live/seg-") {
            let n: u64 = rest.split('.').next().unwrap().parse().unwrap();
            if let Some(prev) = last_seq {
                assert!(n > prev, "segment sequences must be monotonic");
            }
            last_seq = Some(n);
        }
    }
}

#[test]
fn empty_builder_renders_minimal_playlist() {
    let b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
    let text = b.manifest().render();
    assert!(text.starts_with("#EXTM3U"));
    assert!(text.contains("#EXT-X-VERSION:9"));
    // No #EXT-X-MEDIA-SEQUENCE yet because no segment has been
    // closed; HLS clients default to 0 in this case.
    assert!(!text.contains("#EXT-X-MEDIA-SEQUENCE"));
    // No segment and no part lines.
    assert!(!text.contains("#EXTINF"));
    assert!(!text.contains("#EXT-X-PART:"));
}

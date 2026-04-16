#![no_main]
//! libfuzzer target for `lvqr_hls::PlaylistBuilder`.
//!
//! The builder is a stateful machine reachable from publisher-
//! controlled chunk metadata (DTS, duration, kind). Two
//! session-level changes widened its exposure surface:
//!
//! * Session 33 coalesces closed-segment bytes on every
//!   `close_pending_segment` by walking `manifest.segments[prev..]`
//!   and cloning every constituent part URI. A broken part URI
//!   could trip a panic in the renderer or in the UTF-8 `String`
//!   output path.
//! * Session 34 evicts oldest-first from `manifest.segments` when
//!   `config.max_segments` is exceeded and drains the dropped
//!   URIs via `drain_evicted_uris`. The eviction runs inside
//!   `close_pending_segment`, so the render path must tolerate a
//!   freshly-shrunk segment vector (empty, one, or N entries).
//!
//! This target feeds the fuzzer's bytes as a sequence of
//! `(duration, kind)` tuples into a `PlaylistBuilder` configured
//! with `max_segments = Some(4)` so the eviction path runs on any
//! non-trivial input. Durations are forced non-zero and DTS is
//! made strictly monotonic by the target itself so `push` never
//! fails for spec-compliant reasons; any error return is still
//! tolerated and skipped rather than asserted against. After
//! every successful push the target calls `Manifest::render` and
//! asserts:
//!
//! 1. `render` never panics.
//! 2. The output begins with `#EXTM3U` (the HLS header is
//!    mandatory and is emitted by the renderer's first `writeln!`).
//! 3. Exactly one `#EXTM3U` tag is emitted per render (no stray
//!    duplication from a future path that calls the renderer
//!    recursively).
//!
//! At the end of each fuzz iteration the target also calls
//! `close_pending_segment` and `drain_evicted_uris` so the last
//! open segment and any trailing eviction go through the same
//! render assertion. The final render must still satisfy the
//! invariants above.

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use lvqr_cmaf::{CmafChunk, CmafChunkKind};
use lvqr_hls::{PlaylistBuilder, PlaylistBuilderConfig};

fn assert_render_invariants(b: &PlaylistBuilder) {
    let text = b.manifest().render();
    assert!(
        text.starts_with("#EXTM3U"),
        "playlist must begin with #EXTM3U; got:\n{text}"
    );
    assert_eq!(
        text.matches("#EXTM3U").count(),
        1,
        "playlist must contain exactly one #EXTM3U header; got:\n{text}"
    );
}

fuzz_target!(|data: &[u8]| {
    let cfg = PlaylistBuilderConfig {
        // Small window so any non-trivial input exercises the
        // session-34 eviction path and the session-33 coalesce
        // path against a freshly-shrunk segment vector.
        max_segments: Some(4),
        ..PlaylistBuilderConfig::default()
    };
    let mut b = PlaylistBuilder::new(cfg);
    let mut dts: u64 = 0;

    for pair in data.chunks_exact(2) {
        // Force a non-zero duration in [1, 256] ticks so `push`
        // never short-circuits on ZeroDuration. The builder's
        // non-monotonic check is satisfied by advancing `dts` by
        // the accepted duration only after a successful push.
        let duration = 1u64 + u64::from(pair[0]);
        let kind = match pair[1] % 3 {
            0 => CmafChunkKind::Segment,
            1 => CmafChunkKind::Partial,
            _ => CmafChunkKind::PartialIndependent,
        };
        let chunk = CmafChunk {
            track_id: "0.mp4".into(),
            payload: Bytes::from_static(b""),
            dts,
            duration,
            kind,
        };
        if b.push(&chunk).is_err() {
            continue;
        }
        dts = dts.saturating_add(duration);
        assert_render_invariants(&b);
    }

    // Force-close any still-open segment so the end-of-stream
    // coalesce + eviction path runs under the same invariants.
    b.close_pending_segment();
    // Drain the session-34 eviction buffer; a broken eviction
    // path would surface here via a panic inside `mem::take` or
    // via a later render that referenced a URI that was supposed
    // to be dropped.
    let _ = b.drain_evicted_uris();
    assert_render_invariants(&b);
});

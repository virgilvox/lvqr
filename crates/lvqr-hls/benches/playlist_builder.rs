//! Criterion microbenches for `lvqr_hls::PlaylistBuilder`.
//!
//! These are the first criterion benches in the workspace. The
//! Tier 1 test-infra audit listed "zero criterion benches" as a
//! carry-over gap for several sessions; this harness closes that
//! for `lvqr-hls` first because sessions 33 and 34 put the two
//! hottest mutation paths in this crate: the session-33
//! closed-segment bytes coalesce and the session-34 sliding-
//! window eviction. The benches let a future session verify that
//! either path's cost stays flat as the window grows.
//!
//! Run with:
//!
//!     cargo bench -p lvqr-hls --bench playlist_builder
//!
//! The suite intentionally stays at the unit level: every bench
//! constructs a `PlaylistBuilder` in-memory and feeds it
//! synthetic `CmafChunk` values. No axum router, no cache, no
//! tokio runtime -- those layers are out of scope for a pure
//! renderer microbench and would introduce measurement noise
//! that drowns the signal we care about.

use bytes::Bytes;
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use lvqr_cmaf::{CmafChunk, CmafChunkKind};
use lvqr_hls::{PlaylistBuilder, PlaylistBuilderConfig};

fn mk_chunk(dts: u64, duration: u64, kind: CmafChunkKind) -> CmafChunk {
    CmafChunk {
        track_id: "0.mp4".into(),
        payload: Bytes::from_static(b""),
        dts,
        duration,
        kind,
    }
}

/// Prime a builder to a steady-state window of `segment_count`
/// closed segments, each with 10 partials. Leaves the builder
/// ready to accept the next push so a bench iteration measures
/// the marginal cost of one mutation on a realistic manifest.
fn primed_builder(segment_count: u64, max_segments: Option<usize>) -> (PlaylistBuilder, u64) {
    let cfg = PlaylistBuilderConfig {
        max_segments,
        ..PlaylistBuilderConfig::default()
    };
    let mut b = PlaylistBuilder::new(cfg);
    let part_dur = 18_000u64;
    let mut dts = 0u64;
    for _seg in 0..segment_count {
        for i in 0..10u64 {
            let kind = if i == 0 {
                CmafChunkKind::Segment
            } else {
                CmafChunkKind::Partial
            };
            // `push` cannot fail on monotonic synthetic input.
            b.push(&mk_chunk(dts, part_dur, kind)).unwrap();
            dts += part_dur;
        }
    }
    (b, dts)
}

/// `push(Partial)` in a steady-state builder. Dominant runtime
/// path: the ingest pipeline generates one Partial push per
/// ~200 ms of video, so a linear slowdown in this path would
/// show up as a publisher-side backpressure signal long before
/// it surfaces on the client side.
fn bench_push_partial(c: &mut Criterion) {
    let mut group = c.benchmark_group("push_partial");
    group.throughput(Throughput::Elements(1));
    for &seg_count in &[10u64, 60, 240] {
        group.bench_function(format!("primed_{seg_count}_unbounded"), |b| {
            b.iter_batched(
                || primed_builder(seg_count, None),
                |(mut builder, mut dts)| {
                    builder.push(&mk_chunk(dts, 18_000, CmafChunkKind::Partial)).unwrap();
                    dts += 18_000;
                    (builder, dts)
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// `push(Segment)` in a steady-state builder. This is the path
/// that drives `close_pending_segment`, which in turn runs the
/// session-33 coalesce and (when `max_segments` is set) the
/// session-34 sliding-window eviction. The unbounded variant
/// measures pure close cost; the capped variant measures close
/// cost + eviction overhead.
fn bench_push_segment_boundary(c: &mut Criterion) {
    let mut group = c.benchmark_group("push_segment_boundary");
    group.throughput(Throughput::Elements(1));
    for &seg_count in &[10u64, 60, 240] {
        group.bench_function(format!("primed_{seg_count}_unbounded"), |b| {
            b.iter_batched(
                || primed_builder(seg_count, None),
                |(mut builder, mut dts)| {
                    builder.push(&mk_chunk(dts, 18_000, CmafChunkKind::Segment)).unwrap();
                    dts += 18_000;
                    (builder, dts)
                },
                BatchSize::SmallInput,
            );
        });
        // Match the production cap (60 segments) set in
        // `lvqr-cli` session-34 commit `99b514d`, so the bench
        // measures the same eviction load the serve path sees.
        group.bench_function(format!("primed_{seg_count}_capped60"), |b| {
            b.iter_batched(
                || primed_builder(seg_count, Some(60)),
                |(mut builder, mut dts)| {
                    builder.push(&mk_chunk(dts, 18_000, CmafChunkKind::Segment)).unwrap();
                    dts += 18_000;
                    (builder, dts)
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// `Manifest::render()` cost as a function of the retained
/// window. Dominates the LL-HLS HTTP handler's read path on a
/// `GET /playlist.m3u8` because the renderer rewrites the whole
/// manifest on every call. A quadratic blowup here would surface
/// as a read-side latency spike on a long broadcast before the
/// session-34 eviction cap was in place; now that the cap is
/// wired, this bench documents the expected ceiling.
fn bench_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("render");
    for &seg_count in &[10u64, 60, 240] {
        let (builder, _) = primed_builder(seg_count, None);
        let manifest = builder.manifest().clone();
        group.bench_function(format!("segments_{seg_count}"), |b| {
            b.iter(|| manifest.render());
        });
    }
    group.finish();
}

criterion_group!(benches, bench_push_partial, bench_push_segment_boundary, bench_render);
criterion_main!(benches);

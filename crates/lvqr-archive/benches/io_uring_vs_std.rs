//! Criterion bench for `lvqr_archive::writer::write_segment`.
//!
//! **Tier 4 item 4.1 session B.** Measures throughput + per-segment
//! latency across a realistic segment-size range so the crossover
//! point between the `std::fs` baseline and the `tokio-uring` path
//! is visible. The bench harness itself is path-agnostic: it calls
//! the public `write_segment` API, which routes internally based on
//! the `io-uring` feature + target-OS gates. To compare the two
//! paths, operators use criterion's saved-baseline workflow on a
//! Linux host with kernel >= 5.6:
//!
//! ```text
//! # Capture the std::fs baseline.
//! cargo bench -p lvqr-archive --bench io_uring_vs_std -- \
//!     --save-baseline std
//!
//! # Capture the tokio-uring variant and diff against std.
//! cargo bench -p lvqr-archive --features io-uring \
//!     --bench io_uring_vs_std -- --baseline std
//! ```
//!
//! On non-Linux hosts (macOS, Windows) this bench still compiles and
//! runs -- it measures the `std::fs` path only, because the tokio-
//! uring dep is target-gated out in `Cargo.toml`. That is useful as a
//! smoke test that the bench harness itself is healthy, but the
//! numbers are not comparable to Linux-kernel io_uring numbers
//! because the underlying filesystem + syscall interface is
//! different.
//!
//! # Why these segment sizes
//!
//! | Size | Production shape |
//! |------|------------------|
//! | 4 KiB | AAC-LC access unit or small inter-frame NAL; LVQR's smallest addressable fragment |
//! | 64 KiB | Typical H.264 inter-frame at 3-5 Mb/s |
//! | 256 KiB | Typical H.264 keyframe at 3-5 Mb/s, or low-bitrate HEVC keyframe |
//! | 1 MiB | High-bitrate H.264/HEVC keyframe; 4K ladder top rung |
//!
//! Picking the crossover point between std::fs and io-uring at each
//! size is what decides whether operators should enable the feature:
//! io_uring's per-call `tokio_uring::start` setup cost is fixed, so
//! small segments pay the fixed cost without amortising it across a
//! large write; large segments get the wins io_uring is designed to
//! deliver (batched SQE/CQE, kernel-side dispatch, reduced syscall
//! count).
//!
//! # Bench sizing notes
//!
//! criterion's default measurement time is 5 seconds, which at 1 MiB
//! per iter on a fast NVMe can produce ~10 GB of tempdir writes per
//! variant. That is too aggressive for a shared runner. The harness
//! caps `measurement_time` to 2s and `sample_size` to 30 so a full
//! run stays under 1 GB of tempdir writes at the top segment size.
//! Operators comparing kernels / filesystems can raise the cap with
//! `--measurement-time 10` on the command line.
//!
//! # Tempdir hygiene
//!
//! Each parameterised variant creates a fresh `TempDir`. Criterion
//! runs all iterations for one variant before moving on, so the dir
//! is populated in-run and cleaned up on drop between variants. If
//! your `/tmp` is small, set `TMPDIR=/var/tmp` (or a dedicated
//! benchmark disk) before running. On Linux runs intended to
//! measure the io_uring path specifically, `TMPDIR=/dev/shm` is
//! NOT recommended -- tmpfs bypasses the block-device IO scheduler
//! and hides the very effect this bench is trying to measure.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use lvqr_archive::writer::write_segment;
use tempfile::TempDir;

const SEGMENT_SIZES: &[usize] = &[4 * 1024, 64 * 1024, 256 * 1024, 1024 * 1024];

fn payload(size: usize) -> Vec<u8> {
    // Non-constant bytes so filesystems that deduplicate zero-pages
    // (some tmpfs, some block-device caches) do not give the write
    // path a free ride. The pattern itself is cheap to generate and
    // identical across runs so comparisons stay apples-to-apples.
    (0..size).map(|i| (i & 0xff) as u8).collect()
}

fn bench_write_segment(c: &mut Criterion) {
    let mut group = c.benchmark_group("write_segment");
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(30);

    for &size in SEGMENT_SIZES {
        let data = payload(size);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &data, |b, data| {
            // One TempDir per variant. The seq counter rolls forward
            // across iters so each write lands on its own file; that
            // matches the production workload (monotonic sequence
            // numbers per broadcast + track) and exercises
            // `create_dir_all` exactly once per variant rather than
            // fsyncing an overwrite on every iter.
            let dir = TempDir::new().expect("create bench tempdir");
            let mut seq: u64 = 0;
            b.iter(|| {
                seq += 1;
                let out = write_segment(
                    black_box(dir.path()),
                    black_box("live/bench"),
                    black_box("0.mp4"),
                    black_box(seq),
                    black_box(data.as_slice()),
                )
                .expect("write_segment bench");
                black_box(out);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_write_segment);
criterion_main!(benches);

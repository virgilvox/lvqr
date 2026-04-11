use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use lvqr_core::{Frame, Registry, TrackName};

fn make_frame(seq: u64, keyframe: bool, size: usize) -> Frame {
    Frame::new(seq, seq * 3000, keyframe, Bytes::from(vec![0u8; size]))
}

fn bench_publish_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("registry_fanout");

    for num_subs in [1, 10, 100, 500] {
        group.bench_with_input(BenchmarkId::new("publish_to_n_subs", num_subs), &num_subs, |b, &n| {
            let registry = Registry::new();
            let track = TrackName::new("live/bench");

            // Create N subscribers (hold them so they stay alive)
            let _subs: Vec<_> = (0..n).map(|_| registry.subscribe(&track)).collect();

            let mut seq = 0u64;
            b.iter(|| {
                let frame = make_frame(seq, seq % 30 == 0, 4096);
                black_box(registry.publish(&track, frame));
                seq += 1;
            });
        });
    }

    group.finish();
}

fn bench_publish_payload_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("registry_payload_size");

    for size in [1024, 4096, 65536, 262144] {
        group.bench_with_input(BenchmarkId::new("publish_10_subs", size), &size, |b, &sz| {
            let registry = Registry::new();
            let track = TrackName::new("live/bench");
            let _subs: Vec<_> = (0..10).map(|_| registry.subscribe(&track)).collect();

            let mut seq = 0u64;
            b.iter(|| {
                let frame = make_frame(seq, seq % 30 == 0, sz);
                black_box(registry.publish(&track, frame));
                seq += 1;
            });
        });
    }

    group.finish();
}

fn bench_multi_track(c: &mut Criterion) {
    c.bench_function("publish_50_tracks_10_subs_each", |b| {
        let registry = Registry::new();
        let tracks: Vec<TrackName> = (0..50).map(|i| TrackName::new(format!("live/stream{i}"))).collect();

        let _subs: Vec<Vec<_>> = tracks
            .iter()
            .map(|t| (0..10).map(|_| registry.subscribe(t)).collect())
            .collect();

        let mut seq = 0u64;
        b.iter(|| {
            for track in &tracks {
                let frame = make_frame(seq, seq % 30 == 0, 4096);
                black_box(registry.publish(track, frame));
            }
            seq += 1;
        });
    });
}

criterion_group!(
    benches,
    bench_publish_fanout,
    bench_publish_payload_sizes,
    bench_multi_track
);
criterion_main!(benches);

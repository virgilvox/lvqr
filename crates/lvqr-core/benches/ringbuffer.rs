use bytes::Bytes;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use lvqr_core::RingBuffer;

fn bench_push(c: &mut Criterion) {
    let rb = RingBuffer::new(1024);
    let data = Bytes::from(vec![0u8; 4096]); // typical video frame chunk

    c.bench_function("ringbuf_push_4kb", |b| {
        b.iter(|| {
            rb.push(black_box(data.clone()));
        })
    });
}

fn bench_get(c: &mut Criterion) {
    let rb = RingBuffer::new(1024);
    for i in 0..1024u64 {
        rb.push(Bytes::from(vec![i as u8; 4096]));
    }

    c.bench_function("ringbuf_get_existing", |b| {
        b.iter(|| {
            rb.get(black_box(512));
        })
    });
}

fn bench_push_and_fanout_clone(c: &mut Criterion) {
    let rb = RingBuffer::new(256);

    c.bench_function("ringbuf_push_then_100_clones", |b| {
        b.iter(|| {
            let data = Bytes::from(vec![0u8; 10_000]); // ~10KB frame
            let seq = rb.push(data);
            // Simulate 100 subscribers reading the same frame
            for _ in 0..100 {
                let _ = black_box(rb.get(seq));
            }
        })
    });
}

fn bench_snapshot(c: &mut Criterion) {
    let rb = RingBuffer::new(256);
    for i in 0..256u64 {
        rb.push(Bytes::from(vec![i as u8; 4096]));
    }

    c.bench_function("ringbuf_snapshot_256_entries", |b| {
        b.iter(|| {
            black_box(rb.snapshot());
        })
    });
}

criterion_group!(
    benches,
    bench_push,
    bench_get,
    bench_push_and_fanout_clone,
    bench_snapshot
);
criterion_main!(benches);

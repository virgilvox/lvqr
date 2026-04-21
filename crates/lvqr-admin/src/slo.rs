//! Latency SLO tracker for the `/api/v1/slo` admin route.
//!
//! Tier 4 item 4.7 session A. Records per-subscriber glass-to-glass
//! latency samples on every egress fragment delivery and exposes a
//! percentile snapshot for the admin JSON route + a Prometheus
//! histogram (`lvqr_subscriber_glass_to_glass_ms`) for long-term
//! observability.
//!
//! # Shape
//!
//! The tracker is a shared `Arc<LatencyTracker>` handed to every
//! egress surface that wants to contribute samples. Each
//! `record(broadcast, transport, latency_ms)` call:
//!
//! 1. Fires a `metrics::histogram!` value on the process-wide
//!    metrics recorder (one metric per sample, tagged with
//!    `broadcast` + `transport` labels). The default Prometheus
//!    histogram bucket layout covers the 0..=60_000 ms range we
//!    care about for live video.
//! 2. Pushes the latency sample into a per-(`broadcast`, `transport`)
//!    ring buffer of up to `MAX_SAMPLES_PER_KEY` samples. The ring
//!    buffer is the source of truth for the JSON `/api/v1/slo`
//!    route; we compute p50 / p95 / p99 on demand by sorting the
//!    buffer (cheap: 1024 u64 sort is ~10 us on a modern host, the
//!    route is low-QPS).
//!
//! # Anti-scope (107 A)
//!
//! * **Streaming-quantile estimators (CKMS, HDR, etc.)**. Simple
//!   fixed-size ring buffer + sort-on-query is fine for the admin
//!   route's low-QPS use case and avoids a new dep.
//! * **Per-subscriber breakdowns**. The tracker keys on
//!   `(broadcast, transport)` so admin operators see the aggregate
//!   picture per egress surface; per-subscriber latency drilldown
//!   is a future Grafana-side query atop the histogram output.
//! * **Time-windowed retention**. The ring buffer is size-bounded,
//!   not time-bounded. Samples are FIFO-evicted regardless of age;
//!   a busy broadcast keeps ~1024 recent samples, a quiet one keeps
//!   older but still-relevant samples until new traffic arrives.

use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Cap on how many samples we keep per (broadcast, transport) key.
/// 1024 is a compromise: big enough that p99 is statistically
/// meaningful (10 tail samples in the window), small enough that the
/// sort-on-query path stays cheap.
const MAX_SAMPLES_PER_KEY: usize = 1024;

/// Combined key for a single tracker bucket. Kept separate from
/// `SloEntry`'s shape so the tracker can hash/compare the key
/// without double-cloning strings on every `record` call.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TrackerKey {
    broadcast: String,
    transport: String,
}

/// Ring buffer of latency samples. Oldest samples evict first.
#[derive(Default)]
struct SampleBuffer {
    samples: Vec<u64>,
    next_index: usize,
    total: u64,
}

impl SampleBuffer {
    fn push(&mut self, sample: u64) {
        if self.samples.len() < MAX_SAMPLES_PER_KEY {
            self.samples.push(sample);
        } else {
            self.samples[self.next_index] = sample;
            self.next_index = (self.next_index + 1) % MAX_SAMPLES_PER_KEY;
        }
        self.total = self.total.saturating_add(1);
    }

    /// Compute p50 / p95 / p99 / max over the retained samples.
    /// Returns all-zero values when the buffer is empty.
    fn percentiles(&self) -> (u64, u64, u64, u64) {
        if self.samples.is_empty() {
            return (0, 0, 0, 0);
        }
        let mut sorted: Vec<u64> = self.samples.clone();
        sorted.sort_unstable();
        let n = sorted.len();
        let p50 = sorted[(n * 50 / 100).min(n - 1)];
        let p95 = sorted[(n * 95 / 100).min(n - 1)];
        let p99 = sorted[(n * 99 / 100).min(n - 1)];
        let max = *sorted.last().unwrap();
        (p50, p95, p99, max)
    }
}

/// Per-broadcast + per-transport latency snapshot returned by
/// [`LatencyTracker::snapshot`] and serialized on the
/// `GET /api/v1/slo` route.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SloEntry {
    /// Broadcast name (e.g. `"live/demo"`).
    pub broadcast: String,
    /// Egress surface: `"hls"`, `"ws"`, `"dash"`, `"moq"`, etc.
    pub transport: String,
    /// 50th percentile latency in milliseconds across the retained
    /// sample window.
    pub p50_ms: u64,
    /// 95th percentile latency.
    pub p95_ms: u64,
    /// 99th percentile latency.
    pub p99_ms: u64,
    /// Peak observed latency in the retained sample window.
    pub max_ms: u64,
    /// Count of samples retained in the ring buffer (`<=
    /// MAX_SAMPLES_PER_KEY`).
    pub sample_count: usize,
    /// Total samples ever observed for this key since tracker
    /// construction (not bounded by the ring buffer).
    pub total_observed: u64,
}

/// Thread-safe latency SLO tracker. Cheap to clone (internal state
/// is behind `Arc`).
#[derive(Clone, Default)]
pub struct LatencyTracker {
    buckets: Arc<RwLock<std::collections::HashMap<TrackerKey, SampleBuffer>>>,
}

impl LatencyTracker {
    /// Build a fresh tracker with no samples. Cheap; the typical
    /// caller constructs one per server and clones the handle out
    /// to every egress surface + the admin state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one latency sample. `broadcast` is the source
    /// broadcast name (e.g. `"live/demo"`); `transport` is the
    /// egress surface producing the delivery (e.g. `"hls"`,
    /// `"ws"`). `latency_ms` is the UNIX-wall-clock delta between
    /// the ingest-side fragment stamp and the egress-side emit
    /// point. A sample of `0` is still recorded -- zero latency is
    /// valid (same-tick delivery) and we do not want to conflate
    /// that with an unset ingest timestamp (the caller is expected
    /// to skip the call entirely when the ingest-time stamp is
    /// missing).
    pub fn record(&self, broadcast: &str, transport: &str, latency_ms: u64) {
        metrics::histogram!(
            "lvqr_subscriber_glass_to_glass_ms",
            "broadcast" => broadcast.to_string(),
            "transport" => transport.to_string(),
        )
        .record(latency_ms as f64);

        let key = TrackerKey {
            broadcast: broadcast.to_string(),
            transport: transport.to_string(),
        };
        let mut guard = self.buckets.write();
        guard.entry(key).or_default().push(latency_ms);
    }

    /// Snapshot every tracked `(broadcast, transport)` key with the
    /// current p50 / p95 / p99 / max + sample counts. Sorted by
    /// broadcast then transport so the admin route's JSON output is
    /// deterministic.
    pub fn snapshot(&self) -> Vec<SloEntry> {
        let guard = self.buckets.read();
        let mut out: Vec<SloEntry> = guard
            .iter()
            .map(|(key, buf)| {
                let (p50, p95, p99, max) = buf.percentiles();
                SloEntry {
                    broadcast: key.broadcast.clone(),
                    transport: key.transport.clone(),
                    p50_ms: p50,
                    p95_ms: p95,
                    p99_ms: p99,
                    max_ms: max,
                    sample_count: buf.samples.len(),
                    total_observed: buf.total,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            a.broadcast
                .cmp(&b.broadcast)
                .then_with(|| a.transport.cmp(&b.transport))
        });
        out
    }

    /// Clear every bucket. Test-oriented; no production caller
    /// should need this.
    #[cfg(test)]
    pub(crate) fn clear(&self) {
        self.buckets.write().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_populates_snapshot_with_percentiles() {
        let tracker = LatencyTracker::new();
        // Monotonically increasing samples so p50 / p95 / p99 map
        // cleanly to indices.
        for ms in 1..=100u64 {
            tracker.record("live/demo", "hls", ms);
        }
        let snap = tracker.snapshot();
        assert_eq!(snap.len(), 1);
        let e = &snap[0];
        assert_eq!(e.broadcast, "live/demo");
        assert_eq!(e.transport, "hls");
        assert_eq!(e.sample_count, 100);
        assert_eq!(e.total_observed, 100);
        assert_eq!(e.p50_ms, 51);
        assert_eq!(e.p95_ms, 96);
        assert_eq!(e.p99_ms, 100);
        assert_eq!(e.max_ms, 100);
    }

    #[test]
    fn ring_buffer_evicts_oldest_past_cap() {
        let tracker = LatencyTracker::new();
        for ms in 1..=(MAX_SAMPLES_PER_KEY as u64 + 500) {
            tracker.record("live/demo", "hls", ms);
        }
        let snap = tracker.snapshot();
        let e = &snap[0];
        assert_eq!(e.sample_count, MAX_SAMPLES_PER_KEY);
        assert_eq!(e.total_observed, MAX_SAMPLES_PER_KEY as u64 + 500);
        // Smallest sample in the retained window is at least
        // (MAX+500 - MAX + 1) = 501.
        assert!(e.p50_ms >= 501, "p50 should have shifted after eviction: {}", e.p50_ms);
    }

    #[test]
    fn separate_keys_track_separately() {
        let tracker = LatencyTracker::new();
        tracker.record("live/demo", "hls", 100);
        tracker.record("live/demo", "hls", 200);
        tracker.record("live/demo", "ws", 50);
        tracker.record("live/other", "hls", 10);

        let snap = tracker.snapshot();
        assert_eq!(snap.len(), 3);
        // Sorted by (broadcast, transport).
        assert_eq!(
            (snap[0].broadcast.as_str(), snap[0].transport.as_str()),
            ("live/demo", "hls")
        );
        assert_eq!(
            (snap[1].broadcast.as_str(), snap[1].transport.as_str()),
            ("live/demo", "ws")
        );
        assert_eq!(
            (snap[2].broadcast.as_str(), snap[2].transport.as_str()),
            ("live/other", "hls")
        );
        assert_eq!(snap[0].sample_count, 2);
        assert_eq!(snap[1].sample_count, 1);
        assert_eq!(snap[2].sample_count, 1);
    }

    #[test]
    fn empty_tracker_snapshots_to_empty_vec() {
        let tracker = LatencyTracker::new();
        assert!(tracker.snapshot().is_empty());
    }

    #[test]
    fn clear_resets_the_tracker() {
        let tracker = LatencyTracker::new();
        tracker.record("a", "hls", 10);
        tracker.record("b", "hls", 20);
        assert_eq!(tracker.snapshot().len(), 2);
        tracker.clear();
        assert!(tracker.snapshot().is_empty());
    }
}

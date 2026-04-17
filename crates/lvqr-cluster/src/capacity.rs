//! Capacity advertisement (Tier 3 session D).
//!
//! Every node gossips a rolling snapshot of its CPU utilization,
//! resident memory, and outbound bandwidth so a future load-aware
//! router can spread subscribers without pinging each peer.
//! Tier 3 scope ends at *advertisement*: the data is available on
//! every node, but no routing policy consumes it yet. That policy
//! lands in Tier 4 per `tracking/TIER_3_PLAN.md`.
//!
//! ## Wire shape
//!
//! Each node publishes a single KV entry on its own chitchat state
//! under the key `capacity` containing:
//!
//! ```text
//! {"cpu_pct":0.0,"rss_bytes":0,"bytes_out_per_sec":0}
//! ```
//!
//! Peers read the entry from `node_states()` when assembling their
//! view of cluster membership. A missing or unparseable entry maps
//! to `capacity: None` on the corresponding [`ClusterNode`].
//!
//! ## Sampling is out of scope
//!
//! This module owns the *transport* for capacity values, not the
//! sampler. Callers (lvqr-cli, lvqr-relay, future observability
//! wiring) write into the [`CapacityGauge`] whenever they have a
//! fresh reading; the advertiser task picks up whatever is in the
//! gauge on every tick. Keeping the sampler external means the
//! cluster crate stays dependency-light and test-deterministic --
//! tests feed exact values through the gauge and assert on the
//! wire.
//!
//! LBD #5 (chitchat scope discipline): capacity is a *coarse*
//! 5-second-resolution snapshot. Per-fragment counters and
//! per-subscriber bitrate stay node-local.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use chitchat::Chitchat;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// Well-known KV key every node uses for its capacity entry.
/// Kept short to keep the chitchat delta payload compact at scale.
pub const CAPACITY_KEY: &str = "capacity";

/// Advertised capacity snapshot for one node. Three coarse numbers,
/// updated once per `capacity_advertise_interval` (default 5 s).
///
/// `PartialEq` is derived but `Eq` is not -- `cpu_pct: f32` does not
/// satisfy reflexivity on NaN. Callers that need hashing should
/// quantize to an integer first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeCapacity {
    /// Process CPU utilization as a percentage, `0.0..=100.0` per
    /// logical core aggregate. Caller-supplied; the gauge does not
    /// clamp.
    pub cpu_pct: f32,
    /// Resident set size in bytes.
    pub rss_bytes: u64,
    /// Outbound bandwidth in bytes per second, averaged over the
    /// caller's measurement window.
    pub bytes_out_per_sec: u64,
}

impl NodeCapacity {
    /// Decode a chitchat KV value into a [`NodeCapacity`]. Returns
    /// `None` on any decode failure (missing field, malformed JSON,
    /// unexpected types); callers treat that as "no capacity".
    pub fn decode(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }

    fn encode(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Shared handle for updating this node's advertised capacity.
///
/// Cheap to clone; internally an `Arc` around three atomics. Writes
/// are visible to the advertiser task on its next tick without
/// cross-task coordination. Reads via [`Self::snapshot`] are
/// lock-free.
#[derive(Clone)]
pub struct CapacityGauge {
    inner: Arc<CapacityInner>,
}

impl std::fmt::Debug for CapacityGauge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapacityGauge")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

struct CapacityInner {
    /// `f32` bits so we can update atomically without a lock.
    cpu_pct_bits: AtomicU32,
    rss_bytes: AtomicU64,
    bytes_out_per_sec: AtomicU64,
}

impl Default for CapacityGauge {
    fn default() -> Self {
        Self::new()
    }
}

impl CapacityGauge {
    /// Zero-valued gauge. The first advertisement after bootstrap
    /// therefore publishes zeros until the caller writes real
    /// values; this is the conservative default for a freshly-booted
    /// node that has not had time to measure anything.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CapacityInner {
                cpu_pct_bits: AtomicU32::new(0.0f32.to_bits()),
                rss_bytes: AtomicU64::new(0),
                bytes_out_per_sec: AtomicU64::new(0),
            }),
        }
    }

    /// Atomic read of all three fields. The reads are independent
    /// atomics so in principle a concurrent writer could split the
    /// triple; the 5-second advertisement cadence makes that
    /// indistinguishable from a one-tick-late publish. Acceptable.
    pub fn snapshot(&self) -> NodeCapacity {
        NodeCapacity {
            cpu_pct: f32::from_bits(self.inner.cpu_pct_bits.load(Ordering::Relaxed)),
            rss_bytes: self.inner.rss_bytes.load(Ordering::Relaxed),
            bytes_out_per_sec: self.inner.bytes_out_per_sec.load(Ordering::Relaxed),
        }
    }

    pub fn set_cpu_pct(&self, pct: f32) {
        self.inner.cpu_pct_bits.store(pct.to_bits(), Ordering::Relaxed);
    }

    pub fn set_rss_bytes(&self, bytes: u64) {
        self.inner.rss_bytes.store(bytes, Ordering::Relaxed);
    }

    pub fn set_bytes_out_per_sec(&self, bps: u64) {
        self.inner.bytes_out_per_sec.store(bps, Ordering::Relaxed);
    }

    /// Replace all three fields at once. Equivalent to three
    /// individual setter calls; provided as a convenience for
    /// samplers that compute a full snapshot in one pass.
    pub fn set(&self, capacity: NodeCapacity) {
        self.set_cpu_pct(capacity.cpu_pct);
        self.set_rss_bytes(capacity.rss_bytes);
        self.set_bytes_out_per_sec(capacity.bytes_out_per_sec);
    }
}

/// Write the current gauge snapshot onto `chitchat`'s self node
/// state. Split out so the claim and advertiser paths share the
/// exact same encoder, and so unit tests can drive the write
/// directly without spinning the ticker.
fn publish_snapshot(chitchat: &mut Chitchat, snap: &NodeCapacity) -> Result<(), serde_json::Error> {
    let encoded = snap.encode()?;
    chitchat.self_node_state().set(CAPACITY_KEY, encoded);
    Ok(())
}

/// Spawn the advertiser task. The task ticks every `interval`,
/// snapshots the gauge, and writes the encoded value to the self
/// node state. Exits cleanly when `cancel` fires.
///
/// The first tick fires at `now + interval`, not immediately, so
/// the caller has a predictable window (up to one interval) to set
/// real values on the gauge before the first publish. This matters
/// for deterministic integration tests; in production the 5-second
/// default means at most one cycle of zero-valued advertisement
/// right after bootstrap, which is acceptable.
pub(crate) fn spawn_advertiser(
    chitchat: Arc<Mutex<Chitchat>>,
    gauge: CapacityGauge,
    interval: Duration,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    let effective = interval.max(Duration::from_millis(10));
    tokio::spawn(async move {
        let first = tokio::time::Instant::now() + effective;
        let mut ticker = tokio::time::interval_at(first, effective);
        // Delay on missed ticks so a contended mutex does not
        // produce a burst of catch-up publishes on the next wake.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                _ = ticker.tick() => {
                    let snap = gauge.snapshot();
                    let mut guard = chitchat.lock().await;
                    if let Err(err) = publish_snapshot(&mut guard, &snap) {
                        warn!(%err, "failed to encode capacity snapshot");
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_default_is_zero() {
        let g = CapacityGauge::new();
        let snap = g.snapshot();
        assert_eq!(snap.cpu_pct, 0.0);
        assert_eq!(snap.rss_bytes, 0);
        assert_eq!(snap.bytes_out_per_sec, 0);
    }

    #[test]
    fn gauge_set_individual_fields() {
        let g = CapacityGauge::new();
        g.set_cpu_pct(42.5);
        g.set_rss_bytes(1_234_567);
        g.set_bytes_out_per_sec(890_000);
        let snap = g.snapshot();
        assert!((snap.cpu_pct - 42.5).abs() < f32::EPSILON);
        assert_eq!(snap.rss_bytes, 1_234_567);
        assert_eq!(snap.bytes_out_per_sec, 890_000);
    }

    #[test]
    fn gauge_set_full_snapshot() {
        let g = CapacityGauge::new();
        g.set(NodeCapacity {
            cpu_pct: 12.75,
            rss_bytes: 5,
            bytes_out_per_sec: 7,
        });
        let snap = g.snapshot();
        assert!((snap.cpu_pct - 12.75).abs() < f32::EPSILON);
        assert_eq!(snap.rss_bytes, 5);
        assert_eq!(snap.bytes_out_per_sec, 7);
    }

    #[test]
    fn gauge_clone_shares_state() {
        let a = CapacityGauge::new();
        let b = a.clone();
        a.set_rss_bytes(999);
        assert_eq!(b.snapshot().rss_bytes, 999);
    }

    #[test]
    fn node_capacity_roundtrip_json() {
        let cap = NodeCapacity {
            cpu_pct: 33.25,
            rss_bytes: 10_000_000,
            bytes_out_per_sec: 250_000,
        };
        let encoded = cap.encode().expect("encode");
        let decoded = NodeCapacity::decode(&encoded).expect("decode");
        assert_eq!(decoded, cap);
    }

    #[test]
    fn node_capacity_decode_rejects_garbage() {
        assert!(NodeCapacity::decode("not json").is_none());
        assert!(NodeCapacity::decode("{\"cpu_pct\":\"oops\"}").is_none());
    }

    #[test]
    fn node_capacity_decode_tolerates_extra_fields() {
        // Forward-compat: a future schema bump that adds fields
        // must not break decoding on older readers.
        let raw = r#"{"cpu_pct":1.0,"rss_bytes":2,"bytes_out_per_sec":3,"future_field":"ok"}"#;
        let decoded = NodeCapacity::decode(raw).expect("decode");
        assert_eq!(decoded.cpu_pct, 1.0);
        assert_eq!(decoded.rss_bytes, 2);
        assert_eq!(decoded.bytes_out_per_sec, 3);
    }
}

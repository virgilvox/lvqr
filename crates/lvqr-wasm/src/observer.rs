//! Registry-side installer that runs every new `(broadcast,
//! track)` fragment stream through a [`SharedFilter`].
//!
//! **Tier 4 item 4.2, session B.** Mirrors the
//! [`lvqr_cli::cluster_claim`] and
//! [`lvqr_cli::archive::BroadcasterArchiveIndexer::install`]
//! patterns: one [`lvqr_fragment::FragmentBroadcasterRegistry::
//! on_entry_created`] callback per new broadcaster, one tokio
//! task per broadcaster draining fragments through the filter,
//! per-`(broadcast, track)` atomic counters for observability.
//!
//! The observer runs in tap mode for v1: it sees every fragment
//! but does NOT modify the stream other consumers see. The
//! filter return value drives `fragments_kept` vs
//! `fragments_dropped` counters and a `lvqr_wasm_fragments_total
//! {outcome=keep|drop}` metric, so operators can verify a
//! deployed filter is doing what they expect without the filter
//! having to reach into every downstream egress.
//!
//! Full stream-modifying filter pipelines, where subscribers
//! see the filter's output in place of the original fragment,
//! are deferred. The two clean options for v1.1 are either
//! running the filter on the ingest side so
//! `FragmentBroadcaster::emit` already sees the filtered
//! fragment (requires per-protocol wiring in lvqr-ingest,
//! lvqr-whip, lvqr-srt, and lvqr-rtsp), or wrapping the
//! `FragmentBroadcaster` with an interceptor inside
//! `lvqr-fragment` (a bigger lvqr-fragment change). Both are
//! out of scope for session B.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use lvqr_fragment::{FragmentBroadcasterRegistry, FragmentStream};
use parking_lot::Mutex;

use crate::{FragmentFilter, SharedFilter};

/// Per-`(broadcast, track)` outcome counters.
#[derive(Debug, Default)]
pub struct FilterStats {
    /// Total fragments observed through the filter (kept + dropped).
    pub fragments_seen: AtomicU64,
    /// Fragments the filter returned `Some` for.
    pub fragments_kept: AtomicU64,
    /// Fragments the filter returned `None` for.
    pub fragments_dropped: AtomicU64,
}

type StatsKey = (String, String);

/// Handle returned by [`install_wasm_filter_bridge`]. Holds the
/// spawned per-broadcaster tasks alive for the server lifetime
/// and exposes read-only accessors for tests and the admin
/// API.
#[derive(Clone)]
pub struct WasmFilterBridgeHandle {
    stats: Arc<DashMap<StatsKey, Arc<FilterStats>>>,
    /// Static length of the filter chain installed via
    /// [`install_wasm_filter_bridge`]. Constant for the server's
    /// lifetime; exposed on the handle so operator tooling (the
    /// `/api/v1/wasm-filter` admin route) can report the chain
    /// shape without peeking at the private `SharedFilter` the
    /// bridge was constructed from.
    chain_length: usize,
    // Tasks stay alive until the handle is dropped. Each task
    // ends on its own when the underlying broadcaster closes.
    _tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl std::fmt::Debug for WasmFilterBridgeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmFilterBridgeHandle")
            .field("chain_length", &self.chain_length)
            .field("tracked_broadcasts", &self.stats.len())
            .finish()
    }
}

impl WasmFilterBridgeHandle {
    /// Number of filters in the installed chain. A single-filter
    /// deployment reports 1; an empty `--wasm-filter` deployment
    /// never constructs the bridge, so every live handle carries a
    /// non-zero chain length.
    pub fn chain_length(&self) -> usize {
        self.chain_length
    }

    /// Total fragments observed for `(broadcast, track)` (kept
    /// plus dropped). Returns 0 if the filter has not yet seen
    /// any fragment for this key.
    pub fn fragments_seen(&self, broadcast: &str, track: &str) -> u64 {
        self.stat(broadcast, track)
            .map(|s| s.fragments_seen.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Count of fragments the filter returned `Some` for.
    pub fn fragments_kept(&self, broadcast: &str, track: &str) -> u64 {
        self.stat(broadcast, track)
            .map(|s| s.fragments_kept.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Count of fragments the filter returned `None` for.
    pub fn fragments_dropped(&self, broadcast: &str, track: &str) -> u64 {
        self.stat(broadcast, track)
            .map(|s| s.fragments_dropped.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Enumerate every `(broadcast, track)` pair the filter has
    /// been active on. Useful for the admin API's "show me the
    /// filter tap" endpoint when it lands.
    pub fn tracked(&self) -> Vec<StatsKey> {
        self.stats.iter().map(|e| e.key().clone()).collect()
    }

    fn stat(&self, broadcast: &str, track: &str) -> Option<Arc<FilterStats>> {
        self.stats
            .get(&(broadcast.to_string(), track.to_string()))
            .map(|e| Arc::clone(e.value()))
    }
}

/// Install a [`SharedFilter`] tap on every `(broadcast, track)`
/// pair the registry ever creates. Returns a handle the caller
/// MUST hold for the server lifetime; dropping it aborts every
/// per-broadcaster draining task.
///
/// Each new broadcaster spawns one tokio task that subscribes
/// to the broadcaster's stream and, for every inbound
/// fragment, calls `filter.apply(fragment)`. The return value
/// only drives the per-broadcast counters and a
/// `lvqr_wasm_fragments_total{outcome=keep|drop}` counter; the
/// original fragment still flows to every other subscriber
/// unchanged. See the module-level docs for why the tap is
/// non-modifying in v1.
///
/// `chain_length` is the static number of filters composed inside
/// `filter`. For a single [`crate::WasmFilter`] it is `1`; for a
/// [`crate::ChainFilter`] it is the `len()` of the chain at
/// installation time. Operator tooling reads this via
/// [`WasmFilterBridgeHandle::chain_length`] without needing to
/// pierce the type erasure of the inner `SharedFilter`.
pub fn install_wasm_filter_bridge(
    registry: &FragmentBroadcasterRegistry,
    filter: SharedFilter,
    chain_length: usize,
) -> WasmFilterBridgeHandle {
    let stats: Arc<DashMap<StatsKey, Arc<FilterStats>>> = Arc::new(DashMap::new());
    let tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));

    let stats_cb = Arc::clone(&stats);
    let tasks_cb = Arc::clone(&tasks);
    registry.on_entry_created(move |broadcast, track, bc| {
        let key = (broadcast.to_string(), track.to_string());
        let stat = Arc::clone(
            stats_cb
                .entry(key.clone())
                .or_insert_with(|| Arc::new(FilterStats::default()))
                .value(),
        );
        let mut sub = bc.subscribe();
        let filter = filter.clone();
        let broadcast_label = broadcast.to_string();
        let track_label = track.to_string();
        let task = tokio::spawn(async move {
            while let Some(frag) = sub.next_fragment().await {
                let outcome = filter.apply(frag);
                stat.fragments_seen.fetch_add(1, Ordering::Relaxed);
                match outcome {
                    Some(_) => {
                        stat.fragments_kept.fetch_add(1, Ordering::Relaxed);
                        metrics::counter!("lvqr_wasm_fragments_total", "outcome" => "keep").increment(1);
                    }
                    None => {
                        stat.fragments_dropped.fetch_add(1, Ordering::Relaxed);
                        metrics::counter!("lvqr_wasm_fragments_total", "outcome" => "drop").increment(1);
                    }
                }
            }
            tracing::info!(
                broadcast = %broadcast_label,
                track = %track_label,
                seen = stat.fragments_seen.load(Ordering::Relaxed),
                kept = stat.fragments_kept.load(Ordering::Relaxed),
                dropped = stat.fragments_dropped.load(Ordering::Relaxed),
                "WASM filter tap: broadcaster closed"
            );
        });
        tasks_cb.lock().push(task);
    });

    tracing::info!("WASM filter bridge installed on FragmentBroadcasterRegistry");

    WasmFilterBridgeHandle {
        stats,
        chain_length,
        _tasks: tasks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WasmFilter;
    use bytes::Bytes;
    use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta};
    use std::time::Duration;

    const WAT_NOOP: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "on_fragment") (param i32 i32) (result i32)
            local.get 1))
    "#;

    const WAT_DROP: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "on_fragment") (param i32 i32) (result i32)
            i32.const -1))
    "#;

    fn compile(wat: &str) -> WasmFilter {
        let bytes = wat::parse_str(wat).unwrap();
        WasmFilter::from_bytes(&bytes).unwrap()
    }

    fn sample(i: u64) -> Fragment {
        Fragment::new(
            "0.mp4",
            i,
            0,
            0,
            i * 1000,
            i * 1000,
            1000,
            FragmentFlags::default(),
            Bytes::from(vec![0xAB; 32]),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bridge_counts_fragments_through_no_op_filter() {
        let registry = FragmentBroadcasterRegistry::new();
        let filter = SharedFilter::new(compile(WAT_NOOP));
        let handle = install_wasm_filter_bridge(&registry, filter, 1);

        let bc = registry.get_or_create("live", "0.mp4", FragmentMeta::new("avc1.640028", 90000));
        for i in 0..5 {
            bc.emit(sample(i));
        }
        // The tap task is async; give it a moment to drain.
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(handle.fragments_seen("live", "0.mp4"), 5);
        assert_eq!(handle.fragments_kept("live", "0.mp4"), 5);
        assert_eq!(handle.fragments_dropped("live", "0.mp4"), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drop_filter_increments_only_dropped_counter() {
        let registry = FragmentBroadcasterRegistry::new();
        let filter = SharedFilter::new(compile(WAT_DROP));
        let handle = install_wasm_filter_bridge(&registry, filter, 1);

        let bc = registry.get_or_create("live", "0.mp4", FragmentMeta::new("avc1.640028", 90000));
        for i in 0..3 {
            bc.emit(sample(i));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(handle.fragments_seen("live", "0.mp4"), 3);
        assert_eq!(handle.fragments_kept("live", "0.mp4"), 0);
        assert_eq!(handle.fragments_dropped("live", "0.mp4"), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tap_is_non_modifying_downstream_subscribers_see_original() {
        let registry = FragmentBroadcasterRegistry::new();
        let filter = SharedFilter::new(compile(WAT_DROP));
        let _handle = install_wasm_filter_bridge(&registry, filter, 1);

        let bc = registry.get_or_create("live", "0.mp4", FragmentMeta::new("avc1.640028", 90000));
        let mut downstream = bc.subscribe();
        let expected = sample(42);
        bc.emit(expected.clone());

        let received = downstream.next_fragment().await.expect("downstream receives original");
        assert_eq!(
            received.payload, expected.payload,
            "drop-filter tap must not modify downstream"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tracked_lists_every_broadcast_the_bridge_has_seen() {
        let registry = FragmentBroadcasterRegistry::new();
        let filter = SharedFilter::new(compile(WAT_NOOP));
        let handle = install_wasm_filter_bridge(&registry, filter, 1);

        let _a = registry.get_or_create("live", "0.mp4", FragmentMeta::new("avc1.640028", 90000));
        let _b = registry.get_or_create("live", "1.mp4", FragmentMeta::new("mp4a.40.2", 48000));
        let _c = registry.get_or_create("other", "0.mp4", FragmentMeta::new("avc1.640028", 90000));
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut tracked = handle.tracked();
        tracked.sort();
        assert_eq!(
            tracked,
            vec![
                ("live".to_string(), "0.mp4".to_string()),
                ("live".to_string(), "1.mp4".to_string()),
                ("other".to_string(), "0.mp4".to_string()),
            ]
        );
    }
}

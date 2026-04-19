//! [`FragmentBroadcasterRegistry`]: a `(broadcast, track)`-keyed map of
//! [`FragmentBroadcaster`] handles.
//!
//! A single [`FragmentBroadcaster`] covers one logical track. A real server
//! hosts many concurrent broadcasts, each with a video and an audio track
//! (and potentially more in the future: captions, simulcast layers). The
//! registry is the place where ingest protocols go to look up or create the
//! broadcaster for a `(broadcast_id, track_id)` pair, and where egress
//! paths go to subscribe.
//!
//! Design choices:
//!
//! * Keyed by `(String, String)`. A single string like `"<broadcast>/<track>"`
//!   would be more compact but would require a parser on every lookup and
//!   would be ambiguous for broadcast IDs that contain `/`. Two strings are
//!   unambiguous.
//!
//! * Returns [`Arc<FragmentBroadcaster>`]. The broadcaster is already
//!   clone-cheap (internally `Arc<Shared>` + cloned `Sender`), but wrapping
//!   in an additional `Arc` on the registry side lets the registry hand out
//!   *identity* (pointer-equal handles) rather than *equivalence* (clones of
//!   the inner `Arc`). This matters for tests that assert "all callers got
//!   the same broadcaster" and for future diagnostics that want to hold a
//!   `Weak<FragmentBroadcaster>`.
//!
//! * Concurrent [`FragmentBroadcasterRegistry::get_or_create`] on the same
//!   key is a real scenario: two ingest connections racing to publish the
//!   same `broadcast_id` (which is a misconfiguration but a common one).
//!   The registry resolves the race by double-checking under the write
//!   lock: the second caller finds the first's entry and returns it.
//!
//! * [`FragmentBroadcasterRegistry::remove`] drops the registry-side `Arc`.
//!   Any still-live external clones keep the broadcaster alive until they
//!   drop too. Subscribers see `Closed` only when the *last* producer clone
//!   of the sender is gone, matching [`FragmentBroadcaster`]'s contract.
//!   Session 94 added [`FragmentBroadcasterRegistry::on_entry_removed`]
//!   which fires synchronously from `remove()` after the map write lock is
//!   released. The callback is the mirror of
//!   [`FragmentBroadcasterRegistry::on_entry_created`] and is the
//!   "broadcast-end" lifecycle signal that Tier 4 item 4.3 (C2PA finalize),
//!   item 4.4 (cross-cluster federation gossip), and item 4.5 (AI agent
//!   per-broadcast shutdown) all consume. Firing is driven by explicit
//!   `remove()` calls from ingest protocols at unpublish time (see
//!   `lvqr_ingest::bridge`), not by `Drop`, so callbacks observe a
//!   deterministic ordering and cannot deadlock against producer drops.
//!
//! What this is *not*:
//!
//! * Not a pub/sub topic space. There is no wildcard subscribe, no pattern
//!   matching, no presence API beyond [`keys`](FragmentBroadcasterRegistry::keys)
//!   and [`len`](FragmentBroadcasterRegistry::len). The registry is an
//!   in-process lookup table, not a message bus.
//!
//! * Not a lifecycle manager. The registry does not emit "broadcast started"
//!   / "broadcast stopped" events; that responsibility lives on the existing
//!   `lvqr_core::EventBus`. A future session may adapt the registry to
//!   publish those events alongside its writes.

use crate::broadcaster::FragmentBroadcaster;
use crate::fragment::FragmentMeta;
use crate::stream::FragmentStream;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

type RegistryKey = (String, String);
type RegistryMap = HashMap<RegistryKey, Arc<FragmentBroadcaster>>;

/// Callback invoked by [`FragmentBroadcasterRegistry::get_or_create`] on
/// fresh insertion. Receives `(broadcast, track, broadcaster)` and runs
/// with every registry lock released, so callbacks may freely call back
/// into the registry (e.g. to subscribe) without deadlocking.
///
/// Typical use is the session-59 consumer-side wiring: a broadcaster-
/// native consumer (archive indexer, LL-HLS bridge) registers a callback
/// that spawns a tokio task per new broadcast, subscribes to it, and
/// drains `next_fragment()` into its own sink.
pub type EntryCallback = Arc<dyn Fn(&str, &str, &Arc<FragmentBroadcaster>) + Send + Sync + 'static>;

/// Lookup table for `(broadcast, track)` -> [`FragmentBroadcaster`] handles.
///
/// Thread-safe. Clone-cheap (internal state behind `Arc`).
pub struct FragmentBroadcasterRegistry {
    inner: Arc<RwLock<RegistryMap>>,
    callbacks: Arc<RwLock<Vec<EntryCallback>>>,
    removed_callbacks: Arc<RwLock<Vec<EntryCallback>>>,
}

impl Default for FragmentBroadcasterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FragmentBroadcasterRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            callbacks: Arc::new(RwLock::new(Vec::new())),
            removed_callbacks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register a callback to be invoked whenever
    /// [`FragmentBroadcasterRegistry::get_or_create`] inserts a new
    /// `(broadcast, track)` entry. Racing callers that collapse onto an
    /// existing entry do NOT fire the callback; it runs exactly once per
    /// new broadcaster.
    ///
    /// Callbacks run with every registry lock released so they may freely
    /// call back into the registry (notably `subscribe`) without
    /// deadlocking. They are invoked synchronously on the thread that wins
    /// the double-checked insertion race; long work should be offloaded
    /// via `tokio::spawn` from inside the callback.
    pub fn on_entry_created<F>(&self, callback: F)
    where
        F: Fn(&str, &str, &Arc<FragmentBroadcaster>) + Send + Sync + 'static,
    {
        self.callbacks
            .write()
            .expect("FragmentBroadcasterRegistry callbacks lock poisoned")
            .push(Arc::new(callback));
    }

    /// Register a callback to be invoked whenever
    /// [`FragmentBroadcasterRegistry::remove`] drops a registered
    /// `(broadcast, track)` entry. The callback receives the triple
    /// `(broadcast, track, &Arc<FragmentBroadcaster>)` of the just-removed
    /// entry; callers that need to keep a handle alive past the remove
    /// call can clone the Arc from inside the callback.
    ///
    /// Firing semantics:
    /// * Exactly once per successful `remove()` that returned `Some`.
    ///   `remove()` on an absent key is a no-op and does NOT fire callbacks.
    /// * Synchronously on the thread that called `remove()`, AFTER the map
    ///   write lock is released, so callbacks may freely re-enter the
    ///   registry (`get`, `subscribe`, `remove` another entry, etc.)
    ///   without deadlocking.
    /// * In installation order across multiple registered callbacks.
    /// * Never from `Drop`. Drop-based firing is rejected per design:
    ///   callbacks from `Drop` can deadlock if they take locks the
    ///   dropping thread already holds, and tokio runtime semantics
    ///   inside `Drop` are constrained. Explicit `remove()` gives the
    ///   deterministic fire point Tier 4 items 4.3 / 4.4 / 4.5 need.
    ///
    /// Panics propagate to the `remove()` caller; long work should be
    /// offloaded via `tokio::spawn` from inside the callback, mirroring
    /// [`on_entry_created`](FragmentBroadcasterRegistry::on_entry_created).
    ///
    /// This is the "broadcast-end" lifecycle primitive shared across
    /// Tier 4 items 4.3 (C2PA finalize-on-broadcast-end), 4.4
    /// (federation gossip of broadcast removal), and 4.5 (per-broadcast
    /// AI agent shutdown). The `(broadcast, track, &Arc<...>)` triple
    /// is identical to `on_entry_created`'s so the same closure shape
    /// composes for both signals.
    pub fn on_entry_removed<F>(&self, callback: F)
    where
        F: Fn(&str, &str, &Arc<FragmentBroadcaster>) + Send + Sync + 'static,
    {
        self.removed_callbacks
            .write()
            .expect("FragmentBroadcasterRegistry removed_callbacks lock poisoned")
            .push(Arc::new(callback));
    }

    /// Look up an existing broadcaster for `(broadcast, track)` or create
    /// one with the given metadata.
    ///
    /// Concurrent callers on the same key all receive pointer-equal handles
    /// (the registry double-checks under the write lock on creation).
    /// Metadata passed on creation is installed once; later calls with a
    /// different `meta` are ignored -- callers that need to update metadata
    /// (notably a late init-segment bind) should call
    /// [`FragmentBroadcaster::set_init_segment`] on the returned handle.
    pub fn get_or_create(&self, broadcast: &str, track: &str, meta: FragmentMeta) -> Arc<FragmentBroadcaster> {
        let key = (broadcast.to_string(), track.to_string());
        if let Some(existing) = self
            .inner
            .read()
            .expect("FragmentBroadcasterRegistry read lock poisoned")
            .get(&key)
            .cloned()
        {
            return existing;
        }
        let bc = {
            let mut guard = self
                .inner
                .write()
                .expect("FragmentBroadcasterRegistry write lock poisoned");
            // Double-check: a racing caller may have installed the entry
            // between our read and write-lock acquisition.
            if let Some(existing) = guard.get(&key).cloned() {
                return existing;
            }
            let bc = Arc::new(FragmentBroadcaster::new(track, meta));
            guard.insert(key, Arc::clone(&bc));
            bc
        };
        // Write lock released. Snapshot callbacks and fire outside any
        // registry lock so callbacks may freely subscribe / inspect the
        // registry without deadlocking.
        let callbacks: Vec<EntryCallback> = self
            .callbacks
            .read()
            .expect("FragmentBroadcasterRegistry callbacks lock poisoned")
            .clone();
        for cb in &callbacks {
            cb(broadcast, track, &bc);
        }
        bc
    }

    /// Return a handle to an existing broadcaster, or `None` if no
    /// broadcaster has been registered for `(broadcast, track)` yet. Does
    /// not create on miss; callers that want to create should use
    /// [`FragmentBroadcasterRegistry::get_or_create`].
    pub fn get(&self, broadcast: &str, track: &str) -> Option<Arc<FragmentBroadcaster>> {
        self.inner
            .read()
            .expect("FragmentBroadcasterRegistry read lock poisoned")
            .get(&(broadcast.to_string(), track.to_string()))
            .cloned()
    }

    /// Convenience wrapper over [`FragmentBroadcasterRegistry::get`] that
    /// returns a [`FragmentStream`] subscription directly, or `None` if the
    /// broadcaster for `(broadcast, track)` does not exist yet.
    pub fn subscribe(&self, broadcast: &str, track: &str) -> Option<impl FragmentStream> {
        self.get(broadcast, track).map(|bc| bc.subscribe())
    }

    /// Remove the registry's `Arc` for `(broadcast, track)`. External
    /// clones keep the broadcaster alive until they drop. Returns the
    /// handle that was removed, or `None` if the key was not present.
    ///
    /// On a successful remove, every callback registered via
    /// [`on_entry_removed`](FragmentBroadcasterRegistry::on_entry_removed)
    /// fires synchronously after the map write lock is released and
    /// receives the removed `Arc` by reference. Callbacks may freely
    /// re-enter the registry; see `on_entry_removed` for full firing
    /// semantics. Calls that hit an absent key return `None` without
    /// firing any callback.
    pub fn remove(&self, broadcast: &str, track: &str) -> Option<Arc<FragmentBroadcaster>> {
        let removed = self
            .inner
            .write()
            .expect("FragmentBroadcasterRegistry write lock poisoned")
            .remove(&(broadcast.to_string(), track.to_string()));
        if let Some(ref bc) = removed {
            let callbacks: Vec<EntryCallback> = self
                .removed_callbacks
                .read()
                .expect("FragmentBroadcasterRegistry removed_callbacks lock poisoned")
                .clone();
            for cb in &callbacks {
                cb(broadcast, track, bc);
            }
        }
        removed
    }

    /// Snapshot of every `(broadcast, track)` key currently registered.
    /// The snapshot is cloned from the map under the read lock; callers
    /// may iterate without holding any lock.
    pub fn keys(&self) -> Vec<(String, String)> {
        self.inner
            .read()
            .expect("FragmentBroadcasterRegistry read lock poisoned")
            .keys()
            .cloned()
            .collect()
    }

    /// Count of registered `(broadcast, track)` pairs.
    pub fn len(&self) -> usize {
        self.inner
            .read()
            .expect("FragmentBroadcasterRegistry read lock poisoned")
            .len()
    }

    /// `true` when the registry has no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Clone for FragmentBroadcasterRegistry {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            callbacks: Arc::clone(&self.callbacks),
            removed_callbacks: Arc::clone(&self.removed_callbacks),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fragment::{Fragment, FragmentFlags};
    use bytes::Bytes;

    fn mk_meta() -> FragmentMeta {
        FragmentMeta::new("avc1.640028", 90000)
    }

    fn mk_frag(idx: u64) -> Fragment {
        Fragment::new(
            "0.mp4",
            idx,
            0,
            0,
            idx * 1000,
            idx * 1000,
            1000,
            FragmentFlags::DELTA,
            Bytes::from_static(b"payload"),
        )
    }

    #[test]
    fn get_or_create_twice_same_key_returns_same_arc() {
        let reg = FragmentBroadcasterRegistry::new();
        let a = reg.get_or_create("bcast", "0.mp4", mk_meta());
        let b = reg.get_or_create("bcast", "0.mp4", mk_meta());
        assert!(Arc::ptr_eq(&a, &b), "same-key get_or_create returns the same Arc");
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn distinct_keys_return_distinct_broadcasters() {
        let reg = FragmentBroadcasterRegistry::new();
        let a = reg.get_or_create("bcast", "0.mp4", mk_meta());
        let b = reg.get_or_create("bcast", "1.mp4", mk_meta());
        let c = reg.get_or_create("other", "0.mp4", mk_meta());
        assert!(!Arc::ptr_eq(&a, &b));
        assert!(!Arc::ptr_eq(&a, &c));
        assert!(!Arc::ptr_eq(&b, &c));
        assert_eq!(reg.len(), 3);
    }

    #[test]
    fn get_miss_returns_none() {
        let reg = FragmentBroadcasterRegistry::new();
        assert!(reg.get("missing", "0.mp4").is_none());
        assert!(reg.subscribe("missing", "0.mp4").is_none());
    }

    #[tokio::test]
    async fn emission_via_registry_handle_reaches_registry_subscriber() {
        let reg = FragmentBroadcasterRegistry::new();
        let bc = reg.get_or_create("bcast", "0.mp4", mk_meta());
        let mut sub = reg
            .subscribe("bcast", "0.mp4")
            .expect("broadcaster exists after get_or_create");
        bc.emit(mk_frag(7));
        let f = sub.next_fragment().await.expect("frag");
        assert_eq!(f.group_id, 7);
    }

    #[tokio::test]
    async fn remove_drops_registry_handle_but_keeps_external_clones_alive() {
        let reg = FragmentBroadcasterRegistry::new();
        let external = reg.get_or_create("bcast", "0.mp4", mk_meta());
        let mut sub = external.subscribe();
        assert!(reg.remove("bcast", "0.mp4").is_some());
        assert_eq!(reg.len(), 0);
        // External clone is still alive, so emission still works.
        external.emit(mk_frag(11));
        let f = sub.next_fragment().await.expect("frag");
        assert_eq!(f.group_id, 11);
        // Now drop the external clone: subscriber closes after drain.
        drop(external);
        assert!(sub.next_fragment().await.is_none());
    }

    #[test]
    fn keys_returns_snapshot() {
        let reg = FragmentBroadcasterRegistry::new();
        reg.get_or_create("a", "0.mp4", mk_meta());
        reg.get_or_create("a", "1.mp4", mk_meta());
        reg.get_or_create("b", "0.mp4", mk_meta());
        let mut keys = reg.keys();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                ("a".to_string(), "0.mp4".to_string()),
                ("a".to_string(), "1.mp4".to_string()),
                ("b".to_string(), "0.mp4".to_string()),
            ]
        );
    }

    #[test]
    fn clone_shares_state() {
        let reg = FragmentBroadcasterRegistry::new();
        let reg2 = reg.clone();
        let a = reg.get_or_create("bcast", "0.mp4", mk_meta());
        let b = reg2.get_or_create("bcast", "0.mp4", mk_meta());
        assert!(Arc::ptr_eq(&a, &b), "cloned registry sees the same map");
    }

    #[test]
    fn is_empty_tracks_len() {
        let reg = FragmentBroadcasterRegistry::new();
        assert!(reg.is_empty());
        let _ = reg.get_or_create("bcast", "0.mp4", mk_meta());
        assert!(!reg.is_empty());
        reg.remove("bcast", "0.mp4");
        assert!(reg.is_empty());
    }

    #[test]
    fn on_entry_created_fires_exactly_once_per_new_entry() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reg = FragmentBroadcasterRegistry::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let seen_keys: Arc<std::sync::Mutex<Vec<(String, String)>>> = Arc::new(std::sync::Mutex::new(Vec::new()));

        let c = Arc::clone(&counter);
        let k = Arc::clone(&seen_keys);
        reg.on_entry_created(move |b, t, _bc| {
            c.fetch_add(1, Ordering::Relaxed);
            k.lock().unwrap().push((b.to_string(), t.to_string()));
        });

        // Fires for first get_or_create.
        let _ = reg.get_or_create("a", "0.mp4", mk_meta());
        assert_eq!(counter.load(Ordering::Relaxed), 1);

        // Does NOT fire for repeat get_or_create on same key.
        let _ = reg.get_or_create("a", "0.mp4", mk_meta());
        assert_eq!(
            counter.load(Ordering::Relaxed),
            1,
            "repeat get_or_create is not a fresh insert"
        );

        // Fires for a different key.
        let _ = reg.get_or_create("a", "1.mp4", mk_meta());
        let _ = reg.get_or_create("b", "0.mp4", mk_meta());
        assert_eq!(counter.load(Ordering::Relaxed), 3);

        let keys = seen_keys.lock().unwrap().clone();
        assert_eq!(
            keys,
            vec![
                ("a".to_string(), "0.mp4".to_string()),
                ("a".to_string(), "1.mp4".to_string()),
                ("b".to_string(), "0.mp4".to_string()),
            ]
        );
    }

    #[test]
    fn on_entry_removed_fires_exactly_once_per_successful_remove() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reg = FragmentBroadcasterRegistry::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let seen_keys: Arc<std::sync::Mutex<Vec<(String, String)>>> = Arc::new(std::sync::Mutex::new(Vec::new()));

        let c = Arc::clone(&counter);
        let k = Arc::clone(&seen_keys);
        reg.on_entry_removed(move |b, t, _bc| {
            c.fetch_add(1, Ordering::Relaxed);
            k.lock().unwrap().push((b.to_string(), t.to_string()));
        });

        let _ = reg.get_or_create("a", "0.mp4", mk_meta());
        let _ = reg.get_or_create("a", "1.mp4", mk_meta());
        let _ = reg.get_or_create("b", "0.mp4", mk_meta());

        // Hit on a present key fires once.
        assert!(reg.remove("a", "0.mp4").is_some());
        assert_eq!(counter.load(Ordering::Relaxed), 1);

        // Miss on an absent key does NOT fire.
        assert!(reg.remove("ghost", "0.mp4").is_none());
        assert_eq!(counter.load(Ordering::Relaxed), 1);

        // A second remove on the same key is a miss and does NOT fire.
        assert!(reg.remove("a", "0.mp4").is_none());
        assert_eq!(counter.load(Ordering::Relaxed), 1);

        // Different keys fire independently.
        assert!(reg.remove("a", "1.mp4").is_some());
        assert!(reg.remove("b", "0.mp4").is_some());
        assert_eq!(counter.load(Ordering::Relaxed), 3);

        let keys = seen_keys.lock().unwrap().clone();
        assert_eq!(
            keys,
            vec![
                ("a".to_string(), "0.mp4".to_string()),
                ("a".to_string(), "1.mp4".to_string()),
                ("b".to_string(), "0.mp4".to_string()),
            ]
        );
    }

    #[test]
    fn on_entry_removed_multiple_callbacks_all_fire_in_installation_order() {
        use std::sync::Mutex;

        let reg = FragmentBroadcasterRegistry::new();
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

        let o = Arc::clone(&order);
        reg.on_entry_removed(move |_, _, _| o.lock().unwrap().push("first"));
        let o = Arc::clone(&order);
        reg.on_entry_removed(move |_, _, _| o.lock().unwrap().push("second"));

        let _ = reg.get_or_create("x", "0.mp4", mk_meta());
        assert!(reg.remove("x", "0.mp4").is_some());
        assert_eq!(&*order.lock().unwrap(), &vec!["first", "second"]);
    }

    #[test]
    fn on_entry_removed_callback_receives_the_just_removed_arc() {
        let reg = FragmentBroadcasterRegistry::new();
        let seen: Arc<std::sync::Mutex<Option<Arc<FragmentBroadcaster>>>> = Arc::new(std::sync::Mutex::new(None));
        let seen_clone = Arc::clone(&seen);
        reg.on_entry_removed(move |_, _, bc| {
            *seen_clone.lock().unwrap() = Some(Arc::clone(bc));
        });

        let handle = reg.get_or_create("live", "0.mp4", mk_meta());
        assert!(reg.remove("live", "0.mp4").is_some());
        let captured = seen.lock().unwrap().take().expect("callback ran");
        assert!(
            Arc::ptr_eq(&handle, &captured),
            "callback received the same Arc that get_or_create handed out"
        );
    }

    #[tokio::test]
    async fn on_entry_removed_callback_may_reenter_registry_without_deadlock() {
        use std::sync::atomic::{AtomicBool, Ordering};

        // Re-entrancy is the load-bearing property: remove() drops its
        // write lock before firing callbacks so callbacks can freely
        // call back into the registry (e.g. to gossip, to inspect
        // remaining keys) without tripping RwLock re-entrance panics.
        let reg = FragmentBroadcasterRegistry::new();
        let fired = Arc::new(AtomicBool::new(false));

        let reg_clone = reg.clone();
        let fired_clone = Arc::clone(&fired);
        reg.on_entry_removed(move |b, t, _bc| {
            assert!(
                reg_clone.get(b, t).is_none(),
                "removed entry is already gone by the time the callback fires"
            );
            let _snapshot = reg_clone.keys();
            fired_clone.store(true, Ordering::Relaxed);
        });

        let _ = reg.get_or_create("live", "0.mp4", mk_meta());
        assert!(reg.remove("live", "0.mp4").is_some());
        assert!(fired.load(Ordering::Relaxed), "callback ran");
    }

    #[test]
    fn on_entry_created_multiple_callbacks_all_fire() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reg = FragmentBroadcasterRegistry::new();
        let a = Arc::new(AtomicUsize::new(0));
        let b = Arc::new(AtomicUsize::new(0));

        let ac = Arc::clone(&a);
        reg.on_entry_created(move |_, _, _| {
            ac.fetch_add(1, Ordering::Relaxed);
        });
        let bc = Arc::clone(&b);
        reg.on_entry_created(move |_, _, _| {
            bc.fetch_add(1, Ordering::Relaxed);
        });

        let _ = reg.get_or_create("x", "0.mp4", mk_meta());
        assert_eq!(a.load(Ordering::Relaxed), 1);
        assert_eq!(b.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn on_entry_created_callback_may_subscribe_without_deadlock() {
        use crate::BroadcasterStream;
        use crate::FragmentStream;
        use std::sync::atomic::{AtomicBool, Ordering};

        let reg = FragmentBroadcasterRegistry::new();
        let subscribed = Arc::new(AtomicBool::new(false));

        // The callback both inspects the registry AND subscribes to the
        // fresh broadcaster. Writing this without a deadlock is the load-
        // bearing property: if get_or_create fired callbacks under the
        // registry's write lock, the subscribe() + registry.get() here
        // would deadlock or reach for re-entrant locks.
        let reg_clone = reg.clone();
        let sub_flag = Arc::clone(&subscribed);
        let sub_holder: Arc<std::sync::Mutex<Option<BroadcasterStream>>> = Arc::new(std::sync::Mutex::new(None));
        let sub_holder_c = Arc::clone(&sub_holder);
        reg.on_entry_created(move |b, t, bc| {
            assert!(reg_clone.get(b, t).is_some(), "entry is visible during callback");
            let s = bc.subscribe();
            *sub_holder_c.lock().unwrap() = Some(s);
            sub_flag.store(true, Ordering::Relaxed);
        });

        let bc = reg.get_or_create("live", "0.mp4", mk_meta());
        assert!(subscribed.load(Ordering::Relaxed), "callback ran");

        // Confirm the subscription is live by emitting through bc and
        // reading on the held sub.
        bc.emit(Fragment::new(
            "0.mp4",
            0,
            0,
            0,
            0,
            0,
            1000,
            FragmentFlags::KEYFRAME,
            Bytes::from_static(b"hello"),
        ));
        let mut sub = sub_holder.lock().unwrap().take().expect("sub stashed");
        let f: Fragment = sub.next_fragment().await.expect("frag arrives");
        assert_eq!(f.payload.as_ref(), b"hello");
    }
}

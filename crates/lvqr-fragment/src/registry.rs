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

/// Lookup table for `(broadcast, track)` -> [`FragmentBroadcaster`] handles.
///
/// Thread-safe. Clone-cheap (internal state behind `Arc`).
pub struct FragmentBroadcasterRegistry {
    inner: Arc<RwLock<RegistryMap>>,
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
        }
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
    pub fn remove(&self, broadcast: &str, track: &str) -> Option<Arc<FragmentBroadcaster>> {
        self.inner
            .write()
            .expect("FragmentBroadcasterRegistry write lock poisoned")
            .remove(&(broadcast.to_string(), track.to_string()))
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
}

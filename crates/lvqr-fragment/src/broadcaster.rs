//! [`FragmentBroadcaster`]: single-producer, multi-subscriber fan-out of
//! [`Fragment`] values.
//!
//! This is the primitive every Tier 2.1 ingest path produces into and
//! every Tier 2.1 consumer subscribes through. One
//! [`FragmentBroadcaster`] per `(broadcast, track)` sits behind a
//! shared [`crate::FragmentBroadcasterRegistry`]; every ingest protocol
//! (RTMP, WHIP, SRT, RTSP) publishes via
//! [`crate::FragmentBroadcasterRegistry::get_or_create`] and every
//! consumer (archive, LL-HLS, DASH) subscribes through the uniform
//! [`FragmentStream`] trait. Consumers are decoupled from producers; a
//! new egress can be plugged in by calling
//! [`FragmentBroadcaster::subscribe`] without touching ingest code.
//!
//! Backpressure policy:
//!
//! * Built on [`tokio::sync::broadcast`]. A subscriber that cannot keep up
//!   receives [`tokio::sync::broadcast::error::RecvError::Lagged`] and the
//!   adapter emits a warn-log, counts the skip, and continues. The live
//!   datapath is never blocked by a slow consumer; the slow consumer
//!   experiences lossy delivery. This matches the MoQ philosophy (dropping
//!   under pressure beats stalling the source) and matches how the existing
//!   observer hook silently drops on a full channel.
//!
//! * The producer side ([`FragmentBroadcaster::emit`]) returns the number of
//!   active subscribers that *successfully* received the fragment. A zero
//!   return value means the fragment was produced into a broadcaster with
//!   no live subscribers; that is not an error, it is the expected state
//!   before the first egress connects.
//!
//! Init segment handling:
//!
//! * [`FragmentBroadcaster::set_init_segment`] updates the metadata the
//!   broadcaster carries so late subscribers can read it from
//!   [`FragmentStream::meta`] immediately after [`subscribe`]. Fragments
//!   already in flight are not re-emitted; the init segment is pulled from
//!   meta, not from a re-played fragment.
//!
//! What this primitive is *not*:
//!
//! * Not a replacement for the MoQ projection. MoQ groups/objects are still
//!   produced on the consumer side via [`crate::MoqTrackSink`] after a
//!   subscriber drains fragments from the broadcaster. The broadcaster is
//!   an in-process fan-out, not a wire format.
//!
//! * Not a history buffer. Subscribers that connect after the first fragment
//!   has been emitted only see subsequent fragments. DVR replay is a
//!   separate adapter against the archive index.
//!
//! [`subscribe`]: FragmentBroadcaster::subscribe

use crate::fragment::{Fragment, FragmentMeta};
use crate::stream::FragmentStream;
use bytes::Bytes;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tracing::warn;

/// Default capacity of the in-memory ring buffer behind a broadcaster. Sized
/// generously because Fragments are cheap to clone (payload is `Bytes`). A
/// slower subscriber still lags, but it takes a real backpressure problem to
/// fall more than a few seconds of media behind.
pub const DEFAULT_BROADCASTER_CAPACITY: usize = 1024;

/// Single-producer, multi-subscriber fan-out of [`Fragment`] values.
///
/// Clone-cheap (internally wraps an [`Arc`]). The producer side owns the
/// canonical [`FragmentMeta`]; subscribers read a cloned snapshot at the
/// moment [`FragmentBroadcaster::subscribe`] is called.
pub struct FragmentBroadcaster {
    shared: Arc<Shared>,
    tx: broadcast::Sender<Fragment>,
}

/// State shared between the broadcaster and its subscribers. Deliberately
/// does NOT contain the `broadcast::Sender`: subscribers must not extend
/// the sender's lifetime, otherwise `recv()` would never return `Closed`
/// after every producer-side clone has been dropped.
struct Shared {
    track_id: String,
    meta: RwLock<FragmentMeta>,
    emitted: AtomicU64,
    lagged_skips: AtomicU64,
}

fn read_meta(lock: &RwLock<FragmentMeta>) -> FragmentMeta {
    lock.read().expect("FragmentBroadcaster meta lock poisoned").clone()
}

impl FragmentBroadcaster {
    /// Construct a broadcaster with [`DEFAULT_BROADCASTER_CAPACITY`].
    pub fn new(track_id: impl Into<String>, meta: FragmentMeta) -> Self {
        Self::with_capacity(track_id, meta, DEFAULT_BROADCASTER_CAPACITY)
    }

    /// Construct a broadcaster with an explicit ring-buffer capacity. Call
    /// sites that know their worst-case subscriber lag (e.g. an archive sink
    /// that may stall on disk fsync) can tune this up; call sites with
    /// bounded and fast consumers can tune it down.
    pub fn with_capacity(track_id: impl Into<String>, meta: FragmentMeta, capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(1));
        Self {
            shared: Arc::new(Shared {
                track_id: track_id.into(),
                meta: RwLock::new(meta),
                emitted: AtomicU64::new(0),
                lagged_skips: AtomicU64::new(0),
            }),
            tx,
        }
    }

    /// Track identifier (opaque string). Matches the `track_id` on emitted
    /// fragments.
    pub fn track_id(&self) -> &str {
        &self.shared.track_id
    }

    /// Snapshot the current metadata. Cheap clone.
    pub fn meta(&self) -> FragmentMeta {
        read_meta(&self.shared.meta)
    }

    /// Update the init-segment bytes carried on the metadata. Future
    /// subscribers see the new init segment immediately; existing
    /// subscribers must poll [`FragmentStream::meta`] to observe the update.
    pub fn set_init_segment(&self, init: Bytes) {
        let mut guard = self
            .shared
            .meta
            .write()
            .expect("FragmentBroadcaster meta lock poisoned");
        guard.init_segment = Some(init);
    }

    /// Replace the entire metadata. Rarely needed; prefer
    /// [`set_init_segment`] when only the init bytes change.
    ///
    /// [`set_init_segment`]: FragmentBroadcaster::set_init_segment
    pub fn replace_meta(&self, meta: FragmentMeta) {
        let mut guard = self
            .shared
            .meta
            .write()
            .expect("FragmentBroadcaster meta lock poisoned");
        *guard = meta;
    }

    /// Emit a fragment to every live subscriber. Returns the count of
    /// subscribers that received the fragment (zero if no subscribers were
    /// connected at emit time; never an error).
    ///
    /// Subscribers that cannot keep up experience [`broadcast::error::RecvError::Lagged`]
    /// in their stream adapter, which skips and continues. This method does
    /// not block on any subscriber.
    pub fn emit(&self, frag: Fragment) -> usize {
        self.shared.emitted.fetch_add(1, Ordering::Relaxed);
        self.tx.send(frag).unwrap_or_default()
    }

    /// Subscribe a new consumer. Returns a [`FragmentStream`] that will
    /// deliver every fragment emitted from this call forward. The initial
    /// [`FragmentStream::meta`] snapshot is the broadcaster's current meta.
    /// Dropping every producer-side clone of the broadcaster closes the
    /// subscriber's stream (the next `next_fragment` returns `None` after
    /// the in-flight ring buffer drains).
    pub fn subscribe(&self) -> BroadcasterStream {
        let rx = self.tx.subscribe();
        let meta = read_meta(&self.shared.meta);
        BroadcasterStream {
            meta,
            track_id: self.shared.track_id.clone(),
            rx,
            source: Arc::clone(&self.shared),
        }
    }

    /// Current subscriber count. Useful for tests and for deciding whether
    /// to skip expensive emission work when nobody is listening.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Total fragments emitted since construction (across all subscribers).
    pub fn emitted_count(&self) -> u64 {
        self.shared.emitted.load(Ordering::Relaxed)
    }

    /// Total lag-skips observed across all live subscribers. Exposed for
    /// metrics / diagnostics; a non-zero value means at least one subscriber
    /// was too slow to consume the ring buffer in time.
    pub fn lagged_skips(&self) -> u64 {
        self.shared.lagged_skips.load(Ordering::Relaxed)
    }
}

impl Clone for FragmentBroadcaster {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            tx: self.tx.clone(),
        }
    }
}

/// [`FragmentStream`] backed by a [`FragmentBroadcaster`] subscription.
///
/// The adapter skips lag errors silently (counting them via the parent
/// broadcaster) and terminates when every producer-side clone of the
/// broadcaster has been dropped.
pub struct BroadcasterStream {
    meta: FragmentMeta,
    track_id: String,
    rx: broadcast::Receiver<Fragment>,
    source: Arc<Shared>,
}

impl BroadcasterStream {
    /// Track identifier inherited from the source broadcaster.
    pub fn track_id(&self) -> &str {
        &self.track_id
    }

    /// Refresh the local metadata snapshot from the source broadcaster.
    /// Useful after a late init-segment bind: subscribers that connected
    /// before [`FragmentBroadcaster::set_init_segment`] was called can pull
    /// the new meta without resubscribing.
    pub fn refresh_meta(&mut self) {
        self.meta = read_meta(&self.source.meta);
    }
}

impl FragmentStream for BroadcasterStream {
    fn meta(&self) -> &FragmentMeta {
        &self.meta
    }

    fn next_fragment<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Option<Fragment>> + Send + 'a>> {
        Box::pin(async move {
            loop {
                match self.rx.recv().await {
                    Ok(frag) => return Some(frag),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        self.source.lagged_skips.fetch_add(skipped, Ordering::Relaxed);
                        warn!(
                            track = %self.track_id,
                            skipped,
                            "FragmentBroadcaster: subscriber lagged, skipped fragments",
                        );
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fragment::FragmentFlags;

    fn mk_frag(idx: u64, payload: &'static [u8]) -> Fragment {
        Fragment::new(
            "0.mp4",
            idx,
            0,
            0,
            idx * 3000,
            idx * 3000,
            3000,
            FragmentFlags::DELTA,
            Bytes::from_static(payload),
        )
    }

    #[tokio::test]
    async fn single_subscriber_receives_every_emit() {
        let bc = FragmentBroadcaster::new("0.mp4", FragmentMeta::new("avc1.640028", 90000));
        let mut sub = bc.subscribe();
        assert_eq!(bc.emit(mk_frag(0, b"a")), 1, "one live subscriber");
        assert_eq!(bc.emit(mk_frag(1, b"b")), 1);
        let f0 = sub.next_fragment().await.expect("frag 0");
        assert_eq!(f0.payload.as_ref(), b"a");
        let f1 = sub.next_fragment().await.expect("frag 1");
        assert_eq!(f1.payload.as_ref(), b"b");
    }

    #[tokio::test]
    async fn two_subscribers_both_receive_every_emit() {
        let bc = FragmentBroadcaster::new("0.mp4", FragmentMeta::new("avc1.640028", 90000));
        let mut sub_a = bc.subscribe();
        let mut sub_b = bc.subscribe();
        assert_eq!(bc.emit(mk_frag(0, b"x")), 2, "two live subscribers");
        let a = sub_a.next_fragment().await.expect("a");
        let b = sub_b.next_fragment().await.expect("b");
        assert_eq!(a.payload.as_ref(), b"x");
        assert_eq!(b.payload.as_ref(), b"x");
    }

    #[tokio::test]
    async fn late_subscriber_misses_prior_emits_but_gets_future() {
        let bc = FragmentBroadcaster::new("0.mp4", FragmentMeta::new("avc1.640028", 90000));
        // Nobody subscribed yet: emit goes to /dev/null, return 0.
        assert_eq!(bc.emit(mk_frag(0, b"before")), 0);
        let mut sub = bc.subscribe();
        assert_eq!(bc.emit(mk_frag(1, b"after")), 1);
        let f = sub.next_fragment().await.expect("after-subscribe frag");
        assert_eq!(f.payload.as_ref(), b"after");
    }

    #[tokio::test]
    async fn lagged_subscriber_skips_and_continues() {
        // Tiny capacity forces overrun. The slow subscriber misses the early
        // fragments but resumes on the newest ones.
        let bc = FragmentBroadcaster::with_capacity("0.mp4", FragmentMeta::new("avc1.640028", 90000), 2);
        let mut sub = bc.subscribe();
        // Overrun: emit 5 with capacity 2 before consuming.
        for i in 0..5u64 {
            bc.emit(mk_frag(i, b"payload"));
        }
        // The receiver starts picking up the remaining in-ring items; lag
        // adjustment happens transparently inside next_fragment().
        let mut received = Vec::new();
        for _ in 0..2 {
            if let Some(f) = sub.next_fragment().await {
                received.push(f.group_id);
            }
        }
        assert_eq!(received.len(), 2, "received the in-ring tail");
        assert!(bc.lagged_skips() > 0, "lag skip was counted");
    }

    #[tokio::test]
    async fn dropping_all_producers_closes_subscribers() {
        let bc = FragmentBroadcaster::new("0.mp4", FragmentMeta::new("avc1.640028", 90000));
        let mut sub = bc.subscribe();
        bc.emit(mk_frag(0, b"x"));
        drop(bc);
        let f = sub.next_fragment().await.expect("in-flight frag");
        assert_eq!(f.payload.as_ref(), b"x");
        assert!(sub.next_fragment().await.is_none(), "stream ends when producers drop");
    }

    #[tokio::test]
    async fn set_init_segment_visible_to_new_subscribers() {
        let bc = FragmentBroadcaster::new("0.mp4", FragmentMeta::new("avc1.640028", 90000));
        assert!(bc.meta().init_segment.is_none());
        bc.set_init_segment(Bytes::from_static(b"INIT"));
        let sub = bc.subscribe();
        assert_eq!(sub.meta().init_segment.as_ref().unwrap().as_ref(), b"INIT");
    }

    #[tokio::test]
    async fn refresh_meta_observes_late_init_on_existing_subscriber() {
        let bc = FragmentBroadcaster::new("0.mp4", FragmentMeta::new("avc1.640028", 90000));
        let mut sub = bc.subscribe();
        assert!(sub.meta().init_segment.is_none());
        bc.set_init_segment(Bytes::from_static(b"LATE-INIT"));
        // Local snapshot still stale.
        assert!(sub.meta().init_segment.is_none());
        sub.refresh_meta();
        assert_eq!(sub.meta().init_segment.as_ref().unwrap().as_ref(), b"LATE-INIT");
    }

    #[tokio::test]
    async fn clone_broadcaster_shares_state() {
        let bc = FragmentBroadcaster::new("0.mp4", FragmentMeta::new("avc1.640028", 90000));
        let bc2 = bc.clone();
        let mut sub = bc.subscribe();
        bc2.emit(mk_frag(0, b"from-clone"));
        let f = sub.next_fragment().await.expect("frag");
        assert_eq!(f.payload.as_ref(), b"from-clone");
        assert_eq!(bc.emitted_count(), 1);
        assert_eq!(bc2.emitted_count(), 1, "counters shared across clones");
    }
}

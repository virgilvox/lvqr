//! Dual-dispatch helpers for the Tier 2.1 ingest migration.
//!
//! Every ingest crate (lvqr-rtsp, lvqr-srt, lvqr-whip, lvqr-ingest::bridge)
//! is migrating from the direct [`FragmentObserver`] callback pattern onto
//! the unified [`lvqr_fragment::FragmentBroadcasterRegistry`] surface. The
//! migration is gradual: for a period, every emit site publishes through
//! both paths so existing consumers (HLS, archive) keep working unchanged
//! while new consumers can subscribe via
//! [`FragmentBroadcasterRegistry::subscribe`] and read from the broadcaster
//! side.
//!
//! [`publish_init`] and [`publish_fragment`] centralize the dual dispatch.
//! Every ingest crate calls these from its emit sites instead of
//! hand-coding the observer + broadcaster pair. Once every consumer has
//! moved to the broadcaster side, the observer branch inside these helpers
//! is deleted and the observer trait itself goes away.
//!
//! Design notes:
//!
//! * The observer call happens *first*. This preserves the exact
//!   observable behavior of the pre-migration code path: downstream
//!   tests that assert on observer ordering keep passing.
//!
//! * The broadcaster side uses [`FragmentBroadcasterRegistry::get_or_create`],
//!   which is idempotent under contention (double-checked insertion). The
//!   metadata passed in is only installed on first creation; subsequent
//!   calls with a different `meta` are ignored, matching the registry
//!   contract. If an ingest produces a mid-stream codec reconfig that
//!   changes the timescale, [`FragmentBroadcaster::set_init_segment`]
//!   still applies; the codec string and timescale carried on
//!   [`FragmentMeta`] are informational (for late subscribers).
//!
//! * Cloning `Fragment` is cheap (payload is `Bytes`). The helper takes
//!   the fragment by value from the caller, calls the observer with a
//!   reference, then moves it into `broadcaster.emit`. No unnecessary
//!   clone.

use crate::observer::SharedFragmentObserver;
use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentMeta};

/// Publish an init segment through both dispatch paths.
///
/// See module doc for semantics.
pub fn publish_init(
    observer: Option<&SharedFragmentObserver>,
    registry: &FragmentBroadcasterRegistry,
    broadcast: &str,
    track: &str,
    codec: &str,
    timescale: u32,
    init: Bytes,
) {
    if let Some(obs) = observer {
        obs.on_init(broadcast, track, timescale, init.clone());
    }
    let bc = registry.get_or_create(broadcast, track, FragmentMeta::new(codec, timescale));
    bc.set_init_segment(init);
}

/// Publish a fragment through both dispatch paths.
///
/// See module doc for semantics.
pub fn publish_fragment(
    observer: Option<&SharedFragmentObserver>,
    registry: &FragmentBroadcasterRegistry,
    broadcast: &str,
    track: &str,
    codec: &str,
    timescale: u32,
    frag: Fragment,
) {
    if let Some(obs) = observer {
        obs.on_fragment(broadcast, track, &frag);
    }
    let bc = registry.get_or_create(broadcast, track, FragmentMeta::new(codec, timescale));
    bc.emit(frag);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observer::FragmentObserver;
    use lvqr_fragment::{FragmentFlags, FragmentStream};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    struct SpyObs {
        init_count: AtomicU32,
        fragments: Mutex<Vec<Fragment>>,
    }

    impl FragmentObserver for SpyObs {
        fn on_init(&self, _broadcast: &str, _track: &str, _timescale: u32, _init: Bytes) {
            self.init_count.fetch_add(1, Ordering::Relaxed);
        }
        fn on_fragment(&self, _broadcast: &str, _track: &str, fragment: &Fragment) {
            self.fragments.lock().unwrap().push(fragment.clone());
        }
    }

    fn mk_frag(seq: u64, is_key: bool, payload: &'static [u8]) -> Fragment {
        Fragment::new(
            "0.mp4",
            seq,
            0,
            0,
            seq * 1000,
            seq * 1000,
            1000,
            if is_key {
                FragmentFlags::KEYFRAME
            } else {
                FragmentFlags::DELTA
            },
            Bytes::from_static(payload),
        )
    }

    #[tokio::test]
    async fn publish_init_writes_to_both_observer_and_registry() {
        let spy = Arc::new(SpyObs {
            init_count: AtomicU32::new(0),
            fragments: Mutex::new(Vec::new()),
        });
        let obs: SharedFragmentObserver = spy.clone();
        let reg = FragmentBroadcasterRegistry::new();

        publish_init(
            Some(&obs),
            &reg,
            "bcast",
            "0.mp4",
            "avc1",
            90_000,
            Bytes::from_static(b"INIT"),
        );

        assert_eq!(spy.init_count.load(Ordering::Relaxed), 1);
        let bc = reg.get("bcast", "0.mp4").expect("broadcaster created");
        assert_eq!(bc.meta().init_segment.as_ref().unwrap().as_ref(), b"INIT");
    }

    #[tokio::test]
    async fn publish_fragment_writes_to_both_observer_and_registry() {
        let spy = Arc::new(SpyObs {
            init_count: AtomicU32::new(0),
            fragments: Mutex::new(Vec::new()),
        });
        let obs: SharedFragmentObserver = spy.clone();
        let reg = FragmentBroadcasterRegistry::new();

        // Subscribe before emit so the frag is delivered.
        let bc = reg.get_or_create("bcast", "0.mp4", FragmentMeta::new("avc1", 90_000));
        let mut sub = bc.subscribe();

        publish_fragment(
            Some(&obs),
            &reg,
            "bcast",
            "0.mp4",
            "avc1",
            90_000,
            mk_frag(1, true, b"kf"),
        );

        assert_eq!(spy.fragments.lock().unwrap().len(), 1, "observer saw fragment");
        let delivered = sub.next_fragment().await.expect("broadcaster saw fragment");
        assert_eq!(delivered.payload.as_ref(), b"kf");
        assert!(delivered.flags.keyframe);
    }

    #[tokio::test]
    async fn publish_with_none_observer_still_feeds_registry() {
        let reg = FragmentBroadcasterRegistry::new();
        let bc = reg.get_or_create("bcast", "0.mp4", FragmentMeta::new("avc1", 90_000));
        let mut sub = bc.subscribe();

        publish_fragment(None, &reg, "bcast", "0.mp4", "avc1", 90_000, mk_frag(1, true, b"kf"));

        let delivered = sub.next_fragment().await.expect("broadcaster saw fragment");
        assert_eq!(delivered.payload.as_ref(), b"kf");
    }
}

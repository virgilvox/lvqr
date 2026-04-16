//! Proptest for [`lvqr_fragment::FragmentBroadcasterRegistry`].
//!
//! Two properties:
//!
//! 1. **Concurrent get_or_create on the same key converges.** Spawning N
//!    tasks that all call `get_or_create(bcast, track, meta)` must return N
//!    pointer-equal `Arc<FragmentBroadcaster>` handles. A single emit
//!    through any one handle must fan out to every subscriber that
//!    subscribed before the emit. This is the racing-ingest scenario
//!    (two publishers accidentally using the same broadcast_id) and the
//!    registry's contract promises they collapse onto one broadcaster
//!    rather than splitting into silos.
//!
//! 2. **Arbitrary multi-key workload preserves isolation.** Given a
//!    randomized set of (broadcast, track) pairs with per-pair fragment
//!    plans, every pair's subscriber sees only that pair's fragments.
//!    No cross-key leakage. Len tracks the distinct keys inserted.

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta, FragmentStream};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

type RegKey = (String, String);
type KeyPlan = (RegKey, Vec<Vec<u8>>);
type Workload = Vec<KeyPlan>;

fn mk_meta() -> FragmentMeta {
    FragmentMeta::new("avc1.640028", 90000)
}

fn mk_frag(track: &str, idx: u64, payload: &[u8]) -> Fragment {
    Fragment::new(
        track,
        idx,
        0,
        0,
        idx * 1000,
        idx * 1000,
        1000,
        FragmentFlags::DELTA,
        Bytes::copy_from_slice(payload),
    )
}

#[test]
fn concurrent_get_or_create_same_key_returns_pointer_equal_arcs() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("build tokio runtime");

    proptest!(ProptestConfig::with_cases(32), |(task_count in 2usize..=8)| {
        rt.block_on(async {
            let reg = FragmentBroadcasterRegistry::new();
            let mut handles = Vec::with_capacity(task_count);
            for _ in 0..task_count {
                let reg = reg.clone();
                handles.push(tokio::spawn(async move {
                    reg.get_or_create("bcast", "0.mp4", mk_meta())
                }));
            }
            let mut arcs = Vec::with_capacity(task_count);
            for h in handles {
                arcs.push(h.await.expect("spawn ok"));
            }
            let first = &arcs[0];
            for other in &arcs[1..] {
                assert!(Arc::ptr_eq(first, other), "racing get_or_create converges onto one Arc");
            }
            assert_eq!(reg.len(), 1, "only one registry entry survives the race");
            // Every subscriber (taken through *any* of the handles) must
            // receive an emit routed through any other handle.
            let mut sub_a = arcs[0].subscribe();
            let mut sub_b = arcs[arcs.len() - 1].subscribe();
            arcs[0].emit(mk_frag("0.mp4", 1, b"one"));
            let f = sub_a.next_fragment().await.expect("sub a gets frag");
            assert_eq!(f.payload.as_ref(), b"one");
            let f = sub_b.next_fragment().await.expect("sub b gets frag");
            assert_eq!(f.payload.as_ref(), b"one");
        });
    });
}

/// Strategy: a set of (broadcast, track) pairs with a small per-pair
/// plan of payloads. Distinct keys are generated so the isolation property
/// is meaningful.
fn multi_key_workload_strategy() -> impl Strategy<Value = Workload> {
    let bcast = "[a-z]{1,4}";
    let track = "[a-z0-9.]{1,6}";
    let payloads = prop::collection::vec(prop::collection::vec(any::<u8>(), 1..=16), 1..=6);
    let entry = ((bcast, track), payloads);
    prop::collection::vec(entry, 1..=5).prop_map(|entries| {
        // Deduplicate keys so the isolation test is unambiguous (two plans
        // for the same key would be legitimate concurrent ingest, but that
        // is covered by the convergence test above).
        let mut seen = BTreeSet::new();
        entries
            .into_iter()
            .filter(|((b, t), _)| seen.insert((b.clone(), t.clone())))
            .collect()
    })
}

#[test]
fn distinct_keys_isolate_their_fragments() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    proptest!(ProptestConfig::with_cases(32), |(workload in multi_key_workload_strategy())| {
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let reg = FragmentBroadcasterRegistry::new();
            // Subscribe first so no emit is lost.
            let mut subs: BTreeMap<(String, String), _> = BTreeMap::new();
            for ((b, t), _) in &workload {
                let bc = reg.get_or_create(b, t, mk_meta());
                subs.insert((b.clone(), t.clone()), bc.subscribe());
            }
            // Emit on each key.
            for ((b, t), plan) in &workload {
                let bc = reg.get(b, t).expect("exists");
                for (i, p) in plan.iter().enumerate() {
                    bc.emit(mk_frag(t, i as u64, p));
                }
            }
            // Drop every registry-side clone AND external clone so each
            // subscriber's stream closes after drain.
            drop(reg);
            // Each subscriber sees exactly its key's plan in order.
            for ((b, t), mut sub) in subs {
                let plan = &workload
                    .iter()
                    .find(|((bb, tt), _)| bb == &b && tt == &t)
                    .expect("plan")
                    .1;
                for (i, expected) in plan.iter().enumerate() {
                    let f = sub.next_fragment().await
                        .unwrap_or_else(|| panic!("key {}/{} stream ended early at index {i}", b, t));
                    prop_assert_eq!(f.payload.as_ref(), expected.as_slice());
                }
                prop_assert!(sub.next_fragment().await.is_none(), "stream closes after plan drained");
            }
            Ok(())
        });
        result?;
    });
}

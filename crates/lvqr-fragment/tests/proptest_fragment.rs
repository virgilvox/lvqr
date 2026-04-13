//! Proptest harness for [`lvqr_fragment::Fragment`] field invariants and the
//! [`lvqr_fragment::MoqTrackSink`] adapter round-trip.
//!
//! The sink round-trip is the load-bearing property: whatever bytes go in
//! through `sink.push(Fragment { payload, .. })` must come back out the MoQ
//! subscriber side bit-for-bit, regardless of how many keyframes or deltas
//! are interleaved. This is the Tier 2.1 foundation the whole unified
//! fragment model rests on.

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta, MoqTrackSink};
use lvqr_moq::{OriginProducer, Track};
use proptest::prelude::*;

/// Strategy that generates a list of fragment "payloads" paired with a
/// keyframe flag. At least one entry is guaranteed to be a keyframe (the
/// first one); that matches real streams where a subscriber cannot decode
/// anything before the first I-frame.
fn fragment_plan_strategy() -> impl Strategy<Value = Vec<(bool, Vec<u8>)>> {
    // Each payload is between 1 and 64 bytes. 1..=12 fragments per plan keeps
    // proptest cases cheap while still exercising group boundaries.
    let entry = (proptest::bool::ANY, prop::collection::vec(any::<u8>(), 1..=64));
    prop::collection::vec(entry, 1..=12).prop_map(|mut plan| {
        // Force the first fragment to be a keyframe so there is always at
        // least one open group.
        if let Some(first) = plan.first_mut() {
            first.0 = true;
        }
        plan
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn fragment_new_preserves_every_field(
        track in "[a-z0-9.]{1,16}",
        group_id in any::<u64>(),
        object_id in any::<u64>(),
        priority in any::<u8>(),
        dts in any::<u64>(),
        pts in any::<u64>(),
        duration in any::<u64>(),
        keyframe in any::<bool>(),
        independent in any::<bool>(),
        discardable in any::<bool>(),
        payload in prop::collection::vec(any::<u8>(), 0..=256),
    ) {
        let flags = FragmentFlags { keyframe, independent, discardable };
        let payload_bytes = Bytes::from(payload.clone());
        let f = Fragment::new(
            track.clone(), group_id, object_id, priority, dts, pts, duration, flags, payload_bytes.clone(),
        );
        prop_assert_eq!(&f.track_id, &track);
        prop_assert_eq!(f.group_id, group_id);
        prop_assert_eq!(f.object_id, object_id);
        prop_assert_eq!(f.priority, priority);
        prop_assert_eq!(f.dts, dts);
        prop_assert_eq!(f.pts, pts);
        prop_assert_eq!(f.duration, duration);
        prop_assert_eq!(f.flags.keyframe, keyframe);
        prop_assert_eq!(f.flags.independent, independent);
        prop_assert_eq!(f.flags.discardable, discardable);
        prop_assert_eq!(f.payload_len(), payload.len());
        prop_assert_eq!(f.payload.as_ref(), payload.as_slice());
    }
}

/// Adapter round-trip: push N fragments through a real MoQ origin, then read
/// them back. Every payload byte that went in must come back out. This is
/// the single most important guarantee the Fragment model makes: the MoQ
/// projection is lossless with respect to payload bytes.
#[test]
fn sink_roundtrip_preserves_every_payload_byte() {
    // Tokio runtime inside the proptest body so every case gets a fresh
    // origin and no state leaks between iterations.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    proptest!(|(plan in fragment_plan_strategy())| {
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let origin = OriginProducer::new();
            let mut broadcast = origin
                .create_broadcast("rt-test")
                .expect("create broadcast");
            let track = broadcast
                .create_track(Track::new("0.mp4"))
                .expect("create track");
            let meta = FragmentMeta::new("avc1.640028", 90000);
            let mut sink = MoqTrackSink::new(track, meta);

            // Push every planned fragment.
            let expected_groups: Vec<Vec<Vec<u8>>> = {
                let mut groups: Vec<Vec<Vec<u8>>> = Vec::new();
                for (is_key, payload) in &plan {
                    if *is_key {
                        groups.push(Vec::new());
                    }
                    if let Some(last) = groups.last_mut() {
                        last.push(payload.clone());
                    }
                }
                groups
            };

            for (i, (is_key, payload)) in plan.iter().enumerate() {
                let flags = if *is_key { FragmentFlags::KEYFRAME } else { FragmentFlags::DELTA };
                let f = Fragment::new(
                    "0.mp4", i as u64, i as u64, 0, i as u64, i as u64, 3000, flags,
                    Bytes::from(payload.clone()),
                );
                sink.push(&f).expect("push fragment");
            }
            sink.finish_current_group();

            // Read them back.
            let consumer = origin.consume();
            let bc = consumer
                .consume_broadcast("rt-test")
                .expect("consume broadcast");
            let mut track_consumer = bc
                .subscribe_track(&Track::new("0.mp4"))
                .expect("subscribe");

            for expected_group in &expected_groups {
                let mut g = track_consumer
                    .next_group()
                    .await
                    .expect("next_group ok")
                    .expect("group present");
                for expected_payload in expected_group {
                    let frame: bytes::Bytes = g
                        .read_frame()
                        .await
                        .expect("read_frame ok")
                        .expect("frame present");
                    prop_assert_eq!(frame.as_ref(), expected_payload.as_slice());
                }
            }
            Ok(())
        });
        result?;
    });
}

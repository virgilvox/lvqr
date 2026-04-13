//! Proptest harness for the `lvqr-moq` facade.
//!
//! The facade is a thin re-export layer, so the only invariant worth
//! property-testing is "arbitrary track names round-trip through
//! `Track::new` and `BroadcastProducer::create_track` without loss".
//! If upstream moq-lite ever starts normalizing or validating names, this
//! test is the first place where the behavior change will surface.

use lvqr_moq::{OriginProducer, Track};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn track_name_is_preserved_through_facade(name in "[A-Za-z0-9._/-]{1,32}") {
        let t = Track::new(name.clone());
        prop_assert_eq!(&t.name, &name);
    }
}

#[tokio::test]
async fn multiple_broadcasts_on_one_origin() {
    // Sanity check: a single OriginProducer can host many broadcasts, and
    // every broadcast can host multiple tracks, all through the facade. If
    // moq-lite ever adds per-origin broadcast quotas this test will surface
    // it immediately.
    let origin = OriginProducer::new();
    for i in 0..8u32 {
        let name = format!("broadcast-{i}");
        let mut bc = origin.create_broadcast(&name).expect("create broadcast");
        let _v = bc.create_track(Track::new("0.mp4")).expect("create video");
        let _a = bc.create_track(Track::new("1.mp4")).expect("create audio");
    }
}

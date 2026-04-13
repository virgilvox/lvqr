//! Integration test for the `lvqr-moq` facade: real publish / subscribe over
//! a real `moq_lite::OriginProducer`, exercised entirely through the re-export
//! surface. If a future facade adds newtypes on top of moq-lite, this test
//! is the first place where the newtype wrappers are exercised end-to-end.

use bytes::Bytes;
use lvqr_moq::{OriginProducer, Track};

#[tokio::test]
async fn publish_subscribe_through_facade() {
    let origin = OriginProducer::new();
    let mut broadcast = origin.create_broadcast("facade-integration").expect("create broadcast");
    let mut track = broadcast.create_track(Track::new("0.mp4")).expect("create track");

    // Publish: one group with two frames.
    let mut group = track.append_group().expect("append_group");
    group
        .write_frame(Bytes::from_static(b"frame-1"))
        .expect("write frame 1");
    group
        .write_frame(Bytes::from_static(b"frame-2"))
        .expect("write frame 2");
    let _ = group.finish();

    // Subscribe via the origin consumer side.
    let consumer = origin.consume();
    let bc = consumer
        .consume_broadcast("facade-integration")
        .expect("consume broadcast");
    let mut track_consumer = bc.subscribe_track(&Track::new("0.mp4")).expect("subscribe");

    let mut g = track_consumer
        .next_group()
        .await
        .expect("next_group ok")
        .expect("group present");

    let f1: Bytes = g.read_frame().await.expect("frame 1 ok").expect("frame 1");
    assert_eq!(f1.as_ref(), b"frame-1");
    let f2: Bytes = g.read_frame().await.expect("frame 2 ok").expect("frame 2");
    assert_eq!(f2.as_ref(), b"frame-2");
}

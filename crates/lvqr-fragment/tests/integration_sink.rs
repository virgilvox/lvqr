//! End-to-end integration scenario for the Fragment -> MoQ projection.
//!
//! Scenario: a deterministic "RTMP-like" sequence is driven through a
//! [`MoqTrackSink`] with a late-binding init segment, and the entire output
//! is read back through a real `lvqr_moq::OriginConsumer`. This is the
//! concrete shape the RTMP bridge in `lvqr-ingest` now produces: a
//! sequence header arrives after the track already exists, then keyframes
//! interleaved with delta frames populate a series of MoQ groups.
//!
//! The proptest at `tests/proptest_fragment.rs` covers the randomized
//! property. This file covers one fixed scenario in detail so that failures
//! produce a focused diff rather than a shrunk proptest seed.

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta, MoqTrackSink};
use lvqr_moq::{OriginProducer, Track};

#[tokio::test]
async fn late_binding_init_segment_roundtrips_through_sink() {
    // Arrange: origin + track + sink with no init segment yet.
    let origin = OriginProducer::new();
    let mut broadcast = origin.create_broadcast("integration-sink").expect("create broadcast");
    let track = broadcast.create_track(Track::new("0.mp4")).expect("create track");
    let meta = FragmentMeta::new("avc1.640028", 90000);
    let mut sink = MoqTrackSink::new(track, meta);

    // Act 1: RTMP sequence header arrives. Bind the init segment.
    let init = Bytes::from_static(b"INIT-SEGMENT-BYTES");
    sink.set_init_segment(init.clone());

    // Act 2: push two groups. Group 1 = keyframe + 2 deltas. Group 2 =
    // keyframe only. The sink must emit init as frame 0 of every group.
    let kf1 = Fragment::new(
        "0.mp4",
        1,
        0,
        0,
        0,
        0,
        3000,
        FragmentFlags::KEYFRAME,
        Bytes::from_static(b"kf1-payload"),
    );
    let d1 = Fragment::new(
        "0.mp4",
        1,
        1,
        0,
        3000,
        3000,
        3000,
        FragmentFlags::DELTA,
        Bytes::from_static(b"d1-payload"),
    );
    let d2 = Fragment::new(
        "0.mp4",
        1,
        2,
        0,
        6000,
        6000,
        3000,
        FragmentFlags::DELTA,
        Bytes::from_static(b"d2-payload"),
    );
    let kf2 = Fragment::new(
        "0.mp4",
        2,
        0,
        0,
        9000,
        9000,
        3000,
        FragmentFlags::KEYFRAME,
        Bytes::from_static(b"kf2-payload"),
    );
    sink.push(&kf1).expect("push kf1");
    sink.push(&d1).expect("push d1");
    sink.push(&d2).expect("push d2");
    sink.push(&kf2).expect("push kf2");
    sink.finish_current_group();

    // Assert: subscribe and read both groups back in order.
    let consumer = origin.consume();
    let bc = consumer
        .consume_broadcast("integration-sink")
        .expect("consume broadcast");
    let mut track_consumer = bc.subscribe_track(&Track::new("0.mp4")).expect("subscribe");

    // Group 1: init, kf1, d1, d2.
    let mut g1 = track_consumer
        .next_group()
        .await
        .expect("next_group ok")
        .expect("group 1 present");
    let f: Bytes = g1.read_frame().await.expect("ok").expect("frame");
    assert_eq!(f.as_ref(), init.as_ref(), "group 1 frame 0 is init");
    let f: Bytes = g1.read_frame().await.expect("ok").expect("frame");
    assert_eq!(f.as_ref(), b"kf1-payload", "group 1 frame 1 is kf1");
    let f: Bytes = g1.read_frame().await.expect("ok").expect("frame");
    assert_eq!(f.as_ref(), b"d1-payload", "group 1 frame 2 is d1");
    let f: Bytes = g1.read_frame().await.expect("ok").expect("frame");
    assert_eq!(f.as_ref(), b"d2-payload", "group 1 frame 3 is d2");

    // Group 2: init, kf2.
    let mut g2 = track_consumer
        .next_group()
        .await
        .expect("next_group ok")
        .expect("group 2 present");
    let f: Bytes = g2.read_frame().await.expect("ok").expect("frame");
    assert_eq!(f.as_ref(), init.as_ref(), "group 2 frame 0 is init");
    let f: Bytes = g2.read_frame().await.expect("ok").expect("frame");
    assert_eq!(f.as_ref(), b"kf2-payload", "group 2 frame 1 is kf2");
}

#[tokio::test]
async fn delta_without_prior_keyframe_is_silently_dropped() {
    // The sink contract says "no open group, no decoder context, drop it".
    // This mirrors the RTMP bridge's real behavior when a decoder writes a
    // delta before the first keyframe has arrived (rare in practice but
    // possible on a mid-stream reconnect).
    //
    // The subscribe happens before we drop the sink so the track is still
    // alive on the consumer side; then we poll with a short timeout and
    // assert that no group ever shows up.
    let origin = OriginProducer::new();
    let mut broadcast = origin.create_broadcast("drop-scenario").expect("create broadcast");
    let track = broadcast.create_track(Track::new("0.mp4")).expect("create track");
    let mut sink = MoqTrackSink::new(track, FragmentMeta::new("avc1.640028", 90000));

    // Subscribe before pushing so the consumer side sees every subsequent
    // group (or lack thereof).
    let consumer = origin.consume();
    let bc = consumer.consume_broadcast("drop-scenario").expect("consume");
    let mut track_consumer = bc.subscribe_track(&Track::new("0.mp4")).expect("subscribe");

    let d = Fragment::new(
        "0.mp4",
        1,
        0,
        0,
        0,
        0,
        3000,
        FragmentFlags::DELTA,
        Bytes::from_static(b"stray-delta"),
    );
    sink.push(&d).expect("push ok (dropped internally)");

    // Poll: no group should ever materialize. A short timeout is enough
    // because the sink runs synchronously and would have produced a group
    // immediately if the contract were broken.
    let result = tokio::time::timeout(std::time::Duration::from_millis(150), track_consumer.next_group()).await;
    match result {
        Err(_) => {}       // timeout: no group delivered, correct behavior
        Ok(Ok(None)) => {} // track closed cleanly, also correct
        Ok(Ok(Some(_))) => panic!("unexpected group produced for stray delta"),
        Ok(Err(e)) => panic!("next_group errored: {e:?}"),
    }
}

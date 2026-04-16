//! Fixed-scenario round-trip for [`MoqTrackSink`] -> [`MoqTrackStream`].
//!
//! This is the symmetric partner of `integration_sink.rs`: that file proves
//! the producer direction (Fragment -> MoQ), this one proves that the
//! consumer direction reconstructs an equivalent Fragment sequence from the
//! same MoQ bytes. Together they pin the unified-fragment <-> MoQ bridge
//! contract.
//!
//! The proptest at `tests/proptest_fragment.rs` covers the randomized shape;
//! this file fixes one scenario with a late-binding init segment and two
//! groups so a regression produces a targeted diff rather than a shrunk seed.

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta, FragmentStream, MoqTrackSink, MoqTrackStream};
use lvqr_moq::{OriginProducer, Track};

#[tokio::test]
async fn sink_then_track_stream_preserves_payloads_across_groups() {
    // Arrange: origin, broadcast, track, sink with late-binding init.
    let origin = OriginProducer::new();
    let mut broadcast = origin.create_broadcast("integration-stream").expect("create broadcast");
    let track = broadcast.create_track(Track::new("0.mp4")).expect("create track");
    let meta = FragmentMeta::new("avc1.640028", 90000);
    let mut sink = MoqTrackSink::new(track, meta.clone());

    let init = Bytes::from_static(b"INIT-SEGMENT-BYTES");
    sink.set_init_segment(init.clone());

    // Two groups. Group 1 = kf1 + d1 + d2, group 2 = kf2 only.
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

    // Subscribe before dropping sink so the TrackProducer is still alive.
    let consumer = origin.consume();
    let bc = consumer
        .consume_broadcast("integration-stream")
        .expect("consume broadcast");
    let track_consumer = bc.subscribe_track(&Track::new("0.mp4")).expect("subscribe");

    // Meta passed to the stream carries the init segment, so each group's
    // first frame is stripped as init.
    let meta_for_stream = meta.with_init_segment(init.clone());
    let mut stream = MoqTrackStream::new("0.mp4", meta_for_stream, track_consumer);

    // Expected flat sequence: group 1 payloads (kf1, d1, d2) then group 2
    // payloads (kf2). Keyframe flag resets at every group boundary.
    let f = stream.next_fragment().await.expect("g1 kf1");
    assert_eq!(f.payload.as_ref(), b"kf1-payload");
    assert_eq!(f.group_id, 0, "sink's first append is group sequence 0");
    assert_eq!(f.object_id, 0);
    assert!(f.flags.keyframe, "first emitted fragment per group is keyframe");

    let f = stream.next_fragment().await.expect("g1 d1");
    assert_eq!(f.payload.as_ref(), b"d1-payload");
    assert_eq!(f.group_id, 0);
    assert_eq!(f.object_id, 1);
    assert!(!f.flags.keyframe);

    let f = stream.next_fragment().await.expect("g1 d2");
    assert_eq!(f.payload.as_ref(), b"d2-payload");
    assert_eq!(f.group_id, 0);
    assert_eq!(f.object_id, 2);
    assert!(!f.flags.keyframe);

    let f = stream.next_fragment().await.expect("g2 kf2");
    assert_eq!(f.payload.as_ref(), b"kf2-payload");
    assert_eq!(f.group_id, 1, "second append is group sequence 1");
    assert_eq!(f.object_id, 0, "object_id resets per group");
    assert!(f.flags.keyframe, "new group restarts keyframe flagging");
}

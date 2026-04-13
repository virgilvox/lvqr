//! Integration test for `BroadcastRecorder`.
//!
//! Drives a synthesized MoQ broadcast through a real
//! `BroadcastRecorder::record_broadcast` call and asserts the on-disk
//! layout matches the documented structure:
//!
//! ```text
//! {tempdir}/{sanitized_broadcast}/0.init.mp4
//! {tempdir}/{sanitized_broadcast}/0.0001.m4s
//! ```
//!
//! Closes the audit finding (2026-04-13) that `record_track` had zero
//! integration coverage. The pure helpers `sanitize_name`, `track_prefix`,
//! and `looks_like_init` were unit-tested already; this test exercises
//! the actual async filesystem path.

use bytes::Bytes;
use lvqr_record::{BroadcastRecorder, RecordOptions};
use moq_lite::{OriginProducer, Track};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Build a fake fMP4 init segment: an `ftyp` box with minimal content. The
/// recorder detects init segments by checking for `ftyp` at offset 4, so
/// anything starting with a valid ISO BMFF box header whose type is
/// `ftyp` counts.
fn fake_init_segment() -> Bytes {
    // [size=16][ftyp][isom][0 0 0 0]
    let mut b = Vec::with_capacity(16);
    b.extend_from_slice(&16u32.to_be_bytes());
    b.extend_from_slice(b"ftyp");
    b.extend_from_slice(b"isom");
    b.extend_from_slice(&[0, 0, 0, 0]);
    Bytes::from(b)
}

/// Build a fake media segment: `moof` at offset 4. The recorder does not
/// validate this beyond distinguishing it from `ftyp`, so any non-ftyp
/// bytes work.
fn fake_media_segment() -> Bytes {
    let mut b = Vec::with_capacity(16);
    b.extend_from_slice(&16u32.to_be_bytes());
    b.extend_from_slice(b"moof");
    b.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
    Bytes::from(b)
}

#[tokio::test]
async fn records_init_and_media_segments_to_disk() {
    // --- Arrange a MoQ origin with one broadcast and one video track ---
    let origin = OriginProducer::new();
    let broadcast_name = "live/test";

    // Create broadcast + video track on the producer side.
    let mut broadcast = origin.create_broadcast(broadcast_name).expect("create_broadcast");
    let mut video_track = broadcast.create_track(Track::new("0.mp4")).expect("create_track");

    // Spin a tiny publisher task that writes one init + one media segment
    // inside a single MoQ group, then finishes the group. The recorder
    // subscribes to the track and reads the frames off in order.
    let publisher = tokio::spawn(async move {
        let mut group = video_track.append_group().expect("append_group");
        group.write_frame(fake_init_segment()).expect("write init frame");
        group.write_frame(fake_media_segment()).expect("write media frame");
        let _ = group.finish();
        // Keep the track alive long enough for the recorder to read it.
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Dropping `video_track` here closes the track so the recorder exits.
    });

    // --- Set up the recorder on a tempdir ---
    let tempdir = tempfile::tempdir().expect("tempdir");
    let recorder = BroadcastRecorder::new(tempdir.path());

    // Subscribe to the broadcast as a normal MoQ consumer.
    let consumer = origin.consume();
    let broadcast = consumer.consume_broadcast(broadcast_name).expect("consume_broadcast");

    // Record for at most 2 seconds. The recorder exits on its own when
    // both the broadcast and all of its tracks close.
    let cancel = CancellationToken::new();
    let cancel_ticker = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        cancel_ticker.cancel();
    });

    recorder
        .record_broadcast(
            broadcast_name,
            broadcast,
            RecordOptions {
                tracks: vec!["0.mp4".to_string()],
            },
            cancel,
        )
        .await
        .expect("record_broadcast should not error");

    publisher.await.expect("publisher task");

    // --- Assert the on-disk layout ---
    // Broadcast name "live/test" sanitizes to "live_test".
    let dir = tempdir.path().join("live_test");
    assert!(
        dir.exists(),
        "recorder should have created a broadcast directory at {}",
        dir.display()
    );

    let init_path = dir.join("0.init.mp4");
    assert!(
        init_path.exists(),
        "init segment should be written to {}",
        init_path.display()
    );

    let init_bytes = std::fs::read(&init_path).expect("read init");
    assert_eq!(
        &init_bytes[4..8],
        b"ftyp",
        "init segment on disk should start with ftyp box"
    );

    let seg_path = dir.join("0.0001.m4s");
    assert!(
        seg_path.exists(),
        "first media segment should be written to {}",
        seg_path.display()
    );
    let seg_bytes = std::fs::read(&seg_path).expect("read media");
    assert_eq!(
        &seg_bytes[4..8],
        b"moof",
        "media segment on disk should start with moof box"
    );
}

#[tokio::test]
async fn cancellation_stops_recording_cleanly() {
    // Verify that cancelling the token while the recorder is waiting on
    // the next group returns Ok immediately (no panics, no error).
    let origin = OriginProducer::new();
    let broadcast_name = "live/never";
    let mut broadcast = origin.create_broadcast(broadcast_name).expect("create_broadcast");
    let _video_track = broadcast.create_track(Track::new("0.mp4")).expect("create_track");

    let tempdir = tempfile::tempdir().expect("tempdir");
    let recorder = BroadcastRecorder::new(tempdir.path());
    let consumer = origin.consume();
    let broadcast = consumer.consume_broadcast(broadcast_name).expect("consume_broadcast");

    let cancel = CancellationToken::new();
    let cancel_ticker = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel_ticker.cancel();
    });

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        recorder.record_broadcast(broadcast_name, broadcast, RecordOptions::default(), cancel),
    )
    .await
    .expect("recorder should finish within 5s after cancel");

    assert!(result.is_ok(), "cancelled recorder should return Ok");
}

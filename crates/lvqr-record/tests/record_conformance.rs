//! Conformance slot for `lvqr-record`.
//!
//! Drives a real AVC fMP4 init segment (produced by
//! `lvqr_cmaf::write_avc_init_segment`, which is itself backed by the
//! `mp4-atom` library) through `BroadcastRecorder::record_broadcast`,
//! then reads the on-disk init file and feeds it to ffprobe via
//! `lvqr_test_utils::ffprobe_bytes`. The test passes if ffprobe
//! accepts the bytes (confirming the recorder wrote the init segment
//! byte-for-byte without corruption) or if ffprobe is unavailable on
//! PATH (soft-skip).
//!
//! This is the fifth slot of the 5-artifact test contract for
//! `lvqr-record`. Closes the last educational warning for that crate
//! in `scripts/check_test_contract.sh`.
//!
//! The prior `record_integration.rs` used hand-crafted 16-byte ftyp /
//! moof stubs that the recorder's `looks_like_init` heuristic accepts
//! but no real parser will. A conformance check needs bytes a real
//! decoder can walk end-to-end, which is exactly what `mp4-atom`
//! produces.

use bytes::{Bytes, BytesMut};
use lvqr_cmaf::{VideoInitParams, write_avc_init_segment};
use lvqr_moq::{OriginProducer, Track};
use lvqr_record::{BroadcastRecorder, RecordOptions};
use lvqr_test_utils::ffprobe_bytes;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Canonical H.264 Baseline 3.1 SPS/PPS used across the LVQR test
/// fleet (the same NALUs that back `lvqr-ingest`'s golden fMP4 fixture
/// and `lvqr-cmaf`'s init-segment unit tests). Kept inline so this
/// file does not need to cross-read fixtures from another crate.
const SPS: &[u8] = &[
    0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03,
    0x03, 0xC0, 0xF1, 0x83, 0x2A,
];
const PPS: &[u8] = &[0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];

fn build_avc_init_segment() -> Bytes {
    let params = VideoInitParams {
        sps: SPS.to_vec(),
        pps: PPS.to_vec(),
        width: 1280,
        height: 720,
        timescale: 90_000,
    };
    let mut buf = BytesMut::new();
    write_avc_init_segment(&mut buf, &params).expect("encode avc init");
    buf.freeze()
}

#[tokio::test]
async fn recorded_init_segment_round_trips_through_ffprobe() {
    let origin = OriginProducer::new();
    let broadcast_name = "live/conformance";
    let mut broadcast = origin.create_broadcast(broadcast_name).expect("create_broadcast");
    let mut video_track = broadcast.create_track(Track::new("0.mp4")).expect("create_track");

    let init_bytes_for_publisher = build_avc_init_segment();
    let publisher = tokio::spawn(async move {
        let mut group = video_track.append_group().expect("append_group");
        // Recorder's init detector keys on "ftyp" at offset 4, which a
        // real lvqr-cmaf init segment satisfies.
        group.write_frame(init_bytes_for_publisher).expect("write init frame");
        let _ = group.finish();
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    let tempdir = tempfile::tempdir().expect("tempdir");
    let recorder = BroadcastRecorder::new(tempdir.path());

    let consumer = origin.consume();
    let broadcast = consumer.consume_broadcast(broadcast_name).expect("consume_broadcast");

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

    // Recorder sanitizes "live/conformance" -> "live_conformance".
    let init_path = tempdir.path().join("live_conformance").join("0.init.mp4");
    let on_disk = std::fs::read(&init_path).unwrap_or_else(|e| {
        panic!(
            "init segment missing at {} ({e}); the recorder did not write the init file",
            init_path.display()
        )
    });

    // The bytes round-tripped through MoQ + tokio::fs should still
    // decode cleanly in ffprobe. Any corruption on the write path
    // shows up here as a rejection.
    ffprobe_bytes(&on_disk).assert_accepted();

    // Extra guard: the bytes on disk should match the bytes we handed
    // the publisher. Catches any recorder-level transformation that
    // would drift init segments silently.
    assert_eq!(
        on_disk,
        build_avc_init_segment().as_ref(),
        "recorded init segment differs from the bytes we fed the publisher"
    );
}

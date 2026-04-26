//! Broadcaster dispatch helpers for the Tier 2.1 ingest surface.
//!
//! Every ingest crate (lvqr-rtsp, lvqr-srt, lvqr-whip, lvqr-ingest::bridge)
//! publishes fragments into a shared
//! [`lvqr_fragment::FragmentBroadcasterRegistry`]. Consumers (archive,
//! LL-HLS, DASH) subscribe on the registry through
//! [`FragmentBroadcasterRegistry::on_entry_created`] callbacks and
//! drain fragments out of per-broadcaster streams.
//!
//! [`publish_init`] and [`publish_fragment`] are the idempotent
//! helpers every ingest crate calls at its emit sites.
//!
//! Session 60 deleted the dual-dispatch observer branch these
//! helpers used during the migration cycle (sessions 56-59). The
//! registry is now the only dispatch target.
//!
//! Design notes:
//!
//! * [`FragmentBroadcasterRegistry::get_or_create`] is idempotent
//!   under contention (double-checked insertion). The metadata
//!   passed on creation is only installed on first creation;
//!   subsequent calls with a different `meta` are ignored. If a
//!   mid-stream codec reconfig changes the init segment,
//!   [`FragmentBroadcaster::set_init_segment`] overwrites the
//!   `init_segment` field on the meta and consumers pick that up
//!   via [`BroadcasterStream::refresh_meta`] in their drain
//!   loops.
//!
//! * Cloning `Fragment` is cheap (payload is `Bytes`). The helper
//!   takes the fragment by value and moves it into
//!   [`FragmentBroadcaster::emit`]; no unnecessary clone.
//!
//! [`FragmentBroadcaster::set_init_segment`]: lvqr_fragment::FragmentBroadcaster::set_init_segment
//! [`FragmentBroadcaster::emit`]: lvqr_fragment::FragmentBroadcaster::emit
//! [`BroadcasterStream::refresh_meta`]: lvqr_fragment::BroadcasterStream::refresh_meta

use bytes::Bytes;
use lvqr_core::now_unix_ms;
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta, SCTE35_TRACK};

/// Publish an init segment to the broadcaster-registry path.
pub fn publish_init(
    registry: &FragmentBroadcasterRegistry,
    broadcast: &str,
    track: &str,
    codec: &str,
    timescale: u32,
    init: Bytes,
) {
    let bc = registry.get_or_create(broadcast, track, FragmentMeta::new(codec, timescale));
    bc.set_init_segment(init);
}

/// Publish a fragment to the broadcaster-registry path.
///
/// The fragment's [`Fragment::ingest_time_ms`] is stamped with the
/// current UNIX wall-clock milliseconds if unset, so the Tier 4 item
/// 4.7 latency SLO tracker can compute server-side glass-to-glass
/// delta on each subscriber-side delivery. Callers that already have
/// a meaningful ingest stamp (for example a federation relay
/// preserving the upstream stamp) may pre-stamp the fragment via
/// [`Fragment::with_ingest_time_ms`]; the helper only overwrites an
/// unset (`0`) field.
pub fn publish_fragment(
    registry: &FragmentBroadcasterRegistry,
    broadcast: &str,
    track: &str,
    codec: &str,
    timescale: u32,
    mut frag: Fragment,
) {
    if frag.ingest_time_ms == 0 {
        frag.ingest_time_ms = now_unix_ms();
    }
    let bc = registry.get_or_create(broadcast, track, FragmentMeta::new(codec, timescale));
    bc.emit(frag);
}

/// Publish a SCTE-35 splice event onto the broadcast's reserved
/// `"scte35"` track ([`SCTE35_TRACK`]).
///
/// The fragment carries the raw `splice_info_section` bytes
/// (table_id 0xFC through CRC_32) as its payload; egress renderers
/// (HLS DATERANGE, DASH EventStream) base64- or hex-encode the
/// payload directly per their respective spec carriage. The
/// fragment's `pts` is the absolute splice PTS in 90 kHz ticks
/// (publisher's `splice_time.pts_time + pts_adjustment`, masked to
/// 33 bits); `duration` is the splice `break_duration` in 90 kHz
/// ticks, or zero when the splice command sets no duration.
///
/// `event_id` is stamped onto the fragment's `group_id` so renderers
/// that pair SCTE35-OUT with SCTE35-IN can match by ID. Pass zero
/// for command types without an event_id (splice_null, time_signal,
/// bandwidth_reservation, private_command).
pub fn publish_scte35(
    registry: &FragmentBroadcasterRegistry,
    broadcast: &str,
    event_id: u64,
    pts_90k: u64,
    duration_90k: u64,
    section: Bytes,
) {
    let bc = registry.get_or_create(broadcast, SCTE35_TRACK, FragmentMeta::new("scte35", 90_000));
    let frag = Fragment::new(
        SCTE35_TRACK,
        event_id,
        0,
        0,
        pts_90k,
        pts_90k,
        duration_90k,
        FragmentFlags::KEYFRAME,
        section,
    )
    .with_ingest_time_ms(now_unix_ms());
    bc.emit(frag);
}

#[cfg(test)]
mod tests {
    use super::*;
    use lvqr_fragment::FragmentStream;

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
    async fn publish_init_installs_meta_on_registry() {
        let reg = FragmentBroadcasterRegistry::new();
        publish_init(&reg, "bcast", "0.mp4", "avc1", 90_000, Bytes::from_static(b"INIT"));
        let bc = reg.get("bcast", "0.mp4").expect("broadcaster created");
        assert_eq!(bc.meta().init_segment.as_ref().unwrap().as_ref(), b"INIT");
        assert_eq!(bc.meta().timescale, 90_000);
    }

    #[tokio::test]
    async fn publish_fragment_reaches_subscriber() {
        let reg = FragmentBroadcasterRegistry::new();
        let bc = reg.get_or_create("bcast", "0.mp4", FragmentMeta::new("avc1", 90_000));
        let mut sub = bc.subscribe();

        publish_fragment(&reg, "bcast", "0.mp4", "avc1", 90_000, mk_frag(1, true, b"kf"));

        let delivered = sub.next_fragment().await.expect("broadcaster saw fragment");
        assert_eq!(delivered.payload.as_ref(), b"kf");
        assert!(delivered.flags.keyframe);
    }

    #[tokio::test]
    async fn publish_scte35_lands_on_reserved_track() {
        let reg = FragmentBroadcasterRegistry::new();
        let bc = reg.get_or_create("live", SCTE35_TRACK, FragmentMeta::new("scte35", 90_000));
        let mut sub = bc.subscribe();

        let section = Bytes::from_static(&[0xFC, 0x30, 0x11, 0x00, 0x00]);
        publish_scte35(&reg, "live", 0xDEADBEEF, 8_100_000, 2_700_000, section.clone());

        let frag = sub.next_fragment().await.expect("scte35 fragment delivered");
        assert_eq!(frag.track_id, SCTE35_TRACK);
        assert_eq!(frag.group_id, 0xDEADBEEF);
        assert_eq!(frag.pts, 8_100_000);
        assert_eq!(frag.duration, 2_700_000);
        assert_eq!(frag.payload, section);
        assert!(frag.flags.keyframe);
    }

    #[tokio::test]
    async fn publish_fragment_before_subscribe_is_dropped() {
        let reg = FragmentBroadcasterRegistry::new();
        publish_fragment(&reg, "bcast", "0.mp4", "avc1", 90_000, mk_frag(1, true, b"early"));
        let bc = reg.get("bcast", "0.mp4").expect("broadcaster created by publish");
        let mut sub = bc.subscribe();
        publish_fragment(&reg, "bcast", "0.mp4", "avc1", 90_000, mk_frag(2, false, b"late"));
        let delivered = sub.next_fragment().await.expect("late fragment arrives");
        assert_eq!(delivered.payload.as_ref(), b"late");
    }
}

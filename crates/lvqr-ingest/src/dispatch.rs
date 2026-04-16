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
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentMeta};

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
pub fn publish_fragment(
    registry: &FragmentBroadcasterRegistry,
    broadcast: &str,
    track: &str,
    codec: &str,
    timescale: u32,
    frag: Fragment,
) {
    let bc = registry.get_or_create(broadcast, track, FragmentMeta::new(codec, timescale));
    bc.emit(frag);
}

#[cfg(test)]
mod tests {
    use super::*;
    use lvqr_fragment::{FragmentFlags, FragmentStream};

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

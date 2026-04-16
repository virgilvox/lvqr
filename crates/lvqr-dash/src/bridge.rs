//! Broadcaster-native DASH composition helper.
//!
//! Session 60 consumer-side switchover. Before this session the DASH
//! fan-out was a [`lvqr_ingest::FragmentObserver`] that the RTMP +
//! WHIP bridges fired synchronously per fragment. The new
//! [`BroadcasterDashBridge`] replaces that with a
//! [`FragmentBroadcasterRegistry::on_entry_created`] callback that
//! spawns one tokio drain task per `(broadcast, track)` pair. Every
//! fragment emitted by any ingest protocol onto the shared registry
//! is drained into the per-broadcast [`DashServer`] by the spawned
//! task. The rendered MPD + segment cache surface is unchanged.
//!
//! Install via [`BroadcasterDashBridge::install`] exactly once at
//! server startup, *before* any ingest crate publishes. DASH
//! addresses full segments via `SegmentTemplate` `$Number$` URIs, so
//! each drain task owns a monotonic counter per track and stamps it
//! onto every pushed fragment. The counter resets whenever
//! [`FragmentBroadcaster::set_init_segment`] overwrites the init
//! bytes (RTMP reconnect, WHIP rollover, mid-stream codec change),
//! so a client polling the MPD sees `$Number$` restart from 1 off
//! the fresh init segment.
//!
//! The drain task holds only a [`BroadcasterStream`] (Receiver side);
//! it deliberately does not keep a strong [`Arc<FragmentBroadcaster>`]
//! alive. That matches the invariant the archive + HLS switchovers
//! already documented: a stashed producer-side clone would prevent
//! the recv loop from ever seeing `Closed` after every ingest clone
//! dropped.

use bytes::Bytes;
use lvqr_fragment::{BroadcasterStream, FragmentBroadcasterRegistry, FragmentStream};
use tokio::runtime::Handle;

use crate::server::{DashServer, MultiDashServer};

/// Video track id stamped on every broadcaster the ingest crates
/// create for a video publisher. Matches the MoQ catalog convention
/// (`0.mp4` for video, `1.mp4` for audio).
const VIDEO_TRACK: &str = "0.mp4";
/// Audio track id counterpart to [`VIDEO_TRACK`].
const AUDIO_TRACK: &str = "1.mp4";

/// Broadcaster-native DASH composition helper. Stateless: the struct
/// itself carries nothing; [`install`](Self::install) wires an
/// `on_entry_created` callback that owns everything each drain task
/// needs.
pub struct BroadcasterDashBridge;

impl BroadcasterDashBridge {
    /// Register an `on_entry_created` callback on `registry` so every
    /// new `(broadcast, track)` pair published by any ingest crate
    /// gets one drain task that feeds the per-track entry point on
    /// the shared [`MultiDashServer`].
    ///
    /// Callers must invoke this from inside a tokio runtime.
    pub fn install(multi: MultiDashServer, registry: &FragmentBroadcasterRegistry) {
        registry.on_entry_created(move |broadcast, track, bc| {
            let broadcast = broadcast.to_string();
            let track = track.to_string();
            // Decide up-front whether this broadcaster feeds the video
            // or audio AdaptationSet, and skip unknown track ids
            // entirely. DASH has no rendition concept for caption
            // tracks yet; the MPD renderer only declares video +
            // audio AdaptationSets.
            let is_video = match track.as_str() {
                VIDEO_TRACK => true,
                AUDIO_TRACK => false,
                _ => return,
            };
            // Subscribe synchronously inside the callback so no emit
            // can race ahead of the drain loop.
            let sub = bc.subscribe();
            let handle = match Handle::try_current() {
                Ok(h) => h,
                Err(_) => {
                    tracing::warn!(
                        broadcast = %broadcast,
                        track = %track,
                        "BroadcasterDashBridge: callback fired outside tokio runtime; drain not spawned",
                    );
                    return;
                }
            };
            let server = multi.ensure(&broadcast);
            handle.spawn(Self::drain(server, broadcast, track, is_video, sub));
        });
    }

    /// Per-broadcaster drain task. Runs until every producer-side
    /// clone of the broadcaster drops.
    async fn drain(server: DashServer, broadcast: String, track: String, is_video: bool, mut sub: BroadcasterStream) {
        let mut last_init: Option<Bytes> = None;
        let mut seq: u64 = 0;
        while let Some(fragment) = sub.next_fragment().await {
            // Pull the freshest init bytes off the broadcaster each
            // iteration. The first emit arrives after `publish_init`
            // has already set the init segment, so the very first
            // refresh picks it up. Subsequent refreshes detect a
            // reconnect that overwrote the init bytes.
            sub.refresh_meta();
            if let Some(current_init) = sub.meta().init_segment.clone() {
                let changed = match last_init.as_ref() {
                    None => true,
                    Some(prev) => prev != &current_init,
                };
                if changed {
                    if is_video {
                        server.push_video_init(current_init.clone());
                    } else {
                        server.push_audio_init(current_init.clone());
                    }
                    // Reset the monotonic counter so the DASH
                    // SegmentTemplate's `$Number$` restarts at 1 off
                    // the fresh init segment.
                    seq = 0;
                    last_init = Some(current_init);
                }
            }
            seq += 1;
            if is_video {
                server.push_video_segment(seq, fragment.payload.clone());
            } else {
                server.push_audio_segment(seq, fragment.payload.clone());
            }
        }
        tracing::info!(
            broadcast = %broadcast,
            track = %track,
            "BroadcasterDashBridge: drain terminated (producers closed)",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{DashConfig, MultiDashServer};
    use bytes::Bytes;
    use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta};

    fn mk_fragment(track: &str, dts: u64, payload: &'static [u8]) -> Fragment {
        Fragment::new(
            track,
            0,
            0,
            0,
            dts,
            dts,
            180_000,
            FragmentFlags::KEYFRAME,
            Bytes::from_static(payload),
        )
    }

    /// Yield to the tokio scheduler enough times for the drain task
    /// to pick up newly-emitted fragments. The drain awaits on a
    /// `broadcast::Receiver` wakeup and then does a short series of
    /// sync writes; a handful of cooperative yields is sufficient.
    async fn let_drain_run() {
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn video_fragments_get_monotonic_sequence_numbers() {
        let multi = MultiDashServer::new(DashConfig::default());
        let registry = FragmentBroadcasterRegistry::new();
        BroadcasterDashBridge::install(multi.clone(), &registry);

        let bc = registry.get_or_create("live/test", VIDEO_TRACK, FragmentMeta::new("avc1.640028", 90_000));
        bc.set_init_segment(Bytes::from_static(b"\x00init"));
        bc.emit(mk_fragment(VIDEO_TRACK, 0, b"seg1"));
        bc.emit(mk_fragment(VIDEO_TRACK, 180_000, b"seg2"));
        bc.emit(mk_fragment(VIDEO_TRACK, 360_000, b"seg3"));

        let_drain_run().await;

        let server = multi.get("live/test").expect("broadcast ensured");
        assert_eq!(server.video_segment(1).unwrap(), Bytes::from_static(b"seg1"));
        assert_eq!(server.video_segment(2).unwrap(), Bytes::from_static(b"seg2"));
        assert_eq!(server.video_segment(3).unwrap(), Bytes::from_static(b"seg3"));
        assert!(server.video_segment(4).is_none());
    }

    #[tokio::test]
    async fn audio_and_video_sequences_are_independent() {
        let multi = MultiDashServer::new(DashConfig::default());
        let registry = FragmentBroadcasterRegistry::new();
        BroadcasterDashBridge::install(multi.clone(), &registry);

        let video_bc = registry.get_or_create("live/av", VIDEO_TRACK, FragmentMeta::new("avc1.640028", 90_000));
        let audio_bc = registry.get_or_create("live/av", AUDIO_TRACK, FragmentMeta::new("mp4a.40.2", 48_000));
        video_bc.set_init_segment(Bytes::from_static(b"\x00v"));
        audio_bc.set_init_segment(Bytes::from_static(b"\x00a"));
        video_bc.emit(mk_fragment(VIDEO_TRACK, 0, b"v1"));
        audio_bc.emit(mk_fragment(AUDIO_TRACK, 0, b"a1"));
        audio_bc.emit(mk_fragment(AUDIO_TRACK, 960, b"a2"));
        video_bc.emit(mk_fragment(VIDEO_TRACK, 180_000, b"v2"));

        let_drain_run().await;

        let server = multi.get("live/av").expect("broadcast ensured");
        assert_eq!(server.video_segment(1).unwrap(), Bytes::from_static(b"v1"));
        assert_eq!(server.video_segment(2).unwrap(), Bytes::from_static(b"v2"));
        assert_eq!(server.audio_segment(1).unwrap(), Bytes::from_static(b"a1"));
        assert_eq!(server.audio_segment(2).unwrap(), Bytes::from_static(b"a2"));
    }

    #[tokio::test]
    async fn reinit_resets_the_counter() {
        let multi = MultiDashServer::new(DashConfig::default());
        let registry = FragmentBroadcasterRegistry::new();
        BroadcasterDashBridge::install(multi.clone(), &registry);

        let bc = registry.get_or_create("live/rc", VIDEO_TRACK, FragmentMeta::new("avc1.640028", 90_000));
        bc.set_init_segment(Bytes::from_static(b"\x00init-a"));
        bc.emit(mk_fragment(VIDEO_TRACK, 0, b"a1"));
        bc.emit(mk_fragment(VIDEO_TRACK, 180_000, b"a2"));
        let_drain_run().await;

        // Client reconnected: new init bytes overwrite the old ones.
        // The drain task sees the change on its next iteration, pushes
        // the new init into `DashServer`, and restarts the `$Number$`
        // counter at 1 so the rendered MPD's SegmentTemplate resolves
        // to fresh segments rather than colliding with the prior ones.
        bc.set_init_segment(Bytes::from_static(b"\x00init-b"));
        bc.emit(mk_fragment(VIDEO_TRACK, 0, b"b1"));
        let_drain_run().await;

        let server = multi.get("live/rc").expect("broadcast ensured");
        assert_eq!(server.video_segment(1).unwrap(), Bytes::from_static(b"b1"));
    }

    #[tokio::test]
    async fn unknown_track_ids_are_ignored() {
        let multi = MultiDashServer::new(DashConfig::default());
        let registry = FragmentBroadcasterRegistry::new();
        BroadcasterDashBridge::install(multi.clone(), &registry);

        let bc = registry.get_or_create("live/odd", "captions", FragmentMeta::new("wvtt", 1000));
        bc.set_init_segment(Bytes::from_static(b"\x00cap"));
        bc.emit(mk_fragment("captions", 0, b"c1"));
        let_drain_run().await;

        // No video / audio path was fed; the broadcast entry is also
        // never created by the drain because unknown tracks skip
        // `multi.ensure`.
        assert!(multi.get("live/odd").is_none());
    }
}

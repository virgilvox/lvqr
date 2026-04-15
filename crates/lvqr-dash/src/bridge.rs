//! Fragment observer that feeds a [`MultiDashServer`] from the
//! bridge side.
//!
//! Sibling of `lvqr_cli::hls::HlsFragmentBridge`. Where the HLS
//! bridge walks incoming fragments through a `CmafPolicyState` to
//! classify each one as a partial / segment boundary (because
//! LL-HLS addresses each partial individually), the DASH live
//! profile addresses full segments via `SegmentTemplate` `$Number$`
//! URIs, so the bridge just stamps a monotonic counter per track
//! onto every observed fragment and pushes the payload bytes under
//! that number.
//!
//! The counter is restarted whenever a fresh `on_init` arrives for
//! the same broadcast: a republish (RTMP reconnect, WHIP session
//! rollover, mid-stream codec change) also rolls the
//! `SegmentTemplate` `startNumber` view so a client that was
//! polling the MPD sees consistent numbering off the new init
//! segment.
//!
//! Implementations of [`FragmentObserver`] must be cheap and
//! non-blocking: they run from inside the `rml_rtmp` callback chain
//! and from the WHIP `str0m` runloop, which both own the ingest
//! task. This bridge only touches a `Mutex<HashMap>` and the
//! `MultiDashServer` cache (also a `Mutex<HashMap>`), so the
//! bookkeeping stays inside the same tokio task the observer is
//! invoked from without any task spawning.

use std::collections::HashMap;
use std::sync::Mutex;

use bytes::Bytes;
use lvqr_fragment::Fragment;
use lvqr_ingest::FragmentObserver;

use crate::server::MultiDashServer;

/// Video track id stamped on every `FragmentObserver::on_*` call
/// by `RtmpMoqBridge` and `WhipMoqBridge`. Matches the MoQ catalog
/// track name convention (`0.mp4` for video, `1.mp4` for audio).
const VIDEO_TRACK: &str = "0.mp4";
/// Audio track id counterpart to [`VIDEO_TRACK`].
const AUDIO_TRACK: &str = "1.mp4";

/// Which of the two tracks (video or audio) the counter entry
/// belongs to. Kept as a tiny enum rather than a string so the
/// counter map key is cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TrackKind {
    Video,
    Audio,
}

/// Observer that fans bridge-emitted fragments into a
/// [`MultiDashServer`].
///
/// Construct one per `lvqr-cli` instance and hand it to both
/// `RtmpMoqBridge::with_observer` and `WhipMoqBridge::with_observer`
/// so RTMP and WHIP publishers share the same DASH fan-out. The
/// [`MultiDashServer`] inside is cloned into the axum router
/// `lvqr-cli` mounts on the DASH listener.
pub struct DashFragmentBridge {
    multi: MultiDashServer,
    counters: Mutex<HashMap<(String, TrackKind), u64>>,
}

impl DashFragmentBridge {
    /// Build a new bridge around an existing [`MultiDashServer`].
    pub fn new(multi: MultiDashServer) -> Self {
        Self {
            multi,
            counters: Mutex::new(HashMap::new()),
        }
    }

    fn reset_counter(&self, broadcast: &str, kind: TrackKind) {
        let mut map = self.counters.lock().expect("dash bridge counters lock poisoned");
        map.insert((broadcast.to_string(), kind), 0);
    }

    fn next_seq(&self, broadcast: &str, kind: TrackKind) -> u64 {
        let mut map = self.counters.lock().expect("dash bridge counters lock poisoned");
        let entry = map.entry((broadcast.to_string(), kind)).or_insert(0);
        *entry += 1;
        *entry
    }
}

impl FragmentObserver for DashFragmentBridge {
    fn on_init(&self, broadcast: &str, track: &str, _timescale: u32, init: Bytes) {
        let server = self.multi.ensure(broadcast);
        match track {
            VIDEO_TRACK => {
                self.reset_counter(broadcast, TrackKind::Video);
                server.push_video_init(init);
            }
            AUDIO_TRACK => {
                self.reset_counter(broadcast, TrackKind::Audio);
                server.push_audio_init(init);
            }
            _ => {}
        }
    }

    fn on_fragment(&self, broadcast: &str, track: &str, fragment: &Fragment) {
        let server = self.multi.ensure(broadcast);
        match track {
            VIDEO_TRACK => {
                let seq = self.next_seq(broadcast, TrackKind::Video);
                server.push_video_segment(seq, fragment.payload.clone());
            }
            AUDIO_TRACK => {
                let seq = self.next_seq(broadcast, TrackKind::Audio);
                server.push_audio_segment(seq, fragment.payload.clone());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{DashConfig, MultiDashServer};
    use lvqr_fragment::{Fragment, FragmentFlags};

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

    #[test]
    fn video_fragments_get_monotonic_sequence_numbers() {
        let multi = MultiDashServer::new(DashConfig::default());
        let bridge = DashFragmentBridge::new(multi.clone());

        bridge.on_init("live/test", VIDEO_TRACK, 90_000, Bytes::from_static(b"\x00init"));
        bridge.on_fragment("live/test", VIDEO_TRACK, &mk_fragment(VIDEO_TRACK, 0, b"seg1"));
        bridge.on_fragment("live/test", VIDEO_TRACK, &mk_fragment(VIDEO_TRACK, 180_000, b"seg2"));
        bridge.on_fragment("live/test", VIDEO_TRACK, &mk_fragment(VIDEO_TRACK, 360_000, b"seg3"));

        let server = multi.get("live/test").expect("broadcast ensured");
        assert_eq!(server.video_segment(1).unwrap(), Bytes::from_static(b"seg1"));
        assert_eq!(server.video_segment(2).unwrap(), Bytes::from_static(b"seg2"));
        assert_eq!(server.video_segment(3).unwrap(), Bytes::from_static(b"seg3"));
        assert!(server.video_segment(4).is_none());
    }

    #[test]
    fn audio_and_video_sequences_are_independent() {
        let multi = MultiDashServer::new(DashConfig::default());
        let bridge = DashFragmentBridge::new(multi.clone());

        bridge.on_init("live/av", VIDEO_TRACK, 90_000, Bytes::from_static(b"\x00v"));
        bridge.on_init("live/av", AUDIO_TRACK, 48_000, Bytes::from_static(b"\x00a"));
        bridge.on_fragment("live/av", VIDEO_TRACK, &mk_fragment(VIDEO_TRACK, 0, b"v1"));
        bridge.on_fragment("live/av", AUDIO_TRACK, &mk_fragment(AUDIO_TRACK, 0, b"a1"));
        bridge.on_fragment("live/av", AUDIO_TRACK, &mk_fragment(AUDIO_TRACK, 960, b"a2"));
        bridge.on_fragment("live/av", VIDEO_TRACK, &mk_fragment(VIDEO_TRACK, 180_000, b"v2"));

        let server = multi.get("live/av").expect("broadcast ensured");
        assert_eq!(server.video_segment(1).unwrap(), Bytes::from_static(b"v1"));
        assert_eq!(server.video_segment(2).unwrap(), Bytes::from_static(b"v2"));
        assert_eq!(server.audio_segment(1).unwrap(), Bytes::from_static(b"a1"));
        assert_eq!(server.audio_segment(2).unwrap(), Bytes::from_static(b"a2"));
    }

    #[test]
    fn reinit_resets_the_counter() {
        let multi = MultiDashServer::new(DashConfig::default());
        let bridge = DashFragmentBridge::new(multi.clone());

        bridge.on_init("live/rc", VIDEO_TRACK, 90_000, Bytes::from_static(b"\x00init-a"));
        bridge.on_fragment("live/rc", VIDEO_TRACK, &mk_fragment(VIDEO_TRACK, 0, b"a1"));
        bridge.on_fragment("live/rc", VIDEO_TRACK, &mk_fragment(VIDEO_TRACK, 180_000, b"a2"));
        // Second init: client reconnected, counter must restart at 1
        // so the MPD's `$Number$` template resolves to the fresh
        // segments rather than colliding with the prior ones.
        bridge.on_init("live/rc", VIDEO_TRACK, 90_000, Bytes::from_static(b"\x00init-b"));
        bridge.on_fragment("live/rc", VIDEO_TRACK, &mk_fragment(VIDEO_TRACK, 0, b"b1"));

        let server = multi.get("live/rc").expect("broadcast ensured");
        assert_eq!(server.video_segment(1).unwrap(), Bytes::from_static(b"b1"));
    }

    #[test]
    fn unknown_track_ids_are_ignored() {
        let multi = MultiDashServer::new(DashConfig::default());
        let bridge = DashFragmentBridge::new(multi.clone());

        bridge.on_init("live/odd", "captions", 1000, Bytes::from_static(b"\x00cap"));
        bridge.on_fragment("live/odd", "captions", &mk_fragment("captions", 0, b"c1"));

        let server = multi.get("live/odd").expect("broadcast still ensured");
        assert!(server.video_segment(1).is_none());
        assert!(server.audio_segment(1).is_none());
    }
}

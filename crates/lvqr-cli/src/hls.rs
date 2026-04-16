//! HLS composition glue for `lvqr-cli`.
//!
//! This is the consumer side of the [`lvqr_ingest::FragmentObserver`]
//! hook. The bridge in `lvqr-ingest` already produces `Fragment` values
//! out of every RTMP publisher; the [`HlsFragmentBridge`] here observes
//! those fragments, walks them through a per-broadcast / per-track
//! [`CmafPolicyState`] to classify each chunk as a partial /
//! partial-independent / segment boundary, and pushes the resulting
//! [`CmafChunk`] into a shared [`MultiHlsServer`].
//!
//! Session 12 added per-broadcast routing so multiple concurrent
//! broadcasts each get their own LL-HLS surface. Session 13 extends
//! that to per-track audio renditions: audio fragments (`1.mp4`) are
//! routed through `MultiHlsServer::ensure_audio` and the master
//! playlist generated under `/hls/{broadcast}/master.m3u8` declares
//! the audio rendition group when both tracks are present.
//!
//! The observer is invoked synchronously from the `rml_rtmp` callback
//! path, but [`HlsServer::push_chunk_bytes`] is async. Each on_init /
//! on_fragment notification spawns a tiny tokio task instead of
//! blocking the ingest pump.

use bytes::Bytes;
use lvqr_cmaf::{CmafChunk, CmafPolicy, CmafPolicyState};
use lvqr_fragment::Fragment;
use lvqr_hls::{HlsServer, MultiHlsServer};
use lvqr_ingest::FragmentObserver;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::runtime::Handle;

const VIDEO_TRACK: &str = "0.mp4";
const AUDIO_TRACK: &str = "1.mp4";

/// Fans bridge-emitted fragments into a multi-broadcast / multi-track
/// LL-HLS server.
///
/// Construct one per `lvqr-cli` instance, hand it to the bridge via
/// `RtmpMoqBridge::with_observer`, and clone the underlying
/// [`MultiHlsServer`] into the axum router that serves
/// `/hls/{broadcast}/playlist.m3u8` and `/hls/{broadcast}/audio.m3u8`.
pub(crate) struct HlsFragmentBridge {
    multi: MultiHlsServer,
    /// Per-broadcast video policy state machines. Keyed on the
    /// broadcast name. A fresh entry is installed the first time a
    /// broadcast publishes its video init segment; a new init on
    /// the same broadcast resets the entry so a republish (e.g.
    /// mid-stream codec change or RTMP reconnect) starts cleanly.
    video_states: Mutex<HashMap<String, CmafPolicyState>>,
    /// Per-broadcast audio policy state machines. Same structure as
    /// `video_states` but keyed on the audio track id.
    audio_states: Mutex<HashMap<String, CmafPolicyState>>,
    /// Target segment duration in milliseconds. Matches the HLS
    /// `EXT-X-TARGETDURATION` declaration. Used to build a
    /// `CmafPolicy` at the track's native timescale when a new
    /// broadcast publishes its init segment.
    segment_duration_ms: u32,
    /// Target partial (chunk) duration in milliseconds. Matches
    /// the HLS `EXT-X-PART-INF:PART-TARGET` declaration.
    part_duration_ms: u32,
}

impl HlsFragmentBridge {
    pub fn new(multi: MultiHlsServer, segment_duration_ms: u32, part_duration_ms: u32) -> Self {
        Self {
            multi,
            video_states: Mutex::new(HashMap::new()),
            audio_states: Mutex::new(HashMap::new()),
            segment_duration_ms,
            part_duration_ms,
        }
    }

    fn classify(
        states: &Mutex<HashMap<String, CmafPolicyState>>,
        policy: CmafPolicy,
        broadcast: &str,
        fragment: &Fragment,
    ) -> lvqr_cmaf::CmafChunkKind {
        let mut map = states.lock().expect("hls bridge mutex poisoned");
        let state = map
            .entry(broadcast.to_string())
            .or_insert_with(|| CmafPolicyState::new(policy));
        state.step(fragment.flags.keyframe, fragment.dts).kind
    }

    fn reset(states: &Mutex<HashMap<String, CmafPolicyState>>, policy: CmafPolicy, broadcast: &str) {
        let mut map = states.lock().expect("hls bridge mutex poisoned");
        map.insert(broadcast.to_string(), CmafPolicyState::new(policy));
    }

    fn dispatch_init(server: HlsServer, init: Bytes) {
        let Ok(handle) = Handle::try_current() else {
            tracing::warn!("HLS bridge on_init outside tokio runtime; dropping init");
            return;
        };
        handle.spawn(async move {
            server.push_init(init).await;
        });
    }

    fn dispatch_chunk(server: HlsServer, chunk: CmafChunk) {
        let Ok(handle) = Handle::try_current() else {
            tracing::warn!("HLS bridge on_fragment outside tokio runtime; dropping chunk");
            return;
        };
        handle.spawn(async move {
            let payload = chunk.payload.clone();
            if let Err(e) = server.push_chunk_bytes(&chunk, payload).await {
                tracing::debug!(error = ?e, "hls push_chunk_bytes rejected");
            }
        });
    }
}

impl FragmentObserver for HlsFragmentBridge {
    fn on_init(&self, broadcast: &str, track: &str, timescale: u32, init: Bytes) {
        // Build a `CmafPolicy` that matches the track's native
        // timescale so LL-HLS `#EXT-X-PART:DURATION` reporting lines
        // up with the real wall-clock duration of each partial.
        // Previously the audio path hard-wired
        // `AUDIO_48KHZ_DEFAULT`, which overstated AAC 44.1 kHz
        // partial durations by ~8.8% (1024 / 48000 instead of
        // 1024 / 44100). Video still lands on the 90 kHz constant
        // because the bridge's video init writer is hardcoded to
        // that timescale, but the code path is uniform.
        let policy = CmafPolicy::with_durations(timescale, self.segment_duration_ms, self.part_duration_ms);
        match track {
            VIDEO_TRACK => {
                Self::reset(&self.video_states, policy, broadcast);
                Self::dispatch_init(self.multi.ensure_video(broadcast), init);
            }
            AUDIO_TRACK => {
                Self::reset(&self.audio_states, policy, broadcast);
                Self::dispatch_init(self.multi.ensure_audio(broadcast, timescale), init);
            }
            _ => {}
        }
    }

    fn on_fragment(&self, broadcast: &str, track: &str, fragment: &Fragment) {
        // `on_fragment` runs after `on_init` has created the per-track
        // HlsServer, so both branches use pure lookups here instead of
        // `ensure_*`. A fragment arriving before its init is skipped;
        // the bridge invariants guarantee the sequence header always
        // lands first, so this is a defensive branch not a hot path.
        let (server, kind) = match track {
            VIDEO_TRACK => {
                let kind = Self::classify(
                    &self.video_states,
                    CmafPolicy::with_durations(90_000, self.segment_duration_ms, self.part_duration_ms),
                    broadcast,
                    fragment,
                );
                let Some(server) = self.multi.video(broadcast) else {
                    return;
                };
                (server, kind)
            }
            AUDIO_TRACK => {
                let kind = Self::classify(
                    &self.audio_states,
                    CmafPolicy::with_durations(48_000, self.segment_duration_ms, self.part_duration_ms),
                    broadcast,
                    fragment,
                );
                let Some(server) = self.multi.audio(broadcast) else {
                    return;
                };
                (server, kind)
            }
            _ => return,
        };
        let chunk = CmafChunk {
            track_id: fragment.track_id.clone(),
            payload: fragment.payload.clone(),
            dts: fragment.dts,
            duration: fragment.duration,
            kind,
        };
        Self::dispatch_chunk(server, chunk);
    }
}

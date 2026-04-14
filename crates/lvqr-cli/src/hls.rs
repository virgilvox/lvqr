//! HLS composition glue for `lvqr-cli`.
//!
//! This is the consumer side of the [`lvqr_ingest::FragmentObserver`]
//! hook. The bridge in `lvqr-ingest` already produces `Fragment` values
//! out of every RTMP publisher; the [`HlsFragmentBridge`] here observes
//! those fragments, walks them through a per-broadcast [`CmafPolicyState`]
//! to classify each chunk as a partial / partial-independent / segment
//! boundary, and pushes the resulting [`CmafChunk`] into a shared
//! [`MultiHlsServer`].
//!
//! Session 12 (multi-broadcast routing): the bridge now maintains one
//! `CmafPolicyState` per observed broadcast, keyed by the broadcast name
//! the ingest layer reports. Every broadcast that publishes a video track
//! gets its own per-broadcast [`HlsServer`] inside the shared
//! `MultiHlsServer`, and the axum router serves them at
//! `/hls/{broadcast}/playlist.m3u8`. Audio is still ignored here; audio
//! rendition groups land separately when `lvqr-hls` grows master-playlist
//! support.
//!
//! The observer is invoked synchronously from the `rml_rtmp` callback
//! path, but [`HlsServer::push_chunk_bytes`] is async. Each on_init /
//! on_fragment notification spawns a tiny tokio task instead of
//! blocking the ingest pump.

use bytes::Bytes;
use lvqr_cmaf::{CmafChunk, CmafPolicy, CmafPolicyState};
use lvqr_fragment::Fragment;
use lvqr_hls::MultiHlsServer;
use lvqr_ingest::FragmentObserver;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::runtime::Handle;

/// Fans bridge-emitted fragments into a multi-broadcast LL-HLS server.
///
/// Construct one per `lvqr-cli` instance, hand it to the bridge via
/// `RtmpMoqBridge::with_observer`, and clone the underlying
/// [`MultiHlsServer`] into the axum router that serves
/// `/hls/{broadcast}/playlist.m3u8`.
pub(crate) struct HlsFragmentBridge {
    multi: MultiHlsServer,
    /// Per-broadcast video policy state machines. Keyed on the
    /// broadcast name the ingest layer reports. A fresh entry is
    /// installed the first time a broadcast publishes its init
    /// segment; a new init on the same broadcast resets the entry so
    /// a republish (e.g. mid-stream codec change or RTMP reconnect)
    /// starts cleanly.
    video_states: Mutex<HashMap<String, CmafPolicyState>>,
}

impl HlsFragmentBridge {
    pub fn new(multi: MultiHlsServer) -> Self {
        Self {
            multi,
            video_states: Mutex::new(HashMap::new()),
        }
    }

    /// Return the chunk classification for the next video fragment on
    /// `broadcast`, installing a fresh `CmafPolicyState` if this is
    /// the first fragment observed for that broadcast.
    fn classify_video(&self, broadcast: &str, fragment: &Fragment) -> lvqr_cmaf::CmafChunkKind {
        let mut states = self.video_states.lock().expect("hls bridge mutex poisoned");
        let state = states
            .entry(broadcast.to_string())
            .or_insert_with(|| CmafPolicyState::new(CmafPolicy::VIDEO_90KHZ_DEFAULT));
        state.step(fragment.flags.keyframe, fragment.dts).kind
    }

    /// Reset the policy state for `broadcast` to a fresh
    /// `VIDEO_90KHZ_DEFAULT` baseline. Called whenever a new init
    /// segment lands so that a mid-stream codec change starts from a
    /// clean slate.
    fn reset_video_state(&self, broadcast: &str) {
        let mut states = self.video_states.lock().expect("hls bridge mutex poisoned");
        states.insert(
            broadcast.to_string(),
            CmafPolicyState::new(CmafPolicy::VIDEO_90KHZ_DEFAULT),
        );
    }
}

impl FragmentObserver for HlsFragmentBridge {
    fn on_init(&self, broadcast: &str, track: &str, init: Bytes) {
        if track != "0.mp4" {
            return;
        }
        self.reset_video_state(broadcast);
        let server = self.multi.ensure_broadcast(broadcast);
        let Ok(handle) = Handle::try_current() else {
            tracing::warn!("HLS bridge on_init outside tokio runtime; dropping init");
            return;
        };
        handle.spawn(async move {
            server.push_init(init).await;
        });
    }

    fn on_fragment(&self, broadcast: &str, track: &str, fragment: &Fragment) {
        if track != "0.mp4" {
            return;
        }
        let kind = self.classify_video(broadcast, fragment);
        let chunk = CmafChunk {
            track_id: fragment.track_id.clone(),
            payload: fragment.payload.clone(),
            dts: fragment.dts,
            duration: fragment.duration,
            kind,
        };
        let server = self.multi.ensure_broadcast(broadcast);
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

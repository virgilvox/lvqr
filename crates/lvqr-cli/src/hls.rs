//! HLS composition glue for `lvqr-cli`.
//!
//! This is the consumer side of the [`lvqr_ingest::FragmentObserver`]
//! hook. The bridge in `lvqr-ingest` already produces `Fragment` values
//! out of every RTMP publisher; the [`HlsFragmentBridge`] here observes
//! those fragments, walks them through a [`CmafPolicyState`] to classify
//! each chunk as a partial / partial-independent / segment boundary, and
//! pushes the resulting [`CmafChunk`] into a shared [`HlsServer`].
//!
//! Today the wiring is single-rendition: only the first broadcast that
//! the bridge announces is forwarded to the HLS server, and only its
//! video track. The HLS server itself is single-rendition (no
//! `EXT-X-STREAM-INF` master playlist); multi-broadcast routing lands
//! when a `lvqr-cli` flag asks for it. The integration test in
//! `crates/lvqr-cli/tests/rtmp_hls_e2e.rs` publishes exactly one RTMP
//! stream so the limit is invisible at the contract layer.
//!
//! The observer is invoked synchronously from the `rml_rtmp` callback
//! path, but [`HlsServer::push_chunk_bytes`] is async. Each on_init /
//! on_fragment notification spawns a tiny tokio task instead of
//! blocking the ingest pump.

use bytes::Bytes;
use lvqr_cmaf::{CmafChunk, CmafPolicy, CmafPolicyState};
use lvqr_fragment::Fragment;
use lvqr_hls::HlsServer;
use lvqr_ingest::FragmentObserver;
use std::sync::Mutex;
use tokio::runtime::Handle;

/// Fans bridge-emitted fragments into a single LL-HLS server.
///
/// Construct one per `lvqr-cli` instance, hand it to the bridge via
/// `RtmpMoqBridge::with_observer`, and clone the underlying
/// [`HlsServer`] into the axum router that serves `/playlist.m3u8`.
pub(crate) struct HlsFragmentBridge {
    server: HlsServer,
    state: Mutex<HlsBridgeState>,
}

struct HlsBridgeState {
    /// First broadcast we observed. Only this broadcast feeds HLS
    /// today; subsequent broadcasts are tracked but their fragments
    /// are dropped.
    primary: Option<String>,
    /// Per-track policy state machine. Reset whenever the primary
    /// broadcast publishes a new init segment so a republish on the
    /// same broadcast starts cleanly.
    video_policy: CmafPolicyState,
}

impl HlsFragmentBridge {
    pub fn new(server: HlsServer) -> Self {
        Self {
            server,
            state: Mutex::new(HlsBridgeState {
                primary: None,
                video_policy: CmafPolicyState::new(CmafPolicy::VIDEO_90KHZ_DEFAULT),
            }),
        }
    }

    /// Decide whether a `(broadcast, track)` pair belongs to the
    /// active HLS rendition. The first video track to appear becomes
    /// the primary; everything else is ignored.
    fn is_primary_video(&self, broadcast: &str, track: &str) -> bool {
        if track != "0.mp4" {
            return false;
        }
        let mut state = self.state.lock().expect("hls bridge mutex poisoned");
        match &state.primary {
            Some(name) if name == broadcast => true,
            Some(_) => false,
            None => {
                state.primary = Some(broadcast.to_string());
                tracing::info!(broadcast, "HLS bridge attached to first broadcast");
                true
            }
        }
    }
}

impl FragmentObserver for HlsFragmentBridge {
    fn on_init(&self, broadcast: &str, track: &str, init: Bytes) {
        if !self.is_primary_video(broadcast, track) {
            return;
        }
        // Reset the policy state machine whenever a new init segment
        // arrives so a mid-stream codec change starts from a clean
        // baseline. The HLS server itself accepts repeated push_init
        // calls as overwrites.
        {
            let mut state = self.state.lock().expect("hls bridge mutex poisoned");
            state.video_policy = CmafPolicyState::new(CmafPolicy::VIDEO_90KHZ_DEFAULT);
        }
        let server = self.server.clone();
        let Ok(handle) = Handle::try_current() else {
            tracing::warn!("HLS bridge on_init outside tokio runtime; dropping init");
            return;
        };
        handle.spawn(async move {
            server.push_init(init).await;
        });
    }

    fn on_fragment(&self, broadcast: &str, track: &str, fragment: &Fragment) {
        if !self.is_primary_video(broadcast, track) {
            return;
        }
        let chunk = {
            let mut state = self.state.lock().expect("hls bridge mutex poisoned");
            let decision = state.video_policy.step(fragment.flags.keyframe, fragment.dts);
            CmafChunk {
                track_id: fragment.track_id.clone(),
                payload: fragment.payload.clone(),
                dts: fragment.dts,
                duration: fragment.duration,
                kind: decision.kind,
            }
        };
        let server = self.server.clone();
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

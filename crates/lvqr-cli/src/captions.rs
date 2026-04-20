//! Broadcaster-native HLS subtitle composition glue for
//! `lvqr-cli`.
//!
//! **Tier 4 item 4.5 session C.** Mirror of
//! [`crate::hls::BroadcasterHlsBridge`] for the `captions`
//! track that the WhisperCaptionsAgent (or any other future
//! captions producer) publishes onto the shared
//! [`lvqr_fragment::FragmentBroadcasterRegistry`]. The
//! callback fires on the first `get_or_create` for
//! `(broadcast, "captions")`; the spawned drain task consumes
//! `Fragment` values whose payload is the WebVTT cue text in
//! UTF-8 and whose `dts` / `duration` are wall-clock UNIX
//! milliseconds, and feeds them into the per-broadcast
//! [`lvqr_hls::SubtitlesServer`] under
//! [`MultiHlsServer::ensure_subtitles`].
//!
//! No on-disk persistence: captions are ephemeral by design
//! for v1. Late HLS subscribers see only cues emitted from
//! the moment they joined onwards (the captions playlist's
//! sliding window is bounded; see
//! [`SubtitlesServer::with_max_cues`]).

use lvqr_fragment::{BroadcasterStream, FragmentBroadcasterRegistry, FragmentStream};
use lvqr_hls::{CaptionCue, MultiHlsServer};
use tokio::runtime::Handle;

/// Track-id the captions producer publishes under. Matches
/// `lvqr_agent_whisper::factory::CAPTIONS_TRACK_ID`.
pub(crate) const CAPTIONS_TRACK: &str = "captions";

/// Broadcaster-native subtitle bridge. Stateless: `install`
/// wires a registry callback that owns everything it needs
/// for its per-broadcast drain tasks.
pub(crate) struct BroadcasterCaptionsBridge;

impl BroadcasterCaptionsBridge {
    /// Register an `on_entry_created` callback on `registry`
    /// so every new `(broadcast, "captions")` pair feeds the
    /// per-broadcast subtitles rendition under `multi`.
    /// Callers must invoke this from inside a tokio runtime.
    pub fn install(multi: MultiHlsServer, registry: &FragmentBroadcasterRegistry) {
        registry.on_entry_created(move |broadcast, track, bc| {
            if track != CAPTIONS_TRACK {
                return;
            }
            let broadcast = broadcast.to_string();
            let track = track.to_string();
            // Subscribe synchronously inside the callback so
            // no caption emit can race ahead of the drain
            // loop. The `BroadcasterStream` owns only the
            // Receiver side, so the drain task does not
            // extend the broadcaster's lifetime past the
            // producers' (mirror of the HLS + archive
            // bridges' anti-leak comment).
            let sub = bc.subscribe();
            let handle = match Handle::try_current() {
                Ok(h) => h,
                Err(_) => {
                    tracing::warn!(
                        broadcast = %broadcast,
                        track = %track,
                        "BroadcasterCaptionsBridge: callback fired outside tokio runtime; drain not spawned",
                    );
                    return;
                }
            };
            let subs = multi.ensure_subtitles(&broadcast);
            handle.spawn(Self::drain(subs, broadcast, sub));
        });
    }

    /// Per-broadcast drain task. Runs until every producer-
    /// side clone of the captions broadcaster drops, then
    /// finalizes the captions playlist so blocking-reload
    /// subscribers wake up cleanly.
    async fn drain(subs: lvqr_hls::SubtitlesServer, broadcast: String, mut sub: BroadcasterStream) {
        let mut count = 0u64;
        while let Some(fragment) = sub.next_fragment().await {
            // Fragment.payload is UTF-8 cue text; .dts /
            // .duration are wall-clock UNIX ms (the producer
            // converts source-track ticks before publishing).
            let text = match std::str::from_utf8(&fragment.payload) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        broadcast = %broadcast,
                        error = %e,
                        "BroadcasterCaptionsBridge: cue payload not UTF-8; skipping",
                    );
                    continue;
                }
            };
            subs.push_cue(CaptionCue {
                start_ms: fragment.dts,
                end_ms: fragment.dts.saturating_add(fragment.duration),
                text: text.to_string(),
            });
            count += 1;
        }
        subs.finalize();
        tracing::info!(
            broadcast = %broadcast,
            cues = count,
            "BroadcasterCaptionsBridge: drain terminated (producers closed)",
        );
    }
}

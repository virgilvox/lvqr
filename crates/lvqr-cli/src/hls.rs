//! Broadcaster-native HLS composition glue for `lvqr-cli`.
//!
//! Session 60 consumer-side switchover. Before this session the HLS
//! fan-out was a [`lvqr_ingest::FragmentObserver`] that the RTMP bridge
//! fired synchronously per fragment. [`BroadcasterHlsBridge`] replaces
//! that with a [`FragmentBroadcasterRegistry::on_entry_created`]
//! callback that spawns one tokio drain task per `(broadcast, track)`
//! and streams [`lvqr_fragment::Fragment`] values into a shared
//! [`MultiHlsServer`]. The on-wire HLS surface is byte-identical to
//! the observer-path version (same partial / segment classification,
//! same codec-strings, same per-broadcast cache keys); only the
//! producer -> consumer wiring changed.
//!
//! Install via [`BroadcasterHlsBridge::install`] exactly once at
//! server startup, *before* any ingest crate publishes. The callback
//! runs on the first `get_or_create` for each `(broadcast, track)`
//! pair the ingest crates emit; every subsequent fragment is drained
//! into the per-broadcast [`HlsServer`] by the spawned task.
//!
//! Differences from the observer-based version:
//!
//! * No `FragmentObserver` impl. The bridge is a set of per-
//!   broadcaster drain tasks, not a single callback object threaded
//!   through the ingest crates.
//!
//! * Init-segment delivery is pulled from [`FragmentBroadcaster::meta`]
//!   rather than pushed through a dedicated `on_init` hook. The drain
//!   task refreshes its local meta snapshot before classifying the
//!   first fragment (and before every subsequent fragment, so a late
//!   codec reconfig / RTMP reconnect that re-publishes the init bytes
//!   is picked up by resetting the [`CmafPolicyState`] and pushing the
//!   new init into the [`HlsServer`]).
//!
//! * No shared state on the bridge itself. Each drain task owns its
//!   own [`CmafPolicyState`]; there is no cross-broadcast map to lock.

use bytes::Bytes;
use lvqr_admin::LatencyTracker;
use lvqr_cmaf::{CmafChunk, CmafPolicy, CmafPolicyState};
use lvqr_fragment::{BroadcasterStream, FragmentBroadcasterRegistry, FragmentStream};
use lvqr_hls::{HlsServer, MultiHlsServer};
use tokio::runtime::Handle;

const VIDEO_TRACK: &str = "0.mp4";
const AUDIO_TRACK: &str = "1.mp4";
const TRANSPORT_LABEL: &str = "hls";

/// Broadcaster-native HLS composition helper. Stateless: the struct
/// itself carries nothing -- `install` wires a registry callback that
/// owns everything it needs for its per-broadcast drain tasks.
pub(crate) struct BroadcasterHlsBridge;

impl BroadcasterHlsBridge {
    /// Register an `on_entry_created` callback on `registry` so every
    /// new `(broadcast, track)` pair published by any ingest crate
    /// gets one drain task that feeds the per-track [`HlsServer`]
    /// surface under `multi`.
    ///
    /// Callers must invoke this from inside a tokio runtime.
    /// `segment_duration_ms` and `part_duration_ms` configure the
    /// [`CmafPolicy`] each drain task constructs at its track's
    /// native timescale. `slo` is an optional latency tracker that
    /// receives one sample per fragment delivered onto the HLS
    /// server (Tier 4 item 4.7 session A).
    pub fn install(
        multi: MultiHlsServer,
        segment_duration_ms: u32,
        part_duration_ms: u32,
        registry: &FragmentBroadcasterRegistry,
        slo: Option<LatencyTracker>,
    ) {
        registry.on_entry_created(move |broadcast, track, bc| {
            let broadcast = broadcast.to_string();
            let track = track.to_string();
            let timescale = bc.meta().timescale;
            // Subscribe synchronously inside the callback so no emit
            // can race ahead of the drain loop. `subscribe` returns a
            // Receiver handle; deliberately DO NOT hold an
            // `Arc<FragmentBroadcaster>` here. Holding one would keep
            // the `broadcast::Sender` alive via the Arc and the recv
            // loop would never see `Closed` after every ingest clone
            // dropped, exactly the leak the archive indexer's comment
            // documents.
            let sub = bc.subscribe();
            let handle = match Handle::try_current() {
                Ok(h) => h,
                Err(_) => {
                    tracing::warn!(
                        broadcast = %broadcast,
                        track = %track,
                        "BroadcasterHlsBridge: callback fired outside tokio runtime; drain not spawned",
                    );
                    return;
                }
            };
            let server = match track.as_str() {
                VIDEO_TRACK => multi.ensure_video(&broadcast),
                AUDIO_TRACK => multi.ensure_audio(&broadcast, timescale),
                _ => {
                    // Unknown track id: no HLS rendition to feed.
                    return;
                }
            };
            handle.spawn(Self::drain(
                server,
                broadcast,
                track,
                timescale,
                segment_duration_ms,
                part_duration_ms,
                sub,
                slo.clone(),
            ));
        });
    }

    /// Per-broadcaster drain task. Runs until every producer-side
    /// clone of the broadcaster drops.
    #[allow(clippy::too_many_arguments)]
    async fn drain(
        server: HlsServer,
        broadcast: String,
        track: String,
        timescale: u32,
        segment_duration_ms: u32,
        part_duration_ms: u32,
        mut sub: BroadcasterStream,
        slo: Option<LatencyTracker>,
    ) {
        let mut state = CmafPolicyState::new(CmafPolicy::with_durations(
            timescale,
            segment_duration_ms,
            part_duration_ms,
        ));
        let mut last_init: Option<Bytes> = None;
        while let Some(fragment) = sub.next_fragment().await {
            // Re-check the init segment each iteration. The first
            // emit races us in through the on_entry_created callback:
            // by the time the first fragment lands, `publish_init`
            // has already run `set_init_segment`, so the first
            // refresh_meta below picks it up. Later refreshes catch
            // re-publishes (RTMP reconnect or mid-stream codec
            // change) because `lvqr_ingest::publish_init` reuses the
            // existing broadcaster and overwrites the init bytes.
            sub.refresh_meta();
            if let Some(current_init) = sub.meta().init_segment.clone() {
                // Bytes equality is O(n) but the init segment is a
                // few hundred bytes and the check runs once per
                // fragment, so the cost is negligible. A ptr-identity
                // check would miss the "same logical init, new Bytes
                // allocation" case a reconnecting ingest produces.
                let changed = match last_init.as_ref() {
                    None => true,
                    Some(prev) => prev != &current_init,
                };
                if changed {
                    server.push_init(current_init.clone()).await;
                    // Reset the policy state so partial / segment
                    // classification restarts cleanly after an init
                    // change.
                    state = CmafPolicyState::new(CmafPolicy::with_durations(
                        timescale,
                        segment_duration_ms,
                        part_duration_ms,
                    ));
                    last_init = Some(current_init);
                }
            }
            let kind = state.step(fragment.flags.keyframe, fragment.dts).kind;
            let chunk = CmafChunk {
                track_id: fragment.track_id.clone(),
                payload: fragment.payload.clone(),
                dts: fragment.dts,
                duration: fragment.duration,
                kind,
            };
            let payload = chunk.payload.clone();
            if let Err(e) = server.push_chunk_bytes(&chunk, payload).await {
                tracing::debug!(error = ?e, "hls push_chunk_bytes rejected");
            }
            // Tier 4 item 4.7 session A: record one sample per
            // fragment delivered to the HLS server. Skipped when the
            // ingest path did not stamp an `ingest_time_ms` (older
            // callers, test fixtures, federation relays that
            // deliberately preserve zero) or when no tracker was
            // wired in at server startup.
            if let Some(tracker) = slo.as_ref()
                && fragment.ingest_time_ms > 0
            {
                let now_ms = unix_wall_ms();
                let latency = now_ms.saturating_sub(fragment.ingest_time_ms);
                tracker.record(&broadcast, TRANSPORT_LABEL, latency);
            }
        }
        tracing::info!(
            broadcast = %broadcast,
            track = %track,
            "BroadcasterHlsBridge: drain terminated (producers closed)",
        );
    }
}

/// UNIX wall-clock milliseconds. Falls back to `0` when the system
/// clock is set before the UNIX epoch; callers should treat `0` as
/// an unset stamp (mirrors `lvqr_ingest::dispatch::unix_wall_ms`).
fn unix_wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

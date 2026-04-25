//! LVQR server library entry point.
//!
//! `main.rs` parses CLI args and hands off to [`start`]; tests and embedders
//! can call [`start`] directly with a pre-built [`ServeConfig`]. Every
//! listener (MoQ/QUIC, RTMP/TCP, admin/TCP) is bound inside `start` before
//! it returns, so callers who pass `port: 0` can read the real bound
//! addresses back off the returned [`ServerHandle`] and point test clients
//! at them without polling.
//!
//! This is the library target used by `lvqr-test-utils::TestServer` to
//! spin up a full-stack LVQR instance on ephemeral ports inside
//! integration tests.

mod archive;
mod auth_middleware;
mod captions;
#[cfg(feature = "cluster")]
pub mod cluster_claim;
mod config;
mod handle;
mod hls;
mod signed_url;
mod ws;

pub use archive::sign_playback_url;
pub use config::ServeConfig;
#[cfg(feature = "transcode")]
pub use config::{parse_one_transcode_rendition, parse_transcode_renditions};
pub use handle::ServerHandle;
/// Re-export of [`lvqr_admin::LatencyTracker`] so downstream callers
/// (`lvqr-test-utils`, integration tests) do not need to pull
/// `lvqr-admin` in as a direct dep. Tier 4 item 4.7 session A.
pub use lvqr_admin::{LatencyTracker, SloEntry};
pub use signed_url::{LiveScheme, sign_live_url};

use anyhow::Result;
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use lvqr_auth::{NoopAuthProvider, SharedAuth};
use lvqr_core::{EventBus, RelayEvent};
use lvqr_dash::{BroadcasterDashBridge, DashConfig};
use lvqr_fragment::FragmentBroadcasterRegistry;
use lvqr_hls::{MultiHlsServer, PlaylistBuilderConfig};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;

#[cfg(feature = "c2pa")]
use crate::archive::verify_router;
use crate::archive::{BroadcasterArchiveIndexer, playback_router};
use crate::auth_middleware::{
    LivePlaybackAuthState, SignalAuthState, live_playback_auth_middleware, signal_auth_middleware,
};
use crate::hls::BroadcasterHlsBridge;
use crate::ws::{WsRelayState, spawn_recordings, ws_ingest_handler, ws_relay_handler};

/// Start a full-stack LVQR server. All listeners are bound before the
/// function returns, so the [`ServerHandle`] immediately reports real
/// addresses even when the config requested ephemeral ports.
///
/// The returned handle owns a background task that runs the relay, RTMP,
/// and admin subsystems under a shared cancellation token. Use
/// [`ServerHandle::shutdown`] for deterministic teardown.
pub async fn start(config: ServeConfig) -> Result<ServerHandle> {
    tracing::info!(
        relay = %config.relay_addr,
        rtmp = %config.rtmp_addr,
        admin = %config.admin_addr,
        mesh = config.mesh_enabled,
        "starting LVQR server"
    );

    // Metrics recorder install. Process-wide, must be skipped in
    // tests (all four permutations below call
    // `metrics::set_global_recorder` which panics or errors on
    // second install). The four cases are:
    //   Prom + OTel:   install a FanoutBuilder of both.
    //   Prom only:     install the Prometheus recorder (legacy).
    //   OTel only:     install the OTel-forwarding recorder.
    //   Neither:       install nothing; metrics calls are no-ops.
    // The `PrometheusRecorder` handle is exposed on
    // `ServerHandle` for the admin `/metrics` scrape route, so
    // we always capture it before handing the recorder off to a
    // Fanout layer.
    let prom_handle = match (config.install_prometheus, config.otel_metrics_recorder.clone()) {
        (true, Some(otel_recorder)) => {
            let prom_recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
            let handle = prom_recorder.handle();
            let fanout = metrics_util::layers::FanoutBuilder::default()
                .add_recorder(prom_recorder)
                .add_recorder(otel_recorder)
                .build();
            metrics::set_global_recorder(fanout)
                .map_err(|e| anyhow::anyhow!("failed to install metrics fanout recorder: {e}"))?;
            Some(handle)
        }
        (true, None) => Some(
            metrics_exporter_prometheus::PrometheusBuilder::new()
                .install_recorder()
                .map_err(|e| anyhow::anyhow!("failed to install Prometheus recorder: {e}"))?,
        ),
        (false, Some(otel_recorder)) => {
            metrics::set_global_recorder(otel_recorder)
                .map_err(|e| anyhow::anyhow!("failed to install OTLP metrics recorder: {e}"))?;
            None
        }
        (false, None) => None,
    };

    let shutdown = CancellationToken::new();

    // Auth provider: caller-provided, or fall back to open access.
    let auth: SharedAuth = config
        .auth
        .clone()
        .unwrap_or_else(|| Arc::new(NoopAuthProvider) as SharedAuth);

    // Shared lifecycle bus: bridge and WS ingest emit
    // BroadcastStarted/Stopped here; recorder subscribes to it.
    let events = EventBus::default();

    // MoQ relay: init_server() binds the QUIC socket and reports the real
    // bound address, which we surface back through ServerHandle.
    let relay_config = lvqr_relay::RelayConfig::new(config.relay_addr);
    let mut relay = lvqr_relay::RelayServer::new(relay_config);
    relay.set_auth_provider(auth.clone());
    let (mut moq_server, relay_bound) = relay.init_server()?;
    tracing::info!(addr = %relay_bound, "MoQ relay bound");

    // Optional cluster bootstrap. Resolver for `MultiHlsServer` is
    // built up-front so the HLS constructor below can install it in
    // one shot instead of patching the server after the fact.
    #[cfg(feature = "cluster")]
    let cluster = if let Some(listen) = config.cluster_listen {
        let ccfg = lvqr_cluster::ClusterConfig {
            listen,
            seeds: config.cluster_seeds.clone(),
            node_id: config.cluster_node_id.clone().map(lvqr_cluster::NodeId::new),
            cluster_id: config
                .cluster_id
                .clone()
                .unwrap_or_else(|| lvqr_cluster::ClusterConfig::default().cluster_id),
            ..lvqr_cluster::ClusterConfig::default()
        };
        let c = lvqr_cluster::Cluster::bootstrap(ccfg)
            .await
            .map_err(|e| anyhow::anyhow!("cluster bootstrap failed: {e}"))?;
        let c = std::sync::Arc::new(c);
        if config.cluster_advertise_hls.is_some()
            || config.cluster_advertise_dash.is_some()
            || config.cluster_advertise_rtsp.is_some()
        {
            let endpoints = lvqr_cluster::NodeEndpoints {
                hls: config.cluster_advertise_hls.clone(),
                dash: config.cluster_advertise_dash.clone(),
                rtsp: config.cluster_advertise_rtsp.clone(),
            };
            c.set_endpoints(&endpoints)
                .await
                .map_err(|e| anyhow::anyhow!("cluster set_endpoints failed: {e}"))?;
        }
        tracing::info!(
            node = %c.self_id(),
            %listen,
            advertise_hls = ?config.cluster_advertise_hls,
            advertise_dash = ?config.cluster_advertise_dash,
            advertise_rtsp = ?config.cluster_advertise_rtsp,
            "cluster enabled"
        );
        Some(c)
    } else {
        None
    };
    #[cfg(feature = "cluster")]
    let hls_owner_resolver: Option<lvqr_hls::OwnerResolver> = cluster.as_ref().map(|c| {
        let c = c.clone();
        let resolver: lvqr_hls::OwnerResolver = std::sync::Arc::new(move |broadcast: String| {
            let c = c.clone();
            Box::pin(async move {
                let (_, endpoints) = c.find_owner_endpoints(&broadcast).await?;
                endpoints.hls
            })
        });
        resolver
    });
    #[cfg(not(feature = "cluster"))]
    let hls_owner_resolver: Option<lvqr_hls::OwnerResolver> = None;
    #[cfg(feature = "cluster")]
    let dash_owner_resolver: Option<lvqr_dash::OwnerResolver> = cluster.as_ref().map(|c| {
        let c = c.clone();
        let resolver: lvqr_dash::OwnerResolver = std::sync::Arc::new(move |broadcast: String| {
            let c = c.clone();
            Box::pin(async move {
                let (_, endpoints) = c.find_owner_endpoints(&broadcast).await?;
                endpoints.dash
            })
        });
        resolver
    });
    #[cfg(not(feature = "cluster"))]
    let dash_owner_resolver: Option<lvqr_dash::OwnerResolver> = None;
    #[cfg(feature = "cluster")]
    let rtsp_owner_resolver: Option<lvqr_rtsp::OwnerResolver> = cluster.as_ref().map(|c| {
        let c = c.clone();
        let resolver: lvqr_rtsp::OwnerResolver = std::sync::Arc::new(move |broadcast: String| {
            let c = c.clone();
            Box::pin(async move {
                let (_, endpoints) = c.find_owner_endpoints(&broadcast).await?;
                endpoints.rtsp
            })
        });
        resolver
    });
    #[cfg(not(feature = "cluster"))]
    let rtsp_owner_resolver: Option<lvqr_rtsp::OwnerResolver> = None;

    // Optional multi-broadcast LL-HLS server. The broadcaster-native
    // HLS bridge (installed below) subscribes on the shared registry
    // and pumps fragments into the shared `MultiHlsServer` state.
    // Each broadcast gets its own per-broadcast `HlsServer` on first
    // publish; the axum router demultiplexes requests under
    // `/hls/{broadcast}/...`.
    //
    // When clustering is enabled, an `OwnerResolver` redirects
    // subscribers of peer-owned broadcasts to the owning node's HLS
    // URL instead of returning 404.
    let target_dur = config.hls_target_duration_secs;
    let part_target_secs = config.hls_part_target_ms as f32 / 1000.0;
    let max_segments = if config.hls_dvr_window_secs == 0 || target_dur == 0 {
        None
    } else {
        Some((config.hls_dvr_window_secs / target_dur) as usize)
    };
    let hls_server = config.hls_addr.map(|_| {
        let playlist_cfg = PlaylistBuilderConfig {
            target_duration_secs: target_dur,
            part_target_secs,
            max_segments,
            ..PlaylistBuilderConfig::default()
        };
        match hls_owner_resolver.clone() {
            Some(r) => MultiHlsServer::with_owner_resolver(playlist_cfg, r),
            None => MultiHlsServer::new(playlist_cfg),
        }
    });

    // Optional multi-broadcast MPEG-DASH server. Sibling of the
    // LL-HLS fan-out above: a single `MultiDashServer` subscribes
    // on the shared registry and projects fragments onto a
    // per-broadcast axum router mounted under `/dash/{broadcast}/...`.
    // Every ingest protocol (RTMP, WHIP, SRT, RTSP) feeds DASH via
    // the same `BroadcasterDashBridge` install below.
    let dash_server = config.dash_addr.map(|_| match dash_owner_resolver.clone() {
        Some(r) => lvqr_dash::MultiDashServer::with_owner_resolver(DashConfig::default(), r),
        None => lvqr_dash::MultiDashServer::new(DashConfig::default()),
    });

    // Shared FragmentBroadcasterRegistry used by every ingest crate
    // and every consumer. Session 60 completed the Tier 2.1 migration:
    // every ingest protocol publishes to this one registry, and every
    // consumer (archive, LL-HLS, DASH) installs an on_entry_created
    // callback against it.
    let shared_registry = FragmentBroadcasterRegistry::new();

    // Tier 4 item 4.7 session A: one shared `LatencyTracker` per
    // server feeds samples from every instrumented egress surface
    // (currently LL-HLS drain + WS relay) and powers the
    // `/api/v1/slo` admin route + the
    // `lvqr_subscriber_glass_to_glass_ms` Prometheus histogram.
    // Tests read the snapshot directly off `ServerHandle::slo()`.
    let slo_tracker = lvqr_admin::LatencyTracker::new();

    // Auto-claim every new broadcast against the cluster so peers
    // redirect correctly without the operator having to call
    // `Cluster::claim_broadcast` by hand. The bridge holds the
    // `Claim` alive until every ingest publisher for that
    // broadcast disconnects. Feature-gated; no-op when
    // single-node.
    #[cfg(feature = "cluster")]
    if let Some(ref c) = cluster {
        cluster_claim::install_cluster_claim_bridge(c.clone(), cluster_claim::DEFAULT_CLAIM_LEASE, &shared_registry);
    }

    // Optional WASM fragment filter tap. Installed BEFORE any
    // ingest listener accepts traffic so the very first fragment
    // of the first broadcast flows through the filter chain. Each
    // path in `config.wasm_filter` becomes its own
    // `SharedFilter` + `WasmFilterReloader` pair; the bridge sees
    // one `ChainFilter` wrapping the ordered list. Per-slot
    // reloaders watch the module path for changes and call
    // `SharedFilter::replace` atomically when the file changes;
    // in-flight fragments finish on the old module and the next
    // fragment sees the new one, without disturbing the other
    // slots in the chain.
    let (wasm_filter_handle, wasm_reloader_handles, wasm_slot_counters) = if config.wasm_filter.is_empty() {
        (None, Vec::new(), Vec::new())
    } else {
        let mut shareds: Vec<lvqr_wasm::SharedFilter> = Vec::with_capacity(config.wasm_filter.len());
        let mut reloaders: Vec<lvqr_wasm::WasmFilterReloader> = Vec::with_capacity(config.wasm_filter.len());
        for path in &config.wasm_filter {
            let filter = lvqr_wasm::WasmFilter::load(path)
                .map_err(|e| anyhow::anyhow!("WASM filter load at {} failed: {e}", path.display()))?;
            tracing::info!(path = %path.display(), "WASM fragment filter loaded");
            let shared = lvqr_wasm::SharedFilter::new(filter);
            let reloader = lvqr_wasm::WasmFilterReloader::spawn(path, shared.clone())
                .map_err(|e| anyhow::anyhow!("WASM filter hot-reload watcher at {} failed: {e}", path.display()))?;
            shareds.push(shared);
            reloaders.push(reloader);
        }
        let chain = lvqr_wasm::ChainFilter::new(shareds);
        let chain_len = chain.len();
        // PLAN Phase D session 140: extract per-slot counter handles
        // BEFORE wrapping the chain in the bridge's outer SharedFilter.
        // The outer SharedFilter type-erases the ChainFilter; capturing
        // the Arc<SlotCounters> handles here is how the admin closure
        // below reads per-slot seen/kept/dropped for
        // `GET /api/v1/wasm-filter`.
        let slot_counters = chain.slot_counters();
        tracing::info!(chain_len, "WASM fragment filter chain installed");
        let chain_shared = lvqr_wasm::SharedFilter::new(chain);
        let bridge = lvqr_wasm::install_wasm_filter_bridge(&shared_registry, chain_shared, chain_len);
        (Some(bridge), reloaders, slot_counters)
    };

    // RTMP ingest bridged to MoQ. Pre-bind the TCP listener so we can
    // report the real bound port (for ephemeral-port test setups).
    let mut bridge_builder = lvqr_ingest::RtmpMoqBridge::with_auth(relay.origin().clone(), auth.clone())
        .with_events(events.clone())
        .with_registry(shared_registry.clone());

    // Optional DVR archive index. Opened before the bridge is frozen
    // so the BroadcasterArchiveIndexer can install its on_entry_created
    // callback on the shared registry. The index file lives at
    // `<archive_dir>/archive.redb`; the directory is created on
    // demand if it does not already exist.
    let archive_index = if let Some(ref dir) = config.archive_dir {
        std::fs::create_dir_all(dir)
            .map_err(|e| anyhow::anyhow!("archive: failed to create {}: {e}", dir.display()))?;
        let db_path = dir.join("archive.redb");
        let index = lvqr_archive::RedbSegmentIndex::open(&db_path)
            .map_err(|e| anyhow::anyhow!("archive: failed to open {}: {e}", db_path.display()))?;
        tracing::info!(dir = %dir.display(), "DVR archive index enabled");
        Some((dir.clone(), Arc::new(index)))
    } else {
        None
    };

    // Install the broadcaster-based archive indexer on the shared
    // registry. Every subsequent ingest-side emit is drained to disk +
    // redb by a per-broadcaster tokio task the indexer spawns.
    if let Some((ref dir, ref index)) = archive_index {
        #[cfg(feature = "c2pa")]
        BroadcasterArchiveIndexer::install(dir.clone(), Arc::clone(index), &shared_registry, config.c2pa.clone());
        #[cfg(not(feature = "c2pa"))]
        BroadcasterArchiveIndexer::install(dir.clone(), Arc::clone(index), &shared_registry);
    }

    // Install the broadcaster-based LL-HLS composition bridge on the
    // shared registry. Every ingest crate's first `publish_init` for a
    // `(broadcast, track)` pair fires the callback; the callback
    // subscribes and spawns a drain task that projects fragments onto
    // the shared `MultiHlsServer`. Session 60 consumer-side switchover:
    // replaces the FragmentObserver path the HLS bridge used through
    // session 59.
    if let Some(ref hls) = hls_server {
        BroadcasterHlsBridge::install(
            hls.clone(),
            config.hls_target_duration_secs * 1000,
            config.hls_part_target_ms,
            &shared_registry,
            Some(slo_tracker.clone()),
        );
        // Tier 4 item 4.5 session C: feed the captions
        // sub-track into the per-broadcast subtitles
        // rendition. The bridge no-ops on every track that
        // is not `"captions"`, so it composes safely with
        // the LL-HLS bridge above.
        captions::BroadcasterCaptionsBridge::install(hls.clone(), &shared_registry);
    }

    // Tier 4 item 4.5 session D: if the operator passed
    // `--whisper-model <PATH>`, build the
    // `WhisperCaptionsFactory` + `AgentRunner` and install it
    // onto the shared registry so every new
    // `(broadcast, "1.mp4")` triggers a WhisperCaptionsAgent.
    // The agent republishes each caption cue onto
    // `(broadcast, "captions")` where the
    // `BroadcasterCaptionsBridge` above picks it up and feeds
    // the HLS subtitle rendition. Without the flag (or without
    // the `whisper` feature at all) no AI state is constructed.
    #[cfg(feature = "whisper")]
    let agent_runner_handle = if let Some(ref path) = config.whisper_model {
        if hls_server.is_none() {
            // The captions track reaches browser players only via
            // the HLS subtitle rendition that `BroadcasterCaptionsBridge`
            // wires above. With HLS disabled the WhisperCaptionsAgent
            // still runs and publishes cues onto the registry and the
            // in-process `CaptionStream`, but browser subscribers see
            // nothing. Warn so misconfigured deployments surface early
            // rather than through silent captions loss.
            tracing::warn!(
                path = %path.display(),
                "whisper captions agent enabled without HLS surface; browser clients will not receive captions"
            );
        }
        let factory =
            lvqr_agent_whisper::WhisperCaptionsFactory::new(lvqr_agent_whisper::WhisperConfig::new(path.clone()))
                .with_caption_registry(shared_registry.clone());
        tracing::info!(path = %path.display(), "whisper captions agent enabled");
        Some(
            lvqr_agent::AgentRunner::new()
                .with_factory(factory)
                .install(&shared_registry),
        )
    } else {
        None
    };

    // Install the broadcaster-based DASH composition bridge. Same
    // pattern as LL-HLS: the callback spawns a drain task per
    // `(broadcast, track)` that stamps a monotonic `$Number$` counter
    // onto every observed fragment and pushes it into the per-broadcast
    // `DashServer`. Session 60: completes the consumer-side switchover.
    if let Some(ref dash) = dash_server {
        BroadcasterDashBridge::install(dash.clone(), &shared_registry, Some(slo_tracker.clone()));
    }

    // Tier 4 item 4.6 session 106 C: if the operator passed
    // `--transcode-rendition <NAME>` one or more times, install one
    // `SoftwareTranscoderFactory` (GStreamer-backed video encoder) +
    // one `AudioPassthroughTranscoderFactory` (zero-dep audio copy)
    // per rendition against the shared registry. Every source
    // broadcast's video + audio tracks then fan out into
    // `<source>/<rendition>/{0,1}.mp4` output broadcasters the HLS
    // bridge drains automatically. The ladder's metadata is also
    // registered on the HLS server so the master-playlist composer
    // emits one `#EXT-X-STREAM-INF` per rendition sibling.
    #[cfg(feature = "transcode")]
    let transcode_runner_handle = if config.transcode_renditions.is_empty() {
        None
    } else {
        let mut runner = lvqr_transcode::TranscodeRunner::new();
        let skip_suffixes: Vec<String> = config.transcode_renditions.iter().map(|r| r.name.clone()).collect();
        for spec in &config.transcode_renditions {
            let video_factory = lvqr_transcode::SoftwareTranscoderFactory::new(spec.clone(), shared_registry.clone())
                .skip_source_suffixes(skip_suffixes.clone());
            let audio_factory =
                lvqr_transcode::AudioPassthroughTranscoderFactory::new(spec.clone(), shared_registry.clone())
                    .skip_source_suffixes(skip_suffixes.clone());
            runner = runner.with_factory(video_factory).with_factory(audio_factory);
        }
        tracing::info!(
            renditions = ?config
                .transcode_renditions
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>(),
            "transcode ladder enabled",
        );
        // Publish ladder metadata to the HLS master-playlist composer.
        if let Some(ref hls) = hls_server {
            let meta: Vec<lvqr_hls::RenditionMeta> = config
                .transcode_renditions
                .iter()
                .map(|r| lvqr_hls::RenditionMeta {
                    name: r.name.clone(),
                    bandwidth_bps: lvqr_hls::RenditionMeta::bandwidth_bps_with_overhead(
                        r.video_bitrate_kbps + r.audio_bitrate_kbps,
                    ),
                    resolution: Some((r.width, r.height)),
                    // Hard-coded placeholder per session 106 C
                    // decision (d): real SPS / ASC parsing is a
                    // session-107-or-later job.
                    codecs: "avc1.640028,mp4a.40.2".into(),
                })
                .collect();
            hls.set_ladder(meta);
            hls.set_source_bandwidth_bps(
                config
                    .source_bandwidth_kbps
                    .map(|kbps| (kbps as u64).saturating_mul(1_000)),
            );
        }
        Some(runner.install(&shared_registry))
    };

    // Tier 4 item 4.4 session B: start a FederationRunner against any
    // configured peer-cluster MoQ relays. Each runner task opens an
    // outbound MoQ session, drains the remote origin's announcement
    // stream, filters by the link's forwarded_broadcasts list, and
    // re-publishes matched broadcasts into the local relay's origin
    // producer so every MoQ subscriber on this node sees them as if
    // they were ingested locally. No-op when the links list is empty.
    // Feature-gated on `cluster` so single-node builds stay thin.
    #[cfg(feature = "cluster")]
    let federation_runner_handle = if config.federation_links.is_empty() {
        None
    } else {
        tracing::info!(links = config.federation_links.len(), "starting federation runner");
        Some(lvqr_cluster::FederationRunner::start(
            config.federation_links.clone(),
            relay.origin().clone(),
            shutdown.clone(),
        ))
    };

    // Optional WHEP surface. Constructed before the bridge is
    // frozen into an `Arc` so we can attach the `WhepServer` as a
    // `RawSampleObserver`; both the observer clone and the axum
    // router clone share the same underlying session registry, so
    // a POST on the router is immediately visible to the raw-sample
    // fanout path.
    let whep_server = if let Some(addr) = config.whep_addr {
        let str0m_cfg = lvqr_whep::Str0mConfig { host_ip: addr.ip() };
        // Tier 4 item 4.7 session 110 B: thread the shared
        // LatencyTracker into the str0m answerer so every spawned
        // session's poll loop records one sample per successful
        // `Writer::write` under transport="whep".
        let answerer_builder = lvqr_whep::Str0mAnswerer::new(str0m_cfg).with_slo_tracker(slo_tracker.clone());
        // Session 113: when built with the `transcode` meta-feature
        // (which activates `lvqr-whep/aac-opus`), attach the AAC-to-
        // Opus encoder factory so RTMP / SRT / RTSP AAC publishers
        // reach Opus-only WHEP subscribers with audio. The factory
        // probes GStreamer elements once; missing elements log and
        // the factory opts out per-session, so a misconfigured host
        // still serves video to WHEP without panicking.
        #[cfg(feature = "transcode")]
        let answerer_builder =
            answerer_builder.with_aac_to_opus_factory(Arc::new(lvqr_transcode::AacToOpusEncoderFactory::new()));
        let answerer = Arc::new(answerer_builder) as Arc<dyn lvqr_whep::SdpAnswerer>;
        let server = lvqr_whep::WhepServer::new(answerer);
        let observer: lvqr_ingest::SharedRawSampleObserver = Arc::new(server.clone());
        bridge_builder = bridge_builder.with_raw_sample_observer(observer);
        Some(server)
    } else {
        None
    };

    // Optional WHIP ingest surface. The bridge side is a sibling
    // of `RtmpMoqBridge`: it owns its own `BroadcastProducer` state
    // but publishes fragments onto the same shared registry, so every
    // existing egress (MoQ, LL-HLS, DASH, disk record, DVR archive)
    // picks up WHIP publishers with zero additional wiring.
    let (whip_server, whip_bridge) = if let Some(addr) = config.whip_addr {
        let mut whip_bridge = lvqr_whip::WhipMoqBridge::new(relay.origin().clone())
            .with_events(events.clone())
            .with_registry(shared_registry.clone());
        if let Some(ref server) = whep_server {
            let raw_observer: lvqr_ingest::SharedRawSampleObserver = Arc::new(server.clone());
            whip_bridge = whip_bridge.with_raw_sample_observer(raw_observer);
        }
        let whip_bridge_arc = Arc::new(whip_bridge);
        let sink = whip_bridge_arc.clone() as Arc<dyn lvqr_whip::IngestSampleSink>;
        let str0m_cfg = lvqr_whip::Str0mIngestConfig { host_ip: addr.ip() };
        let answerer =
            Arc::new(lvqr_whip::Str0mIngestAnswerer::new(str0m_cfg, sink)) as Arc<dyn lvqr_whip::SdpAnswerer>;
        let server = lvqr_whip::WhipServer::with_auth_provider(answerer, auth.clone());
        (Some(server), Some(whip_bridge_arc))
    } else {
        (None, None)
    };

    // Optional SRT ingest server. Publishes to the shared registry;
    // every broadcaster-native consumer picks up SRT publishers
    // automatically.
    let (srt_server, srt_bound) = if let Some(addr) = config.srt_addr {
        let mut server =
            lvqr_srt::SrtIngestServer::with_registry(addr, shared_registry.clone()).with_auth(auth.clone());
        let bound = server.bind().await?;
        tracing::info!(addr = %bound, "SRT ingest bound");
        (Some(server), Some(bound))
    } else {
        (None, None)
    };
    let srt_events_clone = events.clone();
    let srt_shutdown_token = shutdown.clone();

    // Optional RTSP ingest server. Publishes to the shared registry
    // alongside every other ingest protocol. When clustering is
    // enabled, the owner resolver redirects DESCRIBE / PLAY for
    // peer-owned broadcasts with RTSP 302.
    let (rtsp_server, rtsp_bound) = if let Some(addr) = config.rtsp_addr {
        let mut server = lvqr_rtsp::RtspServer::with_registry(addr, shared_registry.clone()).with_auth(auth.clone());
        if let Some(r) = rtsp_owner_resolver.clone() {
            server = server.with_owner_resolver(r);
        }
        let bound = server.bind().await?;
        tracing::info!(addr = %bound, "RTSP ingest bound");
        (Some(server), Some(bound))
    } else {
        (None, None)
    };
    let rtsp_events_clone = events.clone();
    let rtsp_shutdown_token = shutdown.clone();

    let bridge = Arc::new(bridge_builder);
    let rtmp_config = lvqr_ingest::RtmpConfig {
        bind_addr: config.rtmp_addr,
    };
    let rtmp_server = bridge.create_rtmp_server(rtmp_config);
    let rtmp_listener = tokio::net::TcpListener::bind(config.rtmp_addr).await?;
    let rtmp_bound = rtmp_listener.local_addr()?;
    tracing::info!(addr = %rtmp_bound, "RTMP ingest bound");

    // Admin listener: pre-bind to capture the real port.
    let admin_listener = tokio::net::TcpListener::bind(config.admin_addr).await?;
    let admin_bound = admin_listener.local_addr()?;
    tracing::info!(addr = %admin_bound, "admin HTTP bound");

    // HLS listener: pre-bind so the test harness can read the
    // ephemeral port back via `ServerHandle::hls_addr` immediately
    // after `start()` returns.
    let (hls_listener, hls_bound) = if let Some(addr) = config.hls_addr {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        tracing::info!(addr = %bound, "LL-HLS HTTP bound");
        (Some(listener), Some(bound))
    } else {
        (None, None)
    };

    // WHEP listener: pre-bind the same way so test harnesses can
    // read the ephemeral port back immediately. `whep_server` was
    // built earlier and is `None` if `config.whep_addr` is `None`.
    let (whep_listener, whep_bound) = if let Some(addr) = config.whep_addr {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        tracing::info!(addr = %bound, "WHEP HTTP bound");
        (Some(listener), Some(bound))
    } else {
        (None, None)
    };

    // DASH listener: pre-bind so ephemeral-port test harnesses can
    // read the real port back via `ServerHandle::dash_addr`
    // immediately after `start()` returns.
    let (dash_listener, dash_bound) = if let Some(addr) = config.dash_addr {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        tracing::info!(addr = %bound, "MPEG-DASH HTTP bound");
        (Some(listener), Some(bound))
    } else {
        (None, None)
    };

    // WHIP listener: pre-bind for the same reason. Keeping the
    // bridge arc alive for the lifetime of the server task is
    // important: dropping it would tear down every active MoQ
    // broadcast produced by a WHIP publisher.
    let (whip_listener, whip_bound) = if let Some(addr) = config.whip_addr {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        tracing::info!(addr = %bound, "WHIP HTTP bound");
        (Some(listener), Some(bound))
    } else {
        (None, None)
    };

    // Optional disk recorder.
    if let Some(ref dir) = config.record_dir {
        let recorder = lvqr_record::BroadcastRecorder::new(dir);
        let origin = relay.origin().clone();
        let event_rx = events.subscribe();
        let record_shutdown = shutdown.clone();
        tracing::info!(dir = %dir.display(), "recording enabled");
        tokio::spawn(async move {
            spawn_recordings(recorder, origin, event_rx, record_shutdown).await;
        });
    }

    // HLS finalization subscriber: when a broadcaster disconnects
    // the RTMP bridge emits BroadcastStopped, and this task calls
    // MultiHlsServer::finalize_broadcast so the playlist gains
    // EXT-X-ENDLIST and the retained window becomes a VOD surface.
    if let Some(ref hls) = hls_server {
        let hls_for_finalize = hls.clone();
        let mut hls_event_rx = events.subscribe();
        let hls_finalize_shutdown = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = hls_finalize_shutdown.cancelled() => break,
                    msg = hls_event_rx.recv() => {
                        match msg {
                            Ok(RelayEvent::BroadcastStopped { name }) => {
                                tracing::info!(broadcast = %name, "finalizing HLS broadcast");
                                hls_for_finalize.finalize_broadcast(&name).await;
                            }
                            Ok(_) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(missed = n, "HLS finalize subscriber lagged");
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }

    // DASH finalization subscriber: same pattern as HLS above.
    // Switches the MPD from type="dynamic" to type="static" so
    // DASH clients stop polling for new segments.
    if let Some(ref dash) = dash_server {
        let dash_for_finalize = dash.clone();
        let mut dash_event_rx = events.subscribe();
        let dash_finalize_shutdown = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = dash_finalize_shutdown.cancelled() => break,
                    msg = dash_event_rx.recv() => {
                        match msg {
                            Ok(RelayEvent::BroadcastStopped { name }) => {
                                tracing::info!(broadcast = %name, "finalizing DASH broadcast");
                                dash_for_finalize.finalize_broadcast(&name);
                            }
                            Ok(_) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(missed = n, "DASH finalize subscriber lagged");
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }

    // Admin HTTP state and router.
    let metrics_state = relay.metrics().clone();
    let bridge_for_stats = bridge.clone();
    let bridge_for_streams = bridge.clone();

    let admin_state = lvqr_admin::AdminState::new(
        move || {
            let active = bridge_for_stats.active_stream_count() as u64;
            lvqr_core::RelayStats {
                publishers: active,
                tracks: active * 2,
                subscribers: metrics_state.connections_active.load(Ordering::Relaxed),
                bytes_received: 0,
                bytes_sent: 0,
                uptime_secs: 0,
            }
        },
        move || {
            bridge_for_streams
                .stream_names()
                .into_iter()
                .map(|name| lvqr_admin::StreamInfo { name, subscribers: 0 })
                .collect()
        },
    )
    .with_auth(auth.clone());
    let admin_state = if let Some(prom) = prom_handle {
        admin_state.with_metrics(Arc::new(move || prom.render()))
    } else {
        admin_state
    };
    // Wire the cluster into `/api/v1/cluster/*`. Without this the
    // feature-gated routes in `lvqr-admin` reply 500 with a
    // "cluster not wired" message.
    #[cfg(feature = "cluster")]
    let admin_state = match cluster.as_ref() {
        Some(c) => admin_state.with_cluster(c.clone()),
        None => admin_state,
    };
    // Wire the federation status handle into
    // `/api/v1/cluster/federation`. Tier 4 item 4.4 session C.
    // When no federation links are configured (handle is None),
    // the route serves an empty list.
    #[cfg(feature = "cluster")]
    let admin_state = match federation_runner_handle.as_ref() {
        Some(runner) => admin_state.with_federation_status(runner.status_handle()),
        None => admin_state,
    };
    // Tier 4 item 4.7 session A: expose the shared latency tracker
    // so `GET /api/v1/slo` returns per-(broadcast, transport)
    // p50 / p95 / p99 / max samples.
    let admin_state = admin_state.with_slo(slo_tracker.clone());

    // PLAN Phase D session 137: expose the configured WASM filter
    // chain on `GET /api/v1/wasm-filter` when `--wasm-filter` was
    // set. The handle is already in scope from the bridge install
    // earlier in `start()`; the closure reads a snapshot per call
    // so the route always reflects the current chain_length + per-
    // broadcast counters. When no filter is configured the default
    // closure returns `{enabled: false, chain_length: 0,
    // broadcasts: []}` so dashboards can pre-bake the shape.
    let admin_state = match wasm_filter_handle.clone() {
        Some(bridge) => {
            let slot_counters = wasm_slot_counters.clone();
            admin_state.with_wasm_filter(move || {
                let broadcasts = bridge
                    .tracked()
                    .into_iter()
                    .map(|(broadcast, track)| lvqr_admin::WasmFilterBroadcastStats {
                        seen: bridge.fragments_seen(&broadcast, &track),
                        kept: bridge.fragments_kept(&broadcast, &track),
                        dropped: bridge.fragments_dropped(&broadcast, &track),
                        broadcast,
                        track,
                    })
                    .collect();
                let slots = slot_counters
                    .iter()
                    .enumerate()
                    .map(|(index, c)| lvqr_admin::WasmFilterSlotStats {
                        index,
                        seen: c.seen(),
                        kept: c.kept(),
                        dropped: c.dropped(),
                    })
                    .collect();
                lvqr_admin::WasmFilterState {
                    enabled: true,
                    chain_length: bridge.chain_length(),
                    broadcasts,
                    slots,
                }
            })
        }
        None => admin_state,
    };

    // Session 111-B1: hoist `MeshCoordinator` construction out of
    // the admin-router block so it can be stored on `ServerHandle`
    // and accessed by integration tests + the session 111-B2
    // `ws_relay_session` subscriber-registration wiring. `None`
    // when `mesh_enabled = false`; `Some(Arc::new(..))` otherwise.
    let mesh_coordinator: Option<Arc<lvqr_mesh::MeshCoordinator>> = if config.mesh_enabled {
        let default_mesh = lvqr_mesh::MeshConfig::default();
        let mesh_config = lvqr_mesh::MeshConfig {
            max_children: config.max_peers,
            root_peer_count: config.mesh_root_peer_count.unwrap_or(default_mesh.root_peer_count),
            ..default_mesh
        };
        Some(Arc::new(lvqr_mesh::MeshCoordinator::new(mesh_config)))
    } else {
        None
    };

    // WebSocket fMP4 relay + WebSocket ingest state. Built AFTER
    // `mesh_coordinator` so the mesh field can be wired through
    // to `ws_relay_session` for session-111-B2 subscriber
    // registration; when mesh is disabled the field stays `None`
    // and the relay session behaves exactly as pre-111-B2.
    let ws_state = WsRelayState {
        origin: relay.origin().clone(),
        init_segments: Arc::new(dashmap::DashMap::new()),
        auth: auth.clone(),
        events: events.clone(),
        registry: shared_registry.clone(),
        slo: Some(slo_tracker.clone()),
        mesh: mesh_coordinator.clone(),
    };
    let ws_router = axum::Router::new()
        .route("/ws/{*broadcast}", get(ws_relay_handler))
        .route("/ingest/{*broadcast}", get(ws_ingest_handler))
        .with_state(ws_state);

    // PLAN v1.1 row 121 + session 128: when `ServeConfig.hmac_playback_secret`
    // is set, every playback surface accepts `?sig=...&exp=...` as an
    // alternative auth path that short-circuits the `SharedAuth`
    // subscribe gate. Wrap the secret in `Arc<[u8]>` so every handler
    // and middleware clone shares one copy. Hoisted above the
    // `combined_router` block so the downstream HLS + DASH spawn
    // blocks can also capture it into their `LivePlaybackAuthState`.
    let hmac_playback_secret: Option<Arc<[u8]>> = config.hmac_playback_secret.as_ref().map(|s| Arc::from(s.as_bytes()));

    let combined_router = {
        let admin_router = if let Some(mesh) = mesh_coordinator.clone() {
            let mesh_for_cb = mesh.clone();
            relay.set_connection_callback(Arc::new(move |conn_id, connected| {
                let peer_id = format!("conn-{conn_id}");
                if connected {
                    match mesh_for_cb.add_peer(peer_id.clone(), "default".to_string()) {
                        Ok(a) => {
                            tracing::info!(peer = %peer_id, role = ?a.role, depth = a.depth, "mesh: peer assigned");
                        }
                        Err(e) => {
                            tracing::warn!(peer = %peer_id, error = ?e, "mesh: assign failed");
                        }
                    }
                } else {
                    let orphans = mesh_for_cb.remove_peer(&peer_id);
                    for orphan in orphans {
                        let _ = mesh_for_cb.reassign_peer(&orphan);
                    }
                }
            }));

            let mesh_for_reaper = mesh.clone();
            let reaper_shutdown = shutdown.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            let dead = mesh_for_reaper.find_dead_peers();
                            for peer_id in dead {
                                tracing::info!(peer = %peer_id, "mesh: removing dead peer");
                                let orphans = mesh_for_reaper.remove_peer(&peer_id);
                                for orphan in orphans {
                                    let _ = mesh_for_reaper.reassign_peer(&orphan);
                                }
                            }
                        }
                        _ = reaper_shutdown.cancelled() => {
                            tracing::debug!("mesh reaper shutting down");
                            break;
                        }
                    }
                }
            });

            let mesh_for_signal = mesh.clone();
            let mut signal = lvqr_signal::SignalServer::new();
            // Session 111-B2: the callback is idempotent on
            // Register. A client that already opened `/ws/*`
            // (and therefore already holds a `ws-{n}` peer_id
            // assigned by `ws_relay_session`) can reuse that
            // peer_id on `/signal` without getting a second
            // tree entry: the callback looks the peer up first
            // and returns its existing assignment. Clients
            // that open `/signal` without first opening `/ws`
            // fall through to the pre-111-B2 path of
            // `add_peer` + fresh assignment.
            // Session 143: capture the operator-configured ICE-server
            // list once. Every AssignParent emitted by this callback
            // includes a clone of the snapshot. Empty when
            // `--mesh-ice-servers` was not set; clients then fall
            // back to their constructor-provided list.
            let ice_servers_for_signal = config.mesh_ice_servers.clone();
            signal.set_peer_callback(Arc::new(move |peer_id, track, connected| {
                if connected {
                    if let Some(existing) = mesh_for_signal.get_peer(peer_id) {
                        tracing::debug!(
                            peer = %peer_id,
                            role = ?existing.role,
                            depth = existing.depth,
                            "mesh: signal reusing existing peer entry from WS relay"
                        );
                        return Some(lvqr_signal::SignalMessage::AssignParent {
                            peer_id: peer_id.to_string(),
                            role: format!("{:?}", existing.role),
                            parent_id: existing.parent.clone(),
                            depth: existing.depth,
                            ice_servers: ice_servers_for_signal.clone(),
                        });
                    }
                    match mesh_for_signal.add_peer(peer_id.to_string(), track.to_string()) {
                        Ok(a) => {
                            tracing::info!(peer = %peer_id, role = ?a.role, depth = a.depth, "mesh: signal peer assigned");
                            Some(lvqr_signal::SignalMessage::AssignParent {
                                peer_id: peer_id.to_string(),
                                role: format!("{:?}", a.role),
                                parent_id: a.parent,
                                depth: a.depth,
                                ice_servers: ice_servers_for_signal.clone(),
                            })
                        }
                        Err(e) => {
                            tracing::warn!(peer = %peer_id, error = ?e, "mesh: signal assign failed");
                            None
                        }
                    }
                } else {
                    let orphans = mesh_for_signal.remove_peer(peer_id);
                    for orphan in orphans {
                        let _ = mesh_for_signal.reassign_peer(&orphan);
                    }
                    None
                }
            }));

            // Session 141: bridge ForwardReport signal messages into
            // the mesh coordinator so the admin route can expose
            // actual-vs-intended offload. The callback is invoked with
            // the peer_id resolved from the WS session state, not from
            // a wire field, so a peer can only report for itself.
            let mesh_for_report = mesh.clone();
            signal.set_forward_report_callback(Arc::new(move |peer_id, forwarded_frames| {
                mesh_for_report.record_forward_report(peer_id, forwarded_frames);
            }));

            let mesh_for_admin = mesh.clone();
            let admin_with_mesh = admin_state.with_mesh(move || {
                // Session 141: per-peer stats derived from the tree
                // snapshot. `intended_children` is the topology
                // planner's assignment; `forwarded_frames` is the
                // cumulative value from the most recent ForwardReport.
                let peers = mesh_for_admin
                    .tree_snapshot()
                    .into_iter()
                    .map(|p| lvqr_admin::MeshPeerStats {
                        peer_id: p.id.clone(),
                        role: format!("{:?}", p.role),
                        parent: p.parent.clone(),
                        depth: p.depth,
                        intended_children: p.children.len(),
                        forwarded_frames: p.forwarded_frames,
                    })
                    .collect();
                lvqr_admin::MeshState {
                    enabled: true,
                    peer_count: mesh_for_admin.peer_count(),
                    offload_percentage: mesh_for_admin.offload_percentage(),
                    peers,
                }
            });

            tracing::info!(
                max_children = config.max_peers,
                auth_gate = !config.no_auth_signal,
                "peer mesh enabled (/signal endpoint active)"
            );

            // Session 111-B1: gate /signal with SubscribeAuth
            // unless `--no-auth-signal` was set. `?token=<token>`
            // query parameter carries the bearer; Sec-WebSocket-
            // Protocol support is deferred to 111-B2 pending a
            // subprotocol-echo upstream in `lvqr-signal`.
            let mut signal_router = signal.router();
            if !config.no_auth_signal {
                signal_router = signal_router.layer(from_fn_with_state(
                    SignalAuthState { auth: auth.clone() },
                    signal_auth_middleware,
                ));
            }

            let router = lvqr_admin::build_router(admin_with_mesh);
            router.merge(signal_router)
        } else {
            lvqr_admin::build_router(admin_state)
        };

        let combined = admin_router.merge(ws_router);
        let combined = if let Some((ref dir, ref index)) = archive_index {
            combined.merge(playback_router(
                dir.clone(),
                Arc::clone(index),
                auth.clone(),
                hmac_playback_secret.clone(),
            ))
        } else {
            combined
        };
        // Tier 4 item 4.3 session B3: feature-gated `/playback/verify/
        // {broadcast}` admin route. Mounted only when the `c2pa`
        // feature is on AND an archive directory is configured (the
        // verify route reads `<archive>/<broadcast>/<track>/
        // finalized.*` off disk, so an archive is a hard prerequisite).
        #[cfg(feature = "c2pa")]
        let combined = if let Some((ref dir, _)) = archive_index {
            combined.merge(verify_router(dir.clone(), auth.clone()))
        } else {
            combined
        };
        combined
    }
    .layer(CorsLayer::permissive());

    // Spawn a single background task that joins relay + RTMP + admin and
    // signals the shared shutdown token if any subsystem exits early.
    let relay_shutdown = shutdown.clone();
    let rtmp_shutdown = shutdown.clone();
    let admin_shutdown = shutdown.clone();
    let hls_shutdown = shutdown.clone();
    let dash_shutdown = shutdown.clone();
    let whep_shutdown = shutdown.clone();
    let whip_shutdown = shutdown.clone();
    let bg_shutdown_for_task = shutdown.clone();
    let hls_router_pair =
        hls_listener.map(|listener| (listener, hls_server.expect("hls_server set when listener is set")));
    let dash_router_pair =
        dash_listener.map(|listener| (listener, dash_server.expect("dash_server set when listener is set")));
    let whep_router_pair =
        whep_listener.map(|listener| (listener, whep_server.expect("whep_server set when listener is set")));
    let whip_router_pair =
        whip_listener.map(|listener| (listener, whip_server.expect("whip_server set when listener is set")));
    // Moved into the spawned task below so it lives as long as
    // the WHIP poll loops; see `drop(_whip_bridge_keepalive)` at
    // the end of the join block.
    let whip_bridge_keepalive = whip_bridge;

    // Clone the relay's OriginProducer for the ServerHandle. `relay`
    // itself moves into the accept-loop below, so the clone is how
    // callers (federation tests, admin consumers) reach the origin
    // for the server's lifetime.
    let relay_origin = relay.origin().clone();

    let join = tokio::spawn(async move {
        let shutdown_on_exit_relay = bg_shutdown_for_task.clone();
        let relay_fut = async move {
            if let Err(e) = relay.accept_loop(&mut moq_server, relay_shutdown).await {
                tracing::error!(error = %e, "relay server error");
            }
            shutdown_on_exit_relay.cancel();
        };

        let shutdown_on_exit_rtmp = bg_shutdown_for_task.clone();
        let rtmp_server_task = rtmp_server;
        let rtmp_fut = async move {
            if let Err(e) = rtmp_server_task.run_with_listener(rtmp_listener, rtmp_shutdown).await {
                tracing::error!(error = %e, "RTMP server error");
            }
            shutdown_on_exit_rtmp.cancel();
        };

        let shutdown_on_exit_admin = bg_shutdown_for_task.clone();
        let admin_fut = async move {
            let result = axum::serve(admin_listener, combined_router)
                .with_graceful_shutdown(async move { admin_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "admin server error");
            }
            shutdown_on_exit_admin.cancel();
        };

        let shutdown_on_exit_hls = bg_shutdown_for_task.clone();
        let hls_auth = auth.clone();
        let hls_auth_disabled = config.no_auth_live_playback;
        // PLAN v1.1 session 128: share the same HMAC secret with the
        // live HLS + DASH auth middleware so one --hmac-playback-secret
        // configuration mints signed URLs across all three route trees.
        let hls_hmac_secret = hmac_playback_secret.clone();
        let hls_fut = async move {
            let Some((listener, server)) = hls_router_pair else {
                return;
            };
            // Session 112: apply the subscribe-auth gate to live
            // HLS routes. Noop provider deployments see no
            // behavior change (provider always allows). Configured
            // deployments (static token, JWT) get an automatic
            // 401 on requests without a valid bearer. Escape hatch
            // is `--no-auth-live-playback` for deployments that
            // deliberately want open live playback.
            let mut router = server.router();
            if !hls_auth_disabled {
                let state = LivePlaybackAuthState {
                    auth: hls_auth,
                    entry: "hls_live",
                    scheme: crate::signed_url::LiveScheme::Hls,
                    hmac_secret: hls_hmac_secret,
                };
                router = router.layer(from_fn_with_state(state, live_playback_auth_middleware));
            }
            let router = router.layer(CorsLayer::permissive());
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move { hls_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "HLS server error");
            }
            shutdown_on_exit_hls.cancel();
        };

        let shutdown_on_exit_dash = bg_shutdown_for_task.clone();
        let dash_auth = auth.clone();
        let dash_auth_disabled = config.no_auth_live_playback;
        let dash_hmac_secret = hmac_playback_secret.clone();
        let dash_fut = async move {
            let Some((listener, server)) = dash_router_pair else {
                return;
            };
            let mut router = server.router();
            if !dash_auth_disabled {
                let state = LivePlaybackAuthState {
                    auth: dash_auth,
                    entry: "dash_live",
                    scheme: crate::signed_url::LiveScheme::Dash,
                    hmac_secret: dash_hmac_secret,
                };
                router = router.layer(from_fn_with_state(state, live_playback_auth_middleware));
            }
            let router = router.layer(CorsLayer::permissive());
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move { dash_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "DASH server error");
            }
            shutdown_on_exit_dash.cancel();
        };

        let shutdown_on_exit_whep = bg_shutdown_for_task.clone();
        let whep_fut = async move {
            let Some((listener, server)) = whep_router_pair else {
                return;
            };
            let router = lvqr_whep::router_for(server);
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move { whep_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "WHEP server error");
            }
            shutdown_on_exit_whep.cancel();
        };

        let shutdown_on_exit_whip = bg_shutdown_for_task.clone();
        let whip_fut = async move {
            let Some((listener, server)) = whip_router_pair else {
                return;
            };
            let router = lvqr_whip::router_for(server);
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move { whip_shutdown.cancelled().await })
                .await;
            if let Err(e) = &result {
                tracing::error!(error = %e, "WHIP server error");
            }
            shutdown_on_exit_whip.cancel();
        };

        let srt_shutdown = bg_shutdown_for_task.clone();
        let srt_events = srt_events_clone;
        let srt_cancel = srt_shutdown_token;
        let srt_fut = async move {
            let Some(server) = srt_server else { return };
            if let Err(e) = server.run(srt_events, srt_cancel).await {
                tracing::error!(error = %e, "SRT server error");
            }
            srt_shutdown.cancel();
        };

        let rtsp_shutdown = bg_shutdown_for_task.clone();
        let rtsp_events = rtsp_events_clone;
        let rtsp_cancel = rtsp_shutdown_token;
        let rtsp_fut = async move {
            let Some(server) = rtsp_server else { return };
            if let Err(e) = server.run(rtsp_events, rtsp_cancel).await {
                tracing::error!(error = %e, "RTSP server error");
            }
            rtsp_shutdown.cancel();
        };

        let _ = tokio::join!(
            relay_fut, rtmp_fut, admin_fut, hls_fut, dash_fut, whep_fut, whip_fut, srt_fut, rtsp_fut
        );
        drop(whip_bridge_keepalive);
        tracing::info!("shutdown complete");
    });

    Ok(ServerHandle {
        relay_addr: relay_bound,
        rtmp_addr: rtmp_bound,
        admin_addr: admin_bound,
        hls_addr: hls_bound,
        whep_addr: whep_bound,
        whip_addr: whip_bound,
        dash_addr: dash_bound,
        rtsp_addr: rtsp_bound,
        srt_addr: srt_bound,
        shutdown,
        join: Some(join),
        #[cfg(feature = "cluster")]
        cluster,
        wasm_filter: wasm_filter_handle,
        _wasm_reloaders: wasm_reloader_handles,
        #[cfg(feature = "whisper")]
        agent_runner: agent_runner_handle,
        #[cfg(feature = "transcode")]
        transcode_runner: transcode_runner_handle,
        slo: slo_tracker,
        mesh_coordinator,
        fragment_registry: shared_registry,
        origin: relay_origin,
        #[cfg(feature = "cluster")]
        federation_runner: federation_runner_handle,
    })
}

// Auth middleware extracted to `crate::auth_middleware`.
// WS relay + ingest + recorder event bridge extracted to `crate::ws`.

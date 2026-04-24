//! `ServeConfig` + defaults + CLI rendition parsers.
//!
//! Extracted out of `lib.rs` in the session-111-B1 follow-up refactor
//! so the composition root stays focused on wiring. The public type
//! [`ServeConfig`] is re-exported from `crate::lib` so external
//! callers (`main.rs`, `lvqr-test-utils`, embedders) continue to name
//! it as `lvqr_cli::ServeConfig` with no API churn.
//!
//! `parse_one_transcode_rendition` + `parse_transcode_renditions` live
//! here (not in main.rs) because they turn CLI strings into the
//! `ServeConfig::transcode_renditions` field's value and because they
//! are exercised by the in-crate tests at the bottom of this file.

use lvqr_auth::SharedAuth;
use std::net::SocketAddr;
use std::path::PathBuf;

/// Configuration passed to [`start`] to bring up a full-stack LVQR server.
///
/// Every `*_addr` field accepts port `0` for ephemeral port binding; the
/// real bound address is reported back through [`ServerHandle`].
#[derive(Clone)]
pub struct ServeConfig {
    /// QUIC/MoQ relay bind address.
    pub relay_addr: SocketAddr,
    /// RTMP ingest bind address.
    pub rtmp_addr: SocketAddr,
    /// Admin HTTP (and WS relay/ingest) bind address.
    pub admin_addr: SocketAddr,
    /// Optional LL-HLS HTTP bind address. When `Some`, `start()` spins up a
    /// dedicated `HlsServer` axum router on this address that observes the
    /// RTMP bridge's fragment output and serves `/playlist.m3u8`,
    /// `/init.mp4`, and the per-chunk media URIs the playlist references.
    /// When `None`, no HLS surface is exposed.
    pub hls_addr: Option<SocketAddr>,
    /// DVR window depth in seconds for the LL-HLS sliding-window
    /// eviction. Translated to `max_segments = dvr_secs /
    /// target_duration_secs` at server construction. 0 means
    /// unbounded (no eviction). Default: 120 (60 segments at 2 s).
    pub hls_dvr_window_secs: u32,
    /// LL-HLS target segment duration in seconds. Controls both the
    /// `EXT-X-TARGETDURATION` declaration and the CMAF segmenter's
    /// segment-close policy. Default: 2.
    pub hls_target_duration_secs: u32,
    /// LL-HLS target partial (chunk) duration in milliseconds.
    /// Controls both `EXT-X-PART-INF:PART-TARGET` and the CMAF
    /// segmenter's partial-close policy. Default: 200.
    pub hls_part_target_ms: u32,
    /// Optional WHEP (WebRTC HTTP Egress Protocol) HTTP bind address.
    /// When `Some`, `start()` constructs a `Str0mAnswerer` and a
    /// `WhepServer`, attaches the server as a `RawSampleObserver` on
    /// the RTMP bridge, and spins up an axum router on this address
    /// that accepts `POST /whep/{broadcast}` SDP offers, answers
    /// them via `str0m`, and fans each ingest sample into every
    /// subscribed session. When `None`, no WHEP surface is exposed
    /// and no `str0m` state is constructed.
    pub whep_addr: Option<SocketAddr>,
    /// Optional WHIP (WebRTC HTTP Ingest Protocol) HTTP bind
    /// address. When `Some`, `start()` constructs a
    /// `Str0mIngestAnswerer` and a `WhipMoqBridge`, attaches it
    /// as an ingest sink, and spins up an axum router on this
    /// address that accepts `POST /whip/{broadcast}` SDP offers.
    /// When `None`, no WHIP surface is exposed.
    pub whip_addr: Option<SocketAddr>,
    /// Optional MPEG-DASH HTTP bind address. When `Some`, `start()`
    /// spins up a `MultiDashServer` axum router on this address
    /// that observes the same fragment stream the LL-HLS bridge
    /// observes and serves `/dash/{broadcast}/manifest.mpd` plus
    /// numbered segment URIs. RTMP and WHIP publishers both feed
    /// the DASH egress without any per-protocol wiring.
    pub dash_addr: Option<SocketAddr>,
    /// Optional RTSP ingest bind address. When `Some`, `start()`
    /// spins up an `RtspServer` on this TCP address that accepts
    /// RTSP ANNOUNCE/RECORD sessions with interleaved RTP and fans
    /// depacketized H.264/HEVC through the fragment observer chain.
    pub rtsp_addr: Option<SocketAddr>,
    /// Optional SRT ingest bind address. When `Some`, `start()`
    /// spins up an `SrtIngestServer` on this UDP address that
    /// accepts MPEG-TS streams and fans them through the fragment
    /// observer chain.
    pub srt_addr: Option<SocketAddr>,
    /// Enable the peer mesh coordinator and `/signal` endpoint.
    pub mesh_enabled: bool,
    /// Max children per mesh parent when `mesh_enabled`.
    pub max_peers: usize,
    /// Pre-built auth provider. `None` means open access (`NoopAuthProvider`).
    pub auth: Option<SharedAuth>,
    /// Recording directory. `None` disables recording.
    pub record_dir: Option<PathBuf>,
    /// DVR archive directory. When `Some`, `start()` opens a
    /// `RedbSegmentIndex` under `<archive_dir>/archive.redb` and
    /// attaches an archiving fragment observer to the RTMP bridge
    /// that writes every emitted fragment to
    /// `<archive_dir>/<broadcast>/<track>/<seq>.m4s` and records a
    /// `SegmentRef` against the index. The index + segment files
    /// back the DVR scrub / time-range playback surface (Tier 2.4).
    pub archive_dir: Option<PathBuf>,
    /// Optional HMAC signing secret for short-lived playback URLs
    /// (PLAN v1.1 row 121). When set, every `/playback/*` handler
    /// accepts an alternative auth path: a `?exp=<unix_ts>&sig=
    /// <base64url>` pair where `sig = HMAC-SHA256(secret, "<path>
    /// ?exp=<ts>")`. A valid signature short-circuits the normal
    /// subscribe-token check so an operator can mint a one-off
    /// share link for a third party who cannot authenticate. An
    /// expired or tampered signature returns 403 (NOT 401) so the
    /// client can distinguish missing auth from wrong auth. When
    /// unset, all playback routes fall back to their existing
    /// `SubscribeAuth` gate.
    pub hmac_playback_secret: Option<String>,
    /// Optional C2PA provenance configuration. When set, every
    /// `(broadcast, track)` drained by the archive indexer runs the
    /// broadcast-end finalize path on drain termination (the moment
    /// every producer-side clone of the broadcaster drops), which
    /// writes `finalized.mp4` + `finalized.c2pa` next to the segment
    /// files. The admin router also mounts `GET /playback/verify/
    /// {broadcast}` for verifying the resulting manifest. Feature-
    /// gated: the field is accessible only when `lvqr-cli` is built
    /// with `--features c2pa` so the `c2pa` transitive closure does
    /// not leak into deployments that do not need provenance.
    /// Tier 4 item 4.3 session B3.
    #[cfg(feature = "c2pa")]
    pub c2pa: Option<lvqr_archive::provenance::C2paConfig>,
    /// Optional path to a whisper.cpp `ggml-*.bin` model. When
    /// set, `start()` constructs a
    /// `lvqr_agent_whisper::WhisperCaptionsFactory` wired against
    /// the shared `FragmentBroadcasterRegistry` (so the generated
    /// caption cues flow through the same
    /// `(broadcast, "captions")` track the LL-HLS subtitle
    /// rendition drains) and installs it on a throwaway
    /// `lvqr_agent::AgentRunner`; the returned
    /// `AgentRunnerHandle` is held on `ServerHandle` for the
    /// server lifetime. Without a value the factory is skipped
    /// entirely and no AI-adjacent state is constructed.
    /// Feature-gated: accessible only when `lvqr-cli` is built
    /// with `--features whisper` so the whisper.cpp + symphonia
    /// transitive closure stays out of deployments that do not
    /// want captions. Tier 4 item 4.5 session D.
    #[cfg(feature = "whisper")]
    pub whisper_model: Option<PathBuf>,
    /// Ordered list of WASM fragment filter modules. When
    /// non-empty, `start()` loads + compiles each module via
    /// `lvqr_wasm::WasmFilter::load` and installs a single
    /// `lvqr_wasm::ChainFilter` tap on the shared
    /// `FragmentBroadcasterRegistry` before any ingest listener
    /// starts accepting traffic. Chain order is preserved; the
    /// first filter that drops a fragment short-circuits the rest
    /// of the chain for that fragment. Each path is watched
    /// independently via its own `WasmFilterReloader` so
    /// hot-swapping one slot does not disturb the others.
    ///
    /// The tap observes every fragment flowing through every
    /// broadcaster and drives
    /// `lvqr_wasm_fragments_total{outcome=keep|drop}` counters; in
    /// v1 it does NOT modify what downstream subscribers receive
    /// (session-86 scope narrowing). Leave empty to disable.
    pub wasm_filter: Vec<PathBuf>,
    /// Install the global Prometheus recorder. Must be `false` in tests
    /// because `metrics-exporter-prometheus` panics on the second install
    /// in a process. `main.rs` sets this to `true`.
    pub install_prometheus: bool,
    /// Pre-built OTLP metrics recorder handed off by
    /// `lvqr_observability::init` when `LVQR_OTLP_ENDPOINT` is
    /// set. When `Some`, `start()` installs it as the global
    /// `metrics`-crate recorder -- either on its own or composed
    /// with the Prometheus recorder via
    /// `metrics_util::layers::FanoutBuilder` when
    /// `install_prometheus` is also true. When `None`, only the
    /// Prometheus path runs (legacy behavior).
    pub otel_metrics_recorder: Option<lvqr_observability::OtelMetricsRecorder>,
    /// Path to TLS certificate (PEM). Reserved; not consumed yet. The
    /// relay auto-generates self-signed certs when unset.
    pub tls_cert: Option<PathBuf>,
    /// Path to TLS private key (PEM). Reserved; not consumed yet.
    pub tls_key: Option<PathBuf>,
    /// Optional cluster gossip bind address. When `Some`, `start()`
    /// bootstraps an `lvqr_cluster::Cluster` on this address, wires
    /// it into the admin router so `/api/v1/cluster/*` answers, and
    /// installs an `OwnerResolver` on the HLS server so subscribers
    /// hitting this node for a peer-owned broadcast receive a 302
    /// pointing at the owner. `None` (default) disables clustering
    /// and the node behaves as a standalone single-process server.
    ///
    /// Feature-gated on `cluster`; the field is present regardless
    /// so `ServeConfig` stays ABI-stable across feature flips.
    pub cluster_listen: Option<SocketAddr>,
    /// Cluster peer seed addresses. Each entry is an `ip:port`
    /// string the new node gossips to on boot. Ignored when
    /// [`cluster_listen`](Self::cluster_listen) is `None`.
    pub cluster_seeds: Vec<String>,
    /// Cluster-node identifier. `None` auto-generates a random
    /// `lvqr-<16 alphanumeric>` id at bootstrap.
    pub cluster_node_id: Option<String>,
    /// Cluster tag gossipped in every SYN. Chitchat rejects
    /// cross-cluster gossip so two LVQR deployments on the same
    /// subnet stay isolated. Empty string falls back to the
    /// crate-default (`"lvqr"`).
    pub cluster_id: Option<String>,
    /// Externally-reachable HLS base URL this node advertises
    /// (e.g. `"http://a.local:8888"`). When clustering is enabled,
    /// `start()` writes this URL into the per-node `endpoints` KV
    /// so peers redirecting subscribers know where to send them.
    /// `None` skips the publish; peers will then 404 rather than
    /// redirect for this node's broadcasts.
    pub cluster_advertise_hls: Option<String>,
    /// Externally-reachable DASH base URL this node advertises
    /// (e.g. `"http://a.local:8888"`). Shape matches
    /// [`cluster_advertise_hls`](Self::cluster_advertise_hls);
    /// peers use this when composing a 302 `Location` for
    /// `/dash/...` requests.
    pub cluster_advertise_dash: Option<String>,
    /// Externally-reachable RTSP base URL this node advertises
    /// (e.g. `"rtsp://a.local:8554"`). Used by the RTSP 302
    /// redirect-to-owner path on DESCRIBE / PLAY for peer-owned
    /// broadcasts.
    pub cluster_advertise_rtsp: Option<String>,
    /// Cross-cluster federation links. Each link opens a single
    /// outbound MoQ session to a peer cluster's relay and
    /// re-publishes matching broadcasts into the local origin.
    /// Empty list disables federation. Feature-gated on `cluster`
    /// since `FederationLink` lives in `lvqr-cluster`. Tier 4 item
    /// 4.4.
    #[cfg(feature = "cluster")]
    pub federation_links: Vec<lvqr_cluster::FederationLink>,
    /// ABR ladder the server produces from every source broadcast.
    /// When empty, `start()` installs no transcoders and no master-
    /// playlist variants are emitted for rendition siblings. When
    /// non-empty, `start()` installs one
    /// `SoftwareTranscoderFactory` + one
    /// `AudioPassthroughTranscoderFactory` per rendition against the
    /// shared `FragmentBroadcasterRegistry` and registers the
    /// ladder metadata on the HLS server so the master playlist
    /// composer emits one `#EXT-X-STREAM-INF` per sibling.
    /// Feature-gated: accessible only when `lvqr-cli` is built with
    /// `--features transcode` so the GStreamer transitive closure
    /// stays out of deployments that do not want the ladder.
    /// Tier 4 item 4.6 session 106 C.
    #[cfg(feature = "transcode")]
    pub transcode_renditions: Vec<lvqr_transcode::RenditionSpec>,
    /// Operator override for the source variant's advertised
    /// `BANDWIDTH` in the master playlist, in kilobits per second.
    /// Defaults (`None`) to `highest_rung_kbps * 1.2`. Only
    /// meaningful when `transcode_renditions` is non-empty.
    /// Tier 4 item 4.6 session 106 C.
    #[cfg(feature = "transcode")]
    pub source_bandwidth_kbps: Option<u32>,
    /// Escape hatch for deployments that want open live HLS +
    /// DASH playback with auth scoped to ingest, admin, and
    /// DVR only. When `false` (default), the composition root
    /// wraps the HLS and DASH routers with the same
    /// `SubscribeAuth` gate that already protects `/ws/*`,
    /// `/playback/*`, and WHEP. When `true`, the live HLS and
    /// DASH routers are exposed without an auth layer -- the
    /// pre-session-112 v0.4.0 behavior. Unauthed deployments
    /// (Noop provider) see no behavior change either way
    /// because the provider always allows. Session 112.
    pub no_auth_live_playback: bool,
    /// Escape hatch for deployments that want an unauthenticated
    /// mesh `/signal` WebSocket. When `false` (default), the
    /// composition root wraps `/signal` with the same
    /// `SubscribeAuth` gate pattern that protects other
    /// subscribe-side surfaces: `Sec-WebSocket-Protocol:
    /// lvqr.bearer.<token>` preferred, `?token=<token>` query
    /// fallback. Noop provider deployments see no behavior
    /// change because the provider always allows. Configured
    /// deployments (static token, JWT) now require a bearer
    /// on every `/signal` upgrade. Only meaningful when
    /// `mesh_enabled` is `true`. Session 111-B1.
    pub no_auth_signal: bool,
    /// Override the mesh `root_peer_count` (number of peers
    /// that connect directly to the origin before the tree
    /// starts assigning parents). `None` uses the
    /// `lvqr_mesh::MeshConfig::default()` value of 30. Tests
    /// that want to exercise the `AssignParent` path with a
    /// small number of peers set this to 1 so the second peer
    /// becomes a child of the first. Only meaningful when
    /// `mesh_enabled` is `true`. Session 111-B1.
    pub mesh_root_peer_count: Option<usize>,
}

impl ServeConfig {
    /// Minimal loopback config for tests: every listener on `127.0.0.1:0`,
    /// open access, no recording, no Prometheus install.
    pub fn loopback_ephemeral() -> Self {
        let loopback: std::net::IpAddr = std::net::Ipv4Addr::LOCALHOST.into();
        Self {
            relay_addr: (loopback, 0).into(),
            rtmp_addr: (loopback, 0).into(),
            admin_addr: (loopback, 0).into(),
            hls_addr: Some((loopback, 0).into()),
            hls_dvr_window_secs: 120,
            hls_target_duration_secs: 2,
            hls_part_target_ms: 200,
            whep_addr: None,
            whip_addr: None,
            dash_addr: None,
            rtsp_addr: None,
            srt_addr: None,
            mesh_enabled: false,
            max_peers: 3,
            auth: None,
            record_dir: None,
            archive_dir: None,
            hmac_playback_secret: None,
            #[cfg(feature = "c2pa")]
            c2pa: None,
            #[cfg(feature = "whisper")]
            whisper_model: None,
            wasm_filter: Vec::new(),
            install_prometheus: false,
            otel_metrics_recorder: None,
            tls_cert: None,
            tls_key: None,
            cluster_listen: None,
            cluster_seeds: Vec::new(),
            cluster_node_id: None,
            cluster_id: None,
            cluster_advertise_hls: None,
            cluster_advertise_dash: None,
            cluster_advertise_rtsp: None,
            #[cfg(feature = "cluster")]
            federation_links: Vec::new(),
            #[cfg(feature = "transcode")]
            transcode_renditions: Vec::new(),
            #[cfg(feature = "transcode")]
            source_bandwidth_kbps: None,
            no_auth_live_playback: false,
            no_auth_signal: false,
            mesh_root_peer_count: None,
        }
    }
}
/// Resolve a single `--transcode-rendition` CLI / env value into a
/// [`lvqr_transcode::RenditionSpec`]. Session 106 C accepts three short
/// preset names (`"720p"`, `"480p"`, `"240p"`) and a path ending in
/// `.toml` that is read + deserialized as a custom `RenditionSpec`.
/// Anything else is a hard error so misconfigured ladders surface at
/// CLI parse time instead of via silent drop. Tier 4 item 4.6 session
/// 106 C.
#[cfg(feature = "transcode")]
pub fn parse_one_transcode_rendition(value: &str) -> anyhow::Result<lvqr_transcode::RenditionSpec> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(anyhow::anyhow!("--transcode-rendition value is empty"));
    }
    match trimmed {
        "720p" => Ok(lvqr_transcode::RenditionSpec::preset_720p()),
        "480p" => Ok(lvqr_transcode::RenditionSpec::preset_480p()),
        "240p" => Ok(lvqr_transcode::RenditionSpec::preset_240p()),
        other if other.ends_with(".toml") => {
            let body = std::fs::read_to_string(other)
                .map_err(|e| anyhow::anyhow!("failed to read rendition toml {other}: {e}"))?;
            let spec: lvqr_transcode::RenditionSpec =
                toml::from_str(&body).map_err(|e| anyhow::anyhow!("failed to parse rendition toml {other}: {e}"))?;
            Ok(spec)
        }
        other => Err(anyhow::anyhow!(
            "--transcode-rendition: unknown preset {other:?}; expected one of 720p / 480p / 240p, \
             or a path ending in .toml"
        )),
    }
}

/// Resolve the repeated `--transcode-rendition` flag list into
/// the [`ServeConfig::transcode_renditions`] value. Tier 4 item 4.6
/// session 106 C.
#[cfg(feature = "transcode")]
pub fn parse_transcode_renditions(values: &[String]) -> anyhow::Result<Vec<lvqr_transcode::RenditionSpec>> {
    values.iter().map(|v| parse_one_transcode_rendition(v)).collect()
}

#[cfg(all(test, feature = "transcode"))]
mod transcode_serve_config_tests {
    use super::{ServeConfig, parse_one_transcode_rendition, parse_transcode_renditions};

    #[test]
    fn loopback_ephemeral_defaults_transcode_renditions_to_empty() {
        let cfg = ServeConfig::loopback_ephemeral();
        assert!(cfg.transcode_renditions.is_empty());
        assert!(cfg.source_bandwidth_kbps.is_none());
    }

    #[test]
    fn transcode_rendition_720p_parses_to_preset() {
        let spec = parse_one_transcode_rendition("720p").expect("parse 720p");
        assert_eq!(spec, lvqr_transcode::RenditionSpec::preset_720p());
    }

    #[test]
    fn transcode_rendition_rejects_unknown_preset() {
        let err = parse_one_transcode_rendition("ultra").expect_err("must reject unknown preset");
        let msg = format!("{err:#}");
        assert!(msg.contains("unknown preset"), "error message: {msg}");
    }

    #[test]
    fn parse_list_preserves_order_and_builds_default_ladder() {
        let values = vec!["720p".to_string(), "480p".to_string(), "240p".to_string()];
        let ladder = parse_transcode_renditions(&values).expect("parse list");
        assert_eq!(ladder, lvqr_transcode::RenditionSpec::default_ladder());
    }

    #[test]
    fn transcode_rendition_reads_toml_file() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("rendition.toml");
        std::fs::write(
            &path,
            "name = \"custom\"\nwidth = 1920\nheight = 1080\nvideo_bitrate_kbps = 5000\naudio_bitrate_kbps = 192\n",
        )
        .expect("write");
        let spec = parse_one_transcode_rendition(path.to_str().unwrap()).expect("parse toml");
        assert_eq!(
            spec,
            lvqr_transcode::RenditionSpec::new("custom", 1920, 1080, 5_000, 192)
        );
    }
}
#[cfg(all(test, feature = "whisper"))]
mod whisper_serve_config_tests {
    use super::ServeConfig;
    use std::path::PathBuf;

    #[test]
    fn loopback_ephemeral_defaults_whisper_model_to_none() {
        let cfg = ServeConfig::loopback_ephemeral();
        assert!(cfg.whisper_model.is_none());
    }

    #[test]
    fn whisper_model_round_trips_through_serve_config() {
        let path = PathBuf::from("/nonexistent/ggml-tiny.en.bin");
        let cfg = ServeConfig {
            whisper_model: Some(path.clone()),
            ..ServeConfig::loopback_ephemeral()
        };
        assert_eq!(cfg.whisper_model.as_deref(), Some(path.as_path()));
    }
}

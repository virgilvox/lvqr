use anyhow::Result;
use clap::Parser;
use lvqr_auth::{JwtAuthConfig, JwtAuthProvider, NoopAuthProvider, SharedAuth, StaticAuthConfig, StaticAuthProvider};
#[cfg(feature = "transcode")]
use lvqr_cli::parse_transcode_renditions;
use lvqr_cli::{ServeConfig, start};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "transcode")]
use clap::ArgAction;

#[derive(Parser, Debug)]
#[command(name = "lvqr", version, about = "Live Video QUIC Relay")]
enum Cli {
    /// Start the LVQR relay server.
    Serve(ServeArgs),
}

#[derive(Parser, Debug)]
struct ServeArgs {
    /// QUIC/MoQ listen port.
    #[arg(long, default_value = "4443", env = "LVQR_PORT")]
    port: u16,

    /// RTMP ingest listen port.
    #[arg(long, default_value = "1935", env = "LVQR_RTMP_PORT")]
    rtmp_port: u16,

    /// Admin HTTP API listen port.
    #[arg(long, default_value = "8080", env = "LVQR_ADMIN_PORT")]
    admin_port: u16,

    /// LL-HLS HTTP listen port. Set to 0 to disable HLS composition.
    /// When non-zero, `lvqr serve` spins up a dedicated axum server on
    /// this port that exposes `/playlist.m3u8`, `/init.mp4`, and the
    /// per-chunk media URIs that the playlist references for the
    /// first RTMP broadcast that publishes.
    #[arg(long, default_value = "8888", env = "LVQR_HLS_PORT")]
    hls_port: u16,

    /// LL-HLS DVR window depth in seconds. Controls how many seconds
    /// of closed segments the live playlist retains before oldest-first
    /// eviction. Segments older than this window return 404. After a
    /// broadcast ends (finalize), the retained window becomes a VOD
    /// surface that clients can scrub freely. Set to 0 for unbounded
    /// retention (memory grows linearly with broadcast duration).
    /// Default is 120 seconds (~60 segments at the 2 s target duration).
    #[arg(long, default_value = "120", env = "LVQR_HLS_DVR_WINDOW")]
    hls_dvr_window: u32,

    /// LL-HLS target segment duration in seconds. Affects both the
    /// rendered EXT-X-TARGETDURATION and the CMAF segmenter's
    /// segment-close policy. Lower values reduce startup latency;
    /// higher values improve delivery efficiency.
    #[arg(long, default_value = "2", env = "LVQR_HLS_TARGET_DURATION")]
    hls_target_duration: u32,

    /// LL-HLS target partial (chunk) duration in milliseconds.
    /// Affects both the rendered EXT-X-PART-INF:PART-TARGET and
    /// the CMAF segmenter's partial-close policy. Lower values
    /// reduce glass-to-glass latency; higher values reduce HTTP
    /// request overhead per second of video.
    #[arg(long, default_value = "200", env = "LVQR_HLS_PART_TARGET")]
    hls_part_target: u32,

    /// WHEP HTTP listen port. Set to 0 to disable WHEP egress. When
    /// non-zero, `lvqr serve` binds a dedicated axum server on this
    /// port exposing `POST/PATCH/DELETE /whep/{broadcast}` for
    /// WebRTC subscribers. The WHEP backend uses `str0m` and
    /// completes ICE/DTLS against real browser clients; RTP media
    /// write is not yet wired, so subscribers will connect but see
    /// no frames until the media-write session lands.
    #[arg(long, default_value = "0", env = "LVQR_WHEP_PORT")]
    whep_port: u16,

    /// MPEG-DASH HTTP listen port. Set to 0 to disable DASH egress.
    /// When non-zero, `lvqr serve` binds a dedicated axum server on
    /// this port exposing `/dash/{broadcast}/manifest.mpd`,
    /// `/dash/{broadcast}/init-{video,audio}.m4s`, and the numbered
    /// `seg-{video,audio}-<n>.m4s` segment URIs the MPD references.
    /// Every ingest protocol (RTMP, WHIP, SRT, RTSP) feeds the same
    /// `MultiDashServer` through the shared
    /// `FragmentBroadcasterRegistry` and a `BroadcasterDashBridge`
    /// install, with no per-protocol wiring on the egress side.
    #[arg(long, default_value = "0", env = "LVQR_DASH_PORT")]
    dash_port: u16,

    /// RTSP ingest listen port. Set to 0 to disable RTSP ingest.
    /// When non-zero, `lvqr serve` binds an RTSP/1.0 TCP listener
    /// on this port that accepts ANNOUNCE/RECORD sessions with
    /// interleaved RTP. Depacketized H.264/HEVC NALs are converted
    /// to Fragments that reach every existing egress.
    #[arg(long, default_value = "0", env = "LVQR_RTSP_PORT")]
    rtsp_port: u16,

    /// SRT ingest listen port. Set to 0 to disable SRT ingest.
    /// When non-zero, `lvqr serve` binds an SRT listener on this
    /// UDP port that accepts MPEG-TS streams from broadcast
    /// encoders (OBS, vMix, Larix, ffmpeg). The TS stream is
    /// demuxed and converted to Fragments that reach every
    /// existing egress (HLS, DASH, WHEP, MoQ, archive).
    #[arg(long, default_value = "0", env = "LVQR_SRT_PORT")]
    srt_port: u16,

    /// WHIP HTTP listen port. Set to 0 to disable WHIP ingest. When
    /// non-zero, `lvqr serve` binds a dedicated axum server on this
    /// port exposing `POST/PATCH/DELETE /whip/{broadcast}` for
    /// WebRTC publishers. The WHIP backend uses `str0m`, completes
    /// ICE/DTLS, and converts inbound H.264 Annex B access units
    /// into fragments that flow through every existing egress
    /// (MoQ, LL-HLS, WHEP, disk record, DVR archive).
    #[arg(long, default_value = "0", env = "LVQR_WHIP_PORT")]
    whip_port: u16,

    /// Enable peer mesh relay.
    #[arg(long, env = "LVQR_MESH_ENABLED")]
    mesh_enabled: bool,

    /// Max peer relay connections per viewer.
    #[arg(long, default_value = "3", env = "LVQR_MAX_PEERS")]
    max_peers: usize,

    /// Number of root peers (direct server fanout) before new
    /// subscribers are assigned as children of existing peers.
    /// Defaults to `lvqr_mesh::MeshConfig::default().root_peer_count`
    /// (30). Lower values force earlier promotion of subscribers into
    /// child-of-root roles; useful for small-scale deployments and
    /// end-to-end tests. Only meaningful when `--mesh-enabled`.
    /// Session 116.
    #[arg(long, env = "LVQR_MESH_ROOT_PEER_COUNT")]
    mesh_root_peer_count: Option<usize>,

    /// Path to TLS certificate (PEM). Auto-generates self-signed if omitted.
    #[arg(long, env = "LVQR_TLS_CERT")]
    tls_cert: Option<PathBuf>,

    /// Path to TLS private key (PEM). Auto-generates self-signed if omitted.
    #[arg(long, env = "LVQR_TLS_KEY")]
    tls_key: Option<PathBuf>,

    /// Bearer token required for /api/v1/* admin endpoints. Leave unset for open access.
    #[arg(long, env = "LVQR_ADMIN_TOKEN")]
    admin_token: Option<String>,

    /// Required publish key (RTMP stream key, WS ingest ?token=). Leave unset for open access.
    #[arg(long, env = "LVQR_PUBLISH_KEY")]
    publish_key: Option<String>,

    /// Required viewer token (WS relay/MoQ subscribe ?token=). Leave unset for open access.
    #[arg(long, env = "LVQR_SUBSCRIBE_TOKEN")]
    subscribe_token: Option<String>,

    /// Disable the subscribe-auth gate on live HLS and DASH
    /// routes. When unset (default), the live HLS and DASH
    /// routers are wrapped with the same `SubscribeAuth`
    /// provider that already protects `/ws/*`, `/playback/*`,
    /// and WHEP: Noop provider deployments see no behavior
    /// change (everything allowed); configured deployments
    /// (static token, JWT) get an automatic 401 on unauthed
    /// requests. Set this flag for deployments that want open
    /// live HLS/DASH playback with auth scoped to ingest,
    /// admin, and DVR only. Session 112.
    #[arg(long, env = "LVQR_NO_AUTH_LIVE_PLAYBACK")]
    no_auth_live_playback: bool,

    /// Disable the subscribe-auth gate on the mesh `/signal`
    /// WebSocket. When unset (default, and `--mesh-enabled` is
    /// set), the `/signal` upgrade requires the subscribe token
    /// via a `?token=<token>` query parameter. Noop provider
    /// deployments see no behavior change because the provider
    /// always allows. Only meaningful when `--mesh-enabled`.
    /// Session 111-B1.
    #[arg(long, env = "LVQR_NO_AUTH_SIGNAL")]
    no_auth_signal: bool,

    /// Directory to record broadcasts into. Omit to disable recording.
    #[arg(long, env = "LVQR_RECORD_DIR")]
    record_dir: Option<PathBuf>,

    /// Directory to archive broadcast fragments + redb segment index into.
    /// Enables DVR scrub / time-range playback (Tier 2.4). Omit to disable.
    #[arg(long, env = "LVQR_ARCHIVE_DIR")]
    archive_dir: Option<PathBuf>,

    /// Path to a WASM fragment filter module. When set, `serve`
    /// loads + compiles the module via `lvqr_wasm::WasmFilter::load`
    /// and installs a filter tap on the shared
    /// `FragmentBroadcasterRegistry` before any ingest listener
    /// starts accepting traffic. The tap observes every fragment
    /// and drives `lvqr_wasm_fragments_total{outcome=keep|drop}`
    /// counters. Tier 4 item 4.2 session B (observation only);
    /// stream-modifying filters ship in a later v1.1 pass.
    #[arg(long, env = "LVQR_WASM_FILTER")]
    wasm_filter: Option<PathBuf>,

    /// Path to a whisper.cpp `ggml-*.bin` model file. When set,
    /// `serve` installs a `WhisperCaptionsFactory` on the shared
    /// fragment registry so every new broadcast's audio track
    /// (`1.mp4`) spawns a WhisperCaptionsAgent that transcribes
    /// speech into WebVTT cues and republishes them onto the
    /// `captions` track; the LL-HLS subtitle rendition drains
    /// those cues automatically. Fetch a v1 model via
    /// `curl -L -o ggml-tiny.en.bin
    /// https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin`.
    /// v1 limitations: English only (no `--whisper-language`
    /// flag); captions are not historical -- HLS subscribers who
    /// join an ongoing broadcast see only cues emitted from the
    /// moment they joined onwards. Requires the `whisper` Cargo
    /// feature; the flag is absent from the CLI without it.
    /// Tier 4 item 4.5 session D.
    #[cfg(feature = "whisper")]
    #[arg(long, env = "LVQR_WHISPER_MODEL")]
    whisper_model: Option<PathBuf>,

    /// ABR ladder rendition. Repeatable: `--transcode-rendition 720p
    /// --transcode-rendition 480p` installs both. Each value is one of:
    ///
    /// * a short preset name (`720p`, `480p`, `240p`) -> the matching
    ///   [`lvqr_transcode::RenditionSpec`] preset;
    /// * a path ending in `.toml` -> the file is read + deserialized
    ///   as a custom `RenditionSpec` (fields: `name`, `width`,
    ///   `height`, `video_bitrate_kbps`, `audio_bitrate_kbps`).
    ///
    /// Everything else is a parse error at CLI time so misconfigured
    /// ladders surface up-front instead of via silent drop.
    ///
    /// `LVQR_TRANSCODE_RENDITION` accepts a comma-separated list
    /// because clap's env parser does not repeat.
    ///
    /// Requires the `transcode` Cargo feature; without it the flag
    /// is absent from the CLI. Tier 4 item 4.6 session 106 C.
    #[cfg(feature = "transcode")]
    #[arg(
        long = "transcode-rendition",
        env = "LVQR_TRANSCODE_RENDITION",
        value_delimiter = ',',
        action = ArgAction::Append,
    )]
    transcode_rendition: Vec<String>,

    /// Operator override for the source variant's advertised
    /// `BANDWIDTH` in the LL-HLS master playlist, in kilobits per
    /// second. Defaults to `highest_rung_kbps * 1.2` when unset.
    /// Only meaningful alongside `--transcode-rendition`.
    /// Requires the `transcode` Cargo feature. Tier 4 item 4.6
    /// session 106 C.
    #[cfg(feature = "transcode")]
    #[arg(long, env = "LVQR_SOURCE_BANDWIDTH_KBPS")]
    source_bandwidth_kbps: Option<u32>,

    /// HS256 shared secret enabling JWT authentication. When set, the JWT
    /// provider replaces the static-token provider and all auth surfaces
    /// validate bearer tokens as signed JWTs.
    #[arg(long, env = "LVQR_JWT_SECRET")]
    jwt_secret: Option<String>,

    /// Expected `iss` claim for JWT validation. When unset, issuer is not
    /// checked. Only meaningful with `--jwt-secret`.
    #[arg(long, env = "LVQR_JWT_ISSUER")]
    jwt_issuer: Option<String>,

    /// Expected `aud` claim for JWT validation. When unset, audience is not
    /// checked. Only meaningful with `--jwt-secret`.
    #[arg(long, env = "LVQR_JWT_AUDIENCE")]
    jwt_audience: Option<String>,

    /// Cluster gossip bind address (`ip:port`). When set, this node
    /// joins an LVQR cluster over chitchat gossip; when unset, it
    /// runs standalone. Requires the `cluster` feature (default-on).
    #[arg(long, env = "LVQR_CLUSTER_LISTEN")]
    cluster_listen: Option<SocketAddr>,

    /// Comma-separated seed peers for the chitchat gossip. Each
    /// entry is `ip:port`. Used only when `--cluster-listen` is set.
    #[arg(long, env = "LVQR_CLUSTER_SEEDS", value_delimiter = ',')]
    cluster_seeds: Vec<String>,

    /// Optional explicit cluster-node identifier. Defaults to a
    /// random `lvqr-<16 alphanumeric>` id generated at bootstrap.
    #[arg(long, env = "LVQR_CLUSTER_NODE_ID")]
    cluster_node_id: Option<String>,

    /// Cluster tag gossipped in every SYN. Two deployments sharing
    /// a subnet stay isolated by using different values here.
    /// Defaults to the crate-level `"lvqr"` constant.
    #[arg(long, env = "LVQR_CLUSTER_ID")]
    cluster_id: Option<String>,

    /// Externally-reachable HLS base URL this node advertises to
    /// peers (example: `http://a.local:8888`). Used by the
    /// redirect-to-owner path: when a subscriber hits this node for
    /// a broadcast owned by another node, the HLS handler replies
    /// with a 302 pointing at that owner's advertised URL.
    #[arg(long, env = "LVQR_CLUSTER_ADVERTISE_HLS")]
    cluster_advertise_hls: Option<String>,

    /// Externally-reachable DASH base URL this node advertises.
    /// Same shape as `--cluster-advertise-hls`; used by the DASH
    /// redirect-to-owner path on `/dash/...` requests.
    #[arg(long, env = "LVQR_CLUSTER_ADVERTISE_DASH")]
    cluster_advertise_dash: Option<String>,

    /// Externally-reachable RTSP base URL this node advertises
    /// (example: `rtsp://a.local:8554`). Used by the RTSP 302
    /// redirect-to-owner path on DESCRIBE / PLAY for peer-owned
    /// broadcasts.
    #[arg(long, env = "LVQR_CLUSTER_ADVERTISE_RTSP")]
    cluster_advertise_rtsp: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install the observability subsystem at the top of `main`.
    // Session 80 (G) wired in the `lvqr_observability` facade;
    // session 81 (H) added OTLP span export; session 82 (I)
    // adds OTLP metric export + a pre-built `metrics`-crate
    // bridging recorder we hand off to `start()` via
    // `ServeConfig.otel_metrics_recorder` so it can be composed
    // with the Prometheus scrape recorder via
    // `metrics_util::FanoutBuilder`. The handle is held for the
    // full `main` scope so the OTLP background flushers do not
    // leak; `mut` so we can `take_metrics_recorder` once.
    let mut observability = lvqr_observability::init(lvqr_observability::ObservabilityConfig::from_env())?;
    let otel_metrics_recorder = observability.take_metrics_recorder();

    let cli = Cli::parse();

    let result = match cli {
        Cli::Serve(args) => serve_from_args(args, otel_metrics_recorder).await,
    };

    // Keep `observability` alive here so the tracer / meter
    // providers flush on drop after serve_from_args returns.
    drop(observability);
    result
}

async fn serve_from_args(
    args: ServeArgs,
    otel_metrics_recorder: Option<lvqr_observability::OtelMetricsRecorder>,
) -> Result<()> {
    // Build auth provider from CLI/env. JWT takes precedence when
    // `--jwt-secret` is set; otherwise fall back to the static-token
    // provider when any individual token is configured; otherwise open
    // access (`NoopAuthProvider`).
    let auth: SharedAuth = if let Some(secret) = args.jwt_secret.clone() {
        tracing::info!(
            issuer = args.jwt_issuer.is_some(),
            audience = args.jwt_audience.is_some(),
            "auth: JWT provider enabled"
        );
        let provider = JwtAuthProvider::new(JwtAuthConfig {
            secret,
            issuer: args.jwt_issuer.clone(),
            audience: args.jwt_audience.clone(),
        })
        .map_err(|e| anyhow::anyhow!("failed to init JWT auth provider: {e}"))?;
        Arc::new(provider) as SharedAuth
    } else {
        let auth_config = StaticAuthConfig {
            admin_token: args.admin_token.clone(),
            publish_key: args.publish_key.clone(),
            subscribe_token: args.subscribe_token.clone(),
        };
        if auth_config.has_any() {
            tracing::info!(
                admin = auth_config.admin_token.is_some(),
                publish = auth_config.publish_key.is_some(),
                subscribe = auth_config.subscribe_token.is_some(),
                "auth: static-token provider enabled"
            );
            Arc::new(StaticAuthProvider::new(auth_config)) as SharedAuth
        } else {
            tracing::info!("auth: open access (no tokens configured)");
            Arc::new(NoopAuthProvider) as SharedAuth
        }
    };

    let hls_addr = if args.hls_port == 0 {
        None
    } else {
        Some(([0, 0, 0, 0], args.hls_port).into())
    };

    let whep_addr = if args.whep_port == 0 {
        None
    } else {
        Some(([0, 0, 0, 0], args.whep_port).into())
    };

    let whip_addr = if args.whip_port == 0 {
        None
    } else {
        Some(([0, 0, 0, 0], args.whip_port).into())
    };

    let dash_addr = if args.dash_port == 0 {
        None
    } else {
        Some(([0, 0, 0, 0], args.dash_port).into())
    };

    let config = ServeConfig {
        relay_addr: ([0, 0, 0, 0], args.port).into(),
        rtmp_addr: ([0, 0, 0, 0], args.rtmp_port).into(),
        admin_addr: ([0, 0, 0, 0], args.admin_port).into(),
        hls_addr,
        rtsp_addr: if args.rtsp_port == 0 {
            None
        } else {
            Some(([0, 0, 0, 0], args.rtsp_port).into())
        },
        srt_addr: if args.srt_port == 0 {
            None
        } else {
            Some(([0, 0, 0, 0], args.srt_port).into())
        },
        hls_dvr_window_secs: args.hls_dvr_window,
        hls_target_duration_secs: args.hls_target_duration,
        hls_part_target_ms: args.hls_part_target,
        whep_addr,
        whip_addr,
        dash_addr,
        mesh_enabled: args.mesh_enabled,
        max_peers: args.max_peers,
        auth: Some(auth),
        record_dir: args.record_dir,
        archive_dir: args.archive_dir,
        #[cfg(feature = "c2pa")]
        c2pa: None,
        #[cfg(feature = "whisper")]
        whisper_model: args.whisper_model,
        #[cfg(feature = "transcode")]
        transcode_renditions: parse_transcode_renditions(&args.transcode_rendition)?,
        #[cfg(feature = "transcode")]
        source_bandwidth_kbps: args.source_bandwidth_kbps,
        wasm_filter: args.wasm_filter,
        install_prometheus: true,
        otel_metrics_recorder,
        tls_cert: args.tls_cert,
        tls_key: args.tls_key,
        cluster_listen: args.cluster_listen,
        cluster_seeds: args.cluster_seeds,
        cluster_node_id: args.cluster_node_id,
        cluster_id: args.cluster_id,
        cluster_advertise_hls: args.cluster_advertise_hls,
        cluster_advertise_dash: args.cluster_advertise_dash,
        cluster_advertise_rtsp: args.cluster_advertise_rtsp,
        // Federation links are TOML-only for v1; a `--federation-link`
        // CLI flag gets added in session 103 C alongside the admin
        // route. Keeping the field empty here means default `lvqr serve`
        // invocations do not change behavior.
        #[cfg(feature = "cluster")]
        federation_links: Vec::new(),
        no_auth_live_playback: args.no_auth_live_playback,
        no_auth_signal: args.no_auth_signal,
        mesh_root_peer_count: args.mesh_root_peer_count,
    };

    let handle = start(config).await?;

    tokio::select! {
        res = tokio::signal::ctrl_c() => {
            if res.is_ok() {
                tracing::info!("ctrl-c received, initiating graceful shutdown");
            }
        }
    }

    handle.shutdown().await
}

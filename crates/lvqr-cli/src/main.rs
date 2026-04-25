use anyhow::Result;
use clap::Parser;
#[cfg(feature = "jwks")]
use lvqr_auth::{JwksAuthConfig, JwksAuthProvider};
use lvqr_auth::{JwtAuthConfig, JwtAuthProvider, NoopAuthProvider, SharedAuth, StaticAuthConfig, StaticAuthProvider};
#[cfg(feature = "webhook")]
use lvqr_auth::{WebhookAuthConfig, WebhookAuthProvider};
#[cfg(feature = "transcode")]
use lvqr_cli::parse_transcode_renditions;
use lvqr_cli::{ServeConfig, start};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "transcode")]
use clap::ArgAction;

#[cfg(feature = "c2pa")]
use clap::ValueEnum;

/// Clap-facing enum over the digital-signature algorithms c2pa-rs
/// accepts for on-disk PKCS#8 keys. 1:1 with
/// [`lvqr_archive::provenance::C2paSigningAlg`]; the indirection
/// keeps clap's `ValueEnum` derive away from the `lvqr-archive`
/// crate so the `c2pa` Cargo feature stays the single gate on
/// every provenance-adjacent dep. Rendered lowercase on the CLI
/// (`--c2pa-signing-alg es256`).
#[cfg(feature = "c2pa")]
#[derive(Clone, Copy, Debug, ValueEnum)]
#[clap(rename_all = "lower")]
enum C2paAlgArg {
    Es256,
    Es384,
    Es512,
    Ps256,
    Ps384,
    Ps512,
    Ed25519,
}

#[cfg(feature = "c2pa")]
impl C2paAlgArg {
    fn to_archive_alg(self) -> lvqr_archive::provenance::C2paSigningAlg {
        use lvqr_archive::provenance::C2paSigningAlg;
        match self {
            Self::Es256 => C2paSigningAlg::Es256,
            Self::Es384 => C2paSigningAlg::Es384,
            Self::Es512 => C2paSigningAlg::Es512,
            Self::Ps256 => C2paSigningAlg::Ps256,
            Self::Ps384 => C2paSigningAlg::Ps384,
            Self::Ps512 => C2paSigningAlg::Ps512,
            Self::Ed25519 => C2paSigningAlg::Ed25519,
        }
    }
}

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

    /// Path to a TOML config file (session 147). When set, the file
    /// fills any auth-section fields the operator did not pass via
    /// CLI flag or env var. SIGHUP and `POST /api/v1/config-reload`
    /// re-read the file and atomically swap the live auth provider
    /// without bouncing the relay. When unset, SIGHUP is a no-op
    /// and the admin POST returns 503. See `docs/config-reload.md`.
    #[arg(long, env = "LVQR_CONFIG")]
    config: Option<PathBuf>,

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

    /// Disable the runtime stream-key CRUD admin API. When unset
    /// (default), `start()` wraps the configured auth provider
    /// in a `MultiKeyAuthProvider` backed by an in-memory store,
    /// and mounts `/api/v1/streamkeys/*` so an admin client can
    /// mint, list, revoke, and rotate ingest stream keys at
    /// runtime. The wrap is purely additive: existing publish
    /// auth (`LVQR_PUBLISH_KEY`, JWT, JWKS, webhook) keeps
    /// working unchanged. Set this flag for deployments that
    /// want the pre-146 behavior exactly: no store, no
    /// `/streamkeys` routes, no MultiKey wrap. Session 146.
    #[arg(long, env = "LVQR_NO_STREAMKEYS")]
    no_streamkeys: bool,

    /// JSON array of STUN/TURN servers to push to browser peers
    /// via the mesh `AssignParent` server-push message. Each entry
    /// mirrors WebRTC's `RTCIceServer` shape:
    /// `{"urls":["..."],"username":"u","credential":"p"}`. Empty
    /// (default) means "client decides": JS `MeshPeer` falls back
    /// to whatever was passed to its constructor (or its hardcoded
    /// Google STUN default). Non-empty makes the server
    /// authoritative -- clients rebuild their `RTCPeerConnection`
    /// `iceServers` from this list when AssignParent lands.
    /// Required for deployments where peers sit behind symmetric
    /// NAT and need a TURN relay (see `deploy/turn/` for a
    /// coturn deployment recipe). Only meaningful when
    /// `--mesh-enabled`. Session 143.
    #[arg(long, env = "LVQR_MESH_ICE_SERVERS")]
    mesh_ice_servers: Option<String>,

    /// Directory to record broadcasts into. Omit to disable recording.
    #[arg(long, env = "LVQR_RECORD_DIR")]
    record_dir: Option<PathBuf>,

    /// Directory to archive broadcast fragments + redb segment index into.
    /// Enables DVR scrub / time-range playback (Tier 2.4). Omit to disable.
    #[arg(long, env = "LVQR_ARCHIVE_DIR")]
    archive_dir: Option<PathBuf>,

    /// HMAC signing secret for short-lived playback URLs (PLAN v1.1
    /// row 121). When set, every `/playback/*` handler accepts an
    /// alternative auth path: a `?exp=<unix_ts>&sig=<base64url>`
    /// pair where `sig = HMAC-SHA256(secret, "<path>?exp=<ts>")`.
    /// A valid signature short-circuits the normal subscribe-token
    /// check so operators can mint one-off share links for third
    /// parties who cannot authenticate. Tampered or expired
    /// signatures return 403 (NOT 401) so clients can distinguish
    /// missing auth from wrong auth. Use a long, high-entropy
    /// secret (32+ random bytes); rotating it invalidates every
    /// outstanding signed URL. Omit to disable the signed-URL
    /// path; all playback routes fall back to their existing
    /// subscribe-token gate.
    #[arg(long, env = "LVQR_HMAC_PLAYBACK_SECRET")]
    hmac_playback_secret: Option<String>,

    /// Path to a PEM-encoded C2PA signing certificate chain (leaf
    /// first, then CA). When set together with
    /// `--c2pa-signing-key`, every broadcast that terminates via
    /// drain produces a signed `finalized.mp4` + `finalized.c2pa`
    /// pair in the archive, and `GET /playback/verify/{broadcast}`
    /// returns a JSON manifest-validation report. Requires
    /// `--archive-dir` to be set (signing runs on the archive
    /// drain-termination hook). Leaf EKU MUST be from c2pa-rs's
    /// allow-list (`emailProtection`, `documentSigning`,
    /// `timeStamping`, `OCSPSigning`, MS C2PA 1.3.6.1.4.1.311.76.59.1.9,
    /// or C2PA 1.3.6.1.4.1.62558.2.1) + the `digitalSignature`
    /// key-usage bit; c2pa-rs rejects self-signed leaves per
    /// C2PA spec section 14.5.1. Requires the `c2pa` Cargo
    /// feature; without it the flag is absent from the CLI.
    /// Tier 4 item 4.3.
    #[cfg(feature = "c2pa")]
    #[arg(long, env = "LVQR_C2PA_SIGNING_CERT")]
    c2pa_signing_cert: Option<PathBuf>,

    /// Path to the PEM-encoded PKCS#8 private key matching the
    /// leaf cert's subject public key. Must be set whenever
    /// `--c2pa-signing-cert` is set; either flag alone is a
    /// configuration error.
    #[cfg(feature = "c2pa")]
    #[arg(long, env = "LVQR_C2PA_SIGNING_KEY")]
    c2pa_signing_key: Option<PathBuf>,

    /// Digital signature algorithm matching the private key.
    /// `es256` + ECDSA P-256, `ed25519` + Ed25519, etc. Defaults
    /// to `es256` which matches rcgen's default P-256 output and
    /// the most common C2PA operator-managed key shape.
    #[cfg(feature = "c2pa")]
    #[arg(long, env = "LVQR_C2PA_SIGNING_ALG", default_value = "es256", value_enum)]
    c2pa_signing_alg: C2paAlgArg,

    /// Human-readable creator name embedded in the
    /// `stds.schema-org.CreativeWork` author assertion on every
    /// signed asset. Typical value is the operator's org name or
    /// a broadcast identifier. Defaults to `"lvqr"`.
    #[cfg(feature = "c2pa")]
    #[arg(long, env = "LVQR_C2PA_ASSERTION_CREATOR", default_value = "lvqr")]
    c2pa_assertion_creator: String,

    /// Path to a PEM-encoded trust-anchor bundle surfaced to
    /// `c2pa::Context::with_settings({"trust": {"user_anchors":
    /// ...}})` so c2pa-rs's chain validator accepts certs issued
    /// by this CA. Required for any deployment using a private
    /// CA; leave unset when the leaf chains to a public C2PA
    /// trust anchor. When unset and signing with an unknown CA,
    /// `/playback/verify` reports `validation_state = "Valid"`
    /// (crypto integrity passes) rather than `"Trusted"` (CA in
    /// the trust list).
    #[cfg(feature = "c2pa")]
    #[arg(long, env = "LVQR_C2PA_TRUST_ANCHOR")]
    c2pa_trust_anchor: Option<PathBuf>,

    /// Optional RFC 3161 Timestamp Authority URL. When set, the
    /// signer contacts the TSA during every sign call so the
    /// signing moment is countersigned by the TSA and survives
    /// cert expiry. Leave unset for internal archives; set for
    /// evidentiary-grade signing where the signing time must
    /// remain verifiable after the leaf cert expires.
    #[cfg(feature = "c2pa")]
    #[arg(long, env = "LVQR_C2PA_TIMESTAMP_AUTHORITY")]
    c2pa_timestamp_authority: Option<String>,

    /// Path to a WASM fragment filter module. When set, `serve`
    /// loads + compiles the module via `lvqr_wasm::WasmFilter::load`
    /// and installs a filter tap on the shared
    /// `FragmentBroadcasterRegistry` before any ingest listener
    /// starts accepting traffic. The tap observes every fragment
    /// and drives `lvqr_wasm_fragments_total{outcome=keep|drop}`
    /// counters. Tier 4 item 4.2 session B (observation only).
    ///
    /// The flag accepts multiple values and may be repeated to
    /// compose an ordered chain of filters (PLAN Phase D, session
    /// 136): `--wasm-filter a.wasm --wasm-filter b.wasm` is
    /// equivalent to `--wasm-filter a.wasm,b.wasm`. Chain order is
    /// preserved; the first filter that drops a fragment
    /// short-circuits the rest of the chain for that fragment. Each
    /// path is watched independently by its own reloader, so
    /// hot-swapping one slot does not disturb the others.
    #[arg(long, env = "LVQR_WASM_FILTER", value_delimiter = ',', num_args = 1..)]
    wasm_filter: Vec<PathBuf>,

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
    /// checked. Only meaningful with `--jwt-secret` or `--jwks-url`; both
    /// auth paths honor this flag when present.
    #[arg(long, env = "LVQR_JWT_AUDIENCE")]
    jwt_audience: Option<String>,

    /// JWKS endpoint URL (e.g. `https://idp.example.com/.well-known/jwks.json`)
    /// enabling dynamic asymmetric-key JWT authentication. When set, the JWKS
    /// provider replaces HS256 entirely and every auth surface validates
    /// bearer tokens via RS256 / ES256 / EdDSA signatures against keys fetched
    /// from this URL. `--jwt-secret` is mutually exclusive with this flag;
    /// `--jwt-issuer` and `--jwt-audience` still apply (they constrain the
    /// expected `iss` / `aud` claim on the JWT regardless of signing method).
    /// Requires the `jwks` Cargo feature (included in `--features full`).
    /// PLAN row 120.
    #[cfg(feature = "jwks")]
    #[arg(long, env = "LVQR_JWKS_URL")]
    jwks_url: Option<String>,

    /// Background refresh interval for the JWKS cache, in seconds. Minimum
    /// 10 s (rejected below that so a misconfigured deployment cannot DDoS
    /// the IdP). Defaults to 300 s. Only meaningful with `--jwks-url`.
    #[cfg(feature = "jwks")]
    #[arg(long, env = "LVQR_JWKS_REFRESH_INTERVAL_SECONDS", default_value_t = 300)]
    jwks_refresh_interval_seconds: u64,

    /// Webhook auth endpoint URL. When set, every `AuthContext` decision
    /// (publish / subscribe / admin) is cached and, on miss, delegated to
    /// this endpoint via `POST` with a JSON body shaped `{op, ...}` (see
    /// `docs/auth.md#webhook-auth-provider`). The endpoint must reply
    /// `{"allow": bool, "reason": str?}` on a 2xx. Mutually exclusive with
    /// `--jwks-url` and `--jwt-secret`. Requires the `webhook` Cargo
    /// feature.
    #[cfg(feature = "webhook")]
    #[arg(long, env = "LVQR_WEBHOOK_AUTH_URL")]
    webhook_auth_url: Option<String>,

    /// How long an allow decision stays cached before the next check for
    /// the same context re-consults the webhook. Defaults to 60 s. Minimum
    /// 1 s so the webhook does not get hammered per request. Only
    /// meaningful with `--webhook-auth-url`.
    #[cfg(feature = "webhook")]
    #[arg(long, env = "LVQR_WEBHOOK_AUTH_CACHE_TTL_SECONDS", default_value_t = 60)]
    webhook_auth_cache_ttl_seconds: u64,

    /// How long a deny decision (including failed-webhook-call denies) stays
    /// cached. Defaults to 10 s. Kept shorter than the allow TTL by default
    /// so transient webhook outages recover quickly; must be > 0 so a
    /// flapping webhook is not re-hit on every request.
    #[cfg(feature = "webhook")]
    #[arg(long, env = "LVQR_WEBHOOK_AUTH_DENY_CACHE_TTL_SECONDS", default_value_t = 10)]
    webhook_auth_deny_cache_ttl_seconds: u64,

    /// Per-request HTTP timeout for the webhook POST, in seconds. Defaults
    /// to 5 s. Only meaningful with `--webhook-auth-url`.
    #[cfg(feature = "webhook")]
    #[arg(long, env = "LVQR_WEBHOOK_AUTH_FETCH_TIMEOUT_SECONDS", default_value_t = 5)]
    webhook_auth_fetch_timeout_seconds: u64,

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
    // Session 147: when `--config <path>` is set, the file's
    // `[auth]` section overrides the CLI defaults BEFORE `build_auth`
    // sees them, and the reload handle captures the boot defaults so
    // SIGHUP / admin POST can re-merge them with a fresh file read.
    let auth_boot_defaults = lvqr_cli::AuthBootDefaults {
        admin_token: args.admin_token.clone(),
        publish_key: args.publish_key.clone(),
        subscribe_token: args.subscribe_token.clone(),
        jwt_secret: args.jwt_secret.clone(),
        jwt_issuer: args.jwt_issuer.clone(),
        jwt_audience: args.jwt_audience.clone(),
    };
    // The file-merge (file overrides CLI defaults for auth-section
    // fields) happens inside `start()` via a one-shot
    // `ConfigReloadHandle::reload("boot")` call right after the
    // initial auth chain is built. This keeps the merge logic in
    // ONE place so both this CLI path and `lvqr_test_utils::TestServer`
    // (which calls `start()` directly) get identical behavior.
    let config_reload_seed = args.config.clone().map(|path| {
        tracing::info!(
            path = %path.display(),
            "config file declared; will be applied at boot, then re-applied on SIGHUP / POST /api/v1/config-reload"
        );
        lvqr_cli::ConfigReloadSeed {
            path,
            auth_boot_defaults,
        }
    });

    // Build auth provider from CLI/env (now possibly augmented by the
    // file). Order of precedence:
    //   1. `--jwks-url` (PLAN row 120) -- dynamic asymmetric JWTs.
    //   2. `--webhook-auth-url` (PLAN Phase D) -- external HTTP decision oracle.
    //   3. `--jwt-secret` -- HS256 static secret.
    //   4. `--admin-token` / `--publish-key` / `--subscribe-token` -- static-token provider.
    //   5. Otherwise open access (`NoopAuthProvider`).
    // `--jwks-url`, `--webhook-auth-url`, and `--jwt-secret` are mutually
    // exclusive: each picks a different signing or decision strategy, and a
    // silent pick between them would hide a misconfiguration.
    let auth: SharedAuth = build_auth(&args).await?;

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

    // Build the C2PA config before the ServeConfig literal starts
    // moving fields out of `args`; the helper needs to read
    // multiple c2pa-related fields so it cannot share the move.
    #[cfg(feature = "c2pa")]
    let c2pa_config = build_c2pa_config(&args)?;

    // Session 143: parse the operator's --mesh-ice-servers JSON
    // before the ServeConfig literal. A parse error here surfaces
    // at boot rather than as a silent server-emits-empty surprise.
    let mesh_ice_servers = parse_mesh_ice_servers(args.mesh_ice_servers.as_deref())?;

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
        hmac_playback_secret: args.hmac_playback_secret,
        #[cfg(feature = "c2pa")]
        c2pa: c2pa_config,
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
        mesh_ice_servers,
        streamkeys_enabled: !args.no_streamkeys,
        config_reload: config_reload_seed,
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

/// Resolve the full auth-provider cascade, applying the precedence documented
/// at the call site in `serve_from_args`. Factored out so each feature-gated
/// branch stays local to one `#[cfg]` block without littering the caller.
async fn build_auth(args: &ServeArgs) -> Result<SharedAuth> {
    check_auth_flag_combinations(args)?;

    #[cfg(feature = "jwks")]
    if let Some(jwks_url) = args.jwks_url.clone() {
        tracing::info!(
            url = %jwks_url,
            issuer = args.jwt_issuer.is_some(),
            audience = args.jwt_audience.is_some(),
            refresh_interval_s = args.jwks_refresh_interval_seconds,
            "auth: JWKS provider enabled"
        );
        let cfg = JwksAuthConfig {
            jwks_url,
            issuer: args.jwt_issuer.clone(),
            audience: args.jwt_audience.clone(),
            refresh_interval: std::time::Duration::from_secs(args.jwks_refresh_interval_seconds),
            fetch_timeout: std::time::Duration::from_secs(10),
            allowed_algs: JwksAuthConfig::default_allowed_algs(),
        };
        let provider = JwksAuthProvider::new(cfg)
            .await
            .map_err(|e| anyhow::anyhow!("failed to init JWKS auth provider: {e}"))?;
        return Ok(Arc::new(provider) as SharedAuth);
    }

    #[cfg(feature = "webhook")]
    if let Some(webhook_url) = args.webhook_auth_url.clone() {
        tracing::info!(
            url = %webhook_url,
            allow_ttl_s = args.webhook_auth_cache_ttl_seconds,
            deny_ttl_s = args.webhook_auth_deny_cache_ttl_seconds,
            fetch_timeout_s = args.webhook_auth_fetch_timeout_seconds,
            "auth: webhook provider enabled"
        );
        let cfg = WebhookAuthConfig {
            webhook_url,
            allow_cache_ttl: std::time::Duration::from_secs(args.webhook_auth_cache_ttl_seconds),
            deny_cache_ttl: std::time::Duration::from_secs(args.webhook_auth_deny_cache_ttl_seconds),
            fetch_timeout: std::time::Duration::from_secs(args.webhook_auth_fetch_timeout_seconds),
            cache_capacity: std::num::NonZeroUsize::new(4096).expect("4096 != 0"),
        };
        let provider = WebhookAuthProvider::new(cfg)
            .await
            .map_err(|e| anyhow::anyhow!("failed to init webhook auth provider: {e}"))?;
        return Ok(Arc::new(provider) as SharedAuth);
    }

    build_static_or_jwt_auth(args)
}

/// Reject mutually-exclusive auth flag combinations. Each of `--jwks-url`,
/// `--webhook-auth-url`, and `--jwt-secret` picks a distinct strategy; a
/// silent fall-through between them would hide a misconfiguration. Factored
/// out of `build_auth` so the check is unit-testable without booting a
/// runtime, and gated on the union of feature flags that actually expose
/// the relevant fields on `ServeArgs`.
#[cfg(any(feature = "jwks", feature = "webhook"))]
fn check_auth_flag_combinations(args: &ServeArgs) -> Result<()> {
    #[cfg(feature = "jwks")]
    if args.jwks_url.is_some() && args.jwt_secret.is_some() {
        return Err(anyhow::anyhow!(
            "--jwks-url and --jwt-secret are mutually exclusive; pick one signing strategy"
        ));
    }
    #[cfg(feature = "webhook")]
    if args.webhook_auth_url.is_some() && args.jwt_secret.is_some() {
        return Err(anyhow::anyhow!(
            "--webhook-auth-url and --jwt-secret are mutually exclusive; pick one auth strategy"
        ));
    }
    #[cfg(all(feature = "jwks", feature = "webhook"))]
    if args.jwks_url.is_some() && args.webhook_auth_url.is_some() {
        return Err(anyhow::anyhow!(
            "--jwks-url and --webhook-auth-url are mutually exclusive; pick one auth strategy"
        ));
    }
    Ok(())
}

#[cfg(not(any(feature = "jwks", feature = "webhook")))]
fn check_auth_flag_combinations(_args: &ServeArgs) -> Result<()> {
    Ok(())
}

/// Parse the `--mesh-ice-servers` JSON blob into a `Vec<IceServer>`.
/// `None` (flag unset) yields an empty vec; the server then emits
/// `ice_servers: []` and clients fall back to their constructor
/// defaults. Parse errors surface at boot with a clear message.
/// Session 143 -- TURN deployment recipe.
fn parse_mesh_ice_servers(raw: Option<&str>) -> Result<Vec<lvqr_signal::IceServer>> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<lvqr_signal::IceServer>>(trimmed).map_err(|e| {
        anyhow::anyhow!("--mesh-ice-servers must be a JSON array of {{urls, username?, credential?}} objects: {e}")
    })
}

/// Resolve the non-JWKS auth provider from CLI/env: JWT HS256 if
/// `--jwt-secret` is set, otherwise the static-token provider if any
/// individual token is configured, otherwise open access. Factored out so
/// the JWKS branch in `serve_from_args` can fall through to the same
/// resolution when `--jwks-url` is unset.
fn build_static_or_jwt_auth(args: &ServeArgs) -> Result<SharedAuth> {
    if let Some(secret) = args.jwt_secret.clone() {
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
        return Ok(Arc::new(provider) as SharedAuth);
    }
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
        Ok(Arc::new(StaticAuthProvider::new(auth_config)) as SharedAuth)
    } else {
        tracing::info!("auth: open access (no tokens configured)");
        Ok(Arc::new(NoopAuthProvider) as SharedAuth)
    }
}

/// Resolve `--c2pa-signing-cert` / `--c2pa-signing-key` + siblings
/// into an optional [`lvqr_archive::provenance::C2paConfig`].
///
/// Three outcomes:
///
/// * **Neither flag set** -> returns `Ok(None)`; signing stays off
///   and the archive finalize path runs without a signer.
/// * **Both flags set** -> returns `Ok(Some(C2paConfig { .. }))`
///   with [`C2paSignerSource::CertKeyFiles`] carrying the paths +
///   algorithm + optional TSA, plus the assertion creator and the
///   trust-anchor PEM (file contents read eagerly so the error
///   surfaces at CLI time rather than at the first finalize).
/// * **Only one of the two set** -> returns `Err(anyhow)` with a
///   message naming the missing flag. Either flag alone has no
///   defined behavior (a cert without a matching key cannot sign,
///   and a key without a cert leaves the signer without a chain
///   to advertise), so loud failure is safer than silent drop.
#[cfg(feature = "c2pa")]
fn build_c2pa_config(args: &ServeArgs) -> Result<Option<lvqr_archive::provenance::C2paConfig>> {
    use lvqr_archive::provenance::{C2paConfig, C2paSignerSource};
    match (&args.c2pa_signing_cert, &args.c2pa_signing_key) {
        (None, None) => Ok(None),
        (Some(_), None) => Err(anyhow::anyhow!(
            "--c2pa-signing-cert was set but --c2pa-signing-key is missing; both flags must appear together"
        )),
        (None, Some(_)) => Err(anyhow::anyhow!(
            "--c2pa-signing-key was set but --c2pa-signing-cert is missing; both flags must appear together"
        )),
        (Some(cert_path), Some(key_path)) => {
            let trust_anchor_pem =
                match &args.c2pa_trust_anchor {
                    None => None,
                    Some(path) => Some(std::fs::read_to_string(path).map_err(|e| {
                        anyhow::anyhow!("failed to read --c2pa-trust-anchor at {}: {e}", path.display())
                    })?),
                };
            Ok(Some(C2paConfig {
                signer_source: C2paSignerSource::CertKeyFiles {
                    signing_cert_path: cert_path.clone(),
                    private_key_path: key_path.clone(),
                    signing_alg: args.c2pa_signing_alg.to_archive_alg(),
                    timestamp_authority_url: args.c2pa_timestamp_authority.clone(),
                },
                assertion_creator: args.c2pa_assertion_creator.clone(),
                trust_anchor_pem,
            }))
        }
    }
}

#[cfg(all(test, feature = "c2pa"))]
mod c2pa_cli_tests {
    use super::*;
    use clap::Parser;
    use lvqr_archive::provenance::{C2paSignerSource, C2paSigningAlg};

    fn parse(args: &[&str]) -> ServeArgs {
        match Cli::parse_from(args) {
            Cli::Serve(a) => a,
        }
    }

    #[test]
    fn no_c2pa_flags_yields_none() {
        let a = parse(&["lvqr", "serve"]);
        assert!(a.c2pa_signing_cert.is_none());
        assert!(a.c2pa_signing_key.is_none());
        assert!(build_c2pa_config(&a).unwrap().is_none());
    }

    #[test]
    fn cert_without_key_is_configuration_error() {
        let a = parse(&["lvqr", "serve", "--c2pa-signing-cert", "/tmp/cert.pem"]);
        let err = build_c2pa_config(&a).unwrap_err().to_string();
        assert!(err.contains("--c2pa-signing-cert"), "err: {err}");
        assert!(err.contains("--c2pa-signing-key"), "err: {err}");
    }

    #[test]
    fn key_without_cert_is_configuration_error() {
        let a = parse(&["lvqr", "serve", "--c2pa-signing-key", "/tmp/key.pem"]);
        let err = build_c2pa_config(&a).unwrap_err().to_string();
        assert!(err.contains("--c2pa-signing-cert"), "err: {err}");
        assert!(err.contains("--c2pa-signing-key"), "err: {err}");
    }

    #[test]
    fn both_flags_yields_certkeyfiles_source() {
        let a = parse(&[
            "lvqr",
            "serve",
            "--c2pa-signing-cert",
            "/tmp/cert.pem",
            "--c2pa-signing-key",
            "/tmp/key.pem",
        ]);
        let cfg = build_c2pa_config(&a).unwrap().expect("config populated");
        match cfg.signer_source {
            C2paSignerSource::CertKeyFiles {
                signing_cert_path,
                private_key_path,
                signing_alg,
                timestamp_authority_url,
            } => {
                assert_eq!(signing_cert_path.to_string_lossy(), "/tmp/cert.pem");
                assert_eq!(private_key_path.to_string_lossy(), "/tmp/key.pem");
                // Default alg is Es256.
                assert!(matches!(signing_alg, C2paSigningAlg::Es256));
                assert!(timestamp_authority_url.is_none());
            }
            other => panic!("expected CertKeyFiles, got {other:?}"),
        }
        // Defaults for assertion creator + trust anchor.
        assert_eq!(cfg.assertion_creator, "lvqr");
        assert!(cfg.trust_anchor_pem.is_none());
    }

    #[test]
    fn alg_flag_maps_to_archive_enum() {
        let a = parse(&[
            "lvqr",
            "serve",
            "--c2pa-signing-cert",
            "/tmp/cert.pem",
            "--c2pa-signing-key",
            "/tmp/key.pem",
            "--c2pa-signing-alg",
            "ed25519",
        ]);
        let cfg = build_c2pa_config(&a).unwrap().unwrap();
        match cfg.signer_source {
            C2paSignerSource::CertKeyFiles { signing_alg, .. } => {
                assert!(matches!(signing_alg, C2paSigningAlg::Ed25519));
            }
            other => panic!("expected CertKeyFiles, got {other:?}"),
        }
    }

    #[test]
    fn assertion_creator_override_lands_on_config() {
        let a = parse(&[
            "lvqr",
            "serve",
            "--c2pa-signing-cert",
            "/tmp/cert.pem",
            "--c2pa-signing-key",
            "/tmp/key.pem",
            "--c2pa-assertion-creator",
            "Example Broadcaster",
        ]);
        let cfg = build_c2pa_config(&a).unwrap().unwrap();
        assert_eq!(cfg.assertion_creator, "Example Broadcaster");
    }

    #[test]
    fn timestamp_authority_flag_lands_on_certkeyfiles() {
        let a = parse(&[
            "lvqr",
            "serve",
            "--c2pa-signing-cert",
            "/tmp/cert.pem",
            "--c2pa-signing-key",
            "/tmp/key.pem",
            "--c2pa-timestamp-authority",
            "https://tsa.example.invalid",
        ]);
        let cfg = build_c2pa_config(&a).unwrap().unwrap();
        match cfg.signer_source {
            C2paSignerSource::CertKeyFiles {
                timestamp_authority_url,
                ..
            } => {
                assert_eq!(timestamp_authority_url.as_deref(), Some("https://tsa.example.invalid"));
            }
            other => panic!("expected CertKeyFiles, got {other:?}"),
        }
    }

    #[test]
    fn missing_trust_anchor_file_surfaces_as_configuration_error() {
        let a = parse(&[
            "lvqr",
            "serve",
            "--c2pa-signing-cert",
            "/tmp/cert.pem",
            "--c2pa-signing-key",
            "/tmp/key.pem",
            "--c2pa-trust-anchor",
            "/nonexistent/path/to/anchor.pem",
        ]);
        let err = build_c2pa_config(&a).unwrap_err().to_string();
        assert!(err.contains("--c2pa-trust-anchor"), "err: {err}");
        assert!(err.contains("/nonexistent/path/to/anchor.pem"), "err: {err}");
    }
}

#[cfg(all(test, feature = "jwks"))]
mod jwks_cli_tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> ServeArgs {
        match Cli::parse_from(args) {
            Cli::Serve(a) => a,
        }
    }

    #[test]
    fn jwks_url_unset_passes_combination_check() {
        let a = parse(&["lvqr", "serve"]);
        assert!(a.jwks_url.is_none());
        assert_eq!(a.jwks_refresh_interval_seconds, 300);
        check_auth_flag_combinations(&a).expect("no flags should be fine");
    }

    #[test]
    fn jwks_url_flag_parses() {
        let a = parse(&["lvqr", "serve", "--jwks-url", "https://idp.example.com/jwks.json"]);
        assert_eq!(a.jwks_url.as_deref(), Some("https://idp.example.com/jwks.json"));
        check_auth_flag_combinations(&a).expect("jwks alone is fine");
    }

    #[test]
    fn jwks_url_plus_jwt_secret_is_mutex_error() {
        let a = parse(&[
            "lvqr",
            "serve",
            "--jwks-url",
            "https://idp.example.com/jwks.json",
            "--jwt-secret",
            "hunter2",
        ]);
        let err = check_auth_flag_combinations(&a).unwrap_err().to_string();
        assert!(err.contains("--jwks-url"), "err: {err}");
        assert!(err.contains("--jwt-secret"), "err: {err}");
        assert!(err.contains("mutually exclusive"), "err: {err}");
    }

    #[test]
    fn jwks_refresh_interval_override_applies() {
        let a = parse(&[
            "lvqr",
            "serve",
            "--jwks-url",
            "https://idp.example.com/jwks.json",
            "--jwks-refresh-interval-seconds",
            "60",
        ]);
        assert_eq!(a.jwks_refresh_interval_seconds, 60);
    }

    #[test]
    fn jwt_issuer_audience_still_apply_under_jwks() {
        // The JWKS branch reuses --jwt-issuer and --jwt-audience so operators
        // do not learn two parallel claim-binding vocabularies.
        let a = parse(&[
            "lvqr",
            "serve",
            "--jwks-url",
            "https://idp.example.com/jwks.json",
            "--jwt-issuer",
            "https://idp.example.com/",
            "--jwt-audience",
            "lvqr-prod",
        ]);
        assert_eq!(a.jwt_issuer.as_deref(), Some("https://idp.example.com/"));
        assert_eq!(a.jwt_audience.as_deref(), Some("lvqr-prod"));
    }
}

#[cfg(all(test, feature = "webhook"))]
mod webhook_cli_tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> ServeArgs {
        match Cli::parse_from(args) {
            Cli::Serve(a) => a,
        }
    }

    #[test]
    fn webhook_auth_url_unset_passes_combination_check() {
        let a = parse(&["lvqr", "serve"]);
        assert!(a.webhook_auth_url.is_none());
        assert_eq!(a.webhook_auth_cache_ttl_seconds, 60);
        assert_eq!(a.webhook_auth_deny_cache_ttl_seconds, 10);
        assert_eq!(a.webhook_auth_fetch_timeout_seconds, 5);
        check_auth_flag_combinations(&a).expect("no flags should be fine");
    }

    #[test]
    fn webhook_auth_url_flag_parses() {
        let a = parse(&["lvqr", "serve", "--webhook-auth-url", "https://auth.example.com/check"]);
        assert_eq!(a.webhook_auth_url.as_deref(), Some("https://auth.example.com/check"));
        check_auth_flag_combinations(&a).expect("webhook alone is fine");
    }

    #[test]
    fn webhook_plus_jwt_secret_is_mutex_error() {
        let a = parse(&[
            "lvqr",
            "serve",
            "--webhook-auth-url",
            "https://auth.example.com/check",
            "--jwt-secret",
            "hunter2",
        ]);
        let err = check_auth_flag_combinations(&a).unwrap_err().to_string();
        assert!(err.contains("--webhook-auth-url"), "err: {err}");
        assert!(err.contains("--jwt-secret"), "err: {err}");
        assert!(err.contains("mutually exclusive"), "err: {err}");
    }

    #[cfg(feature = "jwks")]
    #[test]
    fn webhook_plus_jwks_is_mutex_error() {
        let a = parse(&[
            "lvqr",
            "serve",
            "--webhook-auth-url",
            "https://auth.example.com/check",
            "--jwks-url",
            "https://idp.example.com/jwks.json",
        ]);
        let err = check_auth_flag_combinations(&a).unwrap_err().to_string();
        assert!(err.contains("--jwks-url"), "err: {err}");
        assert!(err.contains("--webhook-auth-url"), "err: {err}");
        assert!(err.contains("mutually exclusive"), "err: {err}");
    }

    #[test]
    fn webhook_ttl_override_applies() {
        let a = parse(&[
            "lvqr",
            "serve",
            "--webhook-auth-url",
            "https://auth.example.com/check",
            "--webhook-auth-cache-ttl-seconds",
            "120",
            "--webhook-auth-deny-cache-ttl-seconds",
            "5",
            "--webhook-auth-fetch-timeout-seconds",
            "3",
        ]);
        assert_eq!(a.webhook_auth_cache_ttl_seconds, 120);
        assert_eq!(a.webhook_auth_deny_cache_ttl_seconds, 5);
        assert_eq!(a.webhook_auth_fetch_timeout_seconds, 3);
    }
}

#[cfg(test)]
mod wasm_filter_cli_tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> ServeArgs {
        match Cli::parse_from(args) {
            Cli::Serve(a) => a,
        }
    }

    #[test]
    fn wasm_filter_unset_is_empty_vec() {
        let a = parse(&["lvqr", "serve"]);
        assert!(a.wasm_filter.is_empty());
    }

    #[test]
    fn single_wasm_filter_preserves_legacy_shape() {
        let a = parse(&["lvqr", "serve", "--wasm-filter", "/tmp/a.wasm"]);
        assert_eq!(a.wasm_filter.len(), 1);
        assert_eq!(a.wasm_filter[0], std::path::PathBuf::from("/tmp/a.wasm"));
    }

    #[test]
    fn repeated_wasm_filter_flag_stacks_into_chain_order() {
        let a = parse(&[
            "lvqr",
            "serve",
            "--wasm-filter",
            "/tmp/a.wasm",
            "--wasm-filter",
            "/tmp/b.wasm",
            "--wasm-filter",
            "/tmp/c.wasm",
        ]);
        assert_eq!(
            a.wasm_filter,
            vec![
                std::path::PathBuf::from("/tmp/a.wasm"),
                std::path::PathBuf::from("/tmp/b.wasm"),
                std::path::PathBuf::from("/tmp/c.wasm"),
            ]
        );
    }

    #[test]
    fn comma_delimited_wasm_filter_also_stacks_into_chain() {
        // Matches the LVQR_WASM_FILTER=a.wasm,b.wasm env-var shape.
        let a = parse(&["lvqr", "serve", "--wasm-filter", "/tmp/a.wasm,/tmp/b.wasm"]);
        assert_eq!(
            a.wasm_filter,
            vec![
                std::path::PathBuf::from("/tmp/a.wasm"),
                std::path::PathBuf::from("/tmp/b.wasm"),
            ]
        );
    }
}

#[cfg(test)]
mod mesh_ice_servers_cli_tests {
    use super::*;

    #[test]
    fn unset_resolves_to_empty_vec() {
        let parsed = parse_mesh_ice_servers(None).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn empty_string_resolves_to_empty_vec() {
        // Whitespace-only payloads also resolve to empty so the env-var
        // path (LVQR_MESH_ICE_SERVERS="") matches the "unset" semantics.
        let parsed = parse_mesh_ice_servers(Some("   ")).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn parses_full_stun_plus_turn_payload() {
        let json = r#"[
            {"urls":["stun:stun.l.google.com:19302"]},
            {"urls":["turn:turn.example.com:3478"],"username":"u","credential":"p"}
        ]"#;
        let parsed = parse_mesh_ice_servers(Some(json)).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].urls, vec!["stun:stun.l.google.com:19302".to_string()]);
        assert!(parsed[0].username.is_none());
        assert!(parsed[0].credential.is_none());
        assert_eq!(parsed[1].urls, vec!["turn:turn.example.com:3478".to_string()]);
        assert_eq!(parsed[1].username.as_deref(), Some("u"));
        assert_eq!(parsed[1].credential.as_deref(), Some("p"));
    }

    #[test]
    fn malformed_json_surfaces_helpful_error() {
        let err = parse_mesh_ice_servers(Some("not json")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--mesh-ice-servers must be a JSON array"),
            "expected helpful error, got: {msg}"
        );
    }
}

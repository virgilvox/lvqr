"""Data types for the LVQR admin API.

Every dataclass mirrors a Rust serde struct on the server side. Field
names match the JSON-on-wire encoding exactly so ``json.loads(body)``
can be unpacked via ``**kwargs`` into the constructors. The mapping:

* :class:`RelayStats` mirrors ``lvqr_core::RelayStats``.
* :class:`StreamInfo` mirrors ``lvqr_admin::StreamInfo``.
* :class:`MeshState` mirrors ``lvqr_admin::MeshState``.
* :class:`MeshPeerStats` mirrors ``lvqr_admin::MeshPeerStats``.
* :class:`SloEntry` + :class:`SloSnapshot` mirror
  ``lvqr_admin::SloEntry`` + the ``json!({ "broadcasts": ... })``
  wrapper emitted by ``get_slo``.
* :class:`NodeCapacity` mirrors ``lvqr_cluster::NodeCapacity``.
* :class:`ClusterNodeView` mirrors ``lvqr_admin::cluster_routes::ClusterNodeView``.
* :class:`BroadcastSummary` mirrors ``lvqr_cluster::BroadcastSummary``.
* :class:`ConfigEntry` mirrors ``lvqr_cluster::ConfigEntry``.
* :class:`FederationLinkStatus` mirrors
  ``lvqr_cluster::FederationLinkStatus`` (with
  ``state`` as ``Literal["connecting", "connected", "failed"]``
  matching ``serde(rename_all = "lowercase")`` on the Rust enum).
* :class:`FederationStatus` mirrors
  ``lvqr_admin::cluster_routes::FederationStatusView``.
* :class:`WasmFilterBroadcastStats` mirrors
  ``lvqr_admin::WasmFilterBroadcastStats``.
* :class:`WasmFilterSlotStats` mirrors
  ``lvqr_admin::WasmFilterSlotStats``.
* :class:`WasmFilterState` mirrors ``lvqr_admin::WasmFilterState``.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Literal, Optional


@dataclass
class RelayStats:
    """Relay server statistics."""

    publishers: int = 0
    subscribers: int = 0
    tracks: int = 0
    bytes_received: int = 0
    bytes_sent: int = 0
    uptime_secs: int = 0


@dataclass
class StreamInfo:
    """Information about an active stream."""

    name: str
    subscribers: int = 0


@dataclass
class MeshPeerStats:
    """Per-peer offload stats surfaced by ``/api/v1/mesh``.

    ``intended_children`` is the topology planner's assignment;
    ``forwarded_frames`` is the cumulative count the peer reported
    via the ``/signal`` ``ForwardReport`` message. Session 141 --
    actual-vs-intended offload reporting.
    """

    peer_id: str = ""
    #: Tree role: ``"Root"``, ``"Relay"``, or ``"Leaf"``.
    role: str = "Leaf"
    #: Parent peer id, or ``None`` for roots.
    parent: Optional[str] = None
    depth: int = 0
    intended_children: int = 0
    forwarded_frames: int = 0
    #: Per-peer self-reported relay capacity (max children this peer
    #: is willing to serve), clamped at register time to the
    #: operator's global ``--max-peers``. ``None`` means the client
    #: did not advertise a value and the planner uses the global
    #: ceiling. Session 144 -- per-peer capacity advertisement.
    capacity: Optional[int] = None


@dataclass
class MeshState:
    """Current peer-mesh state from ``/api/v1/mesh``.

    ``peers`` carries per-peer intended-vs-actual offload stats.
    Added in session 141; older servers (pre-141) omit the field and
    :meth:`lvqr.LvqrClient.mesh` defensively defaults to an empty
    list so the dataclass construction does not break against a
    pre-141 deployment."""

    enabled: bool = False
    peer_count: int = 0
    #: Intended offload percentage (topology planner projection),
    #: not measured bandwidth savings. Compare against the per-peer
    #: ``forwarded_frames`` values in ``peers`` for the
    #: actual-vs-intended picture.
    offload_percentage: float = 0.0
    peers: list[MeshPeerStats] = field(default_factory=list)


@dataclass
class SloEntry:
    """One row from the ``/api/v1/slo`` response."""

    broadcast: str
    transport: str
    p50_ms: int = 0
    p95_ms: int = 0
    p99_ms: int = 0
    max_ms: int = 0
    sample_count: int = 0
    total_observed: int = 0


@dataclass
class SloSnapshot:
    """Outer shape of ``/api/v1/slo``. The wrapper exists so the
    response can grow sibling fields without a breaking schema
    change; the server emits ``{ "broadcasts": [...] }`` today."""

    broadcasts: list[SloEntry] = field(default_factory=list)


@dataclass
class NodeCapacity:
    """Resource capacity advertisement for one cluster node."""

    #: CPU utilization, 0.0 through 100.0, per-logical-core aggregate.
    cpu_pct: float = 0.0
    rss_bytes: int = 0
    bytes_out_per_sec: int = 0


@dataclass
class ClusterNodeView:
    """External-facing view of one cluster member."""

    id: str
    generation: int = 0
    #: Stringified gossip socket address (e.g. ``"10.0.0.1:10007"``).
    gossip_addr: str = ""
    #: Most-recent capacity advertisement, or ``None`` until the
    #: first gossip round lands.
    capacity: Optional[NodeCapacity] = None


@dataclass
class BroadcastSummary:
    """One broadcast's current owner per LWW tiebreak."""

    name: str
    owner: str = ""
    expires_at_ms: int = 0


@dataclass
class ConfigEntry:
    """One cluster-wide config entry."""

    key: str
    value: str = ""
    ts_ms: int = 0


#: Phase of one federation link. Matches ``serde(rename_all =
#: "lowercase")`` on the Rust enum.
FederationConnectState = Literal["connecting", "connected", "failed"]


@dataclass
class FederationLinkStatus:
    """External-facing status snapshot for one federation link."""

    remote_url: str
    forwarded_broadcasts: list[str] = field(default_factory=list)
    state: FederationConnectState = "connecting"
    last_connected_at_ms: Optional[int] = None
    last_error: Optional[str] = None
    connect_attempts: int = 0
    forwarded_broadcasts_seen: int = 0


@dataclass
class FederationStatus:
    """Outer shape of ``/api/v1/cluster/federation``. Empty
    ``links`` is returned both when federation is disabled and when
    no links are configured; the server collapses the distinction
    deliberately so tooling can poll unconditionally."""

    links: list[FederationLinkStatus] = field(default_factory=list)


@dataclass
class WasmFilterBroadcastStats:
    """Per-``(broadcast, track)`` WASM filter counters surfaced by
    ``/api/v1/wasm-filter``. Fields mirror the atomic counters that
    the filter bridge increments on every fragment that flows
    through the installed chain."""

    broadcast: str
    track: str
    #: Total fragments observed through the chain (kept + dropped).
    seen: int = 0
    #: Fragments the chain returned ``Some`` for (survived every
    #: slot).
    kept: int = 0
    #: Fragments a slot in the chain returned ``None`` for
    #: (short-circuit drop).
    dropped: int = 0


@dataclass
class WasmFilterSlotStats:
    """Per-slot WASM filter counters. ``index`` is the filter's
    position in the chain (0-based). Later slots in a chain report
    smaller ``seen`` counts when an earlier slot drops, because the
    chain short-circuits on the first ``None``. PLAN Phase D session
    140."""

    index: int = 0
    #: Fragments this slot observed (kept + dropped for this slot).
    seen: int = 0
    #: Fragments this slot returned ``Some`` for.
    kept: int = 0
    #: Fragments this slot returned ``None`` for (short-circuit drop).
    dropped: int = 0


@dataclass
class ConfigReloadStatus:
    """Wire shape for ``/api/v1/config-reload`` (session 147).

    Mirrors ``lvqr_admin::ConfigReloadStatus``. Every field is
    optional with a sensible default so dataclass construction stays
    sound even against a server that omits a field for forwards
    compat.
    """

    #: Resolved path of the ``--config`` file, or ``None`` when the
    #: server booted without ``--config``.
    config_path: Optional[str] = None
    #: Unix milliseconds at the most recent successful reload, or
    #: ``None`` until the first reload succeeds.
    last_reload_at_ms: Optional[int] = None
    #: ``"sighup"``, ``"admin_post"``, ``"boot"``, or ``None`` when
    #: no reload has occurred yet.
    last_reload_kind: Optional[str] = None
    #: Keys the most recent reload effectively re-applied. Currently
    #: always ``["auth"]`` on success; future increments add
    #: ``"mesh_ice"`` and ``"hmac_secret"``.
    applied_keys: list[str] = field(default_factory=list)
    #: Operator-facing warnings -- e.g. structural-key diffs that
    #: require a server restart.
    warnings: list[str] = field(default_factory=list)


@dataclass
class StreamKey:
    """One stream-key as the admin API returns it. Mirrors
    ``lvqr_auth::StreamKey`` (session 146). The ``token`` field
    carries the literal bearer credential a publisher uses;
    operators copy it from the mint response (or
    :meth:`lvqr.LvqrClient.list_streamkeys`) and pass it to
    whatever ingest tool will publish.

    Tokens are formatted as ``lvqr_sk_<43-char base64url-no-pad>``
    (32 bytes of CSPRNG output prefixed for secret-scanning
    recognisability).
    """

    id: str
    token: str
    #: Operator-friendly name. ``None`` when the mint did not set one.
    label: Optional[str] = None
    #: When set, the key only authorises publishes for this exact
    #: broadcast name. ``None`` accepts any broadcast.
    broadcast: Optional[str] = None
    #: Unix seconds at mint or rotate time.
    created_at: int = 0
    #: Unix seconds after which the key stops authenticating
    #: publishes. ``None`` means no expiry.
    expires_at: Optional[int] = None


@dataclass
class StreamKeySpec:
    """Mint / rotate request body. Mirrors
    ``lvqr_auth::StreamKeySpec`` (session 146).
    """

    label: Optional[str] = None
    broadcast: Optional[str] = None
    #: TTL in seconds. The server converts this to
    #: ``expires_at = now + ttl_seconds``. ``0`` or ``None`` means
    #: no expiry.
    ttl_seconds: Optional[int] = None


@dataclass
class WasmFilterState:
    """Outer shape of ``/api/v1/wasm-filter``. When
    ``--wasm-filter`` is unset the server returns
    ``{enabled=False, chain_length=0, broadcasts=[], slots=[]}``
    (200 OK, not 404) so dashboards can pre-bake the shape and
    poll unconditionally.

    ``slots`` was added in PLAN Phase D session 140; older servers
    (pre-140) omit the field. The Python client's ``wasm_filter()``
    defensively defaults it to an empty list so the dataclass
    construction does not break against a pre-140 deployment."""

    enabled: bool = False
    #: Number of filters composed into the installed chain.
    #: Constant for the server's lifetime.
    chain_length: int = 0
    broadcasts: list[WasmFilterBroadcastStats] = field(default_factory=list)
    #: Per-slot counters in insertion order. Contains
    #: ``chain_length`` entries when ``enabled`` is True; empty
    #: otherwise.
    slots: list[WasmFilterSlotStats] = field(default_factory=list)


# ---------------------------------------------------------------------------
# v1.1.0 -- console buildout wave types
# ---------------------------------------------------------------------------
# Every type below mirrors a Rust serde struct in `lvqr-admin` that
# landed in the 1.1.0 release. Field names match the JSON-on-wire encoding
# exactly so `json.loads(body)` can be unpacked via `**kwargs`.


@dataclass
class TrackInfo:
    """One track within a broadcast. Mirrors ``lvqr_admin::TrackInfo``."""

    #: Track name, e.g. ``"0.mp4"`` (video) or ``"1.mp4"`` (audio).
    track: str = ""
    #: ``"video"`` / ``"audio"`` / ``"data"``.
    kind: str = "data"
    #: Codec string, e.g. ``"avc1.640028"``, ``"mp4a.40.2"``, ``"Opus"``.
    codec: str = ""
    #: Track timescale (Hz). 90000 for H.264 video, 48000 for Opus, etc.
    timescale: int = 0
    #: Total fragments observed on this track so far.
    fragments: int = 0
    subscribers: int = 0
    #: Cumulative count of fragments skipped because a slow subscriber
    #: lagged past the broadcaster channel capacity.
    lagged_skips: int = 0


@dataclass
class StreamDetailInfo:
    """Per-broadcast detail returned by ``GET /api/v1/streams/{name}``.
    Mirrors ``lvqr_admin::StreamDetailInfo``. The endpoint returns
    ``None`` (HTTP 404) when no track for that broadcast is registered;
    :meth:`LvqrClient.stream_detail` returns ``None`` in that case."""

    name: str = ""
    subscribers: int = 0
    tracks: list[TrackInfo] = field(default_factory=list)


@dataclass
class RenditionInfo:
    """One configured transcode rendition. Mirrors
    ``lvqr_admin::RenditionInfo``."""

    name: str = ""
    width: int = 0
    height: int = 0
    video_bitrate_kbps: int = 0
    audio_bitrate_kbps: int = 0


@dataclass
class TranscodeActiveStats:
    """Live per-input transcoder stats. Mirrors
    ``lvqr_admin::TranscodeActiveStats``."""

    broadcast: str = ""
    track: str = ""
    rendition: str = ""
    fragments_in: int = 0
    fragments_out: int = 0
    panics: int = 0


@dataclass
class TranscodeState:
    """Outer shape of ``GET /api/v1/transcode/ladders``. Mirrors
    ``lvqr_admin::TranscodeState``. ``enabled`` is ``False`` when the
    relay was built without the ``transcode`` feature OR no rendition
    was configured at startup."""

    enabled: bool = False
    #: Encoder backend label: ``"software"`` / ``"videotoolbox"`` /
    #: ``"nvenc"`` / ``"vaapi"`` / ``"qsv"``.
    encoder: str = ""
    renditions: list[RenditionInfo] = field(default_factory=list)
    active: list[TranscodeActiveStats] = field(default_factory=list)


@dataclass
class AddRenditionRequest:
    """Body for ``POST /api/v1/transcode/ladders``. Mirrors
    ``lvqr_admin::RenditionInfo`` (the route accepts the same shape it
    returns from GET).

    Validation is server-side; the admin route rejects 400 on negative
    bitrates or zero width/height."""

    name: str
    width: int
    height: int
    video_bitrate_kbps: int
    audio_bitrate_kbps: int


@dataclass
class AgentInfo:
    """Configured in-process agent. Mirrors ``lvqr_admin::AgentInfo``."""

    name: str = ""
    #: Display grouping, e.g. ``"captions"``.
    kind: str = ""
    #: Agent-specific config (set for the Whisper captions agent).
    model: Optional[str] = None
    window_ms: Optional[int] = None


@dataclass
class AgentActiveStats:
    """Per-(agent, broadcast, track) live counters. Mirrors
    ``lvqr_admin::AgentActiveStats``. ``panics`` non-zero flags an
    unhealthy agent."""

    agent: str = ""
    broadcast: str = ""
    track: str = ""
    fragments_seen: int = 0
    panics: int = 0


@dataclass
class AgentState:
    """Outer shape of ``GET /api/v1/agents``. Mirrors
    ``lvqr_admin::AgentState``. ``enabled`` is ``True`` only when the
    binary was built with an agent feature (e.g. ``whisper``) AND at
    least one agent was configured at startup; runtime-added agents do
    not flip this flag retroactively (the wider relay knows."""

    enabled: bool = False
    agents: list[AgentInfo] = field(default_factory=list)
    active: list[AgentActiveStats] = field(default_factory=list)


@dataclass
class AddAgentRequest:
    """Body for ``POST /api/v1/agents``. Mirrors
    ``lvqr_admin::AddAgentRequest``. ``model`` must be a non-empty file
    path on the relay's filesystem (the admin route returns 400 on
    empty)."""

    model: str
    window_ms: Optional[int] = None


@dataclass
class ArchiveTrackInfo:
    """One recorded track within a broadcast. Mirrors
    ``lvqr_admin::ArchiveTrackInfo``. ``duration_secs`` is the recorded
    decode span (``(last_end - first_start) / timescale``)."""

    track: str = ""
    segment_count: int = 0
    total_bytes: int = 0
    duration_secs: float = 0.0
    timescale: int = 0


@dataclass
class ArchiveBroadcastInfo:
    """One recorded broadcast: its tracks plus aggregates. Mirrors
    ``lvqr_admin::ArchiveBroadcastInfo``."""

    broadcast: str = ""
    segment_count: int = 0
    total_bytes: int = 0
    #: Longest track's recorded decode span.
    duration_secs: float = 0.0
    tracks: list[ArchiveTrackInfo] = field(default_factory=list)


@dataclass
class ArchiveState:
    """Outer shape of ``GET /api/v1/archive``. Mirrors
    ``lvqr_admin::ArchiveState``. ``enabled`` is ``True`` when the
    relay was booted with ``--archive-dir``."""

    enabled: bool = False
    recordings: list[ArchiveBroadcastInfo] = field(default_factory=list)


@dataclass
class IngestListenerInfo:
    """One bound ingest listener. Mirrors
    ``lvqr_admin::IngestListenerInfo``. One entry per protocol the relay
    actually bound at startup (RTMP / WHIP / SRT / RTSP). ``enabled`` is
    ``True`` until the operator stops it via
    ``DELETE /api/v1/ingest/{protocol}``; STOP is one-way until the
    relay restarts."""

    protocol: str = ""
    addr: str = ""
    enabled: bool = True


@dataclass
class IngestState:
    """Ingest-listener inventory from ``GET /api/v1/ingest``. Mirrors
    ``lvqr_admin::IngestState``."""

    listeners: list[IngestListenerInfo] = field(default_factory=list)


@dataclass
class IngestStopResult:
    """Result of ``DELETE /api/v1/ingest/{protocol}``. Mirrors
    ``lvqr_admin::IngestStopResult``.

    ``result`` is one of ``"stopped"`` / ``"already_stopped"`` /
    ``"not_found"`` (the last one is also surfaced as HTTP 404 by the
    server; the client raises in that case rather than returning this
    variant). Idempotent: ``stopped`` and ``already_stopped`` both
    indicate the listener is now down."""

    result: Literal["stopped", "already_stopped", "not_found"] = "stopped"
    protocol: Optional[str] = None


@dataclass
class BroadcastSessionInfo:
    """One live publisher session. Mirrors
    ``lvqr_admin::BroadcastSessionInfo``."""

    #: Broadcast name the publisher claimed (``<app>/<key>`` for RTMP,
    #: URL path for WHIP, StreamId broadcast field for SRT, ANNOUNCE
    #: path for RTSP).
    broadcast: str = ""
    #: Lower-case protocol tag (``"rtmp"`` / ``"whip"`` / ``"srt"`` /
    #: ``"rtsp"``).
    protocol: str = ""
    #: Session start time, in ms since the Unix epoch.
    started_ms: int = 0
    #: Publisher peer address when the ingest crate captured it at accept
    #: time; ``None`` for WHIP today (UDP source addr is observed later
    #: in the str0m poll loop, not at session creation).
    peer: Optional[str] = None


@dataclass
class BroadcastSessionsState:
    """Outer shape of ``GET /api/v1/broadcasts``. Mirrors
    ``lvqr_admin::BroadcastSessionsState``. Only protocols whose ingest
    crate has wired its session registrar surface here (today RTMP /
    WHIP / SRT / RTSP all wired)."""

    sessions: list[BroadcastSessionInfo] = field(default_factory=list)


@dataclass
class BroadcastStopResult:
    """Result of ``DELETE /api/v1/broadcasts/{name}``. Mirrors
    ``lvqr_admin::BroadcastStopResult``. ``killed`` -> HTTP 200;
    ``not_found`` -> HTTP 404 (client raises). Repeat against an
    already-killed broadcast 404s because the session deregistered on
    the previous teardown."""

    result: Literal["killed", "not_found"] = "killed"
    broadcast: Optional[str] = None
    protocol: Optional[str] = None


@dataclass
class BoundAddresses:
    """Listener bind addresses captured at startup. Mirrors
    ``lvqr_admin::BoundAddresses``. ``None`` for any protocol that was
    not enabled at startup (e.g. ``srt = None`` if the relay was started
    without ``--srt-port``)."""

    admin: Optional[str] = None
    rtmp: Optional[str] = None
    whip: Optional[str] = None
    whep: Optional[str] = None
    hls: Optional[str] = None
    dash: Optional[str] = None
    srt: Optional[str] = None
    rtsp: Optional[str] = None
    moq: Optional[str] = None
    signal: Optional[str] = None


@dataclass
class RuntimeFeatures:
    """Runtime feature snapshot from ``GET /api/v1/server-info``. Mirrors
    ``lvqr_admin::RuntimeFeatures``."""

    mesh_enabled: bool = False
    cluster_enabled: bool = False
    archive_dir: Optional[str] = None
    record_dir: Optional[str] = None
    wasm_filter_chain_length: int = 0
    #: ``"noop"`` / ``"static"`` / ``"jwt"`` / ``"jwks"`` / ``"webhook"``.
    auth_mode: str = "noop"
    hmac_playback_secret_configured: bool = False
    stream_keys_enabled: bool = True


@dataclass
class ServerInfo:
    """Outer shape of ``GET /api/v1/server-info``. Mirrors
    ``lvqr_admin::server_info_routes::ServerInfo``. Snapshots the relay's
    `ServeConfig` + bound addresses + cargo crate version + uptime."""

    version: str = ""
    #: Cargo features the binary was built with: subset of ``rtmp`` /
    #: ``transcode`` / ``whisper`` / ``c2pa`` / ``jwks`` / ``webhook`` /
    #: ``cluster`` / ``aac-opus`` / ``io-uring``.
    build_features: list[str] = field(default_factory=list)
    uptime_secs: int = 0
    bound: BoundAddresses = field(default_factory=BoundAddresses)
    features: RuntimeFeatures = field(default_factory=RuntimeFeatures)
    config_path: Optional[str] = None
    wasm_filter_paths: list[str] = field(default_factory=list)


@dataclass
class LogLine:
    """One captured log line streamed by ``GET /api/v1/logs``. Mirrors
    ``lvqr_observability::LogLine``. Streamed as ``data: <json>\\n\\n``
    SSE events; consumers should parse each ``data:`` payload as a single
    ``LogLine`` JSON object."""

    #: Capture time in ms since the Unix epoch.
    ts_ms: int = 0
    #: ``"ERROR"`` / ``"WARN"`` / ``"INFO"`` / ``"DEBUG"`` / ``"TRACE"``.
    level: str = "INFO"
    #: Event target (usually the emitting module path).
    target: str = ""
    #: Rendered message plus any structured fields.
    message: str = ""

"""LVQR admin API client.

Covers every route the admin router mounts today:
``/healthz``, ``/api/v1/{stats,streams,mesh,slo,wasm-filter}``, and the
cluster-gated ``/api/v1/cluster/{nodes,broadcasts,config,federation}``.
The surface mirrors the TypeScript ``@lvqr/core`` ``LvqrAdminClient``
1:1 so operator tooling in either language lines up.
"""

from __future__ import annotations

from typing import Optional

import httpx

from .types import (
    # v1.0.0 types
    BroadcastSummary,
    ClusterNodeView,
    ConfigEntry,
    ConfigReloadStatus,
    FederationLinkStatus,
    FederationStatus,
    MeshPeerStats,
    MeshState,
    NodeCapacity,
    RelayStats,
    SloEntry,
    SloSnapshot,
    StreamInfo,
    StreamKey,
    StreamKeySpec,
    WasmFilterBroadcastStats,
    WasmFilterSlotStats,
    WasmFilterState,
    # v1.1.0 console wave types (slices 1-9b)
    AddAgentRequest,
    AddRenditionRequest,
    AgentActiveStats,
    AgentInfo,
    AgentState,
    ArchiveBroadcastInfo,
    ArchiveState,
    ArchiveTrackInfo,
    BoundAddresses,
    BroadcastSessionInfo,
    BroadcastSessionsState,
    BroadcastStopResult,
    IngestListenerInfo,
    IngestState,
    IngestStopResult,
    LogLine,
    RenditionInfo,
    RuntimeFeatures,
    ServerInfo,
    StreamDetailInfo,
    TrackInfo,
    TranscodeActiveStats,
    TranscodeState,
)


class LvqrClient:
    """Client for the LVQR admin HTTP API.

    Args:
        base_url: Base URL of the LVQR admin server
            (e.g., ``"http://localhost:8080"``).
        timeout: Request timeout in seconds. Applied to every
            call via ``httpx.Client(timeout=...)``.
        bearer_token: Optional bearer token. When set, every admin
            call sends ``Authorization: Bearer <token>``. Required
            when the server was booted with ``--admin-token`` or a
            JWT provider; Noop-provider deployments can leave it
            unset.

    Example::

        with LvqrClient("http://localhost:8080", bearer_token="s3cr3t") as client:
            if client.healthz():
                stats = client.stats()
                print(f"Tracks: {stats.tracks}, Subscribers: {stats.subscribers}")
                mesh = client.mesh()
                if mesh.enabled:
                    print(f"Mesh: {mesh.peer_count} peer(s)")
                for node in client.cluster_nodes():
                    print(f"Node {node.id} on {node.gossip_addr}")
    """

    def __init__(
        self,
        base_url: str,
        timeout: float = 10.0,
        bearer_token: Optional[str] = None,
    ):
        self.base_url = base_url.rstrip("/")
        headers: dict[str, str] = {}
        if bearer_token:
            headers["Authorization"] = f"Bearer {bearer_token}"
        self._client = httpx.Client(
            base_url=self.base_url,
            timeout=timeout,
            headers=headers,
        )

    def close(self) -> None:
        """Close the HTTP client."""
        self._client.close()

    def __enter__(self) -> LvqrClient:
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    # -----------------------------------------------------------------
    # Probes.
    # -----------------------------------------------------------------

    def healthz(self) -> bool:
        """Check if the relay is healthy.

        Returns:
            True if the server responds with 200 OK. False on any
            non-2xx or network error (the latter is swallowed so
            operators can call ``healthz`` as a simple reachability
            probe without wrapping it in try/except).
        """
        try:
            resp = self._client.get("/healthz")
            return resp.status_code == 200
        except httpx.HTTPError:
            return False

    # -----------------------------------------------------------------
    # Core admin routes (always mounted).
    # -----------------------------------------------------------------

    def stats(self) -> RelayStats:
        """``GET /api/v1/stats`` -- aggregate relay statistics."""
        data = self._get_json("/api/v1/stats")
        return RelayStats(
            publishers=data.get("publishers", 0),
            subscribers=data.get("subscribers", 0),
            tracks=data.get("tracks", 0),
            bytes_received=data.get("bytes_received", 0),
            bytes_sent=data.get("bytes_sent", 0),
            uptime_secs=data.get("uptime_secs", 0),
        )

    def list_streams(self) -> list[StreamInfo]:
        """``GET /api/v1/streams`` -- list of active broadcasts."""
        data = self._get_json("/api/v1/streams")
        return [
            StreamInfo(
                name=s.get("name", ""),
                subscribers=s.get("subscribers", 0),
            )
            for s in data
        ]

    def mesh(self) -> MeshState:
        """``GET /api/v1/mesh`` -- current peer-mesh state.

        The ``peers`` array was added in session 141 for
        actual-vs-intended offload reporting; pre-141 servers omit
        the field and the defensive ``.get("peers", [])`` fallback
        keeps parsing sound against older deployments. Session 144
        added ``MeshPeerStats.capacity``; pre-144 servers omit the
        per-peer field and the ``.get("capacity")`` lookup returns
        ``None``.
        """
        data = self._get_json("/api/v1/mesh")
        peers = [
            MeshPeerStats(
                peer_id=p.get("peer_id", ""),
                role=p.get("role", "Leaf"),
                parent=p.get("parent"),
                depth=int(p.get("depth", 0)),
                intended_children=int(p.get("intended_children", 0)),
                forwarded_frames=int(p.get("forwarded_frames", 0)),
                capacity=p.get("capacity"),
            )
            for p in data.get("peers", [])
        ]
        return MeshState(
            enabled=bool(data.get("enabled", False)),
            peer_count=data.get("peer_count", 0),
            offload_percentage=float(data.get("offload_percentage", 0.0)),
            peers=peers,
        )

    def slo(self) -> SloSnapshot:
        """``GET /api/v1/slo`` -- per-broadcast + per-transport
        latency snapshot. The response wraps the entries in an
        object so callers can distinguish "no tracker wired"
        (``broadcasts == []``) from "tracker configured but no
        samples" (also ``[]``, but the route still returns 200).
        """
        data = self._get_json("/api/v1/slo")
        entries = [
            SloEntry(
                broadcast=e.get("broadcast", ""),
                transport=e.get("transport", ""),
                p50_ms=e.get("p50_ms", 0),
                p95_ms=e.get("p95_ms", 0),
                p99_ms=e.get("p99_ms", 0),
                max_ms=e.get("max_ms", 0),
                sample_count=e.get("sample_count", 0),
                total_observed=e.get("total_observed", 0),
            )
            for e in data.get("broadcasts", [])
        ]
        return SloSnapshot(broadcasts=entries)

    # -----------------------------------------------------------------
    # Cluster-gated admin routes. These require the server to be
    # built with ``--features cluster`` (on by default) and
    # ``--cluster-listen`` to be set. A missing cluster handle yields
    # an HTTP 500 the caller surfaces via httpx.
    # -----------------------------------------------------------------

    def cluster_nodes(self) -> list[ClusterNodeView]:
        """``GET /api/v1/cluster/nodes`` -- live cluster members."""
        data = self._get_json("/api/v1/cluster/nodes")
        return [_cluster_node_from_json(entry) for entry in data]

    def cluster_broadcasts(self) -> list[BroadcastSummary]:
        """``GET /api/v1/cluster/broadcasts`` -- active broadcast leases."""
        data = self._get_json("/api/v1/cluster/broadcasts")
        return [
            BroadcastSummary(
                name=e.get("name", ""),
                owner=e.get("owner", ""),
                expires_at_ms=e.get("expires_at_ms", 0),
            )
            for e in data
        ]

    def cluster_config(self) -> list[ConfigEntry]:
        """``GET /api/v1/cluster/config`` -- cluster-wide LWW config entries."""
        data = self._get_json("/api/v1/cluster/config")
        return [
            ConfigEntry(
                key=e.get("key", ""),
                value=e.get("value", ""),
                ts_ms=e.get("ts_ms", 0),
            )
            for e in data
        ]

    def cluster_federation(self) -> FederationStatus:
        """``GET /api/v1/cluster/federation`` -- status of every
        configured federation link. Returns ``FederationStatus(links=[])``
        both when federation is disabled and when no links are
        configured; the server collapses the distinction deliberately
        so tooling can poll unconditionally."""
        data = self._get_json("/api/v1/cluster/federation")
        links = [
            FederationLinkStatus(
                remote_url=l.get("remote_url", ""),
                forwarded_broadcasts=list(l.get("forwarded_broadcasts", [])),
                state=l.get("state", "connecting"),
                last_connected_at_ms=l.get("last_connected_at_ms"),
                last_error=l.get("last_error"),
                connect_attempts=l.get("connect_attempts", 0),
                forwarded_broadcasts_seen=l.get("forwarded_broadcasts_seen", 0),
            )
            for l in data.get("links", [])
        ]
        return FederationStatus(links=links)

    # -----------------------------------------------------------------
    # Stream-key CRUD admin API (session 146).
    # -----------------------------------------------------------------

    def list_streamkeys(self) -> list[StreamKey]:
        """``GET /api/v1/streamkeys`` -- every stream-key currently
        in the runtime store, including expired entries (operators
        can see what is stale and call :meth:`revoke_streamkey`).

        Returns an empty list when the server booted with
        ``--no-streamkeys`` so polling tooling can run
        unconditionally.
        """
        data = self._get_json("/api/v1/streamkeys")
        return [_streamkey_from_json(k) for k in data.get("keys", [])]

    def mint_streamkey(self, spec: Optional[StreamKeySpec] = None) -> StreamKey:
        """``POST /api/v1/streamkeys`` -- mint a new stream-key.

        Server fills ``id``, ``token``, ``created_at``, and
        ``expires_at`` (from ``ttl_seconds``). Returns the full
        :class:`StreamKey` including the literal bearer token.
        """
        body = _streamkeyspec_to_json(spec)
        resp = self._client.post("/api/v1/streamkeys", json=body)
        resp.raise_for_status()
        return _streamkey_from_json(resp.json())

    def revoke_streamkey(self, id: str) -> None:
        """``DELETE /api/v1/streamkeys/{id}`` -- hard-delete by id.

        Raises :class:`httpx.HTTPStatusError` on 404 (unknown id)
        or any other non-2xx. Idempotent callers can swallow the
        404 to treat "already gone" as success.
        """
        resp = self._client.delete(f"/api/v1/streamkeys/{id}")
        resp.raise_for_status()

    def rotate_streamkey(self, id: str, override: Optional[StreamKeySpec] = None) -> StreamKey:
        """``POST /api/v1/streamkeys/{id}/rotate`` -- swap the
        token while preserving the stable ``id``.

        With ``override`` unset the existing ``label`` /
        ``broadcast`` / ``expires_at`` are preserved; passing an
        override re-scopes the key while rotating (a ``None``
        field on the override CLEARS the existing field).
        """
        url = f"/api/v1/streamkeys/{id}/rotate"
        if override is None:
            # Empty body: rotate handler treats it as "preserve scope".
            resp = self._client.post(url)
        else:
            resp = self._client.post(url, json=_streamkeyspec_to_json(override))
        resp.raise_for_status()
        return _streamkey_from_json(resp.json())

    # -----------------------------------------------------------------
    # Hot config reload (session 147).
    # -----------------------------------------------------------------

    def config_reload_status(self) -> ConfigReloadStatus:
        """``GET /api/v1/config-reload`` -- status of the
        configured ``--config`` file. Returns a default-shaped body
        (every Optional unset) when the server booted without
        ``--config``."""
        data = self._get_json("/api/v1/config-reload")
        return _config_reload_from_json(data)

    def trigger_config_reload(self) -> ConfigReloadStatus:
        """``POST /api/v1/config-reload`` -- trigger a reload of
        the configured ``--config`` file.

        Raises :class:`httpx.HTTPStatusError` on 503 (no
        ``--config`` was passed at boot) or 500 (file malformed /
        rebuild failed; the prior provider stays live)."""
        resp = self._client.post("/api/v1/config-reload")
        resp.raise_for_status()
        return _config_reload_from_json(resp.json())

    def wasm_filter(self) -> WasmFilterState:
        """``GET /api/v1/wasm-filter`` -- configured WASM filter chain
        shape + per-``(broadcast, track)`` counters. Returns
        ``WasmFilterState(enabled=False, chain_length=0, broadcasts=[])``
        when ``--wasm-filter`` is unset; tooling can poll
        unconditionally without a 404 handler."""
        data = self._get_json("/api/v1/wasm-filter")
        broadcasts = [
            WasmFilterBroadcastStats(
                broadcast=b.get("broadcast", ""),
                track=b.get("track", ""),
                seen=b.get("seen", 0),
                kept=b.get("kept", 0),
                dropped=b.get("dropped", 0),
            )
            for b in data.get("broadcasts", [])
        ]
        # `slots` was added in PLAN Phase D session 140; pre-140
        # servers omit it. `.get("slots", [])` keeps parsing sound
        # against older deployments.
        slots = [
            WasmFilterSlotStats(
                index=int(s.get("index", 0)),
                seen=int(s.get("seen", 0)),
                kept=int(s.get("kept", 0)),
                dropped=int(s.get("dropped", 0)),
            )
            for s in data.get("slots", [])
        ]
        return WasmFilterState(
            enabled=bool(data.get("enabled", False)),
            chain_length=int(data.get("chain_length", 0)),
            broadcasts=broadcasts,
            slots=slots,
        )

    # -----------------------------------------------------------------
    # Shared GET helper. Applies the bearer header (via httpx default
    # headers set in __init__) + raises on any non-2xx so callers
    # get an httpx.HTTPStatusError they can catch on auth failure.
    # -----------------------------------------------------------------


    # =================================================================
    # v1.1.0 -- console buildout wave: read-only introspection
    # =================================================================

    def stream_detail(self, name: str) -> Optional["StreamDetailInfo"]:
        """``GET /api/v1/streams/{name}`` -- per-broadcast detail (tracks
        + codec + timescale + fragment counters + subscriber count +
        lagged-skip count).

        Returns ``None`` when the broadcast has no registered tracks
        (the server returns 404 in that case; this method maps it to
        ``None`` so callers can poll a name without 404-handling).

        Other non-2xx statuses still raise ``httpx.HTTPStatusError``.
        Slice 1 of the v1.1.0 console wave."""
        from urllib.parse import quote
        resp = self._client.get(f"/api/v1/streams/{quote(name, safe='')}")
        if resp.status_code == 404:
            return None
        resp.raise_for_status()
        data = resp.json()
        return _stream_detail_from_json(data)

    def transcode_ladders(self) -> "TranscodeState":
        """``GET /api/v1/transcode/ladders`` -- configured renditions
        plus live per-input transcoder stats.

        Always returns 200; ``enabled=False`` when the relay was built
        without the ``transcode`` feature OR no rendition was configured
        at startup (older deployments + Tier 4 transcode-disabled
        builds). Slice 2 + 7 of the v1.1.0 console wave."""
        data = self._get_json("/api/v1/transcode/ladders")
        return _transcode_state_from_json(data)

    def add_rendition(self, req: "AddRenditionRequest") -> None:
        """``POST /api/v1/transcode/ladders`` -- add a transcode
        rendition at runtime against new and already-live broadcasts.

        Requires a ladder configured at startup (the runner only
        installs when ``config.transcode_renditions`` is non-empty).
        Raises ``httpx.HTTPStatusError`` on 503 (no runner), 400
        (invalid request), or 409 (a rendition with that name already
        exists). Slice 7."""
        body = {
            "name": req.name,
            "width": req.width,
            "height": req.height,
            "video_bitrate_kbps": req.video_bitrate_kbps,
            "audio_bitrate_kbps": req.audio_bitrate_kbps,
        }
        resp = self._client.post("/api/v1/transcode/ladders", json=body)
        resp.raise_for_status()

    def remove_rendition(self, name: str) -> None:
        """``DELETE /api/v1/transcode/ladders/{name}`` -- stop a
        rendition at runtime. Drops all live drain tasks for that
        rendition; future broadcasts won't get it.

        Raises ``httpx.HTTPStatusError`` on 503 (no runner) or 404 (no
        rendition with that name). Slice 7."""
        from urllib.parse import quote
        resp = self._client.delete(f"/api/v1/transcode/ladders/{quote(name, safe='')}")
        resp.raise_for_status()

    def agents(self) -> "AgentState":
        """``GET /api/v1/agents`` -- configured in-process agents plus
        live per-attachment counters.

        Always returns 200; ``enabled=False`` when no agent is
        configured. Slice 3 + 8 of the v1.1.0 console wave."""
        data = self._get_json("/api/v1/agents")
        return _agent_state_from_json(data)

    def add_agent(self, req: "AddAgentRequest") -> None:
        """``POST /api/v1/agents`` -- start an in-process agent at
        runtime. Body is an :class:`AddAgentRequest`. Whisper agent
        runner installs even without a startup model so agents can be
        started from zero.

        Raises ``httpx.HTTPStatusError`` on 503 (no agent feature / no
        runner), 400 (model path empty), or 409 (agent of that name
        already running). Slice 8."""
        body: dict[str, object] = {"model": req.model}
        if req.window_ms is not None:
            body["window_ms"] = req.window_ms
        resp = self._client.post("/api/v1/agents", json=body)
        resp.raise_for_status()

    def remove_agent(self, name: str) -> None:
        """``DELETE /api/v1/agents/{name}`` -- stop a running agent.

        Raises ``httpx.HTTPStatusError`` on 503 (no runner) or 404 (no
        agent with that name). Slice 8."""
        from urllib.parse import quote
        resp = self._client.delete(f"/api/v1/agents/{quote(name, safe='')}")
        resp.raise_for_status()

    def archive(self) -> "ArchiveState":
        """``GET /api/v1/archive`` -- recorded broadcasts from the DVR
        segment index.

        Always returns 200; ``enabled=False`` (empty list) when the
        relay was booted without ``--archive-dir``. Slice 4."""
        data = self._get_json("/api/v1/archive")
        return _archive_state_from_json(data)

    # =================================================================
    # v1.1.0 -- runtime mutation (Slices 6 + 9b)
    # =================================================================

    def ingest_listeners(self) -> "IngestState":
        """``GET /api/v1/ingest`` -- inventory of bound ingest listeners
        (RTMP / WHIP / SRT / RTSP) with their live ``enabled`` state.

        Always returns 200; an empty list means the CLI composition
        root did not wire the registry (embedded tests or a build with
        no ingest features). Slice 9b."""
        data = self._get_json("/api/v1/ingest")
        listeners = [
            IngestListenerInfo(
                protocol=l.get("protocol", ""),
                addr=l.get("addr", ""),
                enabled=bool(l.get("enabled", True)),
            )
            for l in data.get("listeners", [])
        ]
        return IngestState(listeners=listeners)

    def stop_ingest_listener(self, protocol: str) -> "IngestStopResult":
        """``DELETE /api/v1/ingest/{protocol}`` -- stop a bound ingest
        listener at runtime. The kernel socket is unbound; existing
        publishers stay until their own teardown, but no NEW connections
        are accepted. STOP is one-way: re-enabling requires a relay
        restart.

        Idempotent: a repeat call against an already-stopped protocol
        resolves with ``result="already_stopped"``.

        Raises ``httpx.HTTPStatusError`` on 404 (unknown protocol) or
        503 (registry not wired). Slice 9b."""
        from urllib.parse import quote
        resp = self._client.delete(f"/api/v1/ingest/{quote(protocol, safe='')}")
        resp.raise_for_status()
        return _ingest_stop_from_json(resp.json())

    def broadcasts(self) -> "BroadcastSessionsState":
        """``GET /api/v1/broadcasts`` -- live publisher sessions (one
        row per active publisher across every ingest protocol whose
        ingest crate has wired its session registrar).

        Always returns 200; empty list when no live sessions exist.
        Slice 6."""
        data = self._get_json("/api/v1/broadcasts")
        sessions = [
            BroadcastSessionInfo(
                broadcast=s.get("broadcast", ""),
                protocol=s.get("protocol", ""),
                started_ms=int(s.get("started_ms", 0)),
                peer=s.get("peer"),
            )
            for s in data.get("sessions", [])
        ]
        return BroadcastSessionsState(sessions=sessions)

    def stop_broadcast(self, name: str) -> "BroadcastStopResult":
        """``DELETE /api/v1/broadcasts/{name}`` -- truly disconnect the
        live publisher session for the named broadcast. The ingest
        crate's per-session cancel token fires, the read loop drops out
        of its ``select!``, the publisher's transport socket is closed,
        and the bridge drains the egress state. Subscribers see
        end-of-stream.

        The publisher can reconnect immediately as a new session.

        Raises ``httpx.HTTPStatusError`` on 404 (no live session for
        that broadcast -- also covers the idempotent repeat case) or
        503 (registry not wired). Slice 6."""
        from urllib.parse import quote
        resp = self._client.delete(f"/api/v1/broadcasts/{quote(name, safe='')}")
        resp.raise_for_status()
        return _broadcast_stop_from_json(resp.json())

    def server_info(self) -> "ServerInfo":
        """``GET /api/v1/server-info`` -- relay version + uptime + bound
        listener addresses + runtime feature snapshot.

        Always returns 200. Useful for dashboards (version banner,
        feature-gated UI affordances)."""
        data = self._get_json("/api/v1/server-info")
        return _server_info_from_json(data)

    def logs_stream_url(self) -> str:
        """Absolute URL for the ``GET /api/v1/logs`` Server-Sent Events
        live-tail stream.

        Because the browser ``EventSource`` API cannot set an
        ``Authorization`` header, the bearer token is appended as a
        ``?token=`` query param. **Note:** tokens in URLs can be captured
        by proxy / access logs -- prefer a short-lived admin token for
        log streaming. Slice 5."""
        from urllib.parse import quote
        base = f"{self.base_url}/api/v1/logs"
        # Pull token out of the httpx client's default headers (set in
        # __init__ from the bearer_token kwarg).
        auth = self._client.headers.get("Authorization", "")
        if auth.startswith("Bearer "):
            token = auth[len("Bearer "):]
            return f"{base}?token={quote(token, safe='')}"
        return base

    def _get_json(self, path: str) -> object:
        resp = self._client.get(path)
        resp.raise_for_status()
        return resp.json()


def _config_reload_from_json(entry: dict) -> ConfigReloadStatus:
    """Build a :class:`ConfigReloadStatus` from a JSON dict.
    Defensive ``.get(...)`` so unknown server bodies never crash
    the dataclass construction (forwards-compat with pre-N
    servers + post-N additions)."""
    return ConfigReloadStatus(
        config_path=entry.get("config_path"),
        last_reload_at_ms=entry.get("last_reload_at_ms"),
        last_reload_kind=entry.get("last_reload_kind"),
        applied_keys=list(entry.get("applied_keys", [])),
        warnings=list(entry.get("warnings", [])),
    )


def _streamkey_from_json(entry: dict) -> StreamKey:
    """Build a :class:`StreamKey` from a JSON dict. Defensive
    ``.get(...)`` parsers tolerate a server that omits any future
    optional field (``#[serde(default)]`` mirroring on the wire)."""
    return StreamKey(
        id=entry.get("id", ""),
        token=entry.get("token", ""),
        label=entry.get("label"),
        broadcast=entry.get("broadcast"),
        created_at=int(entry.get("created_at", 0)),
        expires_at=entry.get("expires_at"),
    )


def _streamkeyspec_to_json(spec: Optional[StreamKeySpec]) -> dict:
    """Convert a :class:`StreamKeySpec` to the JSON object shape
    the admin route expects. ``None`` becomes an empty object
    (matches the server's "default spec" semantics)."""
    if spec is None:
        return {}
    return {
        "label": spec.label,
        "broadcast": spec.broadcast,
        "ttl_seconds": spec.ttl_seconds,
    }


def _cluster_node_from_json(entry: dict) -> ClusterNodeView:
    """Build a :class:`ClusterNodeView` from a JSON dict. Handles
    the optional ``capacity`` sub-object, which the server emits as
    ``null`` until the first gossip round advertises it."""
    cap_raw = entry.get("capacity")
    capacity: Optional[NodeCapacity]
    if cap_raw is None:
        capacity = None
    else:
        capacity = NodeCapacity(
            cpu_pct=float(cap_raw.get("cpu_pct", 0.0)),
            rss_bytes=cap_raw.get("rss_bytes", 0),
            bytes_out_per_sec=cap_raw.get("bytes_out_per_sec", 0),
        )
    return ClusterNodeView(
        id=entry.get("id", ""),
        generation=entry.get("generation", 0),
        gossip_addr=entry.get("gossip_addr", ""),
        capacity=capacity,
    )

# =====================================================================
# v1.1.0 from-JSON helpers (mirror the existing _xxx_from_json pattern)
# =====================================================================


def _track_info_from_json(entry: dict) -> "TrackInfo":
    return TrackInfo(
        track=entry.get("track", ""),
        kind=entry.get("kind", "data"),
        codec=entry.get("codec", ""),
        timescale=int(entry.get("timescale", 0)),
        fragments=int(entry.get("fragments", 0)),
        subscribers=int(entry.get("subscribers", 0)),
        lagged_skips=int(entry.get("lagged_skips", 0)),
    )


def _stream_detail_from_json(entry: dict) -> "StreamDetailInfo":
    return StreamDetailInfo(
        name=entry.get("name", ""),
        subscribers=int(entry.get("subscribers", 0)),
        tracks=[_track_info_from_json(t) for t in entry.get("tracks", [])],
    )


def _transcode_state_from_json(entry: dict) -> "TranscodeState":
    return TranscodeState(
        enabled=bool(entry.get("enabled", False)),
        encoder=entry.get("encoder", ""),
        renditions=[
            RenditionInfo(
                name=r.get("name", ""),
                width=int(r.get("width", 0)),
                height=int(r.get("height", 0)),
                video_bitrate_kbps=int(r.get("video_bitrate_kbps", 0)),
                audio_bitrate_kbps=int(r.get("audio_bitrate_kbps", 0)),
            )
            for r in entry.get("renditions", [])
        ],
        active=[
            TranscodeActiveStats(
                broadcast=a.get("broadcast", ""),
                track=a.get("track", ""),
                rendition=a.get("rendition", ""),
                fragments_in=int(a.get("fragments_in", 0)),
                fragments_out=int(a.get("fragments_out", 0)),
                panics=int(a.get("panics", 0)),
            )
            for a in entry.get("active", [])
        ],
    )


def _agent_state_from_json(entry: dict) -> "AgentState":
    return AgentState(
        enabled=bool(entry.get("enabled", False)),
        agents=[
            AgentInfo(
                name=a.get("name", ""),
                kind=a.get("kind", ""),
                model=a.get("model"),
                window_ms=a.get("window_ms"),
            )
            for a in entry.get("agents", [])
        ],
        active=[
            AgentActiveStats(
                agent=a.get("agent", ""),
                broadcast=a.get("broadcast", ""),
                track=a.get("track", ""),
                fragments_seen=int(a.get("fragments_seen", 0)),
                panics=int(a.get("panics", 0)),
            )
            for a in entry.get("active", [])
        ],
    )


def _archive_state_from_json(entry: dict) -> "ArchiveState":
    return ArchiveState(
        enabled=bool(entry.get("enabled", False)),
        recordings=[
            ArchiveBroadcastInfo(
                broadcast=r.get("broadcast", ""),
                segment_count=int(r.get("segment_count", 0)),
                total_bytes=int(r.get("total_bytes", 0)),
                duration_secs=float(r.get("duration_secs", 0.0)),
                tracks=[
                    ArchiveTrackInfo(
                        track=t.get("track", ""),
                        segment_count=int(t.get("segment_count", 0)),
                        total_bytes=int(t.get("total_bytes", 0)),
                        duration_secs=float(t.get("duration_secs", 0.0)),
                        timescale=int(t.get("timescale", 0)),
                    )
                    for t in r.get("tracks", [])
                ],
            )
            for r in entry.get("recordings", [])
        ],
    )


def _ingest_stop_from_json(entry: dict) -> "IngestStopResult":
    # The server's serde enum uses `#[serde(tag = "result")]` so the
    # response shape is `{"result": "stopped", "protocol": "rtmp"}` etc.
    return IngestStopResult(
        result=entry.get("result", "stopped"),
        protocol=entry.get("protocol"),
    )


def _broadcast_stop_from_json(entry: dict) -> "BroadcastStopResult":
    return BroadcastStopResult(
        result=entry.get("result", "killed"),
        broadcast=entry.get("broadcast"),
        protocol=entry.get("protocol"),
    )


def _server_info_from_json(entry: dict) -> "ServerInfo":
    bound_raw = entry.get("bound", {}) or {}
    features_raw = entry.get("features", {}) or {}
    return ServerInfo(
        version=entry.get("version", ""),
        build_features=list(entry.get("build_features", [])),
        uptime_secs=int(entry.get("uptime_secs", 0)),
        bound=BoundAddresses(
            admin=bound_raw.get("admin"),
            rtmp=bound_raw.get("rtmp"),
            whip=bound_raw.get("whip"),
            whep=bound_raw.get("whep"),
            hls=bound_raw.get("hls"),
            dash=bound_raw.get("dash"),
            srt=bound_raw.get("srt"),
            rtsp=bound_raw.get("rtsp"),
            moq=bound_raw.get("moq"),
            signal=bound_raw.get("signal"),
        ),
        features=RuntimeFeatures(
            mesh_enabled=bool(features_raw.get("mesh_enabled", False)),
            cluster_enabled=bool(features_raw.get("cluster_enabled", False)),
            archive_dir=features_raw.get("archive_dir"),
            record_dir=features_raw.get("record_dir"),
            wasm_filter_chain_length=int(features_raw.get("wasm_filter_chain_length", 0)),
            auth_mode=features_raw.get("auth_mode", "noop"),
            hmac_playback_secret_configured=bool(features_raw.get("hmac_playback_secret_configured", False)),
            stream_keys_enabled=bool(features_raw.get("stream_keys_enabled", True)),
        ),
        config_path=entry.get("config_path"),
        wasm_filter_paths=list(entry.get("wasm_filter_paths", [])),
    )

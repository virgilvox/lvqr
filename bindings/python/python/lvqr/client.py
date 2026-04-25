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
    BroadcastSummary,
    ClusterNodeView,
    ConfigEntry,
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

    def _get_json(self, path: str) -> object:
        resp = self._client.get(path)
        resp.raise_for_status()
        return resp.json()


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

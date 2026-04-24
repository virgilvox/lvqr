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

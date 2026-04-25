"""Tests for the LVQR Python client."""

from unittest.mock import MagicMock, patch

from lvqr import (
    BroadcastSummary,
    ClusterNodeView,
    ConfigEntry,
    FederationLinkStatus,
    FederationStatus,
    LvqrClient,
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


class TestTypes:
    def test_relay_stats_defaults(self):
        stats = RelayStats()
        assert stats.publishers == 0
        assert stats.subscribers == 0
        assert stats.tracks == 0

    def test_stream_info(self):
        info = StreamInfo(name="live/test", subscribers=5)
        assert info.name == "live/test"
        assert info.subscribers == 5

    def test_mesh_state_defaults(self):
        mesh = MeshState()
        assert mesh.enabled is False
        assert mesh.peer_count == 0
        assert mesh.offload_percentage == 0.0
        assert mesh.peers == []

    def test_mesh_peer_stats_defaults(self):
        peer = MeshPeerStats()
        assert peer.peer_id == ""
        assert peer.role == "Leaf"
        assert peer.parent is None
        assert peer.depth == 0
        assert peer.intended_children == 0
        assert peer.forwarded_frames == 0
        assert peer.capacity is None

    def test_slo_snapshot_defaults(self):
        snapshot = SloSnapshot()
        assert snapshot.broadcasts == []

    def test_wasm_filter_state_defaults(self):
        state = WasmFilterState()
        assert state.enabled is False
        assert state.chain_length == 0
        assert state.broadcasts == []
        assert state.slots == []

    def test_wasm_filter_broadcast_stats_defaults(self):
        stats = WasmFilterBroadcastStats(broadcast="live/demo", track="0.mp4")
        assert stats.seen == 0
        assert stats.kept == 0
        assert stats.dropped == 0

    def test_wasm_filter_slot_stats_defaults(self):
        slot = WasmFilterSlotStats()
        assert slot.index == 0
        assert slot.seen == 0
        assert slot.kept == 0
        assert slot.dropped == 0

    def test_federation_status_defaults(self):
        status = FederationStatus()
        assert status.links == []


class TestClient:
    def test_context_manager(self):
        with LvqrClient("http://localhost:8080") as client:
            assert client.base_url == "http://localhost:8080"

    def test_base_url_trailing_slash(self):
        client = LvqrClient("http://localhost:8080/")
        assert client.base_url == "http://localhost:8080"
        client.close()

    def test_bearer_token_header(self):
        client = LvqrClient("http://localhost:8080", bearer_token="s3cr3t")
        # httpx exposes default headers on the underlying client;
        # assert the Authorization header the Client sends on every
        # call is the bearer-shape we configured.
        assert client._client.headers["Authorization"] == "Bearer s3cr3t"
        client.close()

    @patch("httpx.Client.get")
    def test_healthz_ok(self, mock_get):
        mock_get.return_value = MagicMock(status_code=200)
        client = LvqrClient("http://localhost:8080")
        assert client.healthz() is True
        client.close()

    @patch("httpx.Client.get")
    def test_healthz_down(self, mock_get):
        import httpx

        mock_get.side_effect = httpx.ConnectError("connection refused")
        client = LvqrClient("http://localhost:8080")
        assert client.healthz() is False
        client.close()

    @patch("httpx.Client.get")
    def test_stats(self, mock_get):
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {"tracks": 3, "subscribers": 10, "publishers": 1},
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        stats = client.stats()
        assert stats.tracks == 3
        assert stats.subscribers == 10
        assert stats.publishers == 1
        client.close()

    @patch("httpx.Client.get")
    def test_list_streams(self, mock_get):
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: [
                {"name": "live/stream1", "subscribers": 5},
                {"name": "live/stream2", "subscribers": 12},
            ],
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        streams = client.list_streams()
        assert len(streams) == 2
        assert streams[0].name == "live/stream1"
        assert streams[1].subscribers == 12
        client.close()

    @patch("httpx.Client.get")
    def test_mesh(self, mock_get):
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {
                "enabled": True,
                "peer_count": 4,
                "offload_percentage": 27.5,
                "peers": [
                    {
                        "peer_id": "root-1",
                        "role": "Root",
                        "parent": None,
                        "depth": 0,
                        "intended_children": 3,
                        "forwarded_frames": 1200,
                        "capacity": 5,
                    },
                    {
                        "peer_id": "relay-7",
                        "role": "Relay",
                        "parent": "root-1",
                        "depth": 1,
                        "intended_children": 1,
                        "forwarded_frames": 400,
                    },
                ],
            },
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        mesh = client.mesh()
        assert mesh.enabled is True
        assert mesh.peer_count == 4
        assert mesh.offload_percentage == 27.5
        assert len(mesh.peers) == 2
        assert mesh.peers[0].peer_id == "root-1"
        assert mesh.peers[0].role == "Root"
        assert mesh.peers[0].parent is None
        assert mesh.peers[0].intended_children == 3
        assert mesh.peers[0].forwarded_frames == 1200
        assert mesh.peers[1].parent == "root-1"
        assert mesh.peers[1].depth == 1
        assert mesh.peers[1].forwarded_frames == 400
        # Session 144: per-peer capacity round-trips. The Root advertised
        # 5; the Relay omitted the field (None on the wire).
        assert mesh.peers[0].capacity == 5
        assert mesh.peers[1].capacity is None
        client.close()

    @patch("httpx.Client.get")
    def test_mesh_pre_session_144_server_omits_capacity(self, mock_get):
        # Session 144 defensive-parse: pre-144 servers omit the per-peer
        # `capacity` field. `.get("capacity")` returns None and the
        # dataclass picks up the default.
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {
                "enabled": True,
                "peer_count": 1,
                "offload_percentage": 0.0,
                "peers": [
                    {
                        "peer_id": "root-1",
                        "role": "Root",
                        "parent": None,
                        "depth": 0,
                        "intended_children": 0,
                        "forwarded_frames": 0,
                    },
                ],
            },
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        mesh = client.mesh()
        assert len(mesh.peers) == 1
        assert mesh.peers[0].capacity is None
        client.close()

    @patch("httpx.Client.get")
    def test_mesh_pre_session_141_server_omits_peers(self, mock_get):
        # Session 141 defensive-parse: older servers (pre-141) omit the
        # `peers` field entirely. `.get("peers", [])` must keep the
        # dataclass construction sound against those bodies.
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {
                "enabled": True,
                "peer_count": 2,
                "offload_percentage": 50.0,
            },
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        mesh = client.mesh()
        assert mesh.enabled is True
        assert mesh.peer_count == 2
        assert mesh.offload_percentage == 50.0
        assert mesh.peers == []
        client.close()

    @patch("httpx.Client.get")
    def test_slo_empty(self, mock_get):
        # Fresh server with no samples yet. Shape must still parse.
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {"broadcasts": []},
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        snapshot = client.slo()
        assert isinstance(snapshot, SloSnapshot)
        assert snapshot.broadcasts == []
        client.close()

    @patch("httpx.Client.get")
    def test_slo_populated(self, mock_get):
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {
                "broadcasts": [
                    {
                        "broadcast": "live/demo",
                        "transport": "hls",
                        "p50_ms": 300,
                        "p95_ms": 900,
                        "p99_ms": 1500,
                        "max_ms": 2100,
                        "sample_count": 48,
                        "total_observed": 2048,
                    }
                ]
            },
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        snapshot = client.slo()
        assert len(snapshot.broadcasts) == 1
        entry = snapshot.broadcasts[0]
        assert isinstance(entry, SloEntry)
        assert entry.broadcast == "live/demo"
        assert entry.transport == "hls"
        assert entry.p99_ms == 1500
        assert entry.sample_count == 48
        client.close()

    @patch("httpx.Client.get")
    def test_cluster_nodes_with_capacity(self, mock_get):
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: [
                {
                    "id": "lvqr-abcd1234",
                    "generation": 7,
                    "gossip_addr": "10.0.0.1:10007",
                    "capacity": {
                        "cpu_pct": 23.5,
                        "rss_bytes": 104857600,
                        "bytes_out_per_sec": 524288,
                    },
                }
            ],
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        nodes = client.cluster_nodes()
        assert len(nodes) == 1
        node = nodes[0]
        assert isinstance(node, ClusterNodeView)
        assert node.id == "lvqr-abcd1234"
        assert node.generation == 7
        assert node.gossip_addr == "10.0.0.1:10007"
        assert isinstance(node.capacity, NodeCapacity)
        assert node.capacity.cpu_pct == 23.5
        assert node.capacity.rss_bytes == 104857600
        client.close()

    @patch("httpx.Client.get")
    def test_cluster_nodes_null_capacity(self, mock_get):
        # A freshly-joined node has not advertised capacity yet;
        # the server emits `"capacity": null`. The client must
        # accept it as None, not crash on the dict access.
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: [
                {
                    "id": "lvqr-freshjoin",
                    "generation": 1,
                    "gossip_addr": "10.0.0.2:10007",
                    "capacity": None,
                }
            ],
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        nodes = client.cluster_nodes()
        assert nodes[0].capacity is None
        client.close()

    @patch("httpx.Client.get")
    def test_cluster_broadcasts(self, mock_get):
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: [
                {
                    "name": "live/demo",
                    "owner": "lvqr-owner01",
                    "expires_at_ms": 1_700_000_000_000,
                }
            ],
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        broadcasts = client.cluster_broadcasts()
        assert len(broadcasts) == 1
        assert isinstance(broadcasts[0], BroadcastSummary)
        assert broadcasts[0].name == "live/demo"
        assert broadcasts[0].owner == "lvqr-owner01"
        assert broadcasts[0].expires_at_ms == 1_700_000_000_000
        client.close()

    @patch("httpx.Client.get")
    def test_cluster_config(self, mock_get):
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: [
                {
                    "key": "rate_limit",
                    "value": "100",
                    "ts_ms": 1_700_000_000_000,
                }
            ],
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        config = client.cluster_config()
        assert len(config) == 1
        assert isinstance(config[0], ConfigEntry)
        assert config[0].key == "rate_limit"
        assert config[0].value == "100"
        client.close()

    @patch("httpx.Client.get")
    def test_cluster_federation_empty(self, mock_get):
        # No federation links configured -- server returns empty
        # list inside the wrapper so tooling can poll unconditionally.
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {"links": []},
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        status = client.cluster_federation()
        assert isinstance(status, FederationStatus)
        assert status.links == []
        client.close()

    @patch("httpx.Client.get")
    def test_cluster_federation_populated(self, mock_get):
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {
                "links": [
                    {
                        "remote_url": "https://peer.example:4443/",
                        "forwarded_broadcasts": ["live/a", "live/b"],
                        "state": "connected",
                        "last_connected_at_ms": 1_700_000_000_000,
                        "last_error": None,
                        "connect_attempts": 3,
                        "forwarded_broadcasts_seen": 2,
                    },
                    {
                        "remote_url": "https://peer2.example:4443/",
                        "forwarded_broadcasts": ["live/c"],
                        "state": "failed",
                        "last_connected_at_ms": None,
                        "last_error": "handshake refused",
                        "connect_attempts": 7,
                        "forwarded_broadcasts_seen": 0,
                    },
                ]
            },
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        status = client.cluster_federation()
        assert len(status.links) == 2
        assert isinstance(status.links[0], FederationLinkStatus)
        assert status.links[0].state == "connected"
        assert status.links[0].forwarded_broadcasts == ["live/a", "live/b"]
        assert status.links[0].last_error is None
        assert status.links[1].state == "failed"
        assert status.links[1].last_error == "handshake refused"
        assert status.links[1].last_connected_at_ms is None
        client.close()

    @patch("httpx.Client.get")
    def test_wasm_filter_disabled(self, mock_get):
        # No --wasm-filter configured -- server returns the
        # disabled shape (200 OK body), not a 404.
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {
                "enabled": False,
                "chain_length": 0,
                "broadcasts": [],
                "slots": [],
            },
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        state = client.wasm_filter()
        assert isinstance(state, WasmFilterState)
        assert state.enabled is False
        assert state.chain_length == 0
        assert state.broadcasts == []
        assert state.slots == []
        client.close()

    @patch("httpx.Client.get")
    def test_wasm_filter_populated(self, mock_get):
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {
                "enabled": True,
                "chain_length": 2,
                "broadcasts": [
                    {
                        "broadcast": "live/cam1",
                        "track": "0.mp4",
                        "seen": 120,
                        "kept": 100,
                        "dropped": 20,
                    },
                    {
                        "broadcast": "live/cam2",
                        "track": "0.mp4",
                        "seen": 75,
                        "kept": 75,
                        "dropped": 0,
                    },
                ],
                "slots": [
                    {"index": 0, "seen": 195, "kept": 195, "dropped": 0},
                    {"index": 1, "seen": 195, "kept": 175, "dropped": 20},
                ],
            },
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        state = client.wasm_filter()
        assert state.enabled is True
        assert state.chain_length == 2
        assert len(state.broadcasts) == 2
        assert isinstance(state.broadcasts[0], WasmFilterBroadcastStats)
        assert state.broadcasts[0].broadcast == "live/cam1"
        assert state.broadcasts[0].seen == 120
        assert state.broadcasts[0].kept == 100
        assert state.broadcasts[0].dropped == 20
        assert state.broadcasts[1].broadcast == "live/cam2"
        assert state.broadcasts[1].dropped == 0
        # Session 140: per-slot counters decompose the chain's
        # outcome into per-filter activity.
        assert len(state.slots) == 2
        assert isinstance(state.slots[0], WasmFilterSlotStats)
        assert state.slots[0].index == 0
        assert state.slots[0].seen == 195
        assert state.slots[0].kept == 195
        assert state.slots[0].dropped == 0
        assert state.slots[1].index == 1
        assert state.slots[1].kept == 175
        assert state.slots[1].dropped == 20
        client.close()

    @patch("httpx.Client.get")
    def test_wasm_filter_pre_session_140_server_omits_slots(self, mock_get):
        # Defensive: a pre-session-140 server would respond with no
        # `slots` field. Client must default to an empty list, not
        # throw.
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {
                "enabled": True,
                "chain_length": 1,
                "broadcasts": [],
            },
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        state = client.wasm_filter()
        assert state.enabled is True
        assert state.chain_length == 1
        assert state.slots == []
        client.close()

    # -----------------------------------------------------------------
    # Stream-key CRUD admin API (session 146).
    # -----------------------------------------------------------------

    @patch("httpx.Client.get")
    def test_list_streamkeys_empty(self, mock_get):
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {"keys": []},
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        keys = client.list_streamkeys()
        assert keys == []
        client.close()

    @patch("httpx.Client.get")
    def test_list_streamkeys_populated_omitting_optional_fields(self, mock_get):
        # Defensive parse: server omits `label`, `broadcast`, and
        # `expires_at` on a key minted with no scope. The dataclass
        # construction must default each to None and not throw.
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {
                "keys": [
                    {
                        "id": "abc123",
                        "token": "lvqr_sk_xyz",
                        "created_at": 1_700_000_000,
                    },
                    {
                        "id": "def456",
                        "token": "lvqr_sk_abc",
                        "label": "camera-a",
                        "broadcast": "live/cam-a",
                        "created_at": 1_700_001_000,
                        "expires_at": 1_700_005_000,
                    },
                ],
            },
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        keys = client.list_streamkeys()
        assert len(keys) == 2
        assert isinstance(keys[0], StreamKey)
        assert keys[0].id == "abc123"
        assert keys[0].token == "lvqr_sk_xyz"
        assert keys[0].label is None
        assert keys[0].broadcast is None
        assert keys[0].expires_at is None
        assert keys[1].label == "camera-a"
        assert keys[1].broadcast == "live/cam-a"
        assert keys[1].expires_at == 1_700_005_000
        client.close()

    @patch("httpx.Client.get")
    def test_list_streamkeys_pre_146_server_omits_keys(self, mock_get):
        # Defensive: a server that omits the `keys` wrapper field
        # entirely (or returns an empty {} body) must not throw.
        mock_get.return_value = MagicMock(
            status_code=200,
            json=lambda: {},
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        assert client.list_streamkeys() == []
        client.close()

    @patch("httpx.Client.post")
    def test_mint_streamkey(self, mock_post):
        mock_post.return_value = MagicMock(
            status_code=201,
            json=lambda: {
                "id": "new123",
                "token": "lvqr_sk_freshly_minted",
                "label": "rotated-cam",
                "broadcast": None,
                "created_at": 1_700_010_000,
                "expires_at": None,
            },
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        spec = StreamKeySpec(label="rotated-cam")
        key = client.mint_streamkey(spec)
        assert isinstance(key, StreamKey)
        assert key.id == "new123"
        assert key.token == "lvqr_sk_freshly_minted"
        assert key.label == "rotated-cam"
        client.close()

    def test_streamkeyspec_defaults(self):
        spec = StreamKeySpec()
        assert spec.label is None
        assert spec.broadcast is None
        assert spec.ttl_seconds is None

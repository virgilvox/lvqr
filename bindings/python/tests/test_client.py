"""Tests for the LVQR Python client."""

from unittest.mock import MagicMock, patch

from lvqr import (
    BroadcastSummary,
    ClusterNodeView,
    ConfigEntry,
    FederationLinkStatus,
    FederationStatus,
    LvqrClient,
    MeshState,
    NodeCapacity,
    RelayStats,
    SloEntry,
    SloSnapshot,
    StreamInfo,
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

    def test_slo_snapshot_defaults(self):
        snapshot = SloSnapshot()
        assert snapshot.broadcasts == []

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
            },
            raise_for_status=lambda: None,
        )
        client = LvqrClient("http://localhost:8080")
        mesh = client.mesh()
        assert mesh.enabled is True
        assert mesh.peer_count == 4
        assert mesh.offload_percentage == 27.5
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

"""Tests for the LVQR Python client."""

import json
from unittest.mock import MagicMock, patch

from lvqr import LvqrClient, RelayStats, StreamInfo


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


class TestClient:
    def test_context_manager(self):
        with LvqrClient("http://localhost:8080") as client:
            assert client.base_url == "http://localhost:8080"

    def test_base_url_trailing_slash(self):
        client = LvqrClient("http://localhost:8080/")
        assert client.base_url == "http://localhost:8080"
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

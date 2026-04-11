"""LVQR admin API client."""

from __future__ import annotations

import httpx

from .types import RelayStats, StreamInfo


class LvqrClient:
    """Client for the LVQR admin HTTP API.

    Args:
        base_url: Base URL of the LVQR admin server (e.g., "http://localhost:8080").
        timeout: Request timeout in seconds.

    Example::

        client = LvqrClient("http://localhost:8080")
        if client.healthz():
            stats = client.stats()
            print(f"Tracks: {stats.tracks}, Subscribers: {stats.subscribers}")
    """

    def __init__(self, base_url: str, timeout: float = 10.0):
        self.base_url = base_url.rstrip("/")
        self._client = httpx.Client(base_url=self.base_url, timeout=timeout)

    def close(self) -> None:
        """Close the HTTP client."""
        self._client.close()

    def __enter__(self) -> LvqrClient:
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    def healthz(self) -> bool:
        """Check if the relay is healthy.

        Returns:
            True if the server responds with 200 OK.
        """
        try:
            resp = self._client.get("/healthz")
            return resp.status_code == 200
        except httpx.HTTPError:
            return False

    def stats(self) -> RelayStats:
        """Get relay statistics.

        Returns:
            RelayStats with current server metrics.
        """
        resp = self._client.get("/api/v1/stats")
        resp.raise_for_status()
        data = resp.json()
        return RelayStats(
            publishers=data.get("publishers", 0),
            subscribers=data.get("subscribers", 0),
            tracks=data.get("tracks", 0),
            bytes_received=data.get("bytes_received", 0),
            bytes_sent=data.get("bytes_sent", 0),
            uptime_secs=data.get("uptime_secs", 0),
        )

    def list_streams(self) -> list[StreamInfo]:
        """List active streams.

        Returns:
            List of StreamInfo for each active stream.
        """
        resp = self._client.get("/api/v1/streams")
        resp.raise_for_status()
        return [
            StreamInfo(
                name=s.get("name", ""),
                subscribers=s.get("subscribers", 0),
            )
            for s in resp.json()
        ]

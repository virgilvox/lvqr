"""Data types for the LVQR admin API."""

from dataclasses import dataclass


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

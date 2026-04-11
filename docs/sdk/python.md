# Python SDK

The `lvqr` Python package provides an admin client for managing LVQR relay servers.

## Install

```bash
pip install lvqr
```

## Usage

```python
from lvqr import LvqrClient

client = LvqrClient("http://localhost:8080")

# Health check
if client.healthz():
    print("Relay is healthy")

# Get stats
stats = client.stats()
print(f"Tracks: {stats.tracks}, Subscribers: {stats.subscribers}")

# List active streams
for stream in client.list_streams():
    print(f"  {stream.name}: {stream.subscribers} viewers")
```

## API Reference

### `LvqrClient(base_url, timeout=10.0)`

Create a client connected to an LVQR admin API.

**Methods:**

| Method | Returns | Description |
|--------|---------|-------------|
| `healthz()` | `bool` | Check if relay is healthy |
| `stats()` | `RelayStats` | Get server statistics |
| `list_streams()` | `list[StreamInfo]` | List active streams |
| `close()` | `None` | Close the HTTP client |

### `RelayStats`

| Field | Type | Description |
|-------|------|-------------|
| `publishers` | `int` | Active publishers |
| `subscribers` | `int` | Active subscribers |
| `tracks` | `int` | Active tracks |
| `bytes_received` | `int` | Total bytes received |
| `bytes_sent` | `int` | Total bytes sent |
| `uptime_secs` | `int` | Server uptime |

### `StreamInfo`

| Field | Type | Description |
|-------|------|-------------|
| `name` | `str` | Stream name (e.g., "live/test") |
| `subscribers` | `int` | Current viewer count |

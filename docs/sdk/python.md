# Python SDK

The `lvqr` Python package provides a synchronous admin client
for LVQR relay servers. Ships at `0.3.1` on PyPI; the 9-method
admin surface documented below lands for consumers at the next
release cycle.

## Install

```bash
pip install lvqr
# or for the full dev environment:
pip install 'lvqr[dev]'
```

## Quickstart

```python
from lvqr import LvqrClient

with LvqrClient("http://localhost:8080") as client:
    if client.healthz():
        stats = client.stats()
        print(f"Tracks: {stats.tracks}, Subscribers: {stats.subscribers}")

    for stream in client.list_streams():
        print(f"  {stream.name}: {stream.subscribers} viewers")
```

`LvqrClient` is a context manager that closes the underlying
`httpx.Client` on exit. Holding it open across many calls is
fine (and cheaper than re-creating per call).

## Admin API reference

The client covers every `/api/v1/*` route the admin router
mounts today.

### `LvqrClient(base_url, timeout=10.0, bearer_token=None)`

| Arg | Type | Purpose |
|---|---|---|
| `base_url` | `str` | Base URL of the admin server (e.g. `"http://localhost:8080"`). Trailing slash tolerated. |
| `timeout` | `float` | Per-request deadline in seconds. Applied via `httpx.Client(timeout=...)` to every GET. Defaults to `10.0`. |
| `bearer_token` | `Optional[str]` | When set, every admin call sends `Authorization: Bearer <token>`. Required when the server ran with `--admin-token` or a JWT provider; leave `None` for open-access deployments. |

### Methods

| Method | Route | Returns |
|---|---|---|
| `healthz()` | `GET /healthz` | `bool` (`False` on any non-2xx or network error) |
| `stats()` | `GET /api/v1/stats` | `RelayStats` |
| `list_streams()` | `GET /api/v1/streams` | `list[StreamInfo]` |
| `mesh()` | `GET /api/v1/mesh` | `MeshState` |
| `slo()` | `GET /api/v1/slo` | `SloSnapshot` |
| `cluster_nodes()` | `GET /api/v1/cluster/nodes` | `list[ClusterNodeView]` |
| `cluster_broadcasts()` | `GET /api/v1/cluster/broadcasts` | `list[BroadcastSummary]` |
| `cluster_config()` | `GET /api/v1/cluster/config` | `list[ConfigEntry]` |
| `cluster_federation()` | `GET /api/v1/cluster/federation` | `FederationStatus` |
| `wasm_filter()` | `GET /api/v1/wasm-filter` | `WasmFilterState` |
| `close()` | -- | `None` (closes the underlying httpx client) |

Cluster-prefixed methods require the server to be built with
`--features cluster` (on by default) and `--cluster-listen` to
be set. If the feature is on but no `Cluster` handle is wired
the server returns `HTTP 500` and the client surfaces an
`httpx.HTTPStatusError`.

### Response types

Every dataclass mirrors a Rust serde struct on the server side.
Field names match the JSON-on-wire encoding exactly so
`**json.loads(body)` unpacks via the constructor kwargs.

```python
from dataclasses import dataclass
from typing import Literal, Optional

@dataclass
class RelayStats:
    publishers: int = 0
    subscribers: int = 0
    tracks: int = 0
    bytes_received: int = 0
    bytes_sent: int = 0
    uptime_secs: int = 0

@dataclass
class StreamInfo:
    name: str
    subscribers: int = 0

@dataclass
class MeshState:
    enabled: bool = False
    peer_count: int = 0
    offload_percentage: float = 0.0

@dataclass
class SloEntry:
    broadcast: str
    transport: str  # "hls" | "dash" | "ws" | "whep" ...
    p50_ms: int = 0
    p95_ms: int = 0
    p99_ms: int = 0
    max_ms: int = 0
    sample_count: int = 0
    total_observed: int = 0

@dataclass
class SloSnapshot:
    broadcasts: list[SloEntry]

@dataclass
class NodeCapacity:
    cpu_pct: float = 0.0  # 0.0..=100.0 per logical core
    rss_bytes: int = 0
    bytes_out_per_sec: int = 0

@dataclass
class ClusterNodeView:
    id: str
    generation: int = 0
    gossip_addr: str = ""
    capacity: Optional[NodeCapacity] = None

@dataclass
class BroadcastSummary:
    name: str
    owner: str = ""
    expires_at_ms: int = 0

@dataclass
class ConfigEntry:
    key: str
    value: str = ""
    ts_ms: int = 0

FederationConnectState = Literal["connecting", "connected", "failed"]

@dataclass
class FederationLinkStatus:
    remote_url: str
    forwarded_broadcasts: list[str]
    state: FederationConnectState = "connecting"
    last_connected_at_ms: Optional[int] = None
    last_error: Optional[str] = None
    connect_attempts: int = 0
    forwarded_broadcasts_seen: int = 0

@dataclass
class FederationStatus:
    links: list[FederationLinkStatus]

@dataclass
class WasmFilterBroadcastStats:
    broadcast: str          # "live/cam1"
    track: str              # "0.mp4"
    seen: int = 0           # kept + dropped
    kept: int = 0           # survived every slot in the chain
    dropped: int = 0        # a slot returned None (short-circuit)

@dataclass
class WasmFilterState:
    enabled: bool = False   # mirrors whether --wasm-filter was configured
    chain_length: int = 0   # constant for the server's lifetime
    broadcasts: list[WasmFilterBroadcastStats] = field(default_factory=list)
```

## Timeouts + retries

### Per-request timeout

`LvqrClient(timeout=...)` hands the value to
`httpx.Client(timeout=...)`, which applies it to every call:

```python
import httpx
from lvqr import LvqrClient

# Stricter deadline for a health-dashboard poller.
with LvqrClient("http://localhost:8080", timeout=3.0) as client:
    try:
        stats = client.stats()
    except httpx.ReadTimeout:
        # Server accepted the TCP but never responded within 3s.
        # Backoff and retry, or mark the endpoint degraded.
        ...
    except httpx.ConnectTimeout:
        # Never got a TCP connection in time.
        ...
```

Raising the timeout disables nothing; set a very large number
(`timeout=3600.0`) if you deliberately want to wait on a slow
endpoint. httpx does not support `timeout=None` on the
`Client` constructor in recent versions.

### Bearer authentication

Pass `bearer_token=` on construction; the header is sent on
every request automatically via `httpx.Client`'s default
headers mechanism:

```python
import os
from lvqr import LvqrClient

with LvqrClient(
    "http://localhost:8080",
    bearer_token=os.environ["LVQR_ADMIN_TOKEN"],
) as client:
    stats = client.stats()
```

Wrong or missing token produces `401 Unauthorized`, which
httpx raises as `httpx.HTTPStatusError` when `raise_for_status()`
runs. The client calls `raise_for_status()` internally on every
admin route (not on `healthz()`), so the caller sees the
exception on the same call that hit the server.

### Retry recipe

Admin GETs are idempotent, so retrying a failed call is safe.
Capped exponential backoff pattern:

```python
import time
import httpx
from lvqr import LvqrClient

def with_retry(fn, max_attempts=4):
    for attempt in range(max_attempts):
        try:
            return fn()
        except (httpx.ReadTimeout, httpx.ConnectTimeout, httpx.ConnectError):
            if attempt == max_attempts - 1:
                raise
            delay = min(5.0, 0.2 * (2 ** attempt))
            time.sleep(delay)

with LvqrClient("http://localhost:8080", timeout=3.0) as client:
    stats = with_retry(client.stats)
```

The `LvqrClient` is thread-safe only when called from the same
thread; for parallel polling prefer per-thread clients or run
in an async runtime with `httpx.AsyncClient` wrapped by your
own abstraction (LVQR's Python client ships sync-only as of
`0.3.x`; an async wrapper is on the v1.2 roadmap).

### Python client is admin-only

The Python package does not carry a streaming-subscribe
surface. For pulling live video from LVQR into a Python
process, use `ffmpeg-python` or `av` against the LL-HLS or
MPEG-DASH endpoints; the admin client is the supported
integration point for monitoring and ops tooling.

## Migrating from `0.3.1` to `main`

The package on PyPI at `0.3.1` ships three methods
(`healthz`, `stats`, `list_streams`). `main` adds seven more
(`mesh`, `slo`, `cluster_nodes`, `cluster_broadcasts`,
`cluster_config`, `cluster_federation`, `wasm_filter`) + a
`bearer_token` kwarg + 13 new dataclasses. All additive; no
breaking changes. When pinning to a specific release, test
for method existence if your code runs against both
versions:

```python
from lvqr import LvqrClient

with LvqrClient("http://localhost:8080") as client:
    mesh = client.mesh() if hasattr(client, "mesh") else None
```

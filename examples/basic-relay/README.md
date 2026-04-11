# Basic Relay Example

The simplest LVQR setup: a single relay server that accepts RTMP input and serves MoQ output.

## Run

```bash
# From the repo root
cargo run -p lvqr-cli -- serve

# Or if installed
lvqr serve
```

This starts:
- MoQ relay on port 4443 (QUIC/WebTransport)
- RTMP ingest on port 1935
- Admin API on port 8080

## Stream from OBS

1. Open OBS Studio
2. Settings > Stream
3. Service: Custom
4. Server: `rtmp://localhost:1935/live`
5. Stream Key: `test`
6. Start Streaming

## Verify

```bash
# Check health
curl http://localhost:8080/healthz

# List streams (should show "live/test" while OBS is streaming)
curl http://localhost:8080/api/v1/streams

# Stats
curl http://localhost:8080/api/v1/stats
```

## With Config File

```bash
lvqr serve --config lvqr.toml
```

See `lvqr.toml` in this directory for available options.

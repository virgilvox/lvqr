# Basic Relay Example

The simplest LVQR setup: a single relay server that accepts RTMP input and serves MoQ output.

## Run

```bash
# From the repo root
cargo run -p lvqr-cli -- serve

# Or if installed
lvqr serve
```

This starts the zero-config defaults:
- MoQ relay on port 4443 (QUIC/WebTransport)
- RTMP ingest on port 1935
- LL-HLS on port 8888 (HTTP/1.1)
- Admin API + WebSocket fMP4 on port 8080

Adding `--dash-port 8889`, `--whep-port 8443`, `--whip-port 8443`,
`--rtsp-port 8554`, and/or `--srt-port 8890` brings up the
optional protocols. See the top-level README for the full
port matrix.

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

# Play back via LL-HLS
open http://localhost:8888/hls/live/test/playlist.m3u8
```

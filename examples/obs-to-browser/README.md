# OBS to Browser Example

Stream from OBS Studio through LVQR to a browser viewer using Docker.

## Prerequisites

- Docker and Docker Compose
- OBS Studio (or ffmpeg)

## Run

```bash
docker compose up
```

This starts:
- LVQR relay with RTMP ingest and admin API
- Caddy reverse proxy with automatic HTTPS (for WebTransport)

## Stream from OBS

1. Open OBS Studio
2. Settings > Stream > Custom
3. Server: `rtmp://localhost:1935/live`
4. Stream Key: `demo`
5. Start Streaming

## Stream from ffmpeg (no OBS needed)

```bash
# Generate a test pattern and stream it
ffmpeg -re -f lavfi -i testsrc=size=1280x720:rate=30 \
  -c:v libx264 -preset ultrafast -tune zerolatency \
  -g 60 -keyint_min 60 \
  -f flv rtmp://localhost:1935/live/demo
```

## Watch

Open https://localhost/watch/demo in your browser.

Or check the admin API:
```bash
curl http://localhost:8080/api/v1/streams
```

## Architecture

```
OBS/ffmpeg --> RTMP (1935) --> LVQR --> MoQ/WebTransport (4443) --> Browser
                                 |
                                 +--> Admin API (8080)
```

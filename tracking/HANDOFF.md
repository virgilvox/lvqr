# LVQR Handoff Document

## Project Status: v0.3.1 -- Browser Playback Working

**Last Updated**: 2026-04-11
**Tests**: 83 Rust + 8 Python = 91 total (passing)
**CI**: All green
**E2E Verified**: Browser webcam -> WebSocket ingest -> fMP4 remux -> MoQ -> WebSocket relay -> MSE playback

## End-to-End Pipeline (Proven Working)

```
Browser Webcam (getUserMedia)
    |
    v
VideoEncoder (H.264 Baseline, WebCodecs API)
    |
    v
WebSocket (/ingest/{broadcast}) -- binary frames with AVCC NALUs
    |
    v
LVQR Server (Rust)
  - Parses AVCC config (SPS/PPS, width/height)
  - Generates fMP4 init segment (ftyp+moov with avcC box)
  - Remuxes H.264 NALUs to fMP4 media segments (moof+mdat)
  - Publishes to MoQ tracks via OriginProducer
    |
    v
WebSocket (/ws/{broadcast}) -- forwards fMP4 binary frames
    |
    v
Browser Viewer (MSE)
  - Auto-detects codec from avcC box in init segment
  - Creates SourceBuffer in sequence mode
  - Chases live edge (seeks when >500ms behind)
  - Plays video
```

Also works with RTMP ingest (OBS/ffmpeg) via the same fMP4 remux pipeline.

## All Packages Published

### crates.io (8 crates)
lvqr-core, lvqr-signal, lvqr-relay, lvqr-ingest, lvqr-mesh, lvqr-admin, lvqr-wasm, lvqr-cli @ 0.3.1

### npm (2 packages)
@lvqr/core, @lvqr/player @ 0.3.1

### PyPI
lvqr @ 0.3.1

## Test App

`test-app/index.html` -- single-page brutalist test app:
- **Watch**: WebSocket fMP4 viewer with MSE, live-edge chasing
- **Stream**: Webcam capture via WebCodecs VideoEncoder, streams over WebSocket
- **Admin**: Real-time dashboard with stats, streams, mesh state

Run: `./test-app/serve.sh` (serves on port 9000)

## Key Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `:4443` | QUIC | MoQ relay (WebTransport/QUIC) |
| `:1935` | TCP | RTMP ingest (OBS/ffmpeg) |
| `:8080/healthz` | GET | Health check |
| `:8080/api/v1/stats` | GET | Publisher/subscriber/track counts |
| `:8080/api/v1/streams` | GET | Active stream list |
| `:8080/api/v1/mesh` | GET | Mesh peer count, offload % |
| `:8080/ws/{broadcast}` | WS | fMP4 viewer relay |
| `:8080/ingest/{broadcast}` | WS | Browser H.264 ingest |
| `:8080/signal` | WS | WebRTC signaling for mesh |

## Bugs Fixed During E2E Testing

| Bug | Root Cause |
|-----|------------|
| Init segment rejected by MSE | avc1 box had width=0 height=0 -- Chrome requires valid dimensions |
| Duplicate init segments | WS relay sent stored init + group frame 0 (also init) |
| H.264 level too low for 720p | Encoder configured with level 3.0, needed 4.0 |
| CORS blocking admin API | No CORS headers on admin HTTP server |
| Watch pointed at wrong broadcast | Default was "live/test" but streamer publishes "live/webcam" |
| Viewer latency growing | No live-edge seeking -- MSE buffer piled up |

## What Works

- Webcam -> browser -> LVQR -> browser viewer (E2E proven)
- RTMP ingest -> fMP4 remux -> MoQ tracks (integration tested)
- Mesh coordinator assigns peers, pushes assignments via signal (unit tested)
- Admin API with real stats and CORS (tested in browser)
- WebSocket fMP4 relay (E2E proven)
- WebSocket H.264 ingest (E2E proven)

## What's Left

- MoQ/WebTransport browser playback (code exists, untested -- WS fallback works)
- WebRTC DataChannel media forwarding between mesh peers
- Audio playback (needs separate MSE SourceBuffer)
- Stream authentication
- Recording

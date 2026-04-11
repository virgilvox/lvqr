# LVQR Handoff Document

## Project Status: v0.3.1 -- Browser Playback Working

**Last Updated**: 2026-04-11
**Tests**: 83 Rust + 8 Python = 91 total (passing)
**CI**: All 5 jobs green (Format+Lint, Test ubuntu, Test macos, WASM Build, Docker Build)
**E2E Verified**: Browser webcam -> WS ingest -> fMP4 remux -> MoQ -> WS relay -> MSE playback

## End-to-End Pipeline (Proven Working)

```
Browser Webcam (getUserMedia)
    |
    v
VideoEncoder (H.264 Baseline, WebCodecs API)
    |
    v
WebSocket (/ingest/{broadcast}) -- [type][timestamp][AVCC NALUs]
    |
    v
LVQR Server (Rust)
  - Parses AVCC config (SPS/PPS, width/height)
  - Generates fMP4 init segment (ftyp+moov with avcC, correct dimensions)
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

## Repository Structure

```
lvqr/
  crates/
    lvqr-core/          Shared types, ring buffer, GOP cache (25 tests)
    lvqr-relay/         MoQ relay on moq-lite, connection callbacks (4 integration tests)
    lvqr-ingest/        RTMP server + FLV-to-CMAF remuxer (26 lib + 2 integration tests)
    lvqr-mesh/          Peer tree coordinator (13 tests)
    lvqr-signal/        WebRTC signaling + mesh push (7 tests)
    lvqr-admin/         HTTP API: stats, streams, mesh (6 tests)
    lvqr-wasm/          WebTransport browser bindings
    lvqr-cli/           Single binary: relay + RTMP + admin + WS relay/ingest + mesh
    lvqr-test-utils/    Test helpers (publish = false)
  bindings/
    js/packages/
      core/             MoQ client, admin client, mesh peer (@lvqr/core)
      player/           <lvqr-player> web component (@lvqr/player)
    python/             Admin API client (lvqr on PyPI)
  test-app/             Brutalist test UI: stream, watch, admin
  tracking/             Handoff, audit, session notes
```

## Test App

`test-app/index.html` -- single-page brutalist test app with three tabs:

- **Stream**: Webcam capture via WebCodecs VideoEncoder (H.264 Baseline, level 4.0), streams over WebSocket to `/ingest/{broadcast}`. No ffmpeg or OBS needed.
- **Watch**: WebSocket fMP4 viewer with MSE SourceBuffer. Auto-detects codec from avcC box. Chases live edge to keep latency low.
- **Admin**: Real-time dashboard polling `/healthz`, `/api/v1/stats`, `/api/v1/streams`, `/api/v1/mesh`. Auto-refresh toggle.

Run:
```bash
lvqr serve                              # terminal 1
cd test-app && python3 -m http.server 9000  # terminal 2
# open http://localhost:9000
```

## Key Endpoints

| Endpoint | Protocol | Description |
|----------|----------|-------------|
| `:4443` | QUIC/UDP | MoQ relay (WebTransport/QUIC subscribers) |
| `:1935` | TCP | RTMP ingest (OBS/ffmpeg) |
| `:8080/healthz` | HTTP GET | Health check |
| `:8080/api/v1/stats` | HTTP GET | Publisher/subscriber/track counts |
| `:8080/api/v1/streams` | HTTP GET | Active stream list |
| `:8080/api/v1/mesh` | HTTP GET | Mesh peer count, offload % |
| `:8080/ws/{broadcast}` | WebSocket | fMP4 viewer relay (binary frames) |
| `:8080/ingest/{broadcast}` | WebSocket | Browser H.264 ingest |
| `:8080/signal` | WebSocket | WebRTC signaling for mesh peers |

## WS Ingest Wire Format

Binary WebSocket messages: `[u8 type][u32 BE timestamp_ms][payload]`

| Type | Payload |
|------|---------|
| 0 | Config: `[u16 BE width][u16 BE height][AVCDecoderConfigurationRecord]` |
| 1 | Keyframe: AVCC-format NALUs (length-prefixed) |
| 2 | Delta frame: AVCC-format NALUs |

The AVCC record comes from `VideoEncoder.output()` metadata's `decoderConfig.description`. NALUs use `avc: { format: 'avc' }` (length-prefixed, not Annex B).

## All Bugs Found and Fixed (12 total)

### Protocol bugs (found by reading moq-lite source, no browser needed)

| # | Bug | Impact |
|---|-----|--------|
| 1 | CLIENT_SETUP size: varint instead of u16 BE | Every MoQ connection fails |
| 2 | Path encoding: segmented array instead of plain string | Every subscribe returns NotFound |
| 3 | AnnouncePlease: 1 empty segment instead of 0 | Discovery returns nothing |
| 4 | Subscribe priority: varint instead of u8 | Misparse for priority > 63 |
| 5 | trun box version 0 for signed CTS | B-frame timestamps wrong |
| 6 | Player hardcoded codec string | Non-High-profile H.264 fails |
| 7 | Video+audio to single MSE SourceBuffer | MSE crash on first audio frame |

### E2E bugs (found during live browser testing)

| # | Bug | Impact |
|---|-----|--------|
| 8 | Init segment width=0 height=0 in avc1/tkhd | Chrome MSE rejects init |
| 9 | Duplicate init segments (stored + group frame 0) | MSE error after first append |
| 10 | No live-edge seeking | Unbounded latency growth |
| 11 | H.264 level 3.0 for 720p capture | VideoEncoder refuses to encode |
| 12 | No CORS headers on admin HTTP | Admin tab fetch() blocked |

## What Works (verified)

| Feature | How Verified |
|---------|-------------|
| Webcam -> browser -> LVQR -> browser viewer | E2E in Chrome |
| RTMP ingest (OBS/ffmpeg) -> fMP4 -> MoQ | 2 integration tests |
| FLV parsing (SPS/PPS, AAC config) | 12 unit tests |
| fMP4 generation (init + media segments) | 10 unit tests |
| MoQ QUIC fan-out (1 pub, 3 subs) | 3 integration tests |
| Relay connection callback | 1 integration test |
| Mesh tree assignment + orphan reassignment | 13 unit tests |
| Signal server message routing + push | 7 unit tests |
| Admin API (stats, streams, mesh) | 6 unit tests |
| Admin dashboard in browser | Manual test |
| Registry fanout benchmark | ~230ns to 500 subscribers |

## What's Not Done

| Feature | Status |
|---------|--------|
| MoQ/WebTransport browser path | Code written (SETUP, Subscribe, Group/Frame), WS fallback works, WebTransport path untested |
| WebRTC mesh media relay | Coordination works (tree + signal push), DataChannel code written, relay untested |
| Audio playback | Separate MSE SourceBuffer needed, not wired in player |
| Stream authentication | Not started |
| Recording | Not started |
| Multi-server federation | Not started |

## Key Technical Decisions

- **FLV-to-CMAF remux** in Rust (manual fMP4 box writer, no external crate). AVCC NALUs pass through unchanged since both FLV and fMP4 use length-prefixed format.
- **WebSocket for browser ingest** because browsers can't do RTMP (no TCP sockets). WebCodecs VideoEncoder provides hardware H.264 encoding.
- **WebSocket for browser playback** as fallback because MoQ/WebTransport version negotiation hasn't been E2E tested. The WS path is proven working.
- **Init segment per group** in MoQ tracks so late-joining subscribers always get codec config. The WS relay skips duplicate init segments to avoid MSE errors.
- **Live-edge chasing** in the viewer (seek forward when >500ms behind) because MSE in sequence mode accumulates buffer without bound.
- **CORS permissive** on the admin/WS server for development. Should be restricted in production.

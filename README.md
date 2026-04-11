# LVQR - Live Video QUIC Relay

[![CI](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml/badge.svg)](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lvqr-core.svg)](https://crates.io/crates/lvqr-core)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE-MIT)

A Rust binary that relays live video using QUIC/MoQ. Built on moq-lite for zero-copy fan-out from ingest to delivery.

## Status (v0.3.1)

**End-to-end browser streaming works.** Webcam -> VideoEncoder H.264 -> WebSocket -> LVQR server -> fMP4 remux -> MoQ -> WebSocket -> MSE playback. Verified in Chrome.

**Working and tested (83 Rust tests, 8 Python tests, all CI green):**
- Browser-to-browser streaming via WebSocket ingest + relay (E2E verified)
- RTMP ingest via OBS/ffmpeg, remuxed from FLV to CMAF/fMP4 (2 integration tests, 24 remux unit tests)
- MoQ relay: QUIC/WebTransport fan-out to subscribers (4 integration tests)
- Mesh coordinator: tree assignment, orphan reassignment, signal push (13 + 7 tests)
- Admin HTTP API: stats, streams, mesh state, CORS enabled (6 tests)
- Test app with streamer, viewer, and admin dashboard (`test-app/`)

**Known limitations:**
- Video only (no audio in browser playback yet)
- WebRTC mesh peer media forwarding untested (coordination works, DataChannel relay untested)
- No stream authentication or recording

## Install

```bash
cargo install lvqr-cli
```

Or build from source:

```bash
git clone https://github.com/virgilvox/lvqr.git
cd lvqr
cargo build --release
```

## Quickstart

```bash
# Start the relay
lvqr serve

# Open the test app (stream from webcam, watch, monitor)
cd test-app && python3 -m http.server 9000
# Open http://localhost:9000 in Chrome
# Stream tab: Preview -> Go Live (streams webcam via WebCodecs H.264)
# Watch tab: Connect (plays via MSE)
# Admin tab: Refresh (shows live stats)

# Or stream from OBS/ffmpeg
# Server: rtmp://localhost:1935/live  Stream Key: my-stream
# Watch: ws://localhost:8080/ws/live/my-stream
```

## Architecture

```
OBS/ffmpeg --RTMP--> lvqr-ingest --remux--> lvqr-relay --QUIC/WT--> Browser
                     (FLV to CMAF)     |
                                  lvqr-mesh (peer tree coordination)
                                  lvqr-signal (WebRTC signaling)
                                  lvqr-admin (HTTP API + WS relay)
```

### Crates

| Crate | Description |
|-------|-------------|
| `lvqr-core` | Shared types, ring buffer, GOP cache, subscriber registry |
| `lvqr-relay` | MoQ relay on moq-lite with connection callbacks |
| `lvqr-ingest` | RTMP server + FLV-to-CMAF remuxer + MoQ bridge |
| `lvqr-mesh` | Peer tree coordinator with dead peer detection |
| `lvqr-signal` | WebRTC signaling server with mesh assignment push |
| `lvqr-admin` | HTTP API: stats, streams, mesh state |
| `lvqr-wasm` | WebAssembly browser bindings (WebTransport) |
| `lvqr-cli` | Single binary: relay + RTMP + admin + WS relay + mesh |

### How It Works

1. **Ingest**: OBS streams RTMP to LVQR. The bridge parses FLV (H.264/AAC), generates fMP4 init segments and media segments (moof+mdat), and writes them as MoQ track groups.
2. **Relay**: MoQ subscribers receive tracks via QUIC/WebTransport. Data is ref-counted (`bytes::Bytes`), each additional subscriber costs zero copies.
3. **Browser**: The TypeScript client performs MoQ SETUP handshake, subscribes to video tracks, receives fMP4 frames, feeds them to MSE SourceBuffer for playback.
4. **Fallback**: The `/ws/{broadcast}` WebSocket endpoint subscribes to MoQ tracks server-side and forwards fMP4 frames as binary messages for browsers without WebTransport.
5. **Mesh**: When `--mesh-enabled`, peers are assigned tree positions. Root peers receive from the server; relay peers forward to children via WebRTC DataChannels (coordination implemented, media forwarding untested).

## Client Libraries

| Package | Install | Description |
|---------|---------|-------------|
| Rust | `cargo add lvqr-core` | Core types and data structures |
| JavaScript | `npm install @lvqr/core` | MoQ client, admin client, mesh peer |
| JavaScript | `npm install @lvqr/player` | `<lvqr-player>` web component with MSE |
| Python | `pip install lvqr` | Admin API client |

## CLI Reference

```
lvqr serve [OPTIONS]
  --port <PORT>          QUIC/MoQ port [default: 4443]
  --rtmp-port <PORT>     RTMP ingest port [default: 1935]
  --admin-port <PORT>    Admin HTTP port [default: 8080]
  --mesh-enabled         Enable peer mesh relay
  --max-peers <N>        Max children per mesh peer [default: 3]
  --tls-cert <PATH>      TLS certificate (auto-generates if omitted)
  --tls-key <PATH>       TLS private key
  --config <PATH>        TOML config file
```

## Admin API

```bash
# Health check
curl http://localhost:8080/healthz

# List active streams
curl http://localhost:8080/api/v1/streams

# Server stats (connections, publishers, tracks)
curl http://localhost:8080/api/v1/stats

# Mesh state (peer count, offload percentage)
curl http://localhost:8080/api/v1/mesh
```

## Development

```bash
# Run a specific crate's tests (fast)
cargo test -p lvqr-ingest --lib remux
cargo test -p lvqr-ingest --test rtmp_bridge_integration
cargo test -p lvqr-relay --test relay_integration

# Run all tests
cargo test --workspace

# Benchmarks
cargo bench -p lvqr-core

# Format and lint
cargo fmt --all
cargo clippy --workspace
```

## Built On

- [moq-lite](https://github.com/kixelated/moq) - Media over QUIC transport
- [quinn](https://github.com/quinn-rs/quinn) - Rust QUIC implementation
- [rml_rtmp](https://crates.io/crates/rml_rtmp) - RTMP protocol
- [bytes](https://docs.rs/bytes) - Zero-copy byte buffers
- [tokio](https://tokio.rs) - Async runtime

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

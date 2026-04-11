# LVQR - Live Video QUIC Relay

[![CI](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml/badge.svg)](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lvqr-core.svg)](https://crates.io/crates/lvqr-core)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE-MIT)

A Rust binary that relays live video using QUIC/MoQ. Built on moq-lite for zero-copy fan-out from ingest to delivery.

## Status (v0.3.1)

**Working and tested (102 Rust tests, 8 Python tests, all CI green):**
- MoQ relay accepts QUIC/WebTransport connections, fans out tracks to subscribers (4 integration tests)
- RTMP ingest via OBS/ffmpeg, remuxed from FLV to CMAF/fMP4 for browser compatibility (2 integration tests, 24 remux unit tests)
- Mesh coordinator builds balanced trees, assigns peers, handles orphan reassignment (13 tests)
- Mesh wired to relay connections and signal server: peers get tree assignments on connect (7 signal tests)
- Admin HTTP API: relay stats, stream list, mesh state (6 tests)
- WebSocket fMP4 relay fallback for browsers without WebTransport
- Core data structures: ring buffer, GOP cache, subscriber registry (25 tests)
- Python admin client (8 tests)
- Benchmarks: Registry fanout ~230ns per publish to 500 subscribers

**Browser playback pipeline (implemented, needs live testing):**
- FLV-to-CMAF remuxing: H.264 SPS/PPS extraction, fMP4 init segments (ftyp+moov+avcC), media segments (moof+mdat)
- MoQ protocol framing in TypeScript: SETUP handshake (IETF Draft14 wire format), Announce, Subscribe, Group/Frame reading
- MSE player web component: auto-detects codec from init segment avcC box, SourceBuffer in sequence mode
- WebRTC mesh peer client: DataChannel connections via SDP/ICE exchange through signal server

**Known limitations:**
- Browser playback has not been tested end-to-end with a real OBS + browser session
- Peer-to-peer media forwarding via WebRTC DataChannels: peers get tree assignments and can connect, but media forwarding reliability is untested
- Audio playback: video-only by default (audio requires a separate MSE SourceBuffer, not yet wired in the player)
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
# Start the relay (auto-generates self-signed TLS cert)
lvqr serve

# Stream from OBS
# Server: rtmp://your-server:1935/live
# Stream Key: my-stream

# Watch via WebSocket fallback (no WebTransport needed)
# Connect a WebSocket client to ws://your-server:8080/ws/live/my-stream
# Receives fMP4 binary frames (init segments + moof+mdat)

# Watch via browser (WebTransport + MoQ)
# <lvqr-player src="https://your-server:4443/live/my-stream"
#              fingerprint="<cert-sha256-hex>" autoplay muted>
# </lvqr-player>
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

# LVQR - Live Video QUIC Relay

[![CI](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml/badge.svg)](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lvqr-core.svg)](https://crates.io/crates/lvqr-core)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE-MIT)

A Rust binary that relays live video using QUIC/MoQ. Built on moq-lite for zero-copy fan-out from ingest to delivery.

## Status (v0.2.0)

**Working and tested:**
- MoQ relay accepts QUIC/WebTransport connections, fans out tracks to subscribers (3 integration tests)
- RTMP ingest via OBS/ffmpeg, bridged to MoQ tracks (2 integration tests)
- Mesh coordinator builds balanced trees, handles orphan reassignment (13 tests)
- Admin HTTP API reports real relay metrics and active streams (4 tests)
- Core data structures: ring buffer, GOP cache, subscriber registry (25 tests)
- Python admin client (8 tests)

**Not yet working:**
- Browser client (`@lvqr/core`, `@lvqr/player`) connects but cannot play video. The MoQ subscribe protocol framing is not implemented in the WASM/TypeScript layer.
- Mesh relay between browser peers. The coordinator assigns tree positions but no code relays media between peers via WebRTC DataChannels.
- Per-stream subscriber counts in the admin API (requires moq-lite to expose per-broadcast state).
- io_uring zero-copy send path (behind feature flag, not yet implemented).

**Not yet benchmarked at scale.** The "5000+ viewers on a $6 VPS" claim below is a design target, not a measured result. Single-relay fan-out works; mesh offload does not.

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

# Stream from OBS
# Server: rtmp://your-server:1935/live
# Stream Key: my-stream

# Watch via MoQ client (browser playback not yet functional)
```

## Architecture

```
lvqr/
  lvqr-core         Ring buffer, GOP cache, subscriber registry
  lvqr-relay         MoQ relay on moq-lite, fan-out engine
  lvqr-mesh          Peer discovery, tree building, gossip
  lvqr-ingest        RTMP to MoQ track bridge
  lvqr-signal        WebRTC signaling for mesh bootstrap
  lvqr-admin         HTTP API, stats, health checks
  lvqr-wasm          WebAssembly browser bindings (incomplete)
  lvqr-cli           Single binary entry point
```

### How It Works

1. **Ingest**: OBS streams RTMP to LVQR. The bridge translates FLV video/audio to MoQ tracks.
2. **Relay**: MoQ subscribers receive tracks via QUIC/WebTransport. Data is ref-counted (`bytes::Bytes`), so each additional subscriber costs zero copies.
3. **Mesh** (planned): Viewers relay to other viewers via WebRTC DataChannels. The coordinator assigns tree positions but media relay is not yet implemented.

## Crates

| Crate | crates.io | Description |
|-------|-----------|-------------|
| `lvqr-core` | [![crates.io](https://img.shields.io/crates/v/lvqr-core.svg)](https://crates.io/crates/lvqr-core) | Core types, ring buffer, GOP cache |
| `lvqr-relay` | [![crates.io](https://img.shields.io/crates/v/lvqr-relay.svg)](https://crates.io/crates/lvqr-relay) | MoQ relay and fan-out engine |
| `lvqr-cli` | [![crates.io](https://img.shields.io/crates/v/lvqr-cli.svg)](https://crates.io/crates/lvqr-cli) | CLI binary |

## Client Libraries

| Package | Install | Status |
|---------|---------|--------|
| Rust | `cargo add lvqr-core` | Working |
| Python | `pip install lvqr` | Admin client only |
| JavaScript | `npm install @lvqr/core` | Connects but cannot play video |

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

# Server stats (connections, active publishers, tracks)
curl http://localhost:8080/api/v1/stats
```

## Built On

- [moq-lite](https://github.com/kixelated/moq) - Media over QUIC transport
- [quinn](https://github.com/quinn-rs/quinn) - Rust QUIC implementation
- [bytes](https://docs.rs/bytes) - Zero-copy byte buffers with ref-counting
- [tokio](https://tokio.rs) - Async runtime

## Development

```bash
# Run a specific crate's tests (fast)
cargo test -p lvqr-relay --lib
cargo test -p lvqr-ingest --test rtmp_bridge_integration

# Run all tests (slower)
cargo test --workspace

# Benchmarks
cargo bench -p lvqr-core

# Format and lint
cargo fmt --all
cargo clippy --workspace
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

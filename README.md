# LVQR - Live Video QUIC Relay

A single Rust binary that relays live video using QUIC/MoQ. Minimal-copy from ingest to delivery. Viewers become relays. Thousands of concurrent streams on a $6 droplet.

## Key Numbers

| Metric | LVQR | Traditional |
|--------|------|-------------|
| Binary size | ~5MB | 200MB+ (Java/Go) |
| Memory baseline | ~12MB | 200-500MB |
| Buffer copies per viewer | 0 | 4+ |
| Latency | <500ms | 2-6s (HLS) |
| Max viewers (single $6 VPS) | 5000+ (with mesh) | 150-300 |

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

# Watch (browser)
# https://your-server:8080/watch/my-stream
```

## Architecture

```
lvqr/
  lvqr-core         Ring buffer, GOP cache, subscriber registry
  lvqr-relay         MoQ relay on moq-lite, fan-out engine
  lvqr-mesh          Peer discovery, tree building, gossip
  lvqr-ingest        RTMP/WHIP/SRT to MoQ tracks
  lvqr-signal        WebRTC signaling for mesh bootstrap
  lvqr-admin         HTTP API, stats, health checks
  lvqr-wasm          WebAssembly browser bindings
  lvqr-cli           Single binary entry point
```

### How It Works

1. **Ingest**: OBS streams RTMP to LVQR. LVQR translates to MoQ tracks.
2. **Relay**: MoQ subscribers receive tracks via QUIC/WebTransport. Data is ref-counted (`bytes::Bytes`), so each additional subscriber costs zero copies.
3. **Mesh**: Viewers relay to other viewers via WebRTC DataChannels. The server seeds ~30 root peers; the mesh handles the rest. 75%+ CDN offload in production.

### Zero-Copy Data Path

```
QUIC Ingest --> Decrypt (userspace) --> Ring Buffer (Bytes ref) --> QUIC Send (io_uring zc)
                                             |
                                       Subscriber A gets Bytes::clone() (refcount bump, no copy)
                                       Subscriber B gets Bytes::clone()
                                       Subscriber C gets Bytes::clone()
```

## Crates

| Crate | crates.io | Description |
|-------|-----------|-------------|
| `lvqr-core` | [![crates.io](https://img.shields.io/crates/v/lvqr-core.svg)](https://crates.io/crates/lvqr-core) | Core types, ring buffer, GOP cache |
| `lvqr-relay` | [![crates.io](https://img.shields.io/crates/v/lvqr-relay.svg)](https://crates.io/crates/lvqr-relay) | MoQ relay and fan-out engine |
| `lvqr-cli` | [![crates.io](https://img.shields.io/crates/v/lvqr-cli.svg)](https://crates.io/crates/lvqr-cli) | CLI binary |

## Client Libraries

| Package | Install |
|---------|---------|
| JavaScript | `npm install @lvqr/core` |
| Python | `pip install lvqr` |
| Rust | `cargo add lvqr-core` |

## Browser Player

```html
<script src="https://cdn.jsdelivr.net/npm/@lvqr/core/dist/index.global.js"></script>
<lvqr-player src="https://relay.example.com/live/my-stream"></lvqr-player>
```

## Docker

```bash
docker run -p 4443:4443 -p 1935:1935 -p 8080:8080 ghcr.io/virgilvox/lvqr
```

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

# Server stats
curl http://localhost:8080/api/v1/stats
```

## Built On

- [moq-lite](https://github.com/kixelated/moq) - Media over QUIC transport (interoperable with Cloudflare CDN)
- [quinn](https://github.com/quinn-rs/quinn) - Production Rust QUIC implementation
- [bytes](https://docs.rs/bytes) - Zero-copy byte buffers with ref-counting
- [tokio](https://tokio.rs) - Async runtime

## Development

```bash
# Run all tests
cargo test --workspace

# Run relay integration tests
cargo test -p lvqr-relay --test relay_integration

# Format and lint
cargo fmt --all
cargo clippy --workspace
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full development guide.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

# LVQR Handoff Document

## Project Status

**Current Phase**: Milestone 1 complete, ready for Milestone 2 (RTMP ingest)
**Last Updated**: 2026-04-10
**Total Tests**: 32 passing (25 core + 3 relay integration + 4 admin)
**Clippy**: Zero warnings
**WASM**: Builds successfully via wasm-pack

## What's Done

### Scaffolding
- Workspace Cargo.toml with 9 crates, workspace.package/dependency inheritance
- CLAUDE.md, dual license MIT OR Apache-2.0
- CONTRIBUTING.md, SECURITY.md, CODE_OF_CONDUCT.md
- .gitignore, .rustfmt.toml (max_width=120), rust-toolchain.toml (stable + wasm32)
- GitHub Actions CI (fmt, clippy, test on ubuntu+macos, WASM build, Docker)
- Issue/PR templates, Dockerfile
- README.md, per-crate READMEs
- docs/architecture.md, docs/quickstart.md

### lvqr-core (25 tests)
- `RingBuffer`: Fixed-capacity circular buffer with `bytes::Bytes` ref-counted sharing
- `GopCache`: GOP cache with keyframe detection and LRU eviction
- `Registry`: tokio::sync::broadcast-based subscriber fanout with DashMap
- Core types: StreamId, SubscriberId, TrackName, Frame, Gop, RelayStats
- Benchmarks: criterion benchmarks for ring buffer operations

### lvqr-relay (3 integration tests)
- `RelayServer`: MoQ relay using moq-native Server + moq-lite Origin
- Real QUIC integration tests: publish/subscribe, fanout to 3 subscribers, metrics
- Publish/subscribe swap pattern
- Connection metrics tracking

### lvqr-admin (4 tests)
- Axum HTTP router: /healthz, /api/v1/stats, /api/v1/streams
- Tests using axum::test::oneshot

### lvqr-test-utils
- Port allocation, synthetic frame generation, TLS cert generation

### Stub Crates (Scaffolded, compilable, zero clippy warnings)
- lvqr-ingest: RtmpServer config + error types
- lvqr-mesh: MeshCoordinator with MeshConfig, PeerInfo
- lvqr-signal: SignalServer with SignalMessage
- lvqr-wasm: WASM init + version
- lvqr-cli: clap CLI with `serve` subcommand

## What's Next

### Milestone 2: RTMP Ingest
1. Wire rml_rtmp to accept RTMP connections
2. Parse FLV, extract H.264 NALUs
3. Publish as MoQ tracks via `origin.create_broadcast()` + `broadcast.create_track()`
4. Integration test: TCP RTMP handshake -> MoQ subscriber receives frames

### Milestone 3: CLI Wiring
1. Connect relay + ingest + admin in CLI binary
2. TOML config file support
3. Graceful shutdown

### Milestone 4: Peer Mesh
1. MeshCoordinator: tree topology, peer assignment
2. Gossip protocol for health monitoring
3. lvqr-signal: WebRTC signaling over WebSocket

### Milestone 5: Browser Playback
1. lvqr-wasm: WebTransport MoQ client
2. @lvqr/core npm package with TypeScript wrapper
3. @lvqr/player Web Component

## Key Architecture Notes

### moq-lite Origin Pattern
The relay creates a shared `OriginProducer`. Every connection gets:
```rust
request.with_publish(origin.consume()).with_consume(origin).ok().await
```
moq-lite handles ALL track routing internally. The relay is just a connection manager.

### RTMP Ingest Integration
Call `origin.create_broadcast("live/streamkey")` and `broadcast.create_track(Track::new("video"))` to inject media into the Origin. MoQ subscribers see it automatically.

### Edition 2024
The workspace uses Rust edition 2024 (stabilized in Rust 1.85, Feb 2025). This is correct and intentional. The project requires rust-version 1.85+.

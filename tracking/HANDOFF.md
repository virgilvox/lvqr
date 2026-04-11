# LVQR Handoff Document

## Project Status

**Current Phase**: Milestones 1-5 complete. 6 crates published to crates.io.
**Last Updated**: 2026-04-10
**Total Tests**: 51 passing (25 core + 3 relay + 4 admin + 2 ingest + 13 mesh + 4 signal)
**Published**: lvqr-core, lvqr-signal, lvqr-relay, lvqr-ingest, lvqr-mesh, lvqr-admin (all 0.1.0)
**Pending Publish**: lvqr-cli (rate limited, retry after 2026-04-11 01:31 GMT)

## Published Crates

| Crate | Version | crates.io |
|-------|---------|-----------|
| lvqr-core | 0.1.0 | https://crates.io/crates/lvqr-core |
| lvqr-signal | 0.1.0 | https://crates.io/crates/lvqr-signal |
| lvqr-relay | 0.1.0 | https://crates.io/crates/lvqr-relay |
| lvqr-ingest | 0.1.0 | https://crates.io/crates/lvqr-ingest |
| lvqr-mesh | 0.1.0 | https://crates.io/crates/lvqr-mesh |
| lvqr-admin | 0.1.0 | https://crates.io/crates/lvqr-admin |

## What's Implemented

### lvqr-core (25 tests)
- `RingBuffer`: Fixed-capacity circular buffer with `bytes::Bytes` ref-counted sharing
- `GopCache`: GOP cache with keyframe detection and LRU eviction
- `Registry`: tokio::sync::broadcast-based subscriber fanout with DashMap
- Core types, error types, benchmarks

### lvqr-relay (3 integration tests)
- `RelayServer`: MoQ relay using moq-native Server + moq-lite Origin
- Real QUIC integration tests: publish/subscribe, fanout, metrics
- Publish/subscribe swap pattern

### lvqr-ingest (2 tests)
- Full RTMP handshake and session handling via rml_rtmp
- Callback-based media event architecture
- `RtmpMoqBridge`: bridges RTMP to MoQ Origin (creates broadcasts/tracks)
- FLV keyframe detection

### lvqr-mesh (13 tests)
- `MeshCoordinator`: relay tree topology with balanced peer assignment
- Root peer management, relay tree depth control
- Peer removal with orphan detection and reassignment
- Heartbeat-based dead peer detection
- 50-peer large tree formation test

### lvqr-signal (4 tests)
- `SignalServer`: WebSocket signaling for WebRTC peer connections
- Register/Offer/Answer/IceCandidate message forwarding
- Per-peer channels for message delivery

### lvqr-admin (4 tests)
- Axum HTTP: /healthz, /api/v1/stats, /api/v1/streams

### lvqr-cli
- Full CLI with relay + RTMP ingest + admin running concurrently
- Graceful Ctrl+C shutdown

### Infrastructure
- GitHub Actions CI, Dockerfile, README, per-crate READMEs
- docs/architecture.md, docs/quickstart.md
- CONTRIBUTING.md, SECURITY.md, CODE_OF_CONDUCT.md

## What's Next

1. **Publish lvqr-cli** to crates.io (after rate limit expires ~2026-04-11 01:31 GMT)
2. **WASM/JS bindings**: lvqr-wasm WebTransport client, @lvqr/core npm package
3. **Python bindings**: admin client
4. **Examples**: obs-to-browser walkthrough, docker-compose
5. **Wire mesh into CLI**: connect MeshCoordinator + SignalServer to relay

## Key Technical Notes

### DashMap Deadlock Prevention
In `MeshCoordinator::reassign_peer`, we must NOT hold a `get_mut()` write lock while calling `find_best_parent()` (which iterates the map). This causes DashMap deadlock. Fixed by finding the parent first, then acquiring the write lock.

### Publishing Order
Must publish in strict tier order with waits between:
```
Tier 0: lvqr-core
Tier 1: lvqr-signal
Tier 2: lvqr-relay, lvqr-ingest, lvqr-mesh
Tier 3: lvqr-admin
Tier 4: lvqr-cli
```
crates.io has a rate limit: ~10 new crates per period. We hit it after 6.

### Test Performance
`cargo test --workspace` is slow (~5min) due to compilation + doc-tests. For development, test individual crates: `cargo test -p lvqr-mesh --lib`.

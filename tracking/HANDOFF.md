# LVQR Handoff Document

## Project Status: v0.2.0 Released

**Last Updated**: 2026-04-10
**Tests**: 53 Rust + 8 Python = 61 total
**Benchmarks**: 2 (ringbuffer, fanout)

## All Packages Published

### crates.io (Rust)
| Crate | Version | Install |
|-------|---------|---------|
| lvqr-core | 0.2.0 | `cargo add lvqr-core` |
| lvqr-signal | 0.2.0 | `cargo add lvqr-signal` |
| lvqr-relay | 0.2.0 | `cargo add lvqr-relay` |
| lvqr-ingest | 0.2.0 | `cargo add lvqr-ingest` |
| lvqr-mesh | 0.2.0 | `cargo add lvqr-mesh` |
| lvqr-admin | 0.2.0 | `cargo add lvqr-admin` |
| lvqr-wasm | 0.2.0 | `cargo add lvqr-wasm` |
| lvqr-cli | 0.2.0 | `cargo install lvqr-cli` |

### npm (JavaScript)
| Package | Version | Install |
|---------|---------|---------|
| @lvqr/core | 0.2.0 | `npm install @lvqr/core` |
| @lvqr/player | 0.2.0 | `npm install @lvqr/player` |

### PyPI (Python)
| Package | Version | Install |
|---------|---------|---------|
| lvqr | 0.2.0 | `pip install lvqr` |

## Changes in v0.2.0

### Fixed
- Admin API now reports real stats from relay metrics and RTMP bridge state (was reading from empty Registry)
- lvqr-wasm compiles on both native and wasm32 targets (was broken on native due to web-sys cfg gating)
- Full workspace `cargo check/clippy` works including lvqr-wasm

### Added
- RTMP end-to-end integration tests (2 tests: video keyframe + audio data flow through bridge to MoQ subscriber)
- Registry fanout throughput benchmark (~230ns to publish to 500 subscribers with 4KB frames)
- Honest README Status section documenting what works vs. what is planned

### Documented
- lvqr-core's actual role (shared types + standalone data structures, NOT the relay hot path)

## Architecture Summary

```
OBS/ffmpeg --RTMP--> lvqr-ingest --MoQ--> lvqr-relay --QUIC/WT--> Browser
                                              |
                                     lvqr-mesh (peer tree)
                                     lvqr-signal (WebRTC signaling)
                                     lvqr-admin (HTTP API)
```

## What Works (tested)

- MoQ relay: QUIC fan-out to multiple subscribers (3 integration tests)
- RTMP ingest: full OBS/ffmpeg -> bridge -> MoQ pipeline (2 integration tests)
- Mesh coordinator: balanced tree topology, orphan reassignment (13 tests)
- Admin API: real stats from relay metrics and bridge (4 tests)
- Core data structures: ring buffer, GOP cache, registry (25 tests)
- WASM client: WebTransport connection with fingerprint auth
- Python admin client (8 tests)

## What Does Not Work Yet

- Browser video playback: transport connects but MoQ subscribe framing not implemented
- Mesh media relay: tree positions assigned but no peer-to-peer data forwarding
- Per-stream subscriber counts in admin API
- io_uring zero-copy send path

## Future Work

- Full MoQ protocol framing in WASM (currently transport-level only)
- MSE/WebCodecs video rendering pipeline in @lvqr/core
- io_uring send path on Linux (feature-gated, tokio fallback exists)
- WS-fMP4 and LL-HLS fallback output from relay
- Stream authentication (keys, tokens)
- Recording (S3/Spaces segment archival)
- Multi-server federation

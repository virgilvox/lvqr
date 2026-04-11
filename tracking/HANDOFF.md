# LVQR Handoff Document

## Project Status: v0.3.1 Released

**Last Updated**: 2026-04-11
**Tests**: 102 Rust + 8 Python = 110 total
**Benchmarks**: 2 (ringbuffer, fanout)
**CI**: All green (Format+Lint, Test ubuntu, Test macos, WASM Build, Docker Build)

## All Packages Published

### crates.io (Rust)
| Crate | Version | Install |
|-------|---------|---------|
| lvqr-core | 0.3.1 | `cargo add lvqr-core` |
| lvqr-signal | 0.3.1 | `cargo add lvqr-signal` |
| lvqr-relay | 0.3.1 | `cargo add lvqr-relay` |
| lvqr-ingest | 0.3.1 | `cargo add lvqr-ingest` |
| lvqr-mesh | 0.3.1 | `cargo add lvqr-mesh` |
| lvqr-admin | 0.3.1 | `cargo add lvqr-admin` |
| lvqr-wasm | 0.3.1 | `cargo add lvqr-wasm` |
| lvqr-cli | 0.3.1 | `cargo install lvqr-cli` |

### npm (JavaScript)
| Package | Version | Install |
|---------|---------|---------|
| @lvqr/core | 0.3.1 | `npm install @lvqr/core` |
| @lvqr/player | 0.3.1 | `npm install @lvqr/player` |

### PyPI (Python)
| Package | Version | Install |
|---------|---------|---------|
| lvqr | 0.3.1 | `pip install lvqr` |

## What Works (tested)

- **RTMP ingest**: OBS/ffmpeg -> RTMP -> bridge (2 integration tests)
- **FLV-to-CMAF remuxing**: H.264 SPS/PPS extraction, fMP4 init+media segments (24 unit tests)
- **MoQ relay**: QUIC fan-out to multiple subscribers (4 integration tests)
- **Mesh coordination**: tree assignment, orphan reassignment, dead peer detection (13 tests)
- **Mesh wiring**: relay connection callback + signal server push assignments (7+1 tests)
- **Admin API**: stats, streams, mesh state (6 tests)
- **WASM client**: compiles for native + wasm32 (CI verified)
- **Python admin client**: healthz, stats, list_streams (8 tests)
- **Benchmarks**: fanout ~230ns/publish to 500 subs

## What's Implemented But Untested in Production

- **Browser MoQ subscribe**: SETUP handshake (IETF Draft14 u16 size + QUIC varint), Announce, Subscribe, Group/Frame reading
- **MSE player**: auto-detect codec from avcC box, SourceBuffer sequence mode, buffer trimming
- **WebSocket fMP4 relay**: /ws/{broadcast} forwards fMP4 binary frames
- **WebRTC mesh peer**: RTCPeerConnection + DataChannel via SDP/ICE exchange

## Protocol Bugs Found and Fixed (v0.3.0 -> v0.3.1)

| Bug | Impact |
|-----|--------|
| CLIENT/SERVER_SETUP size: varint instead of u16 BE | Every connection would fail |
| Path encoding: segmented array instead of plain string | Every subscribe would 404 |
| AnnouncePlease: 1 empty segment instead of 0 segments | Discovery returns nothing |
| Subscribe priority: varint instead of u8 | Misparse for priority > 63 |
| trun box version 0 for signed CTS offsets | B-frame timestamps wrong |
| Hardcoded codec string in player | Fails for non-High profile H.264 |
| Video+audio to single SourceBuffer | MSE crash on first audio frame |

All found by reading moq-lite Rust source, not by running the code.

## Architecture

```
OBS/ffmpeg --RTMP--> lvqr-ingest --remux--> lvqr-relay --QUIC/WT--> Browser
                     (FLV to CMAF)     |
                                  lvqr-mesh (peer tree)
                                  lvqr-signal (WebRTC signaling + mesh push)
                                  lvqr-admin (HTTP API + WS fMP4 relay)
```

## What's Left

- End-to-end browser test with real OBS + Chrome
- Audio playback (separate MSE SourceBuffer)
- WebRTC DataChannel media forwarding reliability testing
- Stream authentication (keys, tokens)
- Recording (S3/Spaces segment archival)
- Multi-server federation

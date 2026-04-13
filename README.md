# LVQR - Live Video QUIC Relay

[![CI](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml/badge.svg)](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lvqr-core.svg)](https://crates.io/crates/lvqr-core)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE-MIT)

A Rust binary that relays live video using QUIC/MoQ. Built on moq-lite for zero-copy fan-out from ingest to delivery.

## Status (v0.4-dev)

**End-to-end RTMP to browser playback works**, with a real integration
test that drives `rml_rtmp` through the bridge to a `tokio-tungstenite`
WebSocket subscriber and verifies byte-for-byte that an fMP4 init segment
(`ftyp`) and media segment (`moof`) arrive on the wire. Zero mocks.

**Working and tested** (29 test binaries workspace-wide, 130+ individual
tests including 2560 generated proptest cases, all green):

- RTMP ingest via OBS / ffmpeg, remuxed FLV to fMP4 (CMAF) via
  `lvqr-ingest`. Proptest harness asserts the parser never panics on
  arbitrary bytes and the fMP4 writer produces structurally well-formed
  ISO BMFF for any plausible config and sample list.
- WebSocket browser ingest via the `@lvqr/core` and `@lvqr/player`
  TypeScript packages plus the bundled `test-app/`.
- MoQ relay (QUIC / WebTransport fanout) via `lvqr-relay` wrapping
  `moq-lite` 0.15. Five integration tests including fanout to multiple
  subscribers and connection callback wiring.
- Pluggable authentication via `lvqr-auth`: noop, static tokens, and
  feature-gated HS256 JWT (wired into the CLI via `--jwt-secret` /
  `LVQR_JWT_SECRET`). Constant-time comparison verified.
- Disk recording via `lvqr-record` as a MoQ subscriber that writes
  fMP4 init and media segments. Subscribes to lifecycle events on
  `lvqr_core::EventBus`, so WebSocket-ingested broadcasts are recorded
  identically to RTMP-ingested ones. Covered by an integration test.
- Peer mesh topology planner via `lvqr-mesh` with tree assignment,
  orphan reassignment, dead-peer detection, and a regression test for
  live reassignment. **Topology only**: real WebRTC DataChannel media
  forwarding is not yet implemented.
- Admin HTTP API via `lvqr-admin` with stats, streams, mesh state,
  Prometheus metrics, and admin-token auth middleware.
- Tokens travel in `Sec-WebSocket-Protocol: lvqr.bearer.<token>`, not
  query strings.

**Known limitations:**

- No HLS, LL-HLS, DASH, WHIP, WHEP, SRT, or RTSP egress or ingest yet.
  Tracked in `tracking/ROADMAP.md` Tier 2.
- WebRTC mesh is topology only; DataChannel media forwarding is not
  implemented. The offload percentage in the admin API is intended
  offload, not actual.
- `lvqr-wasm` is deprecated. Use `@lvqr/core` and `@lvqr/player` for
  browser clients.
- Single-codec: H.264 Baseline + AAC-LC only. HEVC, VP9, AV1, Opus
  land with `lvqr-codec` in Tier 2.2.

**Read before contributing:**

- [`tracking/ROADMAP.md`](tracking/ROADMAP.md) -- the 18-24 month plan.
- [`tracking/AUDIT-2026-04-13.md`](tracking/AUDIT-2026-04-13.md) --
  competitive audit vs MediaMTX, LiveKit, OvenMediaEngine, SRS, Ant
  Media, AWS KVS, Janus, and Jitsi.
- [`tracking/AUDIT-INTERNAL-2026-04-13.md`](tracking/AUDIT-INTERNAL-2026-04-13.md) --
  internal dead code, bug, and hardening review.
- [`tracking/AUDIT-READINESS-2026-04-13.md`](tracking/AUDIT-READINESS-2026-04-13.md) --
  CI, supply chain, doc drift, and Tier 1 progress inventory.
- [`tests/CONTRACT.md`](tests/CONTRACT.md) -- the 5-artifact test
  contract every new protocol feature must ship.

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
| `lvqr-core` | Shared types, `EventBus` for lifecycle events |
| `lvqr-auth` | `AuthProvider` trait plus noop, static-token, and JWT providers |
| `lvqr-relay` | MoQ relay wrapping `moq-lite` with auth, metrics, and connection callbacks |
| `lvqr-ingest` | RTMP server, FLV-to-fMP4 remuxer, `RtmpMoqBridge` |
| `lvqr-record` | Disk recorder that subscribes to MoQ broadcasts and writes fMP4 |
| `lvqr-mesh` | Peer tree topology planner (topology only; media forwarding TBD) |
| `lvqr-signal` | WebRTC signaling server that pushes mesh assignments |
| `lvqr-admin` | HTTP API: stats, streams, mesh, Prometheus metrics, admin auth |
| `lvqr-conformance` | Reference fixtures and external-validator wrappers |
| `lvqr-cli` | Single binary: relay + RTMP + WS ingest/relay + admin + optional recorder + optional mesh |
| `lvqr-wasm` | **Deprecated**; use `@lvqr/core` and `@lvqr/player` instead |

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
  --port <PORT>            QUIC/MoQ port [default: 4443]
  --rtmp-port <PORT>       RTMP ingest port [default: 1935]
  --admin-port <PORT>      Admin HTTP port [default: 8080]
  --mesh-enabled           Enable peer mesh topology planner
  --max-peers <N>          Max children per mesh peer [default: 3]
  --tls-cert <PATH>        TLS certificate (auto-generates if omitted)
  --tls-key <PATH>         TLS private key
  --admin-token <TOKEN>    Bearer token for /api/v1/* (env: LVQR_ADMIN_TOKEN)
  --publish-key <KEY>      Required RTMP / WS publish key (env: LVQR_PUBLISH_KEY)
  --subscribe-token <TOK>  Required viewer token (env: LVQR_SUBSCRIBE_TOKEN)
  --record-dir <PATH>      Directory to record broadcasts into (env: LVQR_RECORD_DIR)
  --jwt-secret <SECRET>    HS256 secret enabling JWT auth (env: LVQR_JWT_SECRET)
  --jwt-issuer <ISS>       Expected JWT `iss` claim (env: LVQR_JWT_ISSUER)
  --jwt-audience <AUD>     Expected JWT `aud` claim (env: LVQR_JWT_AUDIENCE)
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

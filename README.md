# LVQR - Live Video QUIC Relay

[![CI](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml/badge.svg)](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lvqr-core.svg)](https://crates.io/crates/lvqr-core)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE-MIT)

A Rust binary that relays live video using QUIC/MoQ. Built on moq-lite for zero-copy fan-out from ingest to delivery.

## Status (v0.4-dev, session 28 close)

**Tier 2.3 data plane is closed and Tier 2.4 archive is live.** Real
end-to-end coverage across three browser-facing egress paths lands
from a single RTMP publish, and a redb-backed DVR segment index
plus a JSON + byte-serving HTTP surface lets clients scrub recorded
segments by decode-time window:

- **RTMP -> MoQ -> WebSocket fMP4** (the original path; still honest,
  still tested by `rtmp_ws_e2e`).
- **RTMP -> CMAF -> LL-HLS** with multi-broadcast routing
  (`/hls/{broadcast}/...`), a synthesized master playlist, and an
  audio rendition group. `rtmp_hls_e2e` publishes two concurrent
  RTMP broadcasts plus an RTMP publish with interleaved video and
  AAC audio, then reads back the master playlist, audio playlist,
  and audio init segment over a real TCP HTTP/1.1 client.
- **WHEP video egress** (`/whep/{broadcast}` POST / PATCH / DELETE)
  with a full `str0m` backend behind the `SdpAnswerer` trait: ICE,
  DTLS, SRTP, and H.264 RTP packetization all complete against a
  real browser. `--whep-port` (env `LVQR_WHEP_PORT`, default 0 =
  disabled) attaches the `WhepServer` as a
  `RawSampleObserver` on `RtmpMoqBridge`, and the in-process
  loopback E2E test at `crates/lvqr-whep/tests/e2e_str0m_loopback.rs`
  proves a client `Rtc` receives video frames from a publishing
  `Str0mAnswerer` in ~0.15-0.18s of wall time. Audio (Opus) is out
  of scope pending an AAC -> Opus transcoder.
- **DVR archive + playback HTTP surface** via `lvqr-archive`
  (Tier 2.4). `--archive-dir` (env `LVQR_ARCHIVE_DIR`) opens a
  redb-backed `SegmentIndex` at `<dir>/archive.redb` and attaches
  an `IndexingFragmentObserver` to the RTMP bridge. Every fragment
  is written to `<dir>/<broadcast>/<track>/<seq>.m4s` and recorded
  as a `SegmentRef` row. The admin router gains three routes when
  the flag is set: `GET /playback/{*broadcast}?track=&from=&to=`
  (JSON array of overlapping rows, sorted by `start_dts`),
  `GET /playback/latest/{*broadcast}?track=` (single anchor row
  or 404), and `GET /playback/file/{*rel}` (raw fragment bytes
  with a canonicalized path-traversal guard). Every route
  consults the shared `SharedAuth` provider: with
  `NoopAuthProvider` the archive stays open, with
  `--subscribe-token` or `--jwt-secret` set the archive inherits
  the same credential as live subscribe. `Authorization: Bearer`
  header and `?token=` query fallback are both honored. Covered
  end-to-end by `crates/lvqr-cli/tests/rtmp_archive_e2e.rs`,
  which publishes a real RTMP stream into a temp archive dir and
  then walks the redb file, the JSON API, the latest anchor, the
  file-serve route, a traversal-attack rejection, and a
  protected-auth test that exercises both the header and
  query-parameter bearer transports.

**Working and tested** (80 test binaries workspace-wide, 346
individual tests, 0 failures under the default feature set,
`cargo clippy --workspace --all-targets -- -D warnings` clean,
`cargo fmt --all --check` clean; 0 `todo!()` / `unimplemented!()`
macros and exactly one `TODO` comment in the entire `crates/`
tree):

- **RTMP ingest** via OBS / ffmpeg, parsed into `lvqr_cmaf::RawSample`
  values and routed through `lvqr_cmaf::build_moof_mdat`. The
  hand-rolled fMP4 video media-segment writer was retired in
  session 14; the audio init + media writers remain in `lvqr-ingest`
  until `lvqr-cmaf` grows a matching AAC coalescer. Proptest and
  ffprobe conformance coverage pins the writer invariants.
- **Unified fragment model** via `lvqr-fragment`: `Fragment`,
  `FragmentFlags`, `FragmentMeta`, `MoqTrackSink`. The producer
  shape the RTMP bridge uses and the shape every future egress
  crate will plug into.
- **CMAF segmenter + coalescer** via `lvqr-cmaf`: `CmafPolicy`,
  `CmafPolicyState`, `TrackCoalescer`, `CmafSampleSegmenter`,
  `build_moof_mdat`, `write_avc_init_segment`. Kvazaar
  multi-sub-layer HEVC fixture + AAC parity gate live in the
  conformance crate.
- **LL-HLS egress** via `lvqr-hls`: `PlaylistBuilder`, `HlsServer`,
  `MultiHlsServer` with master-playlist synthesis and audio
  rendition group declaration. `AUDIO_48KHZ_DEFAULT` is no longer
  hardcoded into the audio policy; the bridge picks the real
  sample rate at init time so 44.1 kHz AAC reports the correct
  wall-clock `#EXT-X-PART:DURATION`.
- **WHEP video egress** via `lvqr-whep`: `WhepServer`,
  `SdpAnswerer` / `SessionHandle` trait boundary, `Str0mAnswerer`
  running a per-session sans-IO poll loop over a loopback UDP
  socket, video media write via `str0m::media::Writer` with an
  AVCC -> Annex B boundary conversion (str0m's `H264Packetizer`
  scans for Annex B start codes and silently drops AVCC input;
  the converter is load-bearing), and a four-slot test spread
  covering proptest (RFC 6184 round-trip + never-panic), fuzz
  (`SdpOffer::from_sdp_string` under libFuzzer), integration
  (12 tests against the axum router via `tower::ServiceExt::oneshot`),
  and E2E (client `Rtc` + server `Str0mAnswerer` over loopback,
  asserts decoded `Event::MediaData` on the client side).
- **WHIP video ingest** via `lvqr-whip`: `WhipServer`,
  `SdpAnswerer` / `SessionHandle` trait boundary, `Str0mIngestAnswerer`
  running the same sans-IO poll-loop shape as WHEP but in the
  ingest direction (reading `Event::MediaData` instead of
  writing through `Writer::write`), an Annex B -> AVCC
  converter that is the inverse of the WHEP boundary
  converter, and a `WhipMoqBridge` that lazily builds an AVC
  **or HEVC** init segment from the first parameter-set-bearing
  IRAP and fans every subsequent sample through the
  `SharedFragmentObserver` and `SharedRawSampleObserver`
  taps for H.264 publishers (observer fanout is AVC-only;
  HEVC publishers reach MoQ subscribers via the `0.mp4`
  track but bypass LL-HLS / WHEP / archive until those
  surfaces grow codec-aware sample entries in session 27).
  Both codecs share the sibling-bridge pattern and the
  `VideoCodec` tag on `IngestSample` picks the init path.
  Five-artifact test coverage: 22 unit (including 4 HEVC
  tests for VPS/SPS/PPS extraction + broadcast init), 10
  signaling integration (axum `oneshot`), 3 proptest
  properties on the depacketizer (never-panics + well-formed
  round trip), plus two in-process str0m E2E loopbacks -- one
  H.264 keyframe loopback and one HEVC keyframe loopback that
  uses real x265 VPS/SPS/PPS bytes and asserts the capture
  sink receives an HEVC-tagged keyframe through ICE + DTLS +
  SRTP + str0m's H265 packetizer pair. Enabled via
  `--whip-port` / `LVQR_WHIP_PORT` (default 0 = disabled).
- **WebSocket browser ingest + egress** via the `@lvqr/core` and
  `@lvqr/player` TypeScript packages plus the bundled `test-app/`.
- **MoQ relay** (QUIC / WebTransport fanout) via `lvqr-relay`
  wrapping `moq-lite` 0.15 behind the `lvqr-moq` facade crate.
- **Pluggable authentication** via `lvqr-auth`: noop, static
  tokens, and feature-gated HS256 JWT wired into the CLI via
  `--jwt-secret` / `LVQR_JWT_SECRET`. Constant-time comparison,
  invalid-broadcast-name path rejection, and auth-failure metrics
  (`lvqr_auth_failures_total{entry="..."}`) fire on every entry
  point including the admin middleware.
- **Disk recording** via `lvqr-record`. Subscribes to
  `lvqr_core::EventBus`, so WebSocket-ingested broadcasts are
  recorded identically to RTMP-ingested ones.
- **Peer mesh topology planner** via `lvqr-mesh` with tree
  assignment, orphan reassignment, live rebalance (old-parent
  children list cleanup), and dead-peer detection. **Topology
  only**: real WebRTC DataChannel media forwarding is Tier 4.
- **Admin HTTP API** via `lvqr-admin` with stats, streams, mesh
  state, Prometheus metrics, and admin-token auth middleware.

**Known limitations:**

- WHEP audio is dropped. RTMP carries AAC, WHEP negotiates Opus,
  no in-tree AAC -> Opus transcoder. The audio one-shot warns and
  drops; video works end-to-end. See
  `crates/lvqr-whep/docs/media-write.md` for the rationale.
- WHEP trickle ICE ingestion logs once and returns success; WHEP
  rarely needs trickle when the offer already embeds every host
  candidate.
- Real-browser WHEP E2E in CI is gated on packaging a reference
  WHEP client binary (e.g. `simple-whep-client`) into the CI
  image. The in-process loopback E2E covers every byte of the
  server's send path.
- No DASH, WHIP, SRT, or RTSP egress or ingest. Tracked in
  `tracking/ROADMAP.md` Tier 2.
- WebRTC mesh is topology only; DataChannel media forwarding is
  not implemented. The offload percentage in the admin API is
  intended offload, not actual.
- `lvqr-wasm` is deprecated. Use `@lvqr/core` and `@lvqr/player`
  for browser clients.
- Codec surface: H.264 Baseline + AAC-LC is the tested happy
  path. HEVC init writing and AAC parsing hardening live in
  `lvqr-codec` / `lvqr-cmaf`; real HEVC / AV1 / Opus ingest
  through the bridge is a later session.
- CORS is `permissive()` by default. Tracked as Tier 3 hardening;
  tighten before public deployment.
- The `/playback/*` archive surface inherits the admin router's
  permissive CORS default; browser clients on any origin can
  read DVR segments when the server is unauthenticated. Set
  `--subscribe-token` or `--jwt-secret` to gate the archive
  behind the same credential as live subscribe; both the
  `Authorization: Bearer` header and the `?token=` query
  parameter are honored on every playback route. CORS
  tightening is tracked in `tracking/HANDOFF.md` session-25
  recommended entry point item 2.

**Read before contributing:**

- [`tracking/HANDOFF.md`](tracking/HANDOFF.md) -- the rolling
  session-by-session handoff doc. Start at the top; the most
  recent session entry is the source of truth for current state.
- [`tracking/ROADMAP.md`](tracking/ROADMAP.md) -- the 18-24 month
  plan and 10 load-bearing architectural decisions.
- [`tracking/AUDIT-2026-04-13.md`](tracking/AUDIT-2026-04-13.md) --
  competitive audit vs MediaMTX, LiveKit, OvenMediaEngine, SRS,
  Ant Media, AWS KVS, Janus, and Jitsi.
- [`tracking/AUDIT-INTERNAL-2026-04-13.md`](tracking/AUDIT-INTERNAL-2026-04-13.md) --
  internal dead code, bug, and hardening review. All five "Fix
  Plan for This Session" items closed as of session 17.
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
                                             +-> lvqr-relay --QUIC/WT--> Browser (MoQ)
                                             |
OBS/ffmpeg --RTMP--> lvqr-ingest --Fragment--+-> lvqr-cli WS relay --WebSocket fMP4--> Browser
                    (FLV to CMAF)            |
                                             +-> lvqr-hls MultiHlsServer --HTTP LL-HLS--> Browser
                                             |   (master.m3u8 + audio rendition)
                                             |
                                             +-> lvqr-whep Str0mAnswerer --WebRTC (ICE/DTLS/SRTP)--> Browser
                                             |
                                             +-> lvqr-archive IndexingFragmentObserver
                                                 (redb segment index + on-disk fragments)
                                                 --HTTP /playback/* --> DVR scrub client

Supporting crates:
  lvqr-fragment  -- unified `Fragment` model every egress crate consumes
  lvqr-cmaf      -- RawSample coalescer, CmafPolicy, build_moof_mdat
  lvqr-codec     -- AVC / HEVC / AAC parser surface (shared)
  lvqr-moq       -- facade over moq-lite + moq-native
  lvqr-auth      -- AuthProvider: noop / static / HS256 JWT
  lvqr-record    -- disk recorder driven by EventBus lifecycle
  lvqr-archive   -- redb segment index for DVR scrub / time-range playback
  lvqr-mesh      -- peer tree topology planner (no media forwarding yet)
  lvqr-signal    -- WebRTC signaling server (mesh assignments)
  lvqr-admin     -- HTTP API: stats, streams, mesh, Prometheus metrics
  lvqr-conformance -- reference fixtures + external-validator wrappers
```

### Crates

| Crate | Description |
|-------|-------------|
| `lvqr-core` | Shared types (`StreamId`, `TrackName`, `Frame`, `RelayStats`) and the `EventBus` for lifecycle events |
| `lvqr-moq` | Facade over `moq-lite` / `moq-native`; keeps upstream churn to one crate |
| `lvqr-fragment` | `Fragment` + `FragmentMeta` + `MoqTrackSink` unified media interchange |
| `lvqr-codec` | AVC / HEVC / AAC parsers (SPS, `AudioSpecificConfig`, `read_ue_v`) with proptest + fuzz coverage |
| `lvqr-cmaf` | `RawSample`, `TrackCoalescer`, `CmafPolicy`, `build_moof_mdat`, AVC + HEVC + AAC init writers |
| `lvqr-hls` | `PlaylistBuilder`, `HlsServer`, `MultiHlsServer` with master playlist and audio rendition group |
| `lvqr-whep` | WHEP video egress: `WhepServer` axum router, `Str0mAnswerer` with sans-IO poll loop, `str0m::Writer`-driven media write with AVCC -> Annex B boundary conversion, proptest + fuzz + integration + loopback-E2E test coverage |
| `lvqr-auth` | `AuthProvider` trait plus noop, static-token, and HS256 JWT providers |
| `lvqr-relay` | MoQ relay wrapping `moq-lite` with auth, metrics, and connection callbacks |
| `lvqr-ingest` | RTMP server, FLV parser, `RtmpMoqBridge`, `FragmentObserver` + `RawSampleObserver` hooks |
| `lvqr-record` | Disk recorder that subscribes to MoQ broadcasts and writes fMP4 |
| `lvqr-archive` | redb-backed `SegmentIndex` for DVR scrub and time-range playback (find_range, latest); `IndexingFragmentObserver` in `lvqr-cli` populates it from the RTMP bridge |
| `lvqr-mesh` | Peer tree topology planner (topology only; media forwarding TBD in Tier 4) |
| `lvqr-signal` | WebRTC signaling server that pushes mesh assignments; validated peer IDs and tracks |
| `lvqr-admin` | HTTP API: stats, streams, mesh, Prometheus metrics, admin auth + auth-failure metric |
| `lvqr-conformance` | Reference fixtures (kvazaar HEVC, ffprobe corpus) and external-validator wrappers |
| `lvqr-cli` | Single binary: relay + RTMP + WS ingest/relay + LL-HLS + admin + optional recorder + optional mesh |
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
  --hls-port <PORT>        LL-HLS HTTP port; set to 0 to disable [default: 8888]
  --whep-port <PORT>       WHEP HTTP port; set to 0 to disable [default: 0] (env: LVQR_WHEP_PORT)
  --mesh-enabled           Enable peer mesh topology planner
  --max-peers <N>          Max children per mesh peer [default: 3]
  --tls-cert <PATH>        TLS certificate (auto-generates if omitted)
  --tls-key <PATH>         TLS private key
  --admin-token <TOKEN>    Bearer token for /api/v1/* (env: LVQR_ADMIN_TOKEN)
  --publish-key <KEY>      Required RTMP / WS publish key (env: LVQR_PUBLISH_KEY)
  --subscribe-token <TOK>  Required viewer token (env: LVQR_SUBSCRIBE_TOKEN)
  --record-dir <PATH>      Directory to record broadcasts into (env: LVQR_RECORD_DIR)
  --archive-dir <PATH>     DVR archive directory; enables /playback/* on the admin port (env: LVQR_ARCHIVE_DIR)
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

### DVR playback (only when `--archive-dir` is set)

```bash
# Every archived video segment for a broadcast, oldest first
curl 'http://localhost:8080/playback/live/my-stream'

# Decode-time window scrub (track timescale, not wallclock)
curl 'http://localhost:8080/playback/live/my-stream?track=0.mp4&from=0&to=1800000'

# Most-recent segment anchor (for "jump to live minus 10 seconds" clients)
curl 'http://localhost:8080/playback/latest/live/my-stream'

# Raw fragment bytes by relative path (see the `segment_seq` field on each row)
curl 'http://localhost:8080/playback/file/live/my-stream/0.mp4/00000001.m4s'
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

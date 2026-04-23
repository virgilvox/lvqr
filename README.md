# LVQR

[![CI](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml/badge.svg)](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lvqr-core.svg)](https://crates.io/crates/lvqr-core)
[![License](https://img.shields.io/badge/license-AGPL--3.0%20or%20commercial-blue.svg)](LICENSE)

A single-binary Rust live video server. Ingests RTMP, WHIP, SRT,
RTSP, and WebSocket fMP4; serves LL-HLS, MPEG-DASH, WHEP, MoQ over
QUIC/WebTransport, and WebSocket fMP4; records + archives to disk
with a DVR index and optional C2PA signing; optionally forms a
gossip cluster with broadcast ownership and redirect-to-owner.

```bash
cargo install lvqr-cli
lvqr serve
```

## Why LVQR

Every ingest and every egress is a projection over the same
unified fragment model, so adding a protocol is a projection, not
a rewrite. The data plane is zero-copy (`bytes::Bytes`), the
control plane is `async-trait`, and cluster state is chitchat
gossip with ownership leases rather than a consensus bolt-on.

Target positioning: **MediaMTX-grade ergonomics + Kinesis-grade
archive + MoQ as a first-class transport**, on a path toward
LiveKit-class differentiators (WASM per-fragment filters,
in-process AI agents, cross-cluster federation, peer mesh).

## Feature overview

### Ingest
- **RTMP** over TCP (OBS, ffmpeg, Larix, vMix)
- **WHIP** over HTTPS (WebRTC; H.264, HEVC, Opus)
- **SRT** over UDP (MPEG-TS from broadcast encoders)
- **RTSP/1.0** over TCP (ANNOUNCE/RECORD, interleaved RTP)
- **WebSocket fMP4** (browser publishers)

### Egress
- **LL-HLS** (RFC 8216bis): blocking playlist reload, delta
  playlists, `EXT-X-PART` + `PRELOAD-HINT`, per-segment
  `PROGRAM-DATE-TIME`, configurable DVR, audio renditions, master
  playlist, ABR ladder variants, automatic `ENDLIST` on disconnect
- **MPEG-DASH**: live-profile dynamic MPD with flip to
  `type="static"` on disconnect
- **WHEP** WebRTC egress via `str0m` (H.264 + HEVC video, Opus
  audio). WHIP Opus publishers pass through; RTMP / SRT / RTSP
  AAC publishers reach WHEP subscribers via an in-process
  `AacToOpusEncoder` (GStreamer, behind the `transcode` Cargo
  feature). **Fixed on `main` in session 113**; see
  [Known v0.4.0 limitations](#known-v040-limitations)
- **MoQ** over QUIC / WebTransport via `moq-lite`, zero-copy
  fanout. Chrome / Edge 107+ via the `@lvqr/player` web component
  (published at v0.3.1 on npm).
- **WebSocket fMP4** for browsers without WebTransport
- **DVR scrub** via `/playback/*` backed by a `redb` segment index.
  Segment fetches honor RFC 7233 `Range: bytes=` single-range
  requests, so HTML5 `<video>` seekability works out of the box.

### Programmable data plane
- **WASM per-fragment filters** (`--wasm-filter <path>`,
  `LVQR_WASM_FILTER`) via `wasmtime 25`. Guests observe every
  ingested fragment and may drop it (negative return) or rewrite
  its payload bytes (non-negative length return). `notify`-backed
  hot-reload atomically swaps the running filter. Fragment
  metadata (track id, PTS, DTS, flags) is read-only in v1;
  multi-filter chaining is on the v1.1 roadmap. Examples under
  `crates/lvqr-wasm/examples/`.
- **In-process AI agents** (`lvqr-agent`, `lvqr-agent-whisper`).
  One drain task per agent per `(broadcast, track)`,
  panic-isolated, per-agent metrics. `--whisper-model <path>`
  with `--features whisper` turns on a WhisperCaptionsAgent that
  emits WebVTT at `/hls/{broadcast}/captions/playlist.m3u8`.
- **Server-side transcoding** (`lvqr-transcode`, `--features
  transcode`). Software ABR ladder via a GStreamer pipeline on
  a dedicated worker thread, plus an always-available
  `AudioPassthroughTranscoderFactory`. Drive via
  `--transcode-rendition <NAME>` (presets `720p`/`480p`/`240p` or
  a `.toml` `RenditionSpec`). The LL-HLS master playlist composer
  emits one `#EXT-X-STREAM-INF` per rendition automatically.

### Provenance + signing
- **C2PA signed media** (`--features c2pa`). Drain-terminated
  finalize on broadcast end writes
  `<archive>/<broadcast>/<track>/finalized.mp4` +
  `finalized.c2pa`. Admin route `GET /playback/verify/{*broadcast}`
  returns a JSON validation report
  (`{ signer, signed_at, valid, validation_state, errors }`).
  Dual signer source: on-disk PEMs via `--c2pa-signing-cert` +
  `--c2pa-signing-key` (plus optional `--c2pa-signing-alg`,
  `--c2pa-trust-anchor`, `--c2pa-timestamp-authority`) or a
  custom `Arc<dyn c2pa::Signer>` for HSM/KMS-backed keys passed
  programmatically through `ServeConfig.c2pa`.

### Auth
- Pluggable: noop, static tokens, or HS256 JWT with `iss` + `aud`
  validation.
- **One token, every ingest.** The same JWT admits a publisher
  across RTMP (stream key IS the JWT), WHIP
  (`Authorization: Bearer`), SRT (`streamid=m=publish,r=<broadcast>,t=<jwt>`),
  RTSP (`Authorization: Bearer`), and WebSocket ingest
  (`lvqr.bearer.<jwt>` subprotocol). Per-broadcast claim binding
  enforced where the carrier knows the broadcast name at auth
  time. Subscribe-side: WHEP handshake, WebSocket relay
  (`/ws/*`), live LL-HLS + MPEG-DASH playback, DVR playback
  (`/playback/*`), and admin (`/api/v1/*`) all apply the
  `SubscribeAuth` provider. Tokens ride the
  `Authorization: Bearer` header; live HLS + DASH also accept
  `?token=<token>` as a fallback for native `<video>` players
  that cannot set headers. `--no-auth-live-playback` is the
  escape hatch for deployments that want open live HLS + DASH
  with auth scoped to ingest, admin, and DVR only. **Not gated
  today: the mesh `/signal` WebSocket.** See
  [Known v0.4.0 limitations](#known-v040-limitations) and
  [`docs/auth.md`](docs/auth.md).

### Storage
- **fMP4 recorder** (`--record-dir`) subscribed to `EventBus`.
- **DVR archive** (`--archive-dir`) with a `redb` segment index +
  `/playback/*` scrub routes + Linux `io-uring` writes behind the
  `io-uring` feature flag.

### Observability
- Prometheus scrape at `/metrics`.
- OTLP gRPC span + metric export (`LVQR_OTLP_ENDPOINT`) with
  `metrics-util::FanoutBuilder` composition alongside Prometheus.
- **Latency SLO tracker** + alert pack. `lvqr_subscriber_glass_to_glass_ms`
  histogram captures per-`(broadcast, transport)` server-side
  glass-to-glass latency. Four transports instrumented today:
  `"hls"`, `"dash"`, `"ws"`, `"whep"`. Query
  `GET /api/v1/slo` for a ring-buffered p50/p95/p99/max snapshot;
  scrape the histogram for time-aligned views. Prometheus rule
  pack + Grafana dashboard under `deploy/grafana/`. Operator
  runbook at [`docs/slo.md`](docs/slo.md).
- See [`docs/observability.md`](docs/observability.md) for the
  full surface.

### Cluster
- **Chitchat gossip plane**: broadcast-ownership KV with lease
  renewal, per-node capacity advertisement, LWW config,
  redirect-to-owner for HLS / DASH / RTSP, and a full admin
  surface at `/api/v1/cluster/{nodes,broadcasts,config}`.
- **Cross-cluster federation**: one-way authenticated MoQ pulls
  from peer clusters via `FederationLink`. Exponential-backoff
  reconnect (base 1 s, 60 s cap, +/-10% jitter).
  `GET /api/v1/cluster/federation` returns per-link
  `state` / `last_connected_at_ms` / `last_error` /
  `connect_attempts`. See [`docs/cluster.md`](docs/cluster.md).
- **Peer mesh**: topology planner + WebSocket signaling server
  + server-side subscriber registration + client-side WebRTC
  DataChannel parent/child relay ship today
  (`--mesh-enabled`, `--max-peers`, `--mesh-root-peer-count`).
  A two-browser Playwright E2E (`bindings/js/tests/e2e/mesh/`)
  exercises the full signal-to-DataChannel-delivery chain on
  every CI run. Operator-grade completion (actual-vs-intended
  offload reporting, per-peer capacity advertisement, TURN
  deployment recipe, three-peer matrix) is on the phase-D
  roadmap; see [`docs/mesh.md`](docs/mesh.md).

## Quickstart

### 1. Start the server

```bash
lvqr serve
```

Zero-config defaults:

| Surface | Port | Protocol | Default |
|---|---|---|---|
| MoQ relay | 4443/udp | QUIC / WebTransport | always on |
| RTMP ingest | 1935/tcp | RTMP | always on |
| LL-HLS | 8888/tcp | HTTP/1.1 | always on |
| Admin + WebSocket | 8080/tcp | HTTP/1.1 + WebSocket | always on |
| DASH | `--dash-port` | HTTP/1.1 | off |
| WHEP | `--whep-port` | HTTPS/WebRTC | off |
| WHIP | `--whip-port` | HTTPS/WebRTC | off |
| RTSP | `--rtsp-port` | RTSP/1.0 over TCP | off |
| SRT | `--srt-port` | SRT over UDP | off |

A self-signed TLS cert is generated at boot if `--tls-cert` /
`--tls-key` are not supplied. Fine for local dev, not production.

### 2. Publish

```bash
# RTMP from ffmpeg
ffmpeg -re -f lavfi -i testsrc=size=640x360:rate=30 \
  -f lavfi -i sine=frequency=440:sample_rate=44100 \
  -c:v libx264 -preset ultrafast -tune zerolatency \
  -c:a aac -b:a 128k \
  -f flv rtmp://localhost:1935/live/demo

# RTSP from ffmpeg (requires --rtsp-port 8554)
ffmpeg -re -i source.mp4 -c copy -f rtsp rtsp://localhost:8554/live/demo

# SRT from ffmpeg (requires --srt-port 8890)
ffmpeg -re -i source.mp4 -c copy -f mpegts srt://localhost:8890?streamid=live/demo

# WHIP from OBS 30+ or ffmpeg (requires --whip-port 8443)
# Service: WHIP, URL: https://localhost:8443/whip/live/demo
```

### 3. Play back

- **LL-HLS**: `http://localhost:8888/hls/live/demo/playlist.m3u8`
- **DASH**: `http://localhost:8889/dash/live/demo/manifest.mpd`
- **WHEP**: browser WebRTC player at
  `https://localhost:8443/whep/live/demo`
- **MoQ**: Chrome/Edge 107+ via the `@lvqr/player` web component
  (published at `@lvqr/player@0.3.1` on npm)
- **WebSocket fMP4**: `ws://localhost:8080/ws/live/demo` (MSE
  fallback for browsers without WebTransport)

The `test-app/` directory demonstrates the WebSocket path end to
end: `cd test-app && ./serve.sh` exposes a browser demo at
`http://localhost:3000`.

### 4. Observe

```bash
curl http://localhost:8080/healthz             # liveness
curl http://localhost:8080/api/v1/streams      # active broadcasts
curl http://localhost:8080/api/v1/stats        # connection counts
curl http://localhost:8080/api/v1/slo          # latency SLO snapshot
curl http://localhost:8080/metrics             # Prometheus scrape
```

Set `LVQR_OTLP_ENDPOINT=http://collector:4317` to stream spans +
metrics to an OTLP gRPC collector.

## Running as a cluster

```bash
# Node A
lvqr serve \
  --cluster-listen 10.0.0.1:10007 \
  --cluster-advertise-hls http://10.0.0.1:8888

# Node B joins via seed
lvqr serve \
  --cluster-listen 10.0.0.2:10007 \
  --cluster-seeds 10.0.0.1:10007 \
  --cluster-advertise-hls http://10.0.0.2:8888
```

The first publisher for `live/demo` on either node auto-claims
ownership and renews on a lease. A subscriber hitting the
non-owner receives a 302 to the owner's advertised URL for HLS,
DASH, or RTSP. See [`docs/cluster.md`](docs/cluster.md) for the
full model, ops recipes, and tuning knobs.

## Client libraries

| Language | Install | Version | Description |
|---|---|---|---|
| Rust | `cargo add lvqr-core` | 0.4.0 (crates.io) | Shared types, `EventBus`, admin client |
| JavaScript | `npm i @lvqr/core` | 0.3.1 (npm) | MoQ-Lite subscriber over WebTransport, WebSocket fMP4 fallback, admin client, `MeshPeer` WebRTC DataChannel relay (two-peer happy path verified via a Playwright browser E2E; operator-grade completion on the phase-D roadmap). `main` post-v0.3.1 adds `pushFrame(data)` on `MeshPeer`, an `onChildOpen(id, dc)` callback in `MeshConfig`, and `connectTimeoutMs` / `fetchTimeoutMs` on `LvqrClient` / `LvqrAdminClient`; land at the next publish cycle. |
| JavaScript | `npm i @lvqr/player` | 0.3.1 (npm) | Drop-in `<lvqr-player>` web component with MSE fallback |
| Python | `pip install lvqr` | 0.3.1 (PyPI) | Admin API client (no streaming surface) |

See [`docs/sdk/javascript.md`](docs/sdk/javascript.md) for the JS
API reference and
[`bindings/python/python/lvqr/`](bindings/python/python/lvqr/) for
the Python module.

## Roadmap

Tier 1 (protocols), Tier 2 (unified fragment model + cluster
plane), Tier 3 (cluster auth + redirect-to-owner), and Tier 4
(programmable data plane: WASM filters, io_uring archive, C2PA,
cross-cluster federation, AI agents, ABR transcoding, latency
SLO, one-token auth) all **ship** as of v0.4.0. The Tier 4 exit
criterion -- a working
[`examples/tier4-demos/`](examples/tier4-demos/) public demo
script -- landed on `main` post-v0.4.0 in session 117
(`demo-01.sh` chains WASM filter + whisper captions + ABR
transcode + DVR archive end to end; C2PA sign + verify is
opt-in via `LVQR_DEMO_C2PA=1`).

Full v1.1 phase plan with session-by-session sequencing lives in
[`tracking/PLAN_V1.1.md`](tracking/PLAN_V1.1.md). A 29-crate
inventory + codebase reality audit anchors the current state in
`tracking/HANDOFF.md` session-121 block.

### Next up (ranked by impact / ship-ability)

Ordering reflects a 2026-04-22 codebase audit against the v1.1
plan. Higher items are smaller, closer to shippable, and close
gaps explicitly named in Known v0.4.0 limitations.

1. ~~**Expand `@lvqr/core` admin client to cover the 6 missing
   `/api/v1/*` routes**.~~ **Shipped in session 122.** All 9
   admin routes now ship on `LvqrAdminClient` (`healthz`,
   `stats`, `listStreams`, `mesh`, `slo`, `clusterNodes`,
   `clusterBroadcasts`, `clusterConfig`, `clusterFederation`),
   each with a declared TypeScript response type. Lands on
   npm at the next publish cycle.
2. ~~**Wire Vitest + pytest into CI.**~~ **Shipped in session
   122.** New `.github/workflows/sdk-tests.yml` runs the
   `@lvqr/core` Vitest suite (10 admin-client shape tests
   against a live `lvqr serve`) and the Python client pytest
   suite (8 type + mocked-httpx tests) on every push + PR.
   Soft-fail (`continue-on-error: true`) during its initial
   weeks on `main`.
3. ~~**Feature-flag CI matrix.**~~ **Shipped in session 123.**
   New [`feature-matrix.yml`](.github/workflows/feature-matrix.yml)
   has three jobs: `c2pa` (runs `c2pa_verify_e2e` +
   `c2pa_cli_flags_e2e` + `lvqr-archive --features c2pa`),
   `transcode` (installs GStreamer + ffmpeg, runs `aac_opus_roundtrip`
   + `software_ladder` + `transcode_ladder_e2e` +
   `rtmp_whep_audio_e2e`), and `whisper` (compile-check +
   `whisper_basic` unit tests; full `whisper_cli_e2e` remains
   `#[ignore]` pending a cached-model workflow). Each cell
   explicitly lists every feature-gated test target so new ones
   get a visible cell to update.
4. ~~**HMAC-signed playback URLs**~~ **Shipped in session 124.**
   `--hmac-playback-secret` activates a short-circuit auth path
   on every `/playback/*` route: `?exp=<unix_ts>&sig=<base64url>`
   where `sig = HMAC-SHA256(secret, "<path>?exp=<ts>")`. Valid
   signature grants access without a bearer; tampered / expired
   returns 403 (not 401) so clients can distinguish missing auth
   from wrong auth. Operator helper `lvqr_cli::sign_playback_url`
   generates the query suffix from a secret + path + expiry.
5. **OAuth2 / JWKS dynamic key discovery** (PLAN row 120).
   Larger auth surface than HMAC; adds operator-grade JWT
   flexibility alongside the existing static-secret HS256 path.
6. **Nightly 24h soak CI job** (PLAN row 119). Scheduled
   workflow running `lvqr-soak` for a full day, scoped
   soft-fail for the first week.
7. **Mesh data-plane phase D**: actual-vs-intended offload
   reporting, per-peer capacity advertisement, TURN deployment
   recipe, three-peer Playwright E2E. Unblocks flipping
   `docs/mesh.md` to IMPLEMENTED.
8. **One hardware encoder backend** (VideoToolbox on macOS or
   NVENC on Linux). The three others stay deferred to v1.2.
9. **Stream-modifying WASM filter chains** (multi-filter
   composition). v1 single-filter drop + rewrite ships today.
10. **SDK reconnect / retry docs**, **webhook auth provider**,
    **stream-key CRUD admin API**, **hot config reload**,
    **dedicated DVR scrub web UI**, **SCTE-35 passthrough** --
    smaller or more-speculative items; pick based on operator
    demand.

The list below groups the same remaining work by logical area.

### Client SDKs (shipped; completion work pending)
JavaScript (`@lvqr/core`, `@lvqr/player` at 0.3.1 on npm), Python
(`lvqr` at 0.3.1 on PyPI, admin client only), and Rust
(`lvqr-core` at 0.4.0 on crates.io) already ship. Remaining work:
- [x] ~~**Expand `@lvqr/core` admin client** from 3 of 9
  `/api/v1/*` routes to all 9.~~ Shipped in session 122:
  `LvqrAdminClient` now exposes `mesh()`, `slo()`,
  `clusterNodes()`, `clusterBroadcasts()`, `clusterConfig()`,
  `clusterFederation()` alongside the existing `healthz()`,
  `stats()`, `listStreams()`. TypeScript response types for
  every route land at the next npm publish cycle.
- [x] ~~**Vitest + pytest in CI.**~~ Shipped in session 122 as
  [`sdk-tests.yml`](.github/workflows/sdk-tests.yml): boots
  `lvqr serve` with `--mesh-enabled` + `--cluster-listen`,
  then runs `@lvqr/core`'s Vitest suite
  ([10 admin-client shape tests](bindings/js/tests/sdk/admin-client.spec.ts))
  and the Python client's existing pytest suite
  ([8 type + mocked-httpx tests](bindings/python/tests/test_client.py)).
- [x] ~~**Expand Python admin client** from 3 of 9 routes to
  all 9.~~ Shipped in session 123:
  `bindings/python/python/lvqr/client.py` now mirrors the JS
  admin client 1:1 (`mesh`, `slo`, `cluster_nodes`,
  `cluster_broadcasts`, `cluster_config`, `cluster_federation`)
  with matching dataclasses + an optional `bearer_token` kwarg.
  Pytest coverage grows from 8 to 21 tests. Lands on PyPI at
  the next publish cycle.
- [ ] Document reconnect + retry semantics in
  [`docs/sdk/javascript.md`](docs/sdk/javascript.md) (currently
  silent on reconnect; `connectTimeoutMs` + `fetchTimeoutMs`
  shipped on `main` but not explained).
- [x] ~~First `examples/tier4-demos/` public demo script.~~ Shipped
  in session 117 as
  [`examples/tier4-demos/demo-01.sh`](examples/tier4-demos/demo-01.sh),
  chaining the WASM filter, whisper captions, ABR transcode,
  DVR archive surfaces end to end, plus opt-in C2PA sign +
  verify via `LVQR_DEMO_C2PA=1` (session 121). Closes the
  Tier 4 exit criterion that was left open when Tier 4 was
  marked COMPLETE.

### Peer mesh data plane
Topology planner, WebSocket signaling, `/api/v1/mesh` admin route,
and the client-side `MeshPeer` (WebRTC DataChannel forwarding,
opening `RTCPeerConnection` to the assigned parent, forwarding to
children) already exist. The data-plane gap is
browser-to-browser DataChannel media relay and an end-to-end
test.
- [x] ~~Server-side subscriber registration.~~ Every
  `ws_relay_session` now calls `MeshCoordinator::add_peer` at
  connect time (server-generated `ws-{counter}` peer_id) and
  sends a leading `peer_assignment` JSON text frame on the WS
  before any binary MoQ frames. Shipped in session 111-B2.
- [x] ~~Subscribe-token admission on `/signal`.~~ Shipped in
  sessions 111-B1 + 111-B3 via
  `Sec-WebSocket-Protocol: lvqr.bearer.<token>` (preferred) and
  `?token=<token>` query fallback.
- [x] ~~`ServerHandle::mesh_coordinator()` snapshot accessor.~~
  Shipped in session 111-B1 for in-process integration tests.
- [x] ~~MoQ-over-DataChannel wire format decision.~~ Locked in
  session 111-B1 as an 8-byte big-endian `object_id` prefix +
  raw MoQ frame bytes per DataChannel message. Documented in
  `docs/mesh.md`.
- [x] ~~**Two-peer end-to-end browser test** proving a subscriber
  connected through the DataChannel mesh receives the same
  bytes via the peer relay as via the server-direct path.~~
  Shipped in session 115 (row 115) as
  [`bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts`](bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts).
  The `mesh-e2e.yml` CI workflow runs it on every push to `main`.
  `MeshPeer.pushFrame(data)` was added on `main` in the same
  session so the root peer can forward server-drained media
  into the mesh tree; `MeshConfig.onChildOpen(id, dc)` was
  added as a post-116 follow-up for integrators who need a
  deterministic one-shot push on DataChannel open.
- [ ] **Actual-vs-intended offload reporting**: clients report
  "served by peer X"; coordinator aggregates; `/api/v1/mesh`
  returns measured offload.
- [ ] **Per-peer capacity advertisement** so rebalancing uses
  bandwidth + CPU instead of hardcoded `max-children`.
- [ ] **TURN deployment recipe** + STUN fallback config. Document
  coturn integration for peers behind symmetric NAT.
- [ ] **Three-peer browser Playwright E2E** feeding the 5-artifact
  test contract.
- [ ] Flip [`docs/mesh.md`](docs/mesh.md) from "topology planner
  only" to "IMPLEMENTED". (The two-peer slice ships; the phase-D
  items above gate the "IMPLEMENTED" flip.)

### Egress + encoders
- [x] ~~**WHEP audio transcoder (AAC to Opus)** atop the 4.6
  GStreamer pipeline so RTMP publishers reach browser WebRTC
  with audio.~~ Shipped on `main` in session 113 as
  `lvqr-transcode::AacToOpusEncoder` (behind the `transcode`
  Cargo feature). Exercised by
  `crates/lvqr-cli/tests/rtmp_whep_audio_e2e.rs` on the
  GStreamer-enabled CI matrix.
- [x] ~~Live HLS and DASH subscribe auth.~~ Shipped in session
  112 via a tower middleware applied to the HLS and DASH
  routers at the CLI composition root. Auth on by default when
  the `SubscribeAuth` provider is configured (Noop provider
  deployments see no behavior change);
  `--no-auth-live-playback` is the escape hatch for deployments
  that want open live playback with auth scoped to ingest,
  admin, and DVR.
- [ ] **One hardware encoder backend** (VideoToolbox for macOS or
  NVENC for Linux, picked per deployment target). Remaining
  three backends (VAAPI, QSV, and whichever of NVENC or
  VideoToolbox is not picked first) deferred to v1.2.
- [ ] **Stream-modifying WASM filter chains.** v1 already lets
  single filters drop or rewrite fragment payload bytes; v1.1
  lets operators compose multiple filters.
- [ ] **MoQ egress latency SLO.** Server-side measurement would
  require a MoQ wire change that was explicitly rejected in the
  v1.1-B scoping call (keeps foreign MoQ clients compatible).
  Likely path forward: Tier 5 client SDK pushes back sampled
  render-side timestamps to a future
  `POST /api/v1/slo/client-sample` endpoint.

### Auth + ops polish
- [ ] **Webhook auth provider.**
- [ ] **OAuth2 / JWKS dynamic key discovery.**
- [x] ~~**HMAC-signed URLs** for one-off playback links.~~
  Shipped in session 124. `--hmac-playback-secret` enables a
  short-circuit auth path on `/playback/*` via
  `?exp=<unix_ts>&sig=<base64url>`; `lvqr_cli::sign_playback_url`
  is the server-side helper for generating the query suffix.
  See `docs/auth.md#signed-playback-urls`.
- [ ] **Stream-key CRUD admin API.**
- [ ] **Hot config reload.**
- [ ] **Dedicated DVR scrub web UI.**
- [ ] **SCTE-35 passthrough.** (WebVTT captions already ship via
  the whisper-captions HLS rendition.)

**Source of truth for session-by-session progress:**
[`tracking/HANDOFF.md`](tracking/HANDOFF.md).

## Known v0.4.0 limitations

Operators planning deployments should read these before shipping.
Items flagged **Fixed on `main`** have shipped in commits on
`origin/main` after the v0.4.0 crates.io release; they land for
consumers on the next release cycle. Operators who need the fix
today should build from `main` instead of pinning to the
published crate.

- **Mesh `/signal` WebSocket is not auth-gated.** The v0.4.0
  crate accepts `Register` messages from any client. Operators
  not using the peer mesh should leave `--mesh-enabled` off (the
  default); operators using it should front `/signal` with a
  reverse proxy gate. **Fixed on `main`** in sessions 111-B1 +
  111-B3: `/signal` now accepts the subscribe bearer via
  `Sec-WebSocket-Protocol: lvqr.bearer.<token>` (preferred) or
  `?token=<token>` query fallback, with `--no-auth-signal` as
  the escape hatch.
- **Live HLS + DASH routes were not auth-gated in v0.4.0**
  even when `--subscribe-token` or `--jwt-secret` was set.
  **Fixed on `main`** in session 112: the HLS and DASH routers
  are now wrapped with the same `SubscribeAuth` gate as
  `/ws/*`, with `--no-auth-live-playback` as the escape hatch
  for deployments that want open live playback.
- **WHEP has no AAC audio.** The v0.4.0 crate dropped AAC audio
  with a one-shot warning so every RTMP / SRT / RTSP / WS
  publisher reached WHEP subscribers video-only. **Fixed on
  `main`** in session 113: a new `lvqr-transcode::AacToOpusEncoder`
  (behind the `transcode` feature) pipes AAC access units through
  an in-process GStreamer pipeline (`appsrc ! aacparse !
  avdec_aac ! audioresample ! opusenc ! appsink`) and pushes the
  Opus packets back into the WHEP session's Opus writer. The
  transcoder is lazily spawned per session once the publisher's
  AAC AudioSpecificConfig arrives. Builds without the `transcode`
  feature retain the legacy drop-with-warn behaviour.
- **`/metrics` is unauthenticated.** Intentional, but document
  this to your ops team. Scope the scrape endpoint via firewall
  or reverse proxy if the deployment is multi-tenant.
- **Hardware encoders are not shipped.** `lvqr-transcode` only
  offers a software x264 pipeline (behind the `transcode` Cargo
  feature). NVENC, VideoToolbox, VAAPI, and QSV backends are
  on the v1.1 and v1.2 roadmap.
- **C2PA signer paths are covered by two integration tests.**
  `crates/lvqr-cli/tests/c2pa_cli_flags_e2e.rs` exercises the
  on-disk `C2paSignerSource::CertKeyFiles` path through both
  rcgen and openssl cert generation; `c2pa_verify_e2e.rs` covers
  the programmatic `Custom(Arc<dyn Signer>)` path via
  `c2pa::EphemeralSigner`. Any operator using a common PEM
  layout (CA + leaf with `digitalSignature` KU,
  `emailProtection` EKU, `CN` + `O` in the subject DN,
  `AuthorityKeyIdentifier` on the leaf) hits the tested surface.
- **Pure MoQ subscribers do not contribute to the latency SLO
  histogram.** LL-HLS, MPEG-DASH, WebSocket fMP4, and WHEP are
  instrumented; MoQ subscribers are not, by design (the
  alternative required a MoQ wire change that was rejected).
  Client-side SDK push-back is the intended path.
- **No admission control.** The SLO tracker measures latency and
  fires alerts; it does not refuse new subscribers when the SLO
  is already burning.
- **No nightly 24h soak in CI.** `lvqr-soak` has a fast-path
  smoke test; the long-duration soak job is on the v1.1
  checklist.
- **Feature-flag CI matrix initially soft-fail.**
  [`feature-matrix.yml`](.github/workflows/feature-matrix.yml)
  ships as of session 123 with dedicated jobs for the `c2pa`,
  `transcode`, and `whisper` features on `lvqr-cli` (covering
  every feature-gated integration test target explicitly); the
  workflow is `continue-on-error: true` during its first weeks
  on `main` per the convention every other new dedicated
  workflow in this repo has followed. Promotion to a required
  check after the first clean run. `whisper_cli_e2e` remains
  `#[ignore]` because it needs a ~78 MB ggml model download;
  a scheduled-workflow follow-up will cache the model + flip it
  on.
- **Client SDK admin coverage at 9/9 on `main`, 3/9 on the
  last publish.** Both `@lvqr/core` (session 122) and the
  Python `lvqr` package (session 123) now cover every
  `/api/v1/*` route the admin router mounts. The published
  npm + PyPI builds at 0.3.1 still ship the 3-method surface;
  the 9-method surface lands for consumers at the next publish
  cycle.
- **SDK reconnect + retry semantics are undocumented.**
  `@lvqr/core`'s `LvqrClient` + `LvqrAdminClient` ship
  `connectTimeoutMs` / `fetchTimeoutMs` on `main` (both land at
  the next publish cycle), but the SDK docs at
  [`docs/sdk/javascript.md`](docs/sdk/javascript.md) do not yet
  explain when to reconnect, what the backoff should be, or how
  to handle partial fetches. A phase-C row fills this in
  alongside the admin-client expansion.

## CLI reference

```
lvqr serve [OPTIONS]

  Core ports (always on):
  --port <PORT>             QUIC/MoQ port [default: 4443]
  --rtmp-port <PORT>        RTMP ingest port [default: 1935]
  --admin-port <PORT>       Admin HTTP + WS port [default: 8080]
  --hls-port <PORT>         LL-HLS HTTP port; 0 to disable [default: 8888]

  Optional protocols (off unless port set):
  --dash-port <PORT>        MPEG-DASH HTTP port
  --whep-port <PORT>        WHEP WebRTC egress port
  --whip-port <PORT>        WHIP WebRTC ingest port
  --rtsp-port <PORT>        RTSP ingest port
  --srt-port <PORT>         SRT ingest port

  LL-HLS tuning:
  --hls-dvr-window <SECS>   DVR depth [default: 120; 0 = unbounded]
  --hls-target-duration <S> Segment duration [default: 2]
  --hls-part-target <MS>    Partial duration [default: 200]

  Auth (env LVQR_*):
  --admin-token <T>         /api/v1/* bearer
  --publish-key <K>         Required publish credential
  --subscribe-token <T>     Required subscriber credential
  --jwt-secret <S>          Enable HS256 JWT (replaces static tokens)
  --jwt-issuer <I>          Expected iss claim
  --jwt-audience <A>        Expected aud claim

  Storage:
  --record-dir <PATH>       fMP4 recording directory
  --archive-dir <PATH>      DVR archive dir, enables /playback/*
  --hmac-playback-secret <SECRET>
                            HMAC-SHA256 secret for signed
                            playback URLs. When set, every
                            /playback/* handler accepts
                            ?exp=<ts>&sig=<b64url> as an
                            alternative auth path that
                            short-circuits the subscribe-token
                            gate. See docs/auth.md for the URL
                            shape + the `lvqr_cli::sign_playback_url`
                            operator helper.
                            Env: LVQR_HMAC_PLAYBACK_SECRET.

  WASM filter (read-only tap in v1):
  --wasm-filter <PATH>      Path to a .wasm module exporting
                            on_fragment(ptr, len) -> i32. Hot-
                            reloaded on file change.
                            Env: LVQR_WASM_FILTER.

  Captions (requires --features whisper at build):
  --whisper-model <PATH>    Path to a whisper.cpp ggml model
                            file (e.g. ggml-tiny.en.bin).
                            Env: LVQR_WHISPER_MODEL.

  C2PA signing (requires --features c2pa at build; needs
  --archive-dir to be set because signing runs on the archive
  drain-termination hook):
  --c2pa-signing-cert <PATH>       PEM-encoded signing certificate
                                   chain (leaf first, then CA).
                                   Leaf EKU must be one of
                                   emailProtection / documentSigning
                                   / timeStamping / OCSPSigning /
                                   MS C2PA / C2PA per c2pa-rs.
                                   Env: LVQR_C2PA_SIGNING_CERT.
  --c2pa-signing-key <PATH>        PKCS#8 private key matching
                                   the leaf's subject public key.
                                   Must be set together with
                                   --c2pa-signing-cert.
                                   Env: LVQR_C2PA_SIGNING_KEY.
  --c2pa-signing-alg <ALG>         One of es256 / es384 / es512 /
                                   ps256 / ps384 / ps512 / ed25519.
                                   Defaults to es256.
                                   Env: LVQR_C2PA_SIGNING_ALG.
  --c2pa-assertion-creator <STR>   Creator name on the
                                   schema-org CreativeWork
                                   assertion. Defaults to "lvqr".
                                   Env: LVQR_C2PA_ASSERTION_CREATOR.
  --c2pa-trust-anchor <PATH>       PEM trust-anchor bundle for
                                   private CAs; required when the
                                   leaf does not chain to a
                                   public C2PA trust root.
                                   Env: LVQR_C2PA_TRUST_ANCHOR.
  --c2pa-timestamp-authority <URL> RFC 3161 TSA URL for embedded
                                   timestamp countersignatures.
                                   Env: LVQR_C2PA_TIMESTAMP_AUTHORITY.

  Server-side transcoding (requires --features transcode at
  build; pulls gstreamer 0.23 + base/good/bad/ugly + gst-libav
  from the host):
  --transcode-rendition <NAME>    Repeatable. Preset (720p /
                                  480p / 240p) or path to a
                                  .toml RenditionSpec. Env
                                  LVQR_TRANSCODE_RENDITION is
                                  comma-separated.
  --source-bandwidth-kbps <N>     Override master-playlist
                                  source-variant BANDWIDTH.
                                  Env: LVQR_SOURCE_BANDWIDTH_KBPS.

  Cluster:
  --cluster-listen <ADDR>         Gossip bind (enables cluster plane)
  --cluster-seeds <LIST>          Comma-separated peer ip:port seeds
  --cluster-node-id <ID>          Explicit node id
  --cluster-id <ID>               Cluster tag (isolates subnets)
  --cluster-advertise-hls <URL>   Base URL for HLS redirect-to-owner
  --cluster-advertise-dash <URL>  Base URL for DASH redirect-to-owner
  --cluster-advertise-rtsp <URL>  Base URL for RTSP redirect-to-owner

  Peer mesh (topology planner + signaling + client-side relay
  ship; operator-grade completion on the phase-D roadmap):
  --mesh-enabled                  Enable peer mesh coordinator
  --max-peers <N>                 Max children per peer [default: 3]
  --mesh-root-peer-count <N>      Cap on direct-from-origin peers
                                  (additional joiners become
                                  children via AssignParent).
                                  Env: LVQR_MESH_ROOT_PEER_COUNT.
  --no-auth-signal                Disable subscribe-token auth on
                                  the /signal WebSocket.
                                  Env: LVQR_NO_AUTH_SIGNAL.

  TLS:
  --tls-cert <PATH>               TLS cert PEM (auto-generated if omitted)
  --tls-key <PATH>                TLS key PEM

Observability env (unset = stdout fmt only):
  LVQR_OTLP_ENDPOINT              OTLP gRPC target (http://host:4317)
  LVQR_SERVICE_NAME               service.name resource [default: lvqr]
  LVQR_OTLP_RESOURCE              Extra resource attrs (k=v, comma-sep)
  LVQR_TRACE_SAMPLE_RATIO         Head sampling ratio [default: 1.0]
```

## Install

```bash
# From crates.io
cargo install lvqr-cli

# From source
git clone https://github.com/virgilvox/lvqr.git
cd lvqr
cargo build --release
./target/release/lvqr serve
```

## Architecture

The workspace is 29 crates organised along the unified data
plane: one segmenter, every protocol is a projection.

```
Data model + fanout
  lvqr-core           -- StreamId, TrackName, EventBus, RelayStats
  lvqr-fragment      -- Fragment, FragmentMeta, FragmentStream
  lvqr-moq           -- facade over moq-lite

Codecs + segmenter
  lvqr-codec         -- AVC / HEVC / AAC / Opus / AV1 parsers
  lvqr-cmaf          -- RawSample coalescer, CmafPolicy, fMP4 writer

Ingest protocols
  lvqr-ingest        -- RTMP + FLV + bridge
  lvqr-whip          -- WebRTC ingest via str0m (H.264/HEVC/Opus)
  lvqr-srt           -- SRT + MPEG-TS demux
  lvqr-rtsp          -- RTSP/1.0 server with interleaved RTP

Egress protocols
  lvqr-relay         -- MoQ/QUIC relay over moq-lite
  lvqr-hls           -- LL-HLS + MultiHlsServer + DVR + SubtitlesServer
  lvqr-dash          -- MPEG-DASH + MultiDashServer
  lvqr-whep          -- WebRTC egress via str0m
  lvqr-mesh          -- peer mesh topology planner

Auth, storage, admin, signaling
  lvqr-auth          -- noop / static / HS256 JWT providers
  lvqr-record        -- fMP4 recorder subscribed to EventBus
  lvqr-archive       -- redb segment index + C2PA finalize/verify
  lvqr-signal        -- WebRTC signaling (mesh assignments)
  lvqr-admin         -- /api/v1/*, /metrics, /healthz

Cluster + observability
  lvqr-cluster       -- chitchat + FederationRunner
  lvqr-observability -- OTLP export + metrics-crate bridge

Programmable data plane
  lvqr-wasm          -- wasmtime fragment-filter runtime + hot-reload
  lvqr-agent         -- AI-agents framework (trait + runner)
  lvqr-agent-whisper -- WhisperCaptionsAgent (AAC -> PCM -> cues)
  lvqr-transcode     -- GStreamer ABR ladder (feature-gated)

Infrastructure
  lvqr-cli           -- single-binary composition root
  lvqr-conformance   -- reference fixtures + external validators
  lvqr-test-utils    -- TestServer harness
  lvqr-soak          -- long-run soak driver
```

### Load-bearing decisions

Three that every contributor needs to internalise before touching
cross-crate boundaries:

- **Unified Fragment Model.** Every track is a sequence of
  `Fragment { track_id, group_id, object_id, priority, dts, pts,
  duration, flags, payload, ingest_time_ms }`. Every ingest
  produces fragments; every egress is a projection.
- **Control vs hot path split.** Control-plane traits use
  `async-trait`; the data plane uses concrete types or enum
  dispatch. No per-fragment `dyn` dispatch anywhere.
- **chitchat scope discipline.** Gossip carries membership,
  ownership pointers, capacity, config, feature flags.
  Per-fragment / per-subscriber state stays node-local and uses
  direct RPC keyed off chitchat pointers.

Any change that violates one of these is a red flag and must be
re-scoped before implementation starts. The full ten-decision
list lives in [`tracking/ROADMAP.md`](tracking/ROADMAP.md).

## Documentation

- [Quickstart](docs/quickstart.md) -- zero to streaming in five minutes
- [Architecture](docs/architecture.md) -- the 29-crate workspace + the ten load-bearing decisions
- [Deployment](docs/deployment.md) -- systemd, TLS, Prometheus, OTLP
- [Auth](docs/auth.md) -- one-token-all-protocols model
- [Cluster plane](docs/cluster.md) -- chitchat membership, ownership, redirect-to-owner
- [Observability](docs/observability.md) -- OTLP export, Prometheus fanout
- [Latency SLO](docs/slo.md) -- operator runbook + alert tuning
- [Peer mesh](docs/mesh.md) -- topology planner + WebSocket signaling + client-side MeshPeer relay (two-peer happy path verified; operator-grade completion on the phase-D roadmap)
- [Roadmap](tracking/ROADMAP.md) -- the 18-24 month plan
- [Handoff](tracking/HANDOFF.md) -- session-by-session log (current state)
- [Test contract](tests/CONTRACT.md) -- the 5-artifact discipline per wire-format crate

## Development

```bash
# Fast inner loop: one crate's lib + one integration test
cargo test -p lvqr-hls --lib
cargo test -p lvqr-cli --test rtmp_hls_e2e

# Full workspace (the pre-commit gate)
cargo fmt --all --check
cargo clippy --workspace --all-targets --benches -- -D warnings
cargo test --workspace

# Benchmarks
cargo bench -p lvqr-hls
cargo bench -p lvqr-rtsp
cargo bench -p lvqr-cmaf
```

As of the latest close on `main`: 941 workspace tests passing,
0 failing, 1 ignored (pre-existing `moq_sink` doctest), plus a
Playwright browser E2E (`bindings/js/tests/e2e/mesh/`) running
via a dedicated `mesh-e2e.yml` CI workflow. Every close must be
green on fmt + clippy + workspace test; session deltas are
tracked in [`tracking/HANDOFF.md`](tracking/HANDOFF.md).

Feature flags and Docker recipes are in
[`docs/deployment.md`](docs/deployment.md).

## Built on

- [moq-lite](https://github.com/kixelated/moq) -- Media over QUIC
- [quinn](https://github.com/quinn-rs/quinn) -- Rust QUIC
- [str0m](https://github.com/algesten/str0m) -- sans-IO WebRTC
- [rml_rtmp](https://crates.io/crates/rml_rtmp) -- RTMP
- [chitchat](https://github.com/quickwit-oss/chitchat) -- cluster gossip
- [redb](https://github.com/cberner/redb) -- embedded archive index
- [wasmtime](https://wasmtime.dev/) -- WASM runtime for per-fragment filters
- [c2pa-rs](https://github.com/contentauth/c2pa-rs) -- C2PA manifests
- [whisper-rs](https://github.com/tazz4843/whisper-rs) -- whisper.cpp bindings
- [opentelemetry-rust](https://github.com/open-telemetry/opentelemetry-rust) -- OTLP
- [tokio](https://tokio.rs) + [bytes](https://docs.rs/bytes) -- runtime + zero-copy buffers

## License

LVQR is **dual-licensed**: AGPL-3.0-or-later for open-source use,
commercial terms for everyone else.

- **AGPL-3.0-or-later** (see [`LICENSE`](LICENSE)) for personal
  projects, research, education, non-profits, and any commercial
  use willing to release derivative source code under AGPL.
  AGPL-3's network copyleft means hosting LVQR as a SaaS product
  counts as distribution for license purposes; you must publish
  your full SaaS source under AGPL too.
- **Commercial license** for proprietary products, managed /
  hosted services that do not want to open-source their code,
  and deployments that need indemnification, warranty, or
  priority security response. See
  [`COMMERCIAL-LICENSE.md`](COMMERCIAL-LICENSE.md) for the
  process. Contact: `hackbuildvideo@gmail.com`.

Contributions are accepted under AGPL; see
[`CONTRIBUTING.md`](CONTRIBUTING.md) and the "Contributing"
section of the commercial-license document for the CLA-style
relicensing grant that keeps the dual-license model honest.

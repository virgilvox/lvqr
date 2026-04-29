# LVQR

[![CI](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml/badge.svg)](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lvqr-core.svg)](https://crates.io/crates/lvqr-core)
[![License](https://img.shields.io/badge/license-AGPL--3.0%20or%20commercial-blue.svg)](LICENSE)

**Programmable real-time media infrastructure for AI, broadcast,
provenance, and low-latency interactive video.**

LVQR is a single Rust binary that ingests RTMP, WHIP, SRT, RTSP, and
WebSocket fMP4; serves LL-HLS, MPEG-DASH, WHEP, MoQ over
QUIC/WebTransport, and WebSocket fMP4 from one unified fragment
model; carries SCTE-35 ad markers verbatim from publisher to player;
runs WASM per-fragment filter chains and in-process AI agents on the
data plane; signs archived media with C2PA; and forms a
chitchat-gossiped cluster with broadcast ownership, redirect-to-owner,
cross-cluster federation, and a browser peer mesh -- without a
separate Redis, Kafka, external segmenter, or signaling server.

```bash
cargo install lvqr-cli
lvqr serve
```

The server boots with sensible defaults (MoQ on 4443/udp, RTMP on
1935/tcp, LL-HLS on 8888/tcp, admin + WebSocket on 8080/tcp), a
self-signed TLS cert if none is supplied, no auth, and zero external
dependencies.

---

## Contents

- [What's in the binary](#whats-in-the-binary)
- [What LVQR uniquely ships](#what-lvqr-uniquely-ships)
- [Quickstart](#quickstart)
- [Programmable data plane](#programmable-data-plane)
- [Authentication](#authentication)
- [Storage and DVR](#storage-and-dvr)
- [Cluster, federation, and peer mesh](#cluster-federation-and-peer-mesh)
- [Observability](#observability)
- [Client SDKs](#client-sdks)
- [Architecture](#architecture)
- [CLI reference](#cli-reference)
- [Operational notes](#operational-notes)
- [Documentation](#documentation)
- [Built on](#built-on)
- [License](#license)

---

## What's in the binary

### Ingest

| Protocol | Codecs | Auth shape |
|---|---|---|
| **RTMP** over TCP (OBS, ffmpeg, Larix, vMix) | H.264, AAC | Stream key (literal token or JWT-as-stream-key); SCTE-35 onCuePoint scte35-bin64 passthrough |
| **WHIP** over HTTPS (WebRTC, OBS 30+) | H.264, HEVC, Opus | `Authorization: Bearer` |
| **SRT** over UDP (broadcast encoders) | H.264, HEVC, AAC over MPEG-TS | `streamid=m=publish,r=<broadcast>,t=<jwt>`; SCTE-35 PMT 0x86 passthrough |
| **RTSP/1.0** over TCP (ANNOUNCE/RECORD) | H.264, HEVC, AAC | `Authorization: Bearer` |
| **WebSocket fMP4** (browser publishers) | Whatever the publisher sends | `lvqr.bearer.<jwt>` subprotocol or `?token=` |

Every ingest produces `Fragment` records on a shared
`FragmentBroadcasterRegistry`. Adding a new ingest protocol is a
projection into that type, not a rewrite of the egress side.

### Egress

| Protocol | Notable surface |
|---|---|
| **LL-HLS** (RFC 8216bis) | Blocking playlist reload, delta playlists, `EXT-X-PART` + `PRELOAD-HINT`, per-segment `PROGRAM-DATE-TIME`, configurable DVR window, audio + subtitle renditions, multivariant master playlist with one `EXT-X-STREAM-INF` per ABR rendition, automatic `ENDLIST` on disconnect, SCTE-35 markers as `EXT-X-DATERANGE` per spec section 4.4.5.1 with `CLASS="urn:scte:scte35:2014:bin"` and SCTE35-OUT / SCTE35-IN / SCTE35-CMD hex blobs |
| **MPEG-DASH** (live profile) | Dynamic MPD with auto-flip to `type="static"` on disconnect, SCTE-35 markers as Period-level `<EventStream schemeIdUri="urn:scte:scte35:2014:xml+bin">` with base64 `<Signal><Binary>` per ISO/IEC 23009-1 G.7 + SCTE 214-1 |
| **WHEP** (WebRTC egress via str0m) | H.264 + HEVC video, Opus audio; AAC publishers reach WHEP subscribers via the in-process `AacToOpusEncoder` (GStreamer, `aac-opus` feature) |
| **MoQ** over QUIC / WebTransport | First-class egress on `moq-lite`; zero-copy fan-out via `OriginProducer`; sibling `<broadcast>/0.timing` track stamps a 16-byte LE `(group_id, ingest_time_ms)` anchor per video keyframe so subscribers can compute glass-to-glass latency without a wire-format change (foreign MoQ clients ignore the unknown track name per the moq-lite contract) |
| **WebSocket fMP4** | Browser fallback for clients without WebTransport |
| **DVR scrub** via `/playback/*` | `redb` segment index, RFC 7233 `Range: bytes=` single-range support so HTML5 `<video>` seekability works out of the box |

### Programmable data plane

- **WASM per-fragment filter chains.** Repeat `--wasm-filter <path>`
  (or comma-separate `LVQR_WASM_FILTER`) to install an ordered chain
  of `wasmtime`-loaded modules. Each guest exports
  `on_fragment(ptr: i32, len: i32) -> i32`; a negative return drops
  the fragment, a non-negative N keeps it and treats the first N
  bytes of guest memory as the rewritten payload. The first drop
  short-circuits the rest of the chain. Each slot has its own
  `notify`-backed file watcher, so swapping one module hot-reloads
  without disturbing the others. Counters, slot states, and the
  configured chain are exposed at `GET /api/v1/wasm-filter`.
  Examples under [`crates/lvqr-wasm/examples/`](crates/lvqr-wasm/examples/).
- **In-process AI agents** via the `Agent` + `AgentFactory` traits
  on [`lvqr-agent`](crates/lvqr-agent). One drain task per agent per
  `(broadcast, track)`, panic-isolated, with per-agent metrics. The
  shipping `WhisperCaptionsAgent` (`--whisper-model <path>` with
  `--features whisper`) drives `whisper.cpp` over the audio track and
  republishes WebVTT cues onto a `captions` track that the LL-HLS
  composer drains into a subtitles rendition group at
  `/hls/{broadcast}/captions/playlist.m3u8`.
- **Server-side transcoding** via [`lvqr-transcode`](crates/lvqr-transcode)
  (`--features transcode`). Software ABR ladder backed by a GStreamer
  pipeline on a dedicated worker thread, plus an always-available
  `AudioPassthroughTranscoderFactory`. Drive via repeatable
  `--transcode-rendition <NAME>` (presets `720p`/`480p`/`240p` or a
  `.toml` `RenditionSpec`); the LL-HLS master playlist composes one
  `EXT-X-STREAM-INF` per rendition automatically. **Four hardware
  encoder backends** ship behind their own Cargo features and select
  via `--transcode-encoder`:
  | Backend | Feature flag | Platform | GStreamer element |
  |---|---|---|---|
  | VideoToolbox | `hw-videotoolbox` | macOS | `vtenc_h264_hw` (applemedia plugin) |
  | NVENC | `hw-nvenc` | Linux + Nvidia | `nvh264enc` (nvcodec plugin) |
  | VA-API | `hw-vaapi` | Linux + Intel iGPU / AMD | `vah264enc` (va plugin) |
  | Quick Sync | `hw-qsv` | Linux + Intel | `qsvh264enc` (qsv plugin) |

  HW-only path is intentional across all four: a factory that silently
  falls back to CPU encoding under load defeats the point of an
  operator-pickable hardware tier. Each backend's `is_available()`
  probes its required GStreamer element at construction and `build()`
  opts out cleanly with a warn log when the runtime hardware or driver
  is missing.

### Provenance

- **C2PA signed archives** (`--features c2pa`). On broadcast finalize
  the drain task walks the redb segment index in `start_dts` order,
  writes a coalesced
  `<archive>/<broadcast>/<track>/finalized.mp4`, and emits a sidecar
  `finalized.c2pa` manifest. Signing alg is configurable
  (`es256`/`es384`/`es512`/`ps256`/`ps384`/`ps512`/`ed25519`); trust
  anchors and an RFC 3161 timestamp authority can be wired via flags.
  Two signer sources: on-disk PEMs via `--c2pa-signing-cert` +
  `--c2pa-signing-key`, or a custom `Arc<dyn c2pa::Signer>` for
  HSM/KMS-backed keys passed programmatically through
  `ServeConfig.c2pa`. `GET /playback/verify/{broadcast}` returns a
  JSON validation report (`{ signer, signed_at, valid,
  validation_state, errors }`).

### Authentication

- **Pluggable provider chain**: noop, static tokens, HS256 JWT with
  `iss` + `aud` validation, RS256 / ES256 / EdDSA via JWKS
  (`--jwks-url`, `--features jwks`), or a webhook delegating
  `publish` / `subscribe` / `admin` decisions to your own HTTP
  endpoint (`--webhook-auth-url`, `--features webhook`, with
  separate allow / deny TTL caches).
- **Runtime stream-key CRUD**: `POST /api/v1/streamkeys` to mint,
  `GET` to list, `DELETE /api/v1/streamkeys/{id}` to revoke,
  `POST /api/v1/streamkeys/{id}/rotate` to rotate -- all without
  bouncing the server. Tokens are
  `lvqr_sk_<43-char base64url-no-pad>`. Additive over whichever
  provider above is configured; `Subscribe` + `Admin` always delegate
  to the wrapped chain so a misconfigured store cannot lock the
  operator out. Default on; opt out with `--no-streamkeys`.
- **Hot config reload** via `lvqr serve --config <path.toml>` plus
  SIGHUP and `POST /api/v1/config-reload`. Atomically rotates the
  full auth chain (Static / HS256 JWT / JWKS / webhook), the mesh
  ICE server list, and the HMAC playback secret using
  `arc_swap::ArcSwap` handles -- single-digit-ns reads on the
  auth-check fast path; in-flight requests finish against the prior
  snapshot; old providers' background refresh / fetcher tasks abort
  via their `Drop` impls. Every key the file format defines is
  honored at runtime.
- **One token, every protocol.** The same JWT admits a publisher
  across RTMP (stream key IS the JWT), WHIP (`Authorization: Bearer`),
  SRT (`streamid=m=publish,r=<broadcast>,t=<jwt>`), RTSP
  (`Authorization: Bearer`), and WebSocket ingest
  (`lvqr.bearer.<jwt>` subprotocol). Subscribe-side: WHEP, WebSocket
  relay, live LL-HLS + DASH playback, DVR `/playback/*`, and the
  admin API all apply the same `SubscribeAuth` provider. Live HLS
  + DASH also accept `?token=<token>` for native players that cannot
  set headers; `--no-auth-live-playback` is the escape hatch for
  deployments wanting open live playback with auth scoped to ingest,
  admin, and DVR only.
- **HMAC-signed playback URLs** via `--hmac-playback-secret`. One
  secret signs `/playback/*` (DVR), `/hls/*` (live HLS), and
  `/dash/*` (live DASH); valid `?exp=<unix_ts>&sig=<base64url>` query
  params short-circuit the subscribe-token gate.
  `lvqr_cli::sign_playback_url` and `sign_live_url` mint URLs.

### Storage

- **fMP4 recorder** (`--record-dir`) subscribed to the `EventBus`,
  one file per `(broadcast, track)` plus an init segment.
- **DVR archive** (`--archive-dir`) with a `redb` segment index, the
  `/playback/*` scrub routes, range-request support, and Linux
  `io-uring` writes behind the `io-uring` feature flag.

### Observability

- Prometheus scrape at `/metrics` with histograms +
  `lvqr_subscriber_glass_to_glass_ms{broadcast, transport}`,
  counters for SCTE-35, WASM filter outcomes, stream-key changes,
  client SLO samples, and auth failures.
- OTLP gRPC export of spans + metrics via `LVQR_OTLP_ENDPOINT`,
  composed alongside Prometheus through `metrics-util::FanoutBuilder`.
- **Latency SLO tracker** with five transports instrumented:
  `"hls"`, `"dash"`, `"ws"`, `"whep"`, and `"moq"`. Server-side
  glass-to-glass for HLS / DASH / WHEP / WS rides
  `Fragment::ingest_time_ms`. The HLS-side client (the
  `@lvqr/dvr-player` web component's PDT-anchored sampler) and the
  pure-MoQ side (the `0.timing` sidecar track + the
  `lvqr-moq-sample-pusher` reference subscriber bin) close the
  client-render half of the round trip. Both push to the dual-auth
  `POST /api/v1/slo/client-sample` route. Operator runbook:
  [`docs/slo.md`](docs/slo.md).

### Cluster and federation

- **Chitchat gossip plane**: broadcast-ownership KV with lease
  renewal, per-node capacity advertisement, LWW config,
  redirect-to-owner for HLS / DASH / RTSP, full admin surface at
  `/api/v1/cluster/{nodes,broadcasts,config}`.
- **Cross-cluster federation**: authenticated one-way MoQ pulls from
  peer clusters via `FederationLink`. Exponential-backoff reconnect
  (base 1 s, 60 s cap, ±10% jitter); `GET /api/v1/cluster/federation`
  returns per-link `state` / `last_connected_at_ms` / `last_error` /
  `connect_attempts`.
- **Browser peer mesh**: topology planner + WebSocket signaling +
  server-side subscriber registration + WebRTC DataChannel
  parent/child relay. CLI knobs: `--mesh-enabled`, `--max-peers`,
  `--mesh-root-peer-count`, `--mesh-ice-servers`. Per-peer self-
  reported capacity influences the planner so a peer claiming
  `capacity: 1` forces subsequent peers to descend even when the
  global ceiling is higher. Client-side relay ships in
  [`@lvqr/core`](bindings/js/packages/core); the three-peer-chain
  Playwright project exercises signal-to-DataChannel delivery on
  every CI run; TURN deployment recipe + sample `coturn.conf` ship
  in [`deploy/turn/`](deploy/turn/).

---

## What LVQR uniquely ships

The matrix below lists capabilities present in LVQR against the
upstream open-source state of the closest comparable projects as
of April 2026. Marks reflect OSS releases only; commercial tiers
and third-party forks are noted in the footnotes when they shift
the read.

Legend: ✓ supported · ◐ partial / via separate component / commercial
tier · ✗ not supported · ? no public evidence either way.

| Capability | LVQR | MediaMTX | OvenMediaEngine | SRS | MistServer | Ant Media CE |
|---|:---:|:---:|:---:|:---:|:---:|:---:|
| MoQ over QUIC/WebTransport, first-class egress | ✓ | ✗[¹] | ✗ | ✗ | ✗ | ◐[²] |
| Pure-MoQ glass-to-glass latency SLO via sidecar timing track | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| WASM per-fragment filter chain with hot-reload per slot | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| In-process AI agent framework with shipping Whisper VTT captions | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| C2PA signed archive + on-server verify endpoint | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| SCTE-35 passthrough across SRT 0x86 + RTMP onCuePoint, rendered as both HLS DATERANGE and DASH EventStream | ✓ | ✗ | ✗ | ✗ | ◐[³] | ◐[⁴] |
| Browser peer-mesh DataChannel relay with capacity advertisement | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Optional Linux `io_uring` archive writes | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ |
| Hot reload of full auth chain (Static / JWT / JWKS / webhook) atomically without restart | ✓ | ◐[⁵] | ? | ◐[⁶] | ? | ✗ |
| Single JWT across every ingest (RTMP key, WHIP/RTSP bearer, SRT streamid, WS subprotocol) | ✓ | ◐ | ◐[⁷] | ✗ | ◐ | ◐ |
| HMAC-signed playback URLs across `/hls`, `/dash`, and `/playback` | ✓ | ✗ | ✓[⁸] | ✗ | ? | ? |
| Single binary, no Redis / Kafka / external segmenter / signaling | ✓ | ✓ | ✗[⁹] | ✓ | ◐[¹⁰] | ✗[¹¹] |
| Native ingest set: RTMP + WHIP + SRT + RTSP + WS-fMP4 | ✓ | ◐ (no WS-fMP4) | ◐ (no WS-fMP4) | ◐ (no RTSP / WS-fMP4) | ◐ (WS-fMP4 not confirmed) | ◐ (no WS-fMP4) |
| Native egress set: LL-HLS + DASH + WHEP + MoQ + WS-fMP4 | ✓ | ◐ (no DASH, no MoQ, no WS-fMP4) | ◐ (no DASH, no MoQ) | ◐ (no MoQ) | ◐ (no MoQ) | ◐ (no MoQ in CE confirmed) |

[¹]: Upstream MediaMTX has no MoQ; a third-party fork
([winkmichael/mediamtx-moq](https://github.com/winkmichael/mediamtx-moq))
shipped August 2025.

[²]: Demonstrated in Ant Media Enterprise; CE parity not confirmed.
See [Ant Media WebRTC vs MoQ](https://antmedia.io/webrtc-vs-moq-media-over-quic-ant-media-server/)
and [CE vs EE comparison](https://github.com/ant-media/Ant-Media-Server/wiki/Community-Edition-vs-Enterprise-Edition).

[³]: MistServer's SCTE-35 integration emits markers on TS-based
outputs only (UDP/TCP/SRT/RIST) and does not render as HLS DATERANGE
or DASH EventStream. See
[MistServer SCTE-35 docs](https://docs.mistserver.org/mistserver/integration/scte35/).

[⁴]: Ant Media's SCTE-35 SSAI plugin renders SRT markers into HLS
cues only.

[⁵]: MediaMTX supports config reload; JWT header support has known
issues (see issue #3630 referenced in
[authentication docs](https://mediamtx.org/docs/usage/authentication)).

[⁶]: SRS uses HTTP callback hooks rather than an atomic provider
swap. See [SRS HTTP Callback](https://ossrs.net/lts/en-us/docs/v4/doc/http-callback).

[⁷]: OvenMediaEngine offers SignedPolicy (HMAC-SHA1 URL signing) +
AdmissionWebhooks but not a single-JWT-everywhere model. See
[SignedPolicy](https://docs.ovenmediaengine.com/access-control/signedpolicy)
and [AdmissionWebhooks](https://docs.ovenmediaengine.com/access-control/admission-webhooks).

[⁸]: OvenMediaEngine's SignedPolicy is HMAC-SHA1 URL signing.

[⁹]: OvenMediaEngine's Origin-Edge clustering uses a Redis OriginMap.
See [Origin-Edge Clustering](https://docs.ovenmediaengine.com/origin-edge-clustering).

[¹⁰]: MistServer's LoadBalancer is a separate binary; clustering and
several production features sit in the commercial Pro tier.

[¹¹]: Ant Media CE runs on a Java application server; clustering and
many production features (adaptive bitrate, full SCTE-35 plugin
behavior, scaling) are gated to Enterprise.

This is not a horse race -- LVQR is built around a different
operational shape (single Rust binary, unified Fragment model, MoQ
as a peer transport rather than a bolt-on). The matrix exists to
help operators figure out whether LVQR closes a gap they currently
fill with multiple components, not to argue that anyone listed is
inferior at what they do.

---

## Quickstart

### Install

```bash
# From crates.io
cargo install lvqr-cli

# From source
git clone https://github.com/virgilvox/lvqr.git
cd lvqr
cargo build --release
./target/release/lvqr serve
```

Optional features at build time: `c2pa`, `transcode`,
`hw-videotoolbox`, `whisper`, `jwks`, `webhook`, `aac-opus`,
`io-uring`, plus `full` to enable the major optional auth providers.

### Start the server

```bash
lvqr serve
```

| Surface | Port | Protocol | Default |
|---|---|---|---|
| MoQ relay | 4443/udp | QUIC / WebTransport | always on |
| RTMP ingest | 1935/tcp | RTMP | always on |
| LL-HLS | 8888/tcp | HTTP/1.1 | always on |
| Admin + WebSocket | 8080/tcp | HTTP/1.1 + WS | always on |
| DASH | `--dash-port` | HTTP/1.1 | off |
| WHEP | `--whep-port` | HTTPS / WebRTC | off |
| WHIP | `--whip-port` | HTTPS / WebRTC | off |
| RTSP | `--rtsp-port` | RTSP/1.0 | off |
| SRT | `--srt-port` | SRT / UDP | off |

A self-signed TLS cert generates at boot when `--tls-cert` /
`--tls-key` are not supplied -- fine for local dev, not production.

### Publish

```bash
# RTMP from ffmpeg
ffmpeg -re -f lavfi -i testsrc=size=640x360:rate=30 \
  -f lavfi -i sine=frequency=440:sample_rate=44100 \
  -c:v libx264 -preset ultrafast -tune zerolatency \
  -c:a aac -b:a 128k \
  -f flv rtmp://localhost:1935/live/demo

# RTSP (requires --rtsp-port 8554)
ffmpeg -re -i source.mp4 -c copy -f rtsp rtsp://localhost:8554/live/demo

# SRT (requires --srt-port 8890)
ffmpeg -re -i source.mp4 -c copy -f mpegts \
  srt://localhost:8890?streamid=live/demo

# WHIP from OBS 30+ (requires --whip-port 8443)
# Service: WHIP   URL: https://localhost:8443/whip/live/demo
```

For OBS / vMix / Wirecast / AWS Elemental SCTE-35 publisher recipes
see [`docs/scte35.md`](docs/scte35.md).

### Play back

- **LL-HLS**: `http://localhost:8888/hls/live/demo/playlist.m3u8`
- **DASH**: `http://localhost:8889/dash/live/demo/manifest.mpd`
- **WHEP**: browser WebRTC player at
  `https://localhost:8443/whep/live/demo`
- **MoQ**: Chrome / Edge 107+ via the `@lvqr/player` web component
  (`npm i @lvqr/player`)
- **DVR scrub**: `<lvqr-dvr-player>` web component
  (`npm i @lvqr/dvr-player`); see
  [`docs/dvr-scrub.md`](docs/dvr-scrub.md)
- **WebSocket fMP4**: `ws://localhost:8080/ws/live/demo` (MSE
  fallback for browsers without WebTransport)

The [`test-app/`](test-app/) directory demonstrates the WebSocket
path end to end: `cd test-app && ./serve.sh` exposes a browser demo
at `http://localhost:3000`.

### Observe

```bash
curl http://localhost:8080/healthz                # liveness
curl http://localhost:8080/api/v1/streams         # active broadcasts
curl http://localhost:8080/api/v1/stats           # connection counts
curl http://localhost:8080/api/v1/slo             # latency snapshot
curl http://localhost:8080/api/v1/wasm-filter     # WASM chain state
curl http://localhost:8080/api/v1/streamkeys      # stream-key catalog
curl http://localhost:8080/api/v1/config-reload   # hot reload status
curl http://localhost:8080/api/v1/mesh            # mesh topology
curl http://localhost:8080/api/v1/cluster/nodes   # gossip members
curl http://localhost:8080/api/v1/cluster/federation # federation links
curl http://localhost:8080/metrics                # Prometheus scrape
```

Set `LVQR_OTLP_ENDPOINT=http://collector:4317` to stream spans +
metrics to an OTLP gRPC collector.

---

## Programmable data plane

### WASM filter chains

Each module exports `on_fragment(ptr: i32, len: i32) -> i32` against
host memory. The host writes the fragment payload at offset 0 before
the call; the guest:

- returns a negative integer to **drop** the fragment (the rest of
  the chain short-circuits);
- returns a non-negative integer `N` to **keep** it, treating the
  first `N` bytes of guest memory as the rewritten payload (`N=0`
  means keep with empty payload).

Fragment metadata (`track_id`, `group_id`, `object_id`, `priority`,
`dts`, `pts`, `duration`, `flags`) is read-only in v1.

```bash
# Single filter
lvqr serve --wasm-filter ./drop_low_bitrate.wasm

# Ordered chain (first drop short-circuits the rest)
lvqr serve \
  --wasm-filter ./normalize.wasm \
  --wasm-filter ./watermark.wasm \
  --wasm-filter ./rate_limit.wasm

# Or via env, comma-separated
LVQR_WASM_FILTER=normalize.wasm,watermark.wasm,rate_limit.wasm \
  lvqr serve
```

Each slot holds its own `notify`-backed file watcher; rewriting one
`.wasm` file hot-swaps just that slot. Examples and a starter
template under [`crates/lvqr-wasm/examples/`](crates/lvqr-wasm/examples/).
Inspect the chain at runtime via
`GET /api/v1/wasm-filter`.

### AI agents (Whisper captions)

Implement [`Agent`](crates/lvqr-agent/src/lib.rs) on your own type
plus an [`AgentFactory`](crates/lvqr-agent/src/lib.rs) that returns
one per `(broadcast, track)` you care about. The runner gives each
agent a single drain task with panic isolation, per-agent metrics,
and a snapshot `AgentContext` at construction.

The shipping `WhisperCaptionsAgent` (`--features whisper`) drives
`whisper.cpp` over the audio track and republishes WebVTT cues onto
a `captions` track that the LL-HLS composer drains into a subtitles
rendition group:

```bash
lvqr serve --whisper-model ggml-tiny.en.bin
# Subtitles playlist:
# http://localhost:8888/hls/live/demo/captions/playlist.m3u8
```

### Server-side transcoding (ABR)

```bash
# Software ladder: 720p + 480p + 240p plus the source variant
lvqr serve \
  --transcode-rendition 720p \
  --transcode-rendition 480p \
  --transcode-rendition 240p

# Custom rendition from a TOML file
lvqr serve --transcode-rendition ./renditions/360p.toml

# Hardware encoder; build with the matching feature
# macOS VideoToolbox     -- cargo build --features hw-videotoolbox
lvqr serve --transcode-rendition 720p --transcode-encoder videotoolbox

# Linux NVENC (Nvidia)   -- cargo build --features hw-nvenc
lvqr serve --transcode-rendition 720p --transcode-encoder nvenc

# Linux VA-API (Intel iGPU / AMD) -- cargo build --features hw-vaapi
lvqr serve --transcode-rendition 720p --transcode-encoder vaapi

# Linux Quick Sync (Intel) -- cargo build --features hw-qsv
lvqr serve --transcode-rendition 720p --transcode-encoder qsv
```

The LL-HLS master playlist composes one `EXT-X-STREAM-INF` per
rendition automatically. NVENC / VAAPI / QSV backends are on the
v1.2 roadmap; the current macOS HW path is HW-only by design (a
factory that silently falls back to CPU under load defeats the
purpose of an operator-pickable hardware tier).

### C2PA provenance

```bash
lvqr serve \
  --archive-dir ./archive \
  --c2pa-signing-cert ./signer.pem \
  --c2pa-signing-key ./signer.key \
  --c2pa-signing-alg es256 \
  --c2pa-trust-anchor ./trust-anchors.pem \
  --c2pa-timestamp-authority http://timestamp.digicert.com
```

On broadcast finalize the drain task walks the archive index and
writes:

- `<archive>/<broadcast>/<track>/finalized.mp4` -- the coalesced
  CMAF asset
- `<archive>/<broadcast>/<track>/finalized.c2pa` -- the sidecar
  manifest

Verify any signed asset over HTTP:

```bash
curl -H "Authorization: Bearer $ADMIN_TOKEN" \
  http://localhost:8080/playback/verify/live/demo
# {
#   "signer": "CN=Operator, O=Example",
#   "signed_at": "2026-04-28T18:21:09Z",
#   "valid": true,
#   "validation_state": "Trusted",
#   "errors": []
# }
```

For HSM/KMS-backed keys pass an `Arc<dyn c2pa::Signer>`
programmatically through `ServeConfig.c2pa` instead of using the PEM
flags. Trust-anchor PEMs let you sign with private CAs that don't
chain to a public C2PA trust root.

---

## Authentication

### One JWT, every protocol

| Carrier | How the token rides |
|---|---|
| RTMP | Stream key IS the JWT (`rtmp://host/live/demo` with stream key set to the JWT) |
| WHIP / RTSP | `Authorization: Bearer <jwt>` |
| SRT | `streamid=m=publish,r=<broadcast>,t=<jwt>` |
| WebSocket ingest / `/signal` / `/ws/*` | `Sec-WebSocket-Protocol: lvqr.bearer.<jwt>` (preferred) or `?token=<jwt>` query fallback |
| LL-HLS / DASH / WHEP / DVR / admin | `Authorization: Bearer <jwt>`; live HLS + DASH also accept `?token=<jwt>` for native players |

Per-broadcast claim binding is enforced wherever the carrier knows
the broadcast name at auth time. `--no-auth-live-playback` is the
escape hatch for deployments that want open live HLS + DASH with
auth scoped to ingest, admin, and DVR only.

### Providers

```bash
# Static tokens (env or CLI)
lvqr serve \
  --admin-token   $ADMIN \
  --publish-key   $PUBLISH \
  --subscribe-token $SUBSCRIBE

# HS256 JWT
lvqr serve \
  --jwt-secret    $JWT_SECRET \
  --jwt-issuer    https://issuer.example.com \
  --jwt-audience  lvqr-prod

# JWKS (RS256 / ES256 / EdDSA)  -- needs --features jwks at build
lvqr serve \
  --jwks-url https://issuer.example.com/.well-known/jwks.json \
  --jwks-refresh-interval-seconds 300

# Webhook delegation  -- needs --features webhook
lvqr serve \
  --webhook-auth-url https://auth.example.com/decide \
  --webhook-auth-cache-ttl-seconds 60 \
  --webhook-auth-deny-cache-ttl-seconds 10
```

Full reference: [`docs/auth.md`](docs/auth.md).

### Runtime stream-key CRUD

```bash
# Mint a key (the only response that ever shows the literal token)
curl -H "Authorization: Bearer $ADMIN" \
  -H 'Content-Type: application/json' \
  -d '{"broadcasts": ["live/cam1"], "expires_in_secs": 86400}' \
  http://localhost:8080/api/v1/streamkeys

# List
curl -H "Authorization: Bearer $ADMIN" \
  http://localhost:8080/api/v1/streamkeys

# Rotate
curl -X POST -H "Authorization: Bearer $ADMIN" \
  http://localhost:8080/api/v1/streamkeys/$ID/rotate

# Revoke
curl -X DELETE -H "Authorization: Bearer $ADMIN" \
  http://localhost:8080/api/v1/streamkeys/$ID
```

Tokens look like `lvqr_sk_<43-char base64url-no-pad>`. The store is
additive over the configured provider chain, so rotating a key never
risks bricking subscribe + admin auth that flows through the wrapped
provider.
[`docs/auth.md#stream-key-crud-admin-api`](docs/auth.md#stream-key-crud-admin-api).

### Hot config reload

```toml
# /etc/lvqr/config.toml
[auth]
provider = "jwks"
jwks_url = "https://issuer.example.com/.well-known/jwks.json"
issuer = "https://issuer.example.com"
audience = "lvqr-prod"

[mesh]
ice_servers = [
  { urls = ["stun:stun.example.com:3478"] },
  { urls = ["turn:turn.example.com:3478"], username = "lvqr", credential = "..." },
]

[hmac]
playback_secret = "rotated-via-config-reload"
```

```bash
# Run with the config file
lvqr serve --config /etc/lvqr/config.toml

# Apply edits
kill -HUP $(pgrep lvqr)
# or
curl -X POST -H "Authorization: Bearer $ADMIN" \
  http://localhost:8080/api/v1/config-reload

# Inspect last reload
curl -H "Authorization: Bearer $ADMIN" \
  http://localhost:8080/api/v1/config-reload
```

The reload is atomic via `arc_swap::ArcSwap`: in-flight requests
finish against the prior snapshot, the auth-check fast path stays
single-digit-ns, and old JWKS / webhook providers' background tasks
abort via `Drop` when the new provider takes over.
[`docs/config-reload.md`](docs/config-reload.md).

### HMAC-signed playback URLs

```bash
lvqr serve --hmac-playback-secret $HMAC_SECRET --archive-dir ./archive

# Mint a playback URL valid for 5 minutes
EXP=$(($(date +%s) + 300))
SIG=$(printf '%s' "/playback/live/demo/0/00000042.m4s?exp=$EXP" \
       | openssl dgst -binary -sha256 -hmac "$HMAC_SECRET" \
       | basenc --base64url -w0 | tr -d '=')
echo "http://host:8888/playback/live/demo/0/00000042.m4s?exp=$EXP&sig=$SIG"
```

In Rust, the `lvqr_cli::sign_playback_url` and
`lvqr_cli::sign_live_url(secret, LiveScheme::Hls|Dash, broadcast, exp)`
helpers do this for you. Live HLS / DASH signed URLs are
broadcast-scoped (one signed URL grants access to the master
playlist plus every numbered / partial segment under that broadcast,
because LL-HLS partials roll over every ~200 ms making path-bound
signatures impractical).

---

## Storage and DVR

```bash
lvqr serve \
  --record-dir ./recordings \
  --archive-dir ./archive \
  --hls-dvr-window 600
```

- `--record-dir` writes raw fMP4 segments per `(broadcast, track)`
  with init segments for offline post-production.
- `--archive-dir` populates a `redb` segment index, exposes the
  `/playback/*` scrub routes, and (with the right build features
  on Linux) writes via `io_uring`.
- `--hls-dvr-window <secs>` controls the live LL-HLS sliding window
  the `<lvqr-dvr-player>` web component renders against; `0` is
  unbounded.

The [`@lvqr/dvr-player`](bindings/js/packages/dvr-player) web
component drops in as `<lvqr-dvr-player>` against the relay's live
HLS endpoint with a custom seek bar, percentile labels, LIVE pill,
Go Live button, client-side hover thumbnails, SCTE-35 ad-break
marker rendering, and an opt-in client-side glass-to-glass SLO
sampler that posts to `POST /api/v1/slo/client-sample`. See
[`docs/dvr-scrub.md`](docs/dvr-scrub.md).

---

## Cluster, federation, and peer mesh

### Single-cluster setup

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

The first publisher for `live/demo` on either node auto-claims the
broadcast and renews on a lease. Subscribers hitting the non-owner
receive a 302 redirect to the owner's advertised URL for HLS, DASH,
or RTSP. Inspect membership at `/api/v1/cluster/nodes`,
`/api/v1/cluster/broadcasts`, `/api/v1/cluster/config`. Full model:
[`docs/cluster.md`](docs/cluster.md).

### Federation (cross-cluster MoQ pulls)

Configure a `FederationLink` between clusters and authenticated MoQ
fan-in flows through with exponential-backoff reconnect (base 1 s,
60 s cap, ±10% jitter). Per-link state is visible at
`/api/v1/cluster/federation`.

### Browser peer mesh

```bash
lvqr serve \
  --mesh-enabled \
  --max-peers 3 \
  --mesh-root-peer-count 30 \
  --mesh-ice-servers '[{"urls":["stun:stun.example.com:3478"]}]'
```

Browsers use the `MeshPeer` class from `@lvqr/core` to connect to
their assigned parent (root peer of the tree, or another peer)
over a WebRTC `RTCPeerConnection` and forward MoQ frames over a
DataChannel to children. Peers self-report capacity in the
`Register` signal; the planner respects the lower of the global
`--max-peers` ceiling and the peer's claimed capacity. TURN
deployment recipe + sample `coturn.conf` ship in
[`deploy/turn/`](deploy/turn/). Full design:
[`docs/mesh.md`](docs/mesh.md).

---

## Observability

### Prometheus + OTLP

```bash
# Prometheus scrape
curl http://localhost:8080/metrics

# OTLP gRPC export to a collector
LVQR_OTLP_ENDPOINT=http://collector:4317 \
LVQR_SERVICE_NAME=lvqr-edge-1 \
LVQR_TRACE_SAMPLE_RATIO=0.05 \
  lvqr serve
```

Metrics fan out via `metrics-util::FanoutBuilder`, so the same
counters and histograms reach both the Prometheus scrape endpoint
and the OTLP exporter without duplicate instrumentation.

### Latency SLO

```bash
# Server-side ring-buffered snapshot
curl -H "Authorization: Bearer $ADMIN" \
  http://localhost:8080/api/v1/slo
# {
#   "broadcasts": [
#     { "broadcast": "live/demo", "transport": "hls",
#       "p50_ms": 1850, "p95_ms": 2400, "p99_ms": 3100, "max_ms": 4200,
#       "sample_count": 4096 },
#     { "broadcast": "live/demo", "transport": "moq",
#       "p50_ms": 320, "p95_ms": 540, "p99_ms": 880, "max_ms": 1200,
#       "sample_count": 612 }
#   ]
# }

# Client-side push (admin OR per-broadcast subscribe token)
curl -X POST \
  -H "Authorization: Bearer $SUBSCRIBE_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"broadcast":"live/demo","transport":"moq","ingest_ts_ms":1714327219000,"render_ts_ms":1714327219420}' \
  http://localhost:8080/api/v1/slo/client-sample
```

The Prometheus histogram
`lvqr_subscriber_glass_to_glass_ms{broadcast, transport}` exposes the
same data for time-aligned views; alert pack and Grafana dashboard
under [`deploy/grafana/`](deploy/grafana/). Operator runbook:
[`docs/slo.md`](docs/slo.md).

For pure-MoQ subscribers, the MoQ side ships a sibling
`<broadcast>/0.timing` track that stamps a 16-byte LE
`(group_id_u64_le || ingest_time_ms_u64_le)` anchor per video
keyframe. The reference subscriber bin
[`crates/lvqr-test-utils/src/bin/moq_sample_pusher.rs`](crates/lvqr-test-utils/src/bin/moq_sample_pusher.rs)
joins anchors against `0.mp4` group sequences and pushes samples
through the same dual-auth client-sample endpoint. Foreign MoQ
clients ignore the unknown track name per the moq-lite contract,
so the addition is non-breaking.

---

## Client SDKs

| Package | Install | Surface |
|---|---|---|
| `lvqr-core` (Rust) | `cargo add lvqr-core` | Shared types, `EventBus`, admin client. Latest: 1.0.0 (crates.io). |
| `@lvqr/core` (TS) | `npm i @lvqr/core` | MoQ-Lite subscriber over WebTransport, WebSocket fMP4 fallback, full admin client (`configReload` / `triggerConfigReload` + `listStreamKeys` / `mintStreamKey` / `revokeStreamKey` / `rotateStreamKey` plus health / stats / mesh / SLO / wasm-filter), `MeshPeer` WebRTC DataChannel relay with `pushFrame` + `onChildOpen` + `parentPeerId` + `forwardedFrameCount` + `MeshConfig.capacity`. Latest: 1.0.0. |
| `@lvqr/player` | `npm i @lvqr/player` | Drop-in `<lvqr-player>` web component with MSE fallback. Latest: 1.0.0. |
| `@lvqr/dvr-player` | `npm i @lvqr/dvr-player` | Drop-in `<lvqr-dvr-player>` HLS DVR scrub component with custom seek bar, LIVE pill, Go Live, hover thumbnails, SCTE-35 ad-break marker rendering (`markers="visible/hidden"` + `lvqr-dvr-markers-changed` / `lvqr-dvr-marker-crossed` events + `getMarkers()` API), opt-in client-side glass-to-glass SLO sampler. Latest: 1.0.0. |
| `@lvqr/admin-ui` | `npm i @lvqr/admin-ui` | Operator admin console -- Vue 3 SPA wired against every `/api/v1/*` route. Multi-relay connection profiles, themable via CSS custom properties, plugin plumbing via `window.__LVQR_ADMIN_PLUGINS__`. Static-deploy behind any host (nginx, Caddy, Digital Ocean App Platform). Latest: 1.0.0. |
| `lvqr` (Python) | `pip install lvqr` | Admin API client (`config_reload_status` / `trigger_config_reload`, `list_streamkeys` / `mint_streamkey` / `revoke_streamkey` / `rotate_streamkey`, plus health / stats / mesh / SLO / wasm-filter), `bearer_token` kwarg, dataclass returns. Latest: 1.0.0. |

Full TypeScript reference: [`docs/sdk/javascript.md`](docs/sdk/javascript.md).
Python module: [`bindings/python/python/lvqr/`](bindings/python/python/lvqr/).

---

## Architecture

The workspace is 29 Rust crates organised around the unified data
plane: one segmenter, every protocol is a projection.

```
Data model + fanout
  lvqr-core           StreamId, TrackName, EventBus, RelayStats
  lvqr-fragment       Fragment, FragmentMeta, MoqTrackSink, MoqTimingTrackSink
  lvqr-moq            facade over moq-lite

Codecs + segmenter
  lvqr-codec          AVC / HEVC / AAC / Opus / AV1 parsers + SCTE-35 splice_info_section
  lvqr-cmaf           RawSample coalescer, CmafPolicy, fMP4 writer

Ingest protocols
  lvqr-ingest         RTMP + FLV + bridge + onCuePoint scte35-bin64 surface
  lvqr-whip           WebRTC ingest via str0m
  lvqr-srt            SRT + MPEG-TS demux + PMT 0x86 SCTE-35 reassembly
  lvqr-rtsp           RTSP/1.0 server with interleaved RTP

Egress protocols
  lvqr-relay          MoQ/QUIC relay over moq-lite
  lvqr-hls            LL-HLS + MultiHlsServer + DVR + SubtitlesServer + DATERANGE
  lvqr-dash           MPEG-DASH + MultiDashServer + EventStream
  lvqr-whep           WebRTC egress via str0m
  lvqr-mesh           peer mesh topology planner

Auth, storage, admin, signaling
  lvqr-auth           noop / static / HS256 JWT / JWKS / webhook + stream-key store
  lvqr-record         fMP4 recorder subscribed to EventBus
  lvqr-archive        redb segment index + C2PA finalize + verify
  lvqr-signal         WebRTC signaling (mesh assignments)
  lvqr-admin          /api/v1/*, /metrics, /healthz

Cluster + observability
  lvqr-cluster        chitchat + FederationRunner
  lvqr-observability  OTLP export + metrics-crate bridge

Programmable data plane
  lvqr-wasm           wasmtime fragment-filter runtime + hot-reload
  lvqr-agent          AI-agents framework (trait + runner)
  lvqr-agent-whisper  WhisperCaptionsAgent (AAC -> PCM -> WebVTT)
  lvqr-transcode      GStreamer ABR ladder (software + VideoToolbox)

Infrastructure
  lvqr-cli            single-binary composition root
  lvqr-conformance    reference fixtures + external validators
  lvqr-test-utils     TestServer harness + bins
  lvqr-soak           long-run soak driver
```

### Three load-bearing decisions

Every contributor needs to internalise these before touching cross-
crate boundaries:

- **Unified Fragment Model.** Every track is a sequence of
  `Fragment { track_id, group_id, object_id, priority, dts, pts,
  duration, flags, payload, ingest_time_ms }`. Every ingest produces
  fragments; every egress is a projection. New protocols are wiring,
  not architecture.
- **Control vs hot-path split.** Control-plane traits use
  `async-trait`; the data plane uses concrete types or enum
  dispatch. There is no per-fragment `dyn` dispatch anywhere on the
  hot path.
- **chitchat scope discipline.** Gossip carries membership,
  ownership pointers, capacity, config, and feature flags --
  nothing else. Per-fragment / per-subscriber state stays node-local
  and uses direct RPC keyed off chitchat pointers.

The full ten-decision list lives in
[`tracking/ROADMAP.md`](tracking/ROADMAP.md).

---

## CLI reference

Compact tour grouped by area; full set with descriptions is
`lvqr serve --help`.

```
Core ports (always on):
  --port <PORT>              QUIC/MoQ port [default: 4443]
  --rtmp-port <PORT>         RTMP ingest port [default: 1935]
  --admin-port <PORT>        Admin HTTP + WS port [default: 8080]
  --hls-port <PORT>          LL-HLS HTTP port; 0 to disable [default: 8888]

Optional egress / ingest (off unless port set):
  --dash-port <PORT>         MPEG-DASH HTTP port
  --whep-port <PORT>         WHEP WebRTC egress port
  --whip-port <PORT>         WHIP WebRTC ingest port
  --rtsp-port <PORT>         RTSP ingest port
  --srt-port <PORT>          SRT ingest port

LL-HLS tuning:
  --hls-dvr-window <SECS>    DVR depth [default: 120; 0 = unbounded]
  --hls-target-duration <S>  Segment duration [default: 2]
  --hls-part-target <MS>     Partial duration [default: 200]

Auth (env LVQR_*):
  --admin-token <T>          /api/v1/* bearer
  --publish-key <K>          Required publish credential
  --subscribe-token <T>      Required subscriber credential
  --no-streamkeys            Disable runtime stream-key CRUD
  --no-auth-live-playback    Open live HLS+DASH; auth on ingest/admin/DVR
  --no-auth-signal           Disable subscribe-token auth on /signal WS
  --config <PATH>            TOML config file for hot reload (SIGHUP / admin POST)
  --jwt-secret <S>           Enable HS256 JWT
  --jwt-issuer <I>           Expected iss claim
  --jwt-audience <A>         Expected aud claim
  --jwks-url <URL>           Enable JWKS dynamic key discovery (--features jwks)
  --jwks-refresh-interval-seconds <S>     [default: 300; min 10]
  --webhook-auth-url <URL>   Webhook auth provider (--features webhook)
  --webhook-auth-cache-ttl-seconds <S>    [default: 60]
  --webhook-auth-deny-cache-ttl-seconds <S> [default: 10]

Storage:
  --record-dir <PATH>        fMP4 recording directory
  --archive-dir <PATH>       DVR archive dir, enables /playback/*
  --hmac-playback-secret <S> HMAC for signed /playback /hls /dash URLs

WASM filter chain:
  --wasm-filter <PATH>       Repeatable; ordered chain; per-slot hot-reload

Captions (--features whisper):
  --whisper-model <PATH>     ggml-*.bin model file

Server-side transcoding (--features transcode):
  --transcode-rendition <NAME>          Repeatable; preset or .toml
  --transcode-encoder software|videotoolbox  [default: software]
  --source-bandwidth-kbps <N>           Override master variant BANDWIDTH

C2PA signing (--features c2pa, requires --archive-dir):
  --c2pa-signing-cert <PATH>
  --c2pa-signing-key <PATH>
  --c2pa-signing-alg <ALG>              es256/es384/es512/ps256/ps384/ps512/ed25519
  --c2pa-assertion-creator <STR>
  --c2pa-trust-anchor <PATH>
  --c2pa-timestamp-authority <URL>

Cluster:
  --cluster-listen <ADDR>               Gossip bind (enables cluster plane)
  --cluster-seeds <LIST>                Comma-separated peer seeds
  --cluster-node-id <ID>
  --cluster-id <ID>                     Cluster tag (isolates subnets)
  --cluster-advertise-hls <URL>
  --cluster-advertise-dash <URL>
  --cluster-advertise-rtsp <URL>

Browser peer mesh:
  --mesh-enabled
  --max-peers <N>                       [default: 3]
  --mesh-root-peer-count <N>            [default: 30]
  --mesh-ice-servers <JSON>             RTCIceServer array

TLS:
  --tls-cert <PATH>                     Auto-generated if omitted
  --tls-key <PATH>

Observability env (unset = stdout fmt only):
  LVQR_OTLP_ENDPOINT                    OTLP gRPC target (http://host:4317)
  LVQR_SERVICE_NAME                     [default: lvqr]
  LVQR_OTLP_RESOURCE                    Extra resource attrs (k=v, comma-sep)
  LVQR_TRACE_SAMPLE_RATIO               Head sampling ratio [default: 1.0]
```

---

## Operational notes

A few things worth knowing before you ship:

- **`/metrics` is unauthenticated by design.** Scope it via
  firewall or a reverse proxy in multi-tenant deployments.
- **No admission control.** The latency SLO tracker measures and
  alerts on glass-to-glass; it does not refuse new subscribers when
  the SLO is already burning. Wire that into your operator policy.
- **Self-signed TLS certs at boot are for local dev only.** Use
  real certificates in production or front the relay with a TLS-
  terminating proxy.
- **WHEP trickle ICE for inbound candidates is not yet wired.**
  Outbound trickle works; inbound continues to ride the SDP
  exchange.
- **The HLS conformance harness uses internal validators by
  default.** Wiring `mediastreamvalidator` (Apple's reference
  validator) into CI is the single open conformance gap on
  `lvqr-hls`.
- **Hardware encoders.** All four backends ship today: VideoToolbox
  (`hw-videotoolbox`, macOS), NVENC (`hw-nvenc`, Linux + Nvidia),
  VA-API (`hw-vaapi`, Linux + Intel iGPU / AMD), Quick Sync
  (`hw-qsv`, Linux + Intel). Each requires the matching GStreamer
  plugin and runtime hardware on the target host; `is_available()`
  surfaces the missing-element list when the host can't drive the
  selected backend.

---

## Documentation

- [Quickstart](docs/quickstart.md) -- zero to streaming in five minutes
- [Architecture](docs/architecture.md) -- the 29-crate workspace + the load-bearing decisions
- [Deployment](docs/deployment.md) -- systemd, TLS, Prometheus, OTLP
- [Auth](docs/auth.md) -- one-token-all-protocols model, providers, signed URLs
- [Hot config reload](docs/config-reload.md) -- atomic auth + mesh + HMAC + JWKS / webhook reload
- [SCTE-35 ad-marker passthrough](docs/scte35.md) -- standards refs, ingest paths, wire shape examples, publisher quickstart
- [Cluster plane](docs/cluster.md) -- chitchat membership, ownership, redirect-to-owner, federation
- [Observability](docs/observability.md) -- OTLP export, Prometheus fanout
- [Latency SLO](docs/slo.md) -- operator runbook, alert tuning, the MoQ sidecar timing track
- [Peer mesh](docs/mesh.md) -- topology planner + signaling + client-side relay
- [DVR scrub UI](docs/dvr-scrub.md) -- `<lvqr-dvr-player>` operator embedding, theming, SLO sampler
- [Roadmap](tracking/ROADMAP.md) -- the 18-24 month plan
- [Test contract](tests/CONTRACT.md) -- the 5-artifact discipline per wire-format crate

---

## Built on

- [moq-lite](https://github.com/kixelated/moq) -- Media over QUIC
- [quinn](https://github.com/quinn-rs/quinn) -- Rust QUIC
- [str0m](https://github.com/algesten/str0m) -- sans-IO WebRTC
- [rml_rtmp](https://crates.io/crates/rml_rtmp) -- RTMP (vendored fork at `vendor/rml_rtmp/` for AMF0 onCuePoint surfacing)
- [chitchat](https://github.com/quickwit-oss/chitchat) -- cluster gossip
- [redb](https://github.com/cberner/redb) -- embedded archive index
- [wasmtime](https://wasmtime.dev/) -- WASM runtime for per-fragment filters
- [c2pa-rs](https://github.com/contentauth/c2pa-rs) -- C2PA manifests
- [whisper-rs](https://github.com/tazz4843/whisper-rs) -- whisper.cpp bindings
- [opentelemetry-rust](https://github.com/open-telemetry/opentelemetry-rust) -- OTLP
- [tokio](https://tokio.rs) + [bytes](https://docs.rs/bytes) -- runtime + zero-copy buffers

---

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
  hosted services that do not want to open-source their code, and
  deployments that need indemnification, warranty, or priority
  security response. See
  [`COMMERCIAL-LICENSE.md`](COMMERCIAL-LICENSE.md) for the process.
  Contact: `hackbuildvideo@gmail.com`.

Contributions are accepted under AGPL; see
[`CONTRIBUTING.md`](CONTRIBUTING.md) and the "Contributing" section
of the commercial-license document for the CLA-style relicensing
grant that keeps the dual-license model honest.

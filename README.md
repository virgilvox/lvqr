# LVQR

[![CI](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml/badge.svg)](https://github.com/virgilvox/lvqr/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/lvqr-core.svg)](https://crates.io/crates/lvqr-core)
[![License](https://img.shields.io/badge/license-AGPL--3.0%20or%20commercial-blue.svg)](LICENSE)

A Rust live video server. One binary ingests RTMP, WHIP, SRT, and
RTSP; serves LL-HLS, DASH, WHEP, MoQ/QUIC, and WebSocket fMP4; and
optionally forms a chitchat-gossip cluster with broadcast
ownership and redirect-to-owner for every HTTP-facing egress.

```bash
cargo install lvqr-cli
lvqr serve --dash-port 8889 --whip-port 8443 --rtsp-port 8554 --srt-port 8890
```

## Why LVQR

Most Rust media servers get you one protocol at production grade.
LVQR is organised around a single **unified fragment model** so
every ingest feeds every egress through the same segmenter; adding
a protocol is a projection, not a rewrite. The data plane stays
zero-copy (`bytes::Bytes` ref-counted), the control plane is
`async-trait`, and the cluster plane uses chitchat gossip with
ownership leases rather than a consensus bolt-on.

Target positioning: **MediaMTX-grade ergonomics + Kinesis-grade
archive + MoQ as a first-class transport**, with the path to
LiveKit-class differentiators (WASM per-fragment filters, in-process
AI agents, cross-cluster federation) as Tier 4 on the roadmap.

## Capabilities at v0.4

**Ingest:**
- RTMP over TCP (OBS, ffmpeg, Larix, vMix)
- WHIP over HTTPS (WebRTC, H.264 + HEVC + Opus)
- SRT over UDP (MPEG-TS from broadcast encoders)
- RTSP/1.0 over TCP (ANNOUNCE/RECORD, interleaved RTP)
- WebSocket fMP4 (browser publishers via `@lvqr/core`)

**Egress:**
- LL-HLS (RFC 8216bis): blocking playlist reload, delta playlists,
  `EXT-X-PART` / `PRELOAD-HINT`, per-segment `PROGRAM-DATE-TIME`,
  configurable DVR window, master playlist with audio renditions,
  automatic `ENDLIST` on disconnect
- MPEG-DASH: live-profile dynamic MPD, flips to `type="static"` on
  disconnect
- WHEP (WebRTC video egress over HTTPS) via `str0m`
- MoQ over QUIC/WebTransport via `moq-lite` with zero-copy fan-out
- WebSocket fMP4 for browsers without WebTransport
- DVR scrub via `/playback/*` backed by a `redb` segment index

**Ops & cluster:**
- Single-binary zero-config default
- Pluggable auth: noop, static tokens, HS256 JWT with `iss`/`aud`
  validation; `?token=` query and `Authorization: Bearer` both
  honoured
- Disk recording (`--record-dir`) + indexed DVR archive
  (`--archive-dir`)
- Prometheus scrape endpoint
- OTLP gRPC span + metric export (`LVQR_OTLP_ENDPOINT`) with
  `metrics-util::FanoutBuilder` composition alongside Prometheus
- Chitchat cluster plane: broadcast ownership KV with lease
  renewal, per-node capacity advertisement, LWW config, HLS/DASH/
  RTSP redirect-to-owner,
  `/api/v1/cluster/{nodes,broadcasts,config,federation}`, and
  one-way cross-cluster federation pulls with
  exponential-backoff reconnect (item 4.4)

**Programmable data plane (Tier 4 -- 7.5 of 8 items
landed; only the 4.7 Grafana alert pack + docs
remain):**

- **WASM per-fragment filter runtime** (item 4.2,
  COMPLETE) via `wasmtime 25` (`--wasm-filter <path>` /
  `LVQR_WASM_FILTER`): guest modules observe every
  ingested fragment through `on_fragment(ptr, len) ->
  i32`; negative returns drop, non-negative keep.
  Fail-open: a module that fails to compile or traps
  passes the fragment through unchanged. Hot-reload via
  `notify`-watched parent directory atomically swaps the
  running filter; in-flight calls finish on the old
  module, subsequent calls see the new one. Two example
  filters (`frame-counter`, `redact-keyframes`) under
  `crates/lvqr-wasm/examples/`. v1 is a read-only tap;
  downstream egress sees the original fragment
  unchanged.
- **io_uring archive writes** (item 4.1, COMPLETE)
  under `lvqr-archive`'s `io-uring` feature flag on
  Linux. macOS + Windows builds keep the synchronous
  writer; the runtime path is a single feature swap.
- **C2PA signed media** (item 4.3, COMPLETE):
  drain-terminated finalize on broadcast end writes
  `<archive>/<broadcast>/<track>/finalized.mp4` +
  `finalized.c2pa` next to segment files; admin route
  `GET /playback/verify/{*broadcast}` returns a JSON
  validation report (`{ signer, signed_at, valid,
  validation_state, errors }`) backed by `c2pa-rs`'s
  `Reader::with_manifest_data_and_stream`. Behind the
  `c2pa` feature on `lvqr-cli`. Dual signer source:
  `CertKeyFiles { signing_cert_path, private_key_path,
  signing_alg, timestamp_authority_url }` for operators
  with on-disk PEMs; `Custom(Arc<dyn c2pa::Signer +
  Send + Sync>)` for HSM / KMS-backed keys.
- **One-token-all-protocols auth** (item 4.8,
  COMPLETE): one `JwtAuthProvider` admits the same
  publisher across RTMP (stream key IS the JWT), WHIP
  (`Authorization: Bearer`), SRT (`streamid=m=publish,
  r=<broadcast>,t=<jwt>`), RTSP (`Authorization:
  Bearer`), and WS ingest (`lvqr.bearer.<jwt>`
  subprotocol or fallback). Per-broadcast claim binding
  enforced where the carrier knows the broadcast name
  at auth time. End-to-end matrix locked into
  `crates/lvqr-cli/tests/one_token_all_protocols.rs`.
  See [`docs/auth.md`](docs/auth.md).
- **In-process AI agents framework + whisper
  captions** (item 4.5, COMPLETE). `lvqr-agent` ships
  the `Agent` trait + `AgentContext` + factory +
  `AgentRunner` lifecycle wiring: one drain task per
  agent per `(broadcast, track)`, panic-isolated via
  `catch_unwind`, per-`(agent, broadcast, track)`
  stats + `lvqr_agent_fragments_total{agent}` metric.
  `lvqr-agent-whisper` (default-OFF `whisper` feature)
  is the concrete agent: decodes AAC via symphonia,
  feeds PCM to whisper.cpp via `whisper-rs` on a
  dedicated worker thread, emits captions onto both
  the in-process `CaptionStream` AND the
  `FragmentBroadcasterRegistry`'s per-broadcast
  `"captions"` track. `lvqr-hls`'s new
  `SubtitlesServer` publishes a standard HLS
  subtitle rendition on the master playlist
  (`EXT-X-MEDIA TYPE=SUBTITLES`); the cues appear as
  WebVTT at `/hls/{broadcast}/captions/playlist.m3u8`.
  `lvqr-cli` drives the whole chain via
  `--whisper-model <PATH>` / `LVQR_WHISPER_MODEL`
  when built with `--features whisper`.
- **Cross-cluster federation** (item 4.4, COMPLETE).
  `lvqr-cluster::FederationLink { remote_url,
  auth_token, forwarded_broadcasts, disable_tls_verify }`
  configures one-way pulls from a peer cluster's MoQ
  relay; `FederationRunner` spawns one task per link
  that opens an authenticated MoQ session, subscribes
  to the remote origin's announcement stream, and
  re-publishes matched broadcasts into the local
  origin on LVQR's convention tracks (`0.mp4`,
  `1.mp4`, `catalog`). Session 103 C added an
  exponential-backoff reconnect loop (base 1 s,
  doubling to 60 s cap, +/-10% jitter) with a
  `FederationStatusHandle` observability surface read
  by the admin route
  `GET /api/v1/cluster/federation`. Per-link status
  exposes `state` (connecting / connected / failed),
  `last_connected_at_ms`, `last_error`, and
  `connect_attempts` for operator dashboards.
- **Server-side transcoding: software ABR ladder +
  LL-HLS master playlist + AAC passthrough** (item 4.6,
  COMPLETE sessions 104-106). `lvqr-transcode` ships
  the `Transcoder` trait + `TranscoderFactory` +
  `TranscoderContext` + `TranscodeRunner` lifecycle
  wiring, mirroring `lvqr-agent` (panic-isolated
  drain via `catch_unwind`, 4-tuple `(transcoder,
  rendition, broadcast, track)` stats key).
  `RenditionSpec` carries `name` + `width` + `height`
  + `video_bitrate_kbps` + `audio_bitrate_kbps` with
  `preset_720p` / `preset_480p` / `preset_240p` +
  `default_ladder()`. The real GStreamer software
  pipeline (`appsrc ! qtdemux ! h264parse ! avdec_h264
  ! videoscale ! videoconvert ! x264enc ! h264parse
  ! mp4mux ! appsink`) is behind a default-OFF
  `transcode` Cargo feature and runs on a dedicated
  worker thread (same bounded-mpsc pattern as
  `lvqr-agent-whisper`); the always-available
  `AudioPassthroughTranscoderFactory` copies the
  source's AAC `1.mp4` fragments verbatim to
  `<source>/<rendition>/1.mp4`, so each rendition
  broadcaster is a self-contained mp4. `lvqr-cli`
  drives the whole ladder via `--transcode-rendition
  <NAME>` / `LVQR_TRANSCODE_RENDITION` (presets or
  `.toml` custom specs) + `--source-bandwidth-kbps`
  override; the LL-HLS master playlist composer scans
  the registry for `<source>/<rendition>` siblings and
  emits one `#EXT-X-STREAM-INF` per rendition, highest-
  to-lowest `BANDWIDTH`, source variant first at
  `highest_rung * 1.2`. Metrics:
  `lvqr_transcode_fragments_total{transcoder,
  rendition}` +
  `lvqr_transcode_output_fragments_total{...}` +
  `lvqr_transcode_output_bytes_total{...}` +
  `lvqr_transcode_dropped_fragments_total{...}` +
  `lvqr_transcode_panics_total{...,phase}`. Host
  requirement: GStreamer 1.22+ runtime + plugin set
  (base / good / bad / ugly / libav) when building
  with `--features transcode`; the feature is
  default-OFF so CI runners without the install
  continue to build `lvqr-cli` green. Hardware-encoder
  backends (NVENC / VideoToolbox / VAAPI / QSV) are
  post-4.6 follow-ups; the software ladder is the
  feature-complete v1 encode path.
- **Latency SLO tracker + `/api/v1/slo` admin route**
  (item 4.7 session A DONE; session B pending for the
  Grafana alert pack). `Fragment::ingest_time_ms` is
  auto-stamped at `lvqr_ingest::dispatch::publish_fragment`
  time, so every ingest protocol (RTMP / SRT / RTSP /
  WHIP / WS) carries the server-side ingest wall
  clock. `lvqr-admin::slo::LatencyTracker` aggregates
  per-`(broadcast, transport)` samples into a ring
  buffer (1024-sample cap, sort-on-query
  p50 / p95 / p99 / max) and fires the
  `lvqr_subscriber_glass_to_glass_ms` Prometheus
  histogram on every `record()` call. The LL-HLS drain
  loop contributes samples under `transport="hls"`;
  WS / DASH / MoQ / WHEP egress instrumentation is a
  small additive follow-up. Query `GET /api/v1/slo`
  for `{ broadcasts: [{ broadcast, transport, p50_ms,
  p95_ms, p99_ms, max_ms, sample_count,
  total_observed }] }` or read the tracker directly
  off `ServerHandle::slo()` / `TestServer::slo()`
  for integration tests.

**Stability signal:** 909 workspace tests, 0 failures,
1 ignored (the pre-existing `moq_sink` doctest).
`cargo fmt --all --check`, `cargo clippy --workspace
--all-targets --benches -- -D warnings`, and
`cargo test --workspace` all green on every session
close. The 5-artifact test contract (proptest + fuzz +
integration + E2E + conformance) applies to every
wire-format crate; see
[`tests/CONTRACT.md`](tests/CONTRACT.md) for the
current crate-by-crate scorecard.

**What's NOT shipped yet (honest gaps the marketing-
faced docs easily miss):** webhook-based auth
providers, OAuth2 / JWKS dynamic key discovery, HMAC
signed URLs, hot config reload, a dedicated DVR
scrub web UI, SCTE-35 passthrough (WebVTT captions
now ship through the whisper-captions HLS rendition),
stream-key CRUD admin API, WHEP audio (AAC to Opus
transcoder required; a future follow-up atop the 4.6
software transcoder), hardware-encoder feature flags
(NVENC / VAAPI / VideoToolbox -- deferred post-4.6),
the Grafana alert pack + operator docs on top of
`/api/v1/slo` (item 4.7 session B, planned 108),
stream-modifying WASM filter pipelines (v1 WASM
runtime is a read-only tap). Every one of
these is either explicitly on
[`tracking/ROADMAP.md`](tracking/ROADMAP.md) Tier 3
/ Tier 4 or documented as out-of-scope for v1.
None is a silent gap.

## Quickstart

### 1. Start the server

```bash
lvqr serve
```

This binds the zero-config defaults:

| Surface | Port | Protocol | Default |
|---|---|---|---|
| MoQ relay | 4443/udp | QUIC / WebTransport | always on |
| RTMP ingest | 1935/tcp | RTMP | always on |
| LL-HLS | 8888/tcp | HTTP/1.1 | always on |
| Admin + WS | 8080/tcp | HTTP/1.1 + WebSocket | always on |
| DASH | `--dash-port` | HTTP/1.1 | off |
| WHEP | `--whep-port` | HTTPS/WebRTC | off |
| WHIP | `--whip-port` | HTTPS/WebRTC | off |
| RTSP | `--rtsp-port` | RTSP/1.0 over TCP | off |
| SRT | `--srt-port` | SRT over UDP | off |

A self-signed TLS cert is generated at boot if `--tls-cert` /
`--tls-key` are not supplied; fine for local dev, not for
production.

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

- LL-HLS: `http://localhost:8888/hls/live/demo/playlist.m3u8`
- DASH: `http://localhost:8889/dash/live/demo/manifest.mpd`
- WHEP: browser WebRTC player pointed at
  `https://localhost:8443/whep/live/demo`
- MoQ: browsers with WebTransport (Chrome 107+, Edge 107+) via
  `@lvqr/player`
- WebSocket fMP4: `ws://localhost:8080/ws/live/demo` (MSE fallback)

The bundled test app under `test-app/` demonstrates the WebSocket
path end to end; `cd test-app && ./serve.sh` exposes it on
`http://localhost:3000`.

### 4. Observe

```bash
curl http://localhost:8080/healthz             # liveness
curl http://localhost:8080/readyz              # readiness
curl http://localhost:8080/api/v1/streams      # active broadcasts
curl http://localhost:8080/api/v1/stats        # connection / publisher counts
curl http://localhost:8080/metrics             # Prometheus scrape
```

Point the Prometheus scrape at `/metrics`, or set
`LVQR_OTLP_ENDPOINT=http://collector:4317` for OTLP gRPC span +
metric export. See [`docs/observability.md`](docs/observability.md)
for the full observability surface.

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

First publisher for `live/demo` on either node auto-claims
ownership and renews on a lease. A subscriber hitting the
non-owner receives a 302 to the owner's advertised URL for HLS,
DASH, or RTSP. See [`docs/cluster.md`](docs/cluster.md) for the
full cluster plane model, operational recipes, and tuning knobs.

## Client libraries

| Language | Install | Description |
|---|---|---|
| Rust | `cargo add lvqr-core` | Shared types + `EventBus` |
| JavaScript | `npm install @lvqr/core` | MoQ client, admin client, mesh peer |
| JavaScript | `npm install @lvqr/player` | `<lvqr-player>` web component |
| Python | `pip install lvqr` | Admin API client |

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

  WASM filter (read-only tap in v1):
  --wasm-filter <PATH>      Path to a .wasm module exporting
                            on_fragment(ptr, len) -> i32. Hot-
                            reloaded on file change. Env:
                            LVQR_WASM_FILTER.

  Captions (requires `--features whisper` at build):
  --whisper-model <PATH>    Path to a whisper.cpp ggml model file
                            (e.g. ggml-tiny.en.bin). Turns on the
                            in-process WhisperCaptionsAgent
                            against every ingested AAC track and
                            publishes WebVTT cues at
                            /hls/<broadcast>/captions/playlist.m3u8.
                            Env: LVQR_WHISPER_MODEL.

  Server-side transcoding (requires `--features transcode` at
  build; pulls `gstreamer` 0.23 + the plugin set
  base/good/bad/ugly + `gst-libav` from the host):
  --transcode-rendition <NAME>   Repeatable. Preset (`720p` /
                                 `480p` / `240p`) OR a path
                                 ending in `.toml` that
                                 deserializes as a custom
                                 RenditionSpec. Comma-separated
                                 when read from
                                 LVQR_TRANSCODE_RENDITION. Each
                                 value installs one
                                 SoftwareTranscoderFactory +
                                 AudioPassthroughTranscoderFactory
                                 pair on the shared registry, so
                                 every source broadcast produces
                                 `<source>/<rendition>/{0,1}.mp4`
                                 siblings the LL-HLS master
                                 playlist references as variants.
  --source-bandwidth-kbps <N>    Override the master playlist's
                                 source-variant BANDWIDTH
                                 attribute. Defaults to
                                 `highest_rung_kbps * 1.2`. Env:
                                 LVQR_SOURCE_BANDWIDTH_KBPS.

  Cluster:
  --cluster-listen <ADDR>   Gossip bind (enables cluster plane)
  --cluster-seeds <LIST>    Comma-separated peer ip:port seeds
  --cluster-node-id <ID>    Explicit node id (default: random)
  --cluster-id <ID>         Cluster tag (isolates subnets)
  --cluster-advertise-hls <URL>   Base URL for HLS redirect-to-owner
  --cluster-advertise-dash <URL>  Base URL for DASH redirect-to-owner
  --cluster-advertise-rtsp <URL>  Base URL for RTSP redirect-to-owner

  Peer mesh (topology planner only; media relay pending Tier 4):
  --mesh-enabled            Enable peer mesh coordinator
  --max-peers <N>           Max children per peer [default: 3]

  TLS:
  --tls-cert <PATH>         TLS cert PEM (auto-generated if omitted)
  --tls-key <PATH>          TLS key PEM

Observability env (unset = stdout fmt only):
  LVQR_OTLP_ENDPOINT        OTLP gRPC target (http://host:4317)
  LVQR_SERVICE_NAME         service.name resource [default: lvqr]
  LVQR_OTLP_RESOURCE        Extra resource attrs (k=v, comma-sep)
  LVQR_TRACE_SAMPLE_RATIO   Head sampling ratio [default: 1.0]
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

## Documentation

- [Quickstart](docs/quickstart.md) -- zero to streaming in five
  minutes
- [Architecture](docs/architecture.md) -- the 25-crate workspace,
  the unified fragment model, and the ten load-bearing decisions
- [Deployment](docs/deployment.md) -- systemd, TLS, Prometheus, OTLP
- [Cluster plane](docs/cluster.md) -- chitchat membership, ownership,
  redirect-to-owner
- [Observability](docs/observability.md) -- OTLP export, Prometheus
  fanout, resource attribution
- [Peer mesh](docs/mesh.md) -- topology planner (media relay in Tier
  4)
- [Roadmap](tracking/ROADMAP.md) -- 18-24 month plan and the ten
  load-bearing architectural decisions
- [Handoff](tracking/HANDOFF.md) -- rolling session-by-session log
  (source of truth for current state)
- [Test contract](tests/CONTRACT.md) -- the 5-artifact discipline
  every wire-format crate ships with

## Crate map

The workspace is 29 crates organised along the Tier-2 unified
data plane: one segmenter, every protocol is a projection.

```
Data model + fanout
  lvqr-core          -- StreamId, TrackName, EventBus, RelayStats
  lvqr-fragment      -- Fragment, FragmentMeta, FragmentStream
  lvqr-moq           -- facade over moq-lite (version churn boundary)

Codecs + segmenter
  lvqr-codec         -- AVC / HEVC / AAC / Opus / AV1 parsers
  lvqr-cmaf          -- RawSample coalescer, CmafPolicy, fMP4 writer

Ingest protocols
  lvqr-ingest        -- RTMP + FLV + RtmpMoqBridge
  lvqr-whip          -- WebRTC ingest via str0m (H.264/HEVC/Opus)
  lvqr-srt           -- SRT + MPEG-TS demux
  lvqr-rtsp          -- RTSP/1.0 server with interleaved RTP

Egress protocols
  lvqr-relay         -- MoQ/QUIC relay over moq-lite
  lvqr-hls           -- LL-HLS + MultiHlsServer + DVR + SubtitlesServer
  lvqr-dash          -- MPEG-DASH + MultiDashServer
  lvqr-whep          -- WebRTC egress via str0m
  lvqr-mesh          -- peer mesh topology planner (Tier 4 media)

Auth, storage, admin
  lvqr-auth          -- noop / static / HS256 JWT providers
  lvqr-record        -- fMP4 recorder subscribed to EventBus
  lvqr-archive       -- redb segment index + C2PA finalize / verify
  lvqr-signal        -- WebRTC signaling (mesh assignments)
  lvqr-admin         -- /api/v1/*, /metrics, /healthz, /readyz, /api/v1/cluster/federation

Cluster + observability
  lvqr-cluster       -- chitchat plane + FederationRunner (ownership, capacity, config, cross-cluster pulls)
  lvqr-observability -- OTLP span + metric export, metrics-crate bridge

Programmable data plane
  lvqr-wasm          -- wasmtime fragment-filter runtime + notify hot-reload
  lvqr-agent         -- in-process AI agents framework (trait + runner + lifecycle)
  lvqr-agent-whisper -- WhisperCaptionsAgent (AAC -> PCM -> whisper.cpp -> captions track)
  lvqr-transcode     -- server-side transcoder framework + GStreamer software ABR ladder (behind default-OFF `transcode` feature)

Infrastructure
  lvqr-cli           -- single-binary composition root
  lvqr-conformance   -- reference fixtures + external validator wrappers (publish = false)
  lvqr-test-utils    -- TestServer harness (publish = false)
  lvqr-soak          -- long-run soak driver (publish = false)
```

## Load-bearing architectural decisions

LVQR is organised around ten decisions that predate any feature
work; they live in [`tracking/ROADMAP.md`](tracking/ROADMAP.md).
The three every contributor needs to internalise before touching
cross-crate boundaries:

- **Unified Fragment Model.** Every track is a sequence of
  `Fragment { track_id, group_id, object_id, priority, dts, pts,
  duration, flags, payload }`. Every ingest produces fragments;
  every egress is a projection over the same stream.
- **Control vs hot path split.** Control-plane traits use
  `async-trait`; the data plane uses concrete types or enum
  dispatch. No per-fragment `dyn` dispatch anywhere.
- **chitchat scope discipline.** Gossip carries membership,
  ownership pointers, capacity, config, feature flags.
  Per-fragment / per-subscriber state stays node-local and uses
  direct RPC keyed off chitchat pointers.

Any change that violates one of these is a red flag and must be
re-scoped before implementation starts.

## Development

```bash
# Fast inner loop: test one crate's lib + one integration test
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

Feature flags and Docker recipes are in
[`docs/deployment.md`](docs/deployment.md).

## Built on

- [moq-lite](https://github.com/kixelated/moq) -- Media over QUIC
- [quinn](https://github.com/quinn-rs/quinn) -- Rust QUIC
- [str0m](https://github.com/algesten/str0m) -- sans-IO WebRTC
- [rml_rtmp](https://crates.io/crates/rml_rtmp) -- RTMP
- [chitchat](https://github.com/quickwit-oss/chitchat) -- cluster gossip
- [redb](https://github.com/cberner/redb) -- embedded archive index
- [opentelemetry-rust](https://github.com/open-telemetry/opentelemetry-rust) -- OTLP
- [tokio](https://tokio.rs) + [bytes](https://docs.rs/bytes) -- runtime + zero-copy buffers

## License

LVQR is **dual-licensed**: AGPL-3.0-or-later for open-source
use, commercial terms for everyone else.

* **AGPL-3.0-or-later** (see [`LICENSE`](LICENSE)) for
  personal projects, research, education, non-profits, and
  any commercial use willing to release derivative source
  code under AGPL. AGPL-3's network copyleft means hosting
  LVQR as a SaaS product counts as distribution for license
  purposes; you must publish your full SaaS source under AGPL
  too.
* **Commercial license** for proprietary products, managed /
  hosted services that do not want to open-source their code,
  and deployments that need indemnification, warranty, or
  priority security response. See
  [`COMMERCIAL-LICENSE.md`](COMMERCIAL-LICENSE.md) for the
  process. Contact: `hackbuildvideo@gmail.com`.

Contributions are accepted under AGPL; see
[`CONTRIBUTING.md`](CONTRIBUTING.md) and the "Contributing"
section of the commercial-license document for the CLA-style
relicensing grant that keeps the dual-license model honest.

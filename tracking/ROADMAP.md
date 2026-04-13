# LVQR - Roadmap to Surpass the Field

## Context

LVQR is at v0.4-ish: working RTMP → fMP4 → MoQ + WS fallback, mesh tree, optional auth/metrics/recording. A recent honest audit found that what we shipped is real progress but trails LiveKit, MediaMTX, AWS Kinesis Video, and Ant Media on every operational dimension. The user's goal: **surpass them all**, leveraging the Rust ecosystem aggressively, with hardened tests and a coherent product story. This plan is the roadmap to get there.

The bet is **MoQ + Rust + a unified live/DVR data model**, plus operational features that match LiveKit and ergonomics that match MediaMTX. The leapfrog moat is: **MoQ as first-class transport from ingest to playback with subgroups**, **unified live + DVR archive in one engine**, **io_uring zero-copy datapath**, **cross-node room sharding via MoQ relay-of-relays**, **WASM per-fragment filters**, and **in-process AI agents**. None of those exist in any competitor today. Each is achievable with ~3 weeks of focused work *if the foundations are right*.

**Honest timeline: 18–24 months for one focused engineer, 10–14 months for two.** Anything shorter is wishful.

---

## The 10 Load-Bearing Architectural Decisions

These come before any feature work. Get them wrong and Tier 2+ becomes a rewrite.

1. **Unified Fragment Model (Core).** Every track is a sequence of `Fragment { track_id, group_id, object_id, priority, dts, pts, duration, flags (keyframe/independent/discardable), payload: Bytes }`. Every ingest produces fragments; every egress is a projection. MoQ subgroups, LL-HLS partials, CMAF chunks, DASH segments, MoQ DVR fetch, and disk recording are all the same thing addressed differently. **This is the single most important decision in the plan.**

2. **`lvqr-moq` facade crate.** All MoQ usage funnels through one crate that re-exports `Track`, `Group`, `Object` as our newtypes. Pin moq-lite to a git SHA. Upstream churn touches one file.

3. **Control plane vs hot path traits.** `IngestProtocol` and `RelayProtocol` use `async-trait` (control plane, per-connection allocation OK). The data plane (per-fragment dispatch) uses concrete types or enum dispatch - never `dyn`. Migrate to native AFIT in dyn traits when stable.

4. **EventBus split.** Lifecycle events (publisher up/down, viewer join/leave) go through `tokio::sync::broadcast` (current EventBus). Per-fragment / per-byte counters never touch a channel - they go through the `metrics` crate macros directly. The current temptation to put telemetry on the bus is a foot-gun.

5. **chitchat scope discipline.** Cluster gossip carries: membership, node capacity, broadcast → owner pointers, config, feature flags. It does NOT carry: per-frame counters, per-subscriber bitrates, fast-changing state. Hot state stays node-local; cross-node lookups use direct node-to-node RPC keyed off chitchat pointers.

6. **CMAF segmenter is the data plane root.** `lvqr-cmaf` produces fragments. `lvqr-hls`, `lvqr-dash`, `lvqr-record`, `lvqr-archive`, and the WS relay are all pure projections. Recording is not a separate consumer - it's a sink on the segmenter. This kills the "recording watcher only polls RTMP bridge" bug class permanently.

7. **Archive index lives with recording.** Every fragment written to disk gets a redb index entry `(broadcast, track, dts) → (file, offset, length, keyframe)`. The DVR scrub API (Tier 3) is a query against this index. Building it as an afterthought guarantees a rewrite.

8. **Single-binary, zero-config default.** `lvqr serve` with no flags accepts RTMP, WHIP, SRT, and RTSP simultaneously, serves HLS/LL-HLS/WHEP/MoQ, and uses sensible defaults. This is the M1 milestone and it drives Tier 2 scope decisions.

9. **5-artifact test contract** (CI-enforced from Tier 2 onward). Every new protocol/format feature ships with: proptest harness, cargo-fuzz target, integration test (real network, no mocks), E2E test (testcontainers + ffmpeg or playwright), and conformance test against an external validator. No exceptions.

10. **MVP cap on differentiators.** Every Tier 4 item gets a one-page MVP spec before work starts. If it doesn't fit on one page, it's research, not engineering, and it doesn't ship.

---

## Library Decisions (Validated)

| Capability | Crate | Notes |
|---|---|---|
| Cluster gossip + membership + KV + config | **chitchat** | Quickwit, used in prod. No etcd. |
| Object storage (S3/GCS/Azure/local) | **object_store** | Apache Arrow, used by InfluxDB/DataFusion. |
| Embedded archive index | **redb** | Pure Rust ACID B-tree, 1.0 stable. |
| WebRTC for WHIP/WHEP | **str0m** | Sans-IO, right shape for SFU. |
| RTSP client (ONVIF camera pull) | **retina** | Scott Lamb's, mature. Client only. |
| RTSP server | **(write our own)** | No mature Rust RTSP server exists. |
| SRT ingest | **libsrt FFI** | Broadcast-encoder interop > pure Rust port. |
| MPEG-TS demux | **(write focused subset)** | mpeg2ts crate is broken on edges. |
| HLS playlist tags | **m3u8-rs** | Tags only; we own the segmenter. |
| DASH MPD parsing | **dash-mpd** | We write the MPD generator. |
| Hardware-accel transcoding | **gstreamer-rs** | Sebastian Dröge / Centricular. NVENC/VAAPI/QSV. |
| H.264 parser | **h264-reader** | Already in use. |
| HEVC/VP9/AV1 parsers | **(write focused bit-readers)** | Real Rust ecosystem gap. |
| Auth/JWT | **jsonwebtoken** | Already in use. |
| OAuth2 | **oauth2** | Standard Rust crate. |
| C2PA signed media | **c2pa-rs** | Adobe. De-risks the provenance moat. |
| Kubernetes operator | **kube-rs + kube-runtime** | Production-ready. |
| Property testing | **proptest** | Standard. |
| Fuzzing | **cargo-fuzz + arbitrary** | Standard. |
| Integration containers | **testcontainers** | MinIO, Redis, etc. |
| Browser E2E | **playwright** (out-of-cargo) | Trace viewer alone justifies polyglot. |
| HTTP mocking | **wiremock** | Misbehaving upstream simulation. |
| Benchmarks | **criterion** | Already in use. |
| Tracing | **tracing + tracing-opentelemetry** | OTLP to sidecar collector. |
| Structured logs | **tracing-subscriber JSON** | To stdout, then Vector/Promtail. |
| WASM filter sandbox | **wasmtime** | Component model + WASI 0.2. |
| io_uring | **tokio-uring** | Sanity over monoio's thread-per-core. |
| SIP | **(deferred - ecosystem too thin)** | Maybe ezk-sip in Tier 4 if a customer demands it. |
| Captions / WebVTT | **webvtt-parser** | Plus our own segmenter integration. |
| SCTE-35 | **scte35-reader** | For ad insertion passthrough. |

**Crates we keep hand-rolled:** `lvqr-flv` (no viable Rust crate), the existing fMP4 box writer (until mp4-atom replaces it in Tier 2 for HEVC/AV1).

---

## Tier 0 - Fix the Audit Findings (3 weeks)

Entry ticket. Without these the v0.4 release has asterisks on every claim.

**Files modified:**
- `crates/lvqr-cli/src/main.rs` - outer `tokio::select!` race
- `crates/lvqr-cli/src/main.rs` - wire `IngestProtocol` and a new `WsRelay` impl
- `crates/lvqr-ingest/src/bridge.rs` - emit `BroadcastStarted` / `BroadcastStopped` events
- `crates/lvqr-record/src/recorder.rs` - subscribe to events instead of polling
- `bindings/js/packages/player/src/index.ts` - audio SourceBuffer mode
- `crates/lvqr-relay/src/server.rs` - auth: distinguish publisher vs subscriber paths
- WS handlers - accept tokens via `Sec-WebSocket-Protocol` header
- `test-app/index.html` - token-aware
- All `README.md`, `docs/`, `CLAUDE.md` - reflect v0.4 features

**Deliverables:**
1. **Graceful shutdown actually works.** Remove `_ = shutdown.cancelled() =>` from the outer select; let each subsystem return naturally. New integration test: spawn server, push RTMP from `lvqr-test-utils`, ctrl-c, assert exit < 2s with no truncated GOPs.
2. **Wire the extensibility traits.** `serve()` builds `Vec<Box<dyn IngestProtocol>>` and `Vec<Box<dyn RelayProtocol>>`. RTMP and WS relay both implement them. Dead scaffolding becomes load-bearing.
3. **Hook EventBus.** Bridge emits `BroadcastStarted/Stopped`. Recorder subscribes to events instead of polling `bridge.stream_names()`. Closes the WS-ingest-not-recorded gap.
4. **Audio MSE mode fix.** Switch player audio SourceBuffer from `'sequence'` to default mode. Add a 60-second A/V sync soak test in playwright.
5. **Tokens out of query strings.** Use `Sec-WebSocket-Protocol: lvqr.bearer.<token>` for browser-friendly bearer auth. Update the JS client and test app.
6. **MoQ session auth fix.** Look at the requested URL path/query to distinguish publish vs subscribe intent (or split into `/publish/...` and `/subscribe/...` paths in moq-native config). Track in a separate issue if upstream changes are required.
7. **Documentation refresh.** README, deployment guide, metrics reference, recording layout, breaking-change note for the WS wire format, examples updated.
8. **End the dead-test honesty problem.** Delete the tests in the prior plan that test only helper functions in isolation. Replace with one real E2E that pushes RTMP, subscribes via WS, verifies both video and audio frames arrive.

**Verification:** `cargo test --workspace`, `cargo clippy`, `cargo fmt --check`, `playwright test e2e/`. Manual: ffmpeg push, watch in test app, ctrl-c the server, verify clean exit.

---

## Tier 1 - Test Infrastructure (5–6 weeks)

Foundations. **Every later tier depends on this being solid.**

**New crates:**
- `crates/lvqr-conformance/` - reference test fixtures (real OBS captures, ffmpeg outputs, broken-encoder samples), spec test vectors (Apple HLS, DASH-IF, Pion WHIP), licensing notes
- `crates/lvqr-loadgen/` - custom Rust data-plane load generator (concurrent subscriber sessions, byte-rate measurement, stall tracking, OTLP metric emission)
- `crates/lvqr-chaos/` - fault injection helpers (drop frames, reorder, delay, partition); wraps toxiproxy when external network fault is needed

**Test infrastructure:**
1. **Proptest harnesses** for every parser: FLV tag, AVCC record, AAC AudioSpecificConfig, fMP4 box, MoQ wire messages, HLS playlist round-trip
2. **cargo-fuzz targets** for the same set, seeded from `lvqr-conformance` corpus, run 60s/PR + nightly long-run
3. **Real-network integration utilities**: `lvqr-test-utils` gains a `TestServer` that spawns the binary, exposes addresses, and handles cleanup
4. **testcontainers fixtures**: MinIO (S3 mock), Redis (when needed), Postgres (if/when control plane needs it)
5. **playwright E2E suite** at `tests/e2e/`: starts the server, opens Chrome, exercises ingest+playback paths. Trace recording on failure.
6. **ffprobe validators**: every fMP4 init segment and media segment generated in tests is piped through `ffprobe -v error` in CI; non-zero exit fails the build
7. **Comparison harness**: same RTMP input into LVQR and MediaMTX, structural diff of HLS playlists. Catches regressions where we drift from spec.
8. **Soak rig**: 24-hour test run with synthetic publisher + 100 subscribers, asserting no memory growth, no gauge drift, no leaked file descriptors. Runs nightly in a separate CI job.
9. **5-artifact CI rule**: a script that fails CI if a PR touching a parser or protocol crate doesn't add a proptest, fuzz target, integration test, and conformance check. Educational warning at first, hard fail by Tier 2.
10. **Golden-file regression corpus** under `tests/fixtures/golden/` with `BLESS=1` to regenerate. Bytes-exact output for canonical inputs catches bugs like the audio MSE class.

**Verification:** All harnesses run green. The fuzz nightly job has been running for at least 7 days without finding a panic. Soak test has run a full 24h cycle without leaks.

---

## Tier 2 - Unified Data Plane + Protocol Parity (16–20 weeks)

The biggest tier. Builds the unified data model first, then adds every ingest and egress protocol on top.

**New crates:**
- `crates/lvqr-moq/` - facade over moq-lite. **Build this first.** All other crates import from here, never from moq-lite directly.
- `crates/lvqr-fragment/` - the Unified Fragment Model. `Fragment`, `FragmentMeta`, `FragmentStream` trait. Used by every producer and consumer.
- `crates/lvqr-cmaf/` - the segmenter. Takes a `FragmentStream`, produces CMAF chunks aligned for HLS partials, DASH segments, and MoQ groups simultaneously.
- `crates/lvqr-codec/` - HEVC, VP9, AV1, Opus, AAC parsers. Hand-rolled bit readers.
- `crates/lvqr-whip/` - WHIP ingest via str0m. Implements `IngestProtocol`. Outputs to the fragment stream.
- `crates/lvqr-whep/` - WHEP egress via str0m. Implements `RelayProtocol`. Consumes from the fragment stream.
- `crates/lvqr-hls/` - HLS + LL-HLS egress. Owns the blocking `_HLS_msn`/`_HLS_part` HTTP delivery layer.
- `crates/lvqr-dash/` - DASH egress. Owns the MPD generator (quick-xml + typed model).
- `crates/lvqr-srt/` - SRT ingest via libsrt FFI. Includes our focused MPEG-TS demuxer.
- `crates/lvqr-rtsp/` - RTSP server. Hand-rolled state machine + retina for RTSP-pull mode.
- `crates/lvqr-archive/` - recording + redb archive index. **Subscribes to the fragment stream**, not the bridge.

**Sub-deliverables (in dependency order):**

### 2.1 - `lvqr-moq` facade and `lvqr-fragment` model (1.5 weeks)
- `lvqr-moq` re-exports the moq-lite types we use, with newtype wrappers
- `lvqr-fragment::Fragment` and the `FragmentStream` trait
- Adapter from `moq_lite::Group` to `FragmentStream`
- Adapter from `FragmentStream` to `moq_lite::TrackProducer`
- Migration of the existing RTMP bridge to produce `FragmentStream` instead of writing directly to MoQ tracks

### 2.2 - `lvqr-codec` (2 weeks)
- HEVC: VPS/SPS/PPS parsing, codec-string generation (`hev1.<profile>.<tier>.<level>`), resolution extraction
- VP9: uncompressed header, profile, resolution, superframe index
- AV1: OBU parsing, sequence header, sample entry (`av01`)
- Opus: TOC byte parsing, frame counting
- AAC: ASC parsing (already partly hand-rolled, harden it)
- Each codec ships with proptest + fuzz target + a fixture suite from real encoders

### 2.3 - `lvqr-cmaf` segmenter (2.5 weeks)
- Replace the hand-rolled fMP4 writer with `mp4-atom` (kixelated) for HEVC/AV1 sample entry support
- Producer interface: `push_fragment(Fragment) -> ()`. Consumer interface: `next_chunk() -> CmafChunk`
- Per-track configuration of chunk duration and partial-segment alignment
- Independent video/audio segment alignment (the spec requires this)
- Output formats: CMAF init (`ftyp` + `moov`), CMAF segment (`moof` + `mdat`), with optional `senc`/`saiz`/`saio` for DRM hooks
- Tests: bytes-exact golden files, ffprobe validation, proptest on fragment sequences

### 2.4 - `lvqr-archive` (1.5 weeks)
- Subscribes to a `FragmentStream`
- Writes init segments and media segments to disk via tokio fs
- Optional upload to S3-compatible storage via `object_store` (multipart, retry, concurrency)
- redb archive index: `(broadcast, track_id, dts) -> (path, offset, length, keyframe)`
- Recording rotation: by max-segment-duration AND max-recording-size AND max-recording-duration, configurable
- Crash recovery: index reconciliation against on-disk segments at startup
- 5-artifact tests including a 1-hour soak that records to disk and queries the index

### 2.5 - `lvqr-hls` + LL-HLS (4 weeks - the quagmire item)
- HLS playlist generation via m3u8-rs (live + VOD windows)
- LL-HLS partial segments aligned to CMAF chunks
- Blocking playlist reload: `_HLS_msn`, `_HLS_part` query handling, hold the response until the requested part is ready (with timeout)
- `EXT-X-PRELOAD-HINT`, `EXT-X-RENDITION-REPORT`, `EXT-X-PART`, `CAN-BLOCK-RELOAD=YES`
- Byte-range addressing for partial segments
- Conformance: Apple's `mediastreamvalidator` against generated playlists in CI; ffmpeg-as-client soak test
- VOD windows for DVR scrub (consumes archive index)

### 2.6 - `lvqr-dash` (1.5 weeks)
- Live MPD generator (quick-xml + typed model - no existing crate)
- Aligned with the same CMAF segments as HLS
- `timeShiftBufferDepth` for DVR
- Conformance: DASH-IF validator + ffmpeg-as-client

### 2.7 - `lvqr-whip` + `lvqr-whep` (2.5 weeks)
- WHIP via str0m: ICE-lite, DTLS, SRTP, RTP packetization for H.264/Opus + simulcast layer parsing
- WHEP via str0m: subscribe to fragment stream, packetize to RTP
- HTTP endpoints `POST /whip/{broadcast}` and `POST /whep/{broadcast}` returning SDP
- Conformance: live test against WHIP-compliant clients (OBS WHIP plugin, Cloudflare's reference client) and WHEP players
- Note: simulcast forwarding is in scope; SVC and full congestion control are Tier 4

### 2.8 - `lvqr-srt` (2.5 weeks)
- libsrt FFI bindings (or wrapper around an existing crate if interop tests pass)
- Listener mode for ingest
- Focused MPEG-TS demuxer: PAT/PMT, PES reassembly, PCR, H.264/HEVC/AAC payloads, discontinuity handling, SCTE-35 passthrough
- Tests against real broadcast encoder captures
- Conformance: srt-live-transmit as a peer

### 2.9 - `lvqr-rtsp` server (2 weeks)
- RTSP/1.0 state machine (DESCRIBE/SETUP/PLAY/RECORD/TEARDOWN)
- Interleaved TCP transport for the LAN/firewall case
- Digest auth
- ANNOUNCE-based ingest (rare but present in the broadcast world)
- Conformance: ffmpeg `rtsp://` push and pull, GStreamer rtspsrc as a client
- ONVIF Profile S compliance is a stretch goal

### 2.10 - Wire it all into the CLI single-binary default (1 week)
- `lvqr serve` starts every protocol on its default port
- One config file (or one CLI arg per protocol) to disable individual protocols
- `lvqr serve --demo` ships with self-signed certs and prints a public-facing URL with hints
- **M1 milestone gate**: a fresh user can `cargo install lvqr-cli`, run `lvqr serve --demo`, ingest from OBS via RTMP/WHIP/SRT/RTSP, and play back via HLS/LL-HLS/DASH/WHEP/MoQ in a browser, with no config file. This is the public release moment.

**Verification (per tier-wide):**
- All 5-artifact tests green for every new crate
- Comparison harness shows LVQR HLS output structurally equivalent to MediaMTX from the same input
- Soak test passes for HLS + LL-HLS + WHEP simultaneously over 24 hours
- ffmpeg can ingest via RTMP, WHIP, SRT, RTSP and play back via HLS, LL-HLS, DASH, WHEP from the same broadcast

---

## Tier 3 - Cluster, Archive UI, Operational (12–14 weeks)

LVQR becomes a multi-node, well-instrumented platform.

**New crates:**
- `crates/lvqr-cluster/` - chitchat-based gossip and consistent-hash routing
- `crates/lvqr-rpc/` - node-to-node RPC for hot state lookups (gRPC via tonic, or Cap'n Proto)
- `crates/lvqr-otel/` - opentelemetry setup glue
- `crates/lvqr-hooks/` - webhook + OAuth2 + JWKS auth providers (extends `lvqr-auth`)

**Deliverables:**

### 3.1 - Cluster (4 weeks)
- chitchat membership and KV
- Each node publishes: `nodes/<id> -> NodeMeta { addr, capacity, version }`, `broadcasts/<id> -> NodeId` (owner pointer)
- Consistent-hash routing for new broadcasts: rendezvous hash on the gossiped membership view
- Cross-node MoQ relay: subscriber on node B looks up `broadcasts/<id>` in chitchat, opens a MoQ session to the owner node, pulls the fragment stream into local fanout
- Leader election by lowest-stable-node-id with stability delay (no Raft)
- Cluster admin API: list nodes, list broadcasts, drain a node
- 3-node testcontainers integration test that starts a cluster and verifies cross-node fanout

### 3.2 - DVR scrub UI (1.5 weeks)
- HTTP endpoint `GET /broadcast/{id}/playback?from={t1}&to={t2}` returns an LL-HLS or DASH window backed by the archive index
- Player updates to support seeking on a live track
- Conformance: ffplay seeking, hls.js DVR window
- This is the demonstrable feature that no competitor has integrated

### 3.3 - Webhook + OAuth + signed URLs (1.5 weeks)
- `WebhookAuthProvider`: POSTs `AuthContext` to a configured URL, expects 200/401
- `OAuth2AuthProvider`: validates tokens against an authorization server (uses `oauth2` crate)
- HMAC signed URLs: `?expires=...&sig=...` for time-limited access without persistent tokens
- Per-broadcast ACLs in chitchat KV
- Stream-key lifecycle as first-class objects: create/rotate/revoke via admin API
- Tests for each provider, plus admin-API integration tests

### 3.4 - Observability (2.5 weeks)
- tracing-opentelemetry layer with OTLP exporter pointing at a sidecar collector
- Per-broadcast metric labels with cardinality cap (top-N by traffic, others bucketed as "other")
- Built-in Grafana dashboards (JSON, version-controlled)
- Alertmanager rule pack
- Prometheus `/metrics` cardinality test in CI (fail PR if a label set explodes)
- Liveness and readiness probes (`/healthz`, `/readyz`)
- pprof-style profiling endpoint via `tokio-console` (debug builds only)
- A real test that verifies metric counters actually increment when traffic flows (the audit caught us not testing this)

### 3.5 - Hot config reload (1 week)
- Config file watcher (notify-rs)
- SIGHUP handler
- Subset of config that's safely reloadable: auth tokens, log levels, rate limits
- Tests for safe-reload and reject-bad-config

### 3.6 - Captions and SCTE-35 (2 weeks)
- WebVTT parser (`webvtt-parser`) and segmenter
- HLS rendition group for captions
- SCTE-35 passthrough from MPEG-TS via `scte35-reader`
- Ad insertion timeline emitted as a side track (consumable by ad servers later)
- Tests against captioned reference files

### 3.7 - Stream key lifecycle and admin API expansion (1 week)
- Stream keys as first-class objects (chitchat KV `stream_keys/<key_id> -> StreamKey { broadcast, expires, scope }`)
- Admin endpoints: `POST /api/v1/stream_keys`, `DELETE /api/v1/stream_keys/{id}`, list, rotate
- Tests for the lifecycle

**Verification:**
- 3-node cluster handles a 100-subscriber broadcast with one node failing without disconnecting subscribers
- DVR scrub demo: live broadcast for 1 hour, then scrub backwards to minute 15 in the player
- Cardinality test catches a deliberate label-explosion bug
- Soak test on a 3-node cluster runs for 48 hours without state divergence

---

## Tier 4 - Differentiation Moats (10–12 weeks, MVP-capped)

**Hard rule: every item gets a one-page MVP spec before work starts. If it doesn't fit on one page, it's research and doesn't ship.**

### 4.1 - io_uring datapath, scoped (2 weeks)
- **MVP**: `tokio-uring` backend for archive disk writes only, behind a `io-uring` feature flag
- Benchmark vs. tokio::fs and ship the result in docs
- Network datapath io_uring is research; not shipping in v1

### 4.2 - WASM per-fragment filters (3 weeks)
- **MVP**: `wasmtime` host with one host function (read fragment, return fragment or drop), one example filter (text watermark via libdither or simple pixel overlay), hot-reloadable from a file
- Single-filter pipeline only
- No state across fragments, no GPU
- Demo: a `frame-counter` filter that prints to stderr; a `redact` filter that drops frames matching a pattern
- This is the developer-loyalty moat

### 4.3 - C2PA signed media (1 week)
- **MVP**: sign recorded MP4 files at finalization with `c2pa-rs` and a configurable key
- Verify on archive-scrub playback
- Live-signed streams are research; defer

### 4.4 - Cross-cluster federation (2 weeks)
- **MVP**: unidirectional MoQ track forwarding between two clusters over a single authenticated QUIC link
- Config-driven (no auto-discovery)
- Demo: cluster A has the publisher, cluster B subscribers see the broadcast via the federation link
- Conflict resolution, distributed catalog, and bidirectional federation are deferred

### 4.5 - In-process AI agents framework (3 weeks)
- **MVP**: real-time transcription via `whisper.cpp` FFI as a subscriber that publishes a captions track
- A trait `Agent { fn on_fragment(&mut self, f: &Fragment) }` for in-process zero-RTT access to the fragment stream
- Demo: live captions for an English broadcast, latency < 1s
- Multi-language, function calling, agent frameworks are out of scope for v1

### 4.6 - Server-side transcoding (2 weeks)
- gstreamer-rs bridge: subscribe to a fragment stream, push through a GStreamer pipeline, publish output as a new broadcast
- Hardware encoders (NVENC, VAAPI, QSV, VideoToolbox) via gstreamer plugins
- ABR ladder generation policy: configurable rendition list, per-broadcast override, per-room defaults
- Demo: ingest a single 1080p stream, automatically generate 720p/480p/240p renditions

### 4.7 - Latency SLO scheduling (1 week)
- **MVP**: end-to-end latency histogram per subscriber via OTel
- Alert on SLO violation
- "Refuse subscribers that would blow the budget" is research; ship the measurement first

### 4.8 - One-token-all-protocols (1 week)
- A single JWT grants the same identity publish/subscribe rights across RTMP, WHIP, SRT, RTSP, MoQ
- Cross-protocol auth normalization layer in `lvqr-auth`
- Tests verify a single token accepted by all four ingest protocols

**Verification:** Each MVP has a working public demo. Each ships with the 5-artifact test suite. None has blown its 3-week cap.

---

## Tier 5 - Ecosystem (10–14 weeks)

Adoption tooling.

**Deliverables:**
- **Helm chart** for Kubernetes deployment with sane defaults
- **Kubernetes operator** (`crates/lvqr-operator/`) using kube-rs that watches `LvqrCluster` CRDs and provisions StatefulSets
- **Terraform module** for AWS/GCP single-region deployment
- **Web admin UI**: a separate package (Solid or React) that consumes the admin API. Stream list, viewer counts, ABR controls, recording browser, DVR scrubber.
- **Rust client SDK** (`bindings/rust/`): high-level publisher and subscriber, tokio-based
- **Go client SDK** (`bindings/go/`): cgo-free, pure Go
- **Swift/iOS SDK** (`bindings/swift/`): publisher (camera+mic) and subscriber (AVPlayer)
- **Android/Kotlin SDK** (`bindings/kotlin/`): same shape
- **Docs site**: Astro or mdbook, hosted on GitHub Pages, with quickstart, deployment, architecture, API reference, examples
- **Reference deployment guide**: AWS, GCP, bare-metal, Docker Compose
- **Tutorial videos**: 5-minute "from zero to streaming" + 10-minute "production deployment"

**Verification:** A fresh user follows the docs and gets a multi-node cluster running with a working publisher and subscriber within 30 minutes.

---

## The 5-Artifact Test Contract (CI-Enforced from Tier 2)

Every new protocol or format feature ships all five:

| Artifact | Tool | Lives at |
|---|---|---|
| Property test | `proptest` | `<crate>/src/<module>/tests.rs` (or co-located `mod tests`) |
| Fuzz target | `cargo-fuzz` + `arbitrary` | `<crate>/fuzz/fuzz_targets/<name>.rs` |
| Integration test | `lvqr-test-utils::TestServer` | `<crate>/tests/integration_*.rs` |
| E2E test | `playwright` (or `lvqr-test-utils` for non-browser) | `tests/e2e/<feature>.spec.ts` |
| Conformance test | external validator | `tests/conformance/<feature>.rs` |

A CI script greps PRs for new modules under `crates/lvqr-{ingest,whip,whep,hls,dash,srt,rtsp,codec,cmaf,archive}/src/` and fails the build if any of the five are missing. Educational warning during Tier 1, hard fail starting Tier 2.

**Golden file regression corpus** at `tests/fixtures/golden/` with `BLESS=1` env var to regenerate.

**Cross-implementation comparison harness**: same RTMP into LVQR + MediaMTX, structural diff on HLS playlists. Catches silent drift.

---

## Public Milestones (Marketing-Aware)

- **M1 - End of Tier 2 (~26 weeks):** Single binary, all protocols, public demo page. `lvqr serve` and it works. Hacker News post. *This is when LVQR enters the conversation.*
- **M2 - Mid-Tier 3 (~32 weeks):** Multi-node cluster demo. Published benchmarks vs. MediaMTX (latency, CPU/stream, mem/stream). DVR scrub demo. *This is when LVQR enters production evaluations.*
- **M3 - End of Tier 3 (~40 weeks):** First production user. Docs site, Helm chart, reference deployment. *This is when LVQR has a customer.*
- **M4 - Tier 4 (~52 weeks):** MoQ demo with sub-200ms glass-to-glass. WASM filter showcase. C2PA-signed broadcast. *This is when LVQR becomes a choice vs. LiveKit for new projects.*
- **M5 - End of Tier 5 (~68 weeks):** SDK parity with LiveKit on JS, iOS, Android. K8s operator. *This is when LVQR becomes a choice vs. LiveKit for migration projects.*

**Brutal GTM truth:** without SIP and without room-composite egress, LVQR will not displace LiveKit from enterprise contact-center accounts. v1.0 competes with **MediaMTX + KVS** (which is already a substantial market). LiveKit-enterprise parity is v2.0 and another year.

---

## Risks and Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| LL-HLS spec compliance is harder than expected | High | Apple `mediastreamvalidator` in CI from day one. Start LL-HLS in week 1 of Tier 2, not week 8. |
| str0m has SCTP/TURN gaps | Medium | Budget upstream contribution time. Have webrtc-rs as a fallback for the parts str0m can't handle. |
| moq-lite churns | High | `lvqr-moq` facade isolates blast radius. Pin to git SHA. Update deliberately. |
| chitchat hits scale wall on cluster KV | Medium | Discipline: only owner pointers + membership in chitchat. Hot state via direct RPC. Add openraft only if a feature genuinely needs linearizability. |
| Cross-node MoQ relay design fails | High | Build a tiny prototype in week 1 of Tier 3. If it doesn't work, fall back to RTMP-style origin/edge with handover. |
| io_uring is portability-limited | Low (we feature-gated it) | Linux-only feature flag, default off. |
| Tier 2 blows its 16–20 week budget | High | Cut SRT and RTSP from Tier 2 and move them to Tier 3 if Tier 2 hits 14 weeks without reaching M1. |
| WASM filter sandbox performance is too slow | Medium | Benchmark in Tier 4 week 1. If <100Mbps per filter, limit to control-plane filters only and ship WHEP/MoQ-side filters in v2. |
| AI agents framework becomes a black hole | High | MVP rule: ship only the whisper.cpp captions agent. Refuse all generalization until Tier 5 minimum. |
| SIP is missing | Medium | Document it as out-of-scope for v1. Ship a webhook hook for external SIP gateways. Revisit when ezk-sip matures. |
| GTM stalls because we're not LiveKit-enterprise | Medium | Lean into the MediaMTX++ positioning. "MediaMTX-grade ergonomics + KVS-grade archive + MoQ" is a clear story even without SIP. |

---

## Critical Files That Will Be Touched

Tier 0 / 1 (immediate work):
- `crates/lvqr-cli/src/main.rs` - composition root for the entire system
- `crates/lvqr-ingest/src/bridge.rs` - emit lifecycle events
- `crates/lvqr-record/src/recorder.rs` - subscribe to events instead of polling
- `crates/lvqr-relay/src/server.rs` - auth and shutdown fixes
- `crates/lvqr-core/src/events.rs` - split control vs telemetry
- `bindings/js/packages/player/src/index.ts` - audio MSE mode
- `test-app/index.html` - token-aware
- `bindings/js/packages/core/src/client.ts` - Sec-WebSocket-Protocol bearer
- `Cargo.toml` (workspace) - new crates registered
- `.github/workflows/ci.yml` - playwright + cargo-fuzz + 5-artifact rule
- `tests/e2e/` (new directory)
- `tests/fixtures/golden/` (new directory)
- `tests/conformance/` (new directory)

Tier 2 (foundational architecture work):
- `crates/lvqr-moq/` (new)
- `crates/lvqr-fragment/` (new)
- `crates/lvqr-cmaf/` (new)
- `crates/lvqr-codec/` (new)
- `crates/lvqr-archive/` (new, replaces `lvqr-record` or absorbs it)
- `crates/lvqr-whip/`, `lvqr-whep/`, `lvqr-hls/`, `lvqr-dash/`, `lvqr-srt/`, `lvqr-rtsp/` (new)
- `crates/lvqr-ingest/src/bridge.rs` - refactor to produce `FragmentStream`

---

## Verification Plan (Tier-by-Tier)

| Tier | Verification |
|---|---|
| 0 | All audit findings closed. Graceful shutdown integration test passes. New E2E test exercises ingest → playback. Documentation reviewed. |
| 1 | Proptest harnesses for all parsers. Fuzz nightly job has run 7+ days clean. Soak rig completed a 24h cycle. |
| 2 | M1 milestone: `lvqr serve --demo` works for all protocols, no config. Comparison harness shows LVQR HLS structurally equivalent to MediaMTX. ffprobe validates every test output. Apple `mediastreamvalidator` accepts our LL-HLS. |
| 3 | 3-node cluster handles 100-subscriber broadcast with one node failure, no disconnects. DVR scrub returns playback in <500ms. Webhook auth integration test. Cardinality test catches deliberate label explosion. |
| 4 | Each MVP has a public demo. Each shipped with 5-artifact tests. Tier 4 calendar held within 12 weeks total. |
| 5 | Fresh user follows docs, multi-node cluster running with publisher/subscriber in 30 minutes. SDKs pass cross-language interop test. |

---

## Bottom Line

LVQR can plausibly surpass MediaMTX + KVS within a year, become a credible LiveKit alternative for new projects within 18 months, and challenge LiveKit-enterprise within ~24 months - but only if we make the load-bearing architectural decisions in Tier 0/1 correctly and refuse to ship Tier 4 black holes. The MoQ + Rust + unified live/DVR + WASM filters + cross-node mesh combination is a real moat. Everything else is execution.

The single most important call: **build the Unified Fragment Model first**, before any new protocol code. Get that right and every subsequent tier becomes mostly mechanical. Get it wrong and every tier becomes a rewrite.

---

## Kickoff Prompt for the Next Session

Paste this at the start of the next session to bootstrap context:

```
You are continuing work on LVQR, a Rust live video streaming server whose goal
is to surpass AWS Kinesis Video Streams, LiveKit, MediaMTX, and Ant Media.

## Context you must read first, in this order
1. /Users/obsidian/.claude/plans/curious-strolling-cupcake.md - the full roadmap
   (read the entire file, it is the source of truth for the next 18-24 months of
   work). Internalize the 10 load-bearing architectural decisions before writing
   any code.
2. /Users/obsidian/Projects/ossuary-projects/lvqr/CLAUDE.md - project rules.
3. /Users/obsidian/Projects/ossuary-projects/lvqr/tracking/HANDOFF.md - last
   known state.
4. /Users/obsidian/Projects/ossuary-projects/lvqr/tracking/AUDIT-2026-04-10.md
   plus the most recent audit findings embedded in the roadmap file.

## Absolute rules (hard fails if violated)
- NEVER add Claude as an author, co-author, or contributor in git commits.
  No `Co-Authored-By: Claude` trailers, no attribution in commit messages,
  file headers, or docs. Commits must appear written entirely by the human
  developer.
- No emojis in code, commit messages, or documentation.
- No em-dashes or obvious AI language patterns in prose.
- Max line width 120. cargo fmt + cargo clippy --workspace must be clean.
- Integration tests use real network connections, not mocks.
- Only edit files within /Users/obsidian/Projects/ossuary-projects/lvqr/.
- Only commit when explicitly asked.

## Where we are
LVQR is at v0.4-ish. A recent honest audit surfaced real bugs that v0.4
claims as "fixed" but are not:
- graceful shutdown race in the outer tokio::select! cancels subsystems
  before they can drain;
- dead scaffolding: IngestProtocol, RelayProtocol, RtmpIngest, EventBus are
  exported but the CLI does not use them;
- audio SourceBuffer in the player uses mode = 'sequence' which causes A/V
  drift over time (should be default/segments mode);
- MoQ session auth uses AuthContext::Subscribe for publishers (logic error);
- recording watcher only polls the RTMP bridge and misses WS-ingested streams;
- tokens are in `?token=` query strings, logged by proxies and referer headers;
- roughly 16 of the 28 new tests are theatrical (test helper functions in
  isolation, or test dead code like the lvqr-core Registry Drop impl).

## The 10 load-bearing architectural decisions from the roadmap
1. Unified Fragment Model (Fragment { track_id, group_id, object_id,
   priority, dts, pts, duration, flags, payload }) is the most important
   call in the entire plan. Build it first. Every protocol is a projection.
2. lvqr-moq facade crate insulates us from moq-lite upstream churn. Build
   this before any more code lands against moq-lite directly.
3. async-trait on the control plane, concrete types on the data-plane hot
   path. No per-fragment dyn dispatch.
4. EventBus split: lifecycle events on tokio::sync::broadcast, telemetry on
   the metrics crate. Per-frame counters never touch a channel.
5. chitchat scope discipline: membership + owner pointers + config + feature
   flags only. Hot state stays node-local, fetched via direct RPC.
6. CMAF segmenter is the data plane root. HLS, DASH, MoQ, WHEP, recording,
   DVR are all projections over the same fragment stream.
7. redb archive index lives with recording in Tier 2, not bolted on in
   Tier 3.
8. Single-binary zero-config default (`lvqr serve` with no flags accepts
   RTMP+WHIP+SRT+RTSP, serves HLS/LL-HLS/WHEP/MoQ) drives Tier 2 scope.
9. 5-artifact test contract enforced in CI from Tier 2: every protocol or
   format feature ships with proptest + cargo-fuzz target + integration test
   + E2E test + conformance test against an external validator.
10. Tier 4 differentiators (io_uring, WASM filters, C2PA, federation, AI
    agents) each have a one-page MVP spec and a 3-week cap. No exceptions.

## Your first task: execute Tier 0 in this order
1. Fix the graceful shutdown race in crates/lvqr-cli/src/main.rs. Remove the
   `_ = shutdown.cancelled() =>` arm from the outer tokio::select!; let each
   subsystem return naturally when it observes the token. Write an integration
   test that spawns the server, pushes a real RTMP publish via lvqr-test-utils,
   sends SIGINT, and asserts clean exit within 2 seconds with the last GOP
   fully finished.
2. Wire IngestProtocol and a new WsRelay implementation of RelayProtocol into
   serve(). Dead scaffolding becomes load-bearing. Add a test that plugs in a
   mock IngestProtocol and verifies the CLI orchestrator dispatches to it.
3. Hook the EventBus: bridge emits BroadcastStarted/Stopped on publish/unpublish.
   The recorder subscribes to events instead of polling bridge.stream_names().
   This closes the WS-ingest-not-recorded gap. Test: ingest via WS, verify the
   recorder creates segments on disk.
4. Fix player audio SourceBuffer mode from 'sequence' to default. Add a 60-second
   A/V sync soak test in playwright that asserts drift < 100ms.
5. Replace `?token=...` with Sec-WebSocket-Protocol: lvqr.bearer.<token> for
   browser auth. Update the JS client, the test app, and the server handlers.
   MoQ session auth: distinguish publish vs subscribe by URL path (/publish/...
   vs /subscribe/...) if moq-native allows, otherwise document the limitation.
6. Refresh documentation: README.md, CLAUDE.md cross-references, docs/,
   tracking/HANDOFF.md. Document the breaking WS wire format change.
7. Delete the theatrical tests: the lvqr-core Registry Drop tests (dead code),
   the lvqr-record helper-function-only tests, the EventBus round-trip tests,
   the h264-reader-upstream-fixture-replay test, and the IngestProtocol mock
   object-safety test. Replace with one real E2E test that pushes RTMP from
   lvqr-test-utils, subscribes via WebSocket, and verifies both video and
   audio frames arrive and can be parsed by ffprobe.

After Tier 0 is green, move to Tier 1: test infrastructure. Create
crates/lvqr-conformance with a reference fixture corpus (real OBS captures
from OBS 29/30/31 on macOS+Linux+Windows, ffmpeg RTMP pushes, Larix Broadcaster
Android captures). Create crates/lvqr-loadgen for data-plane load testing. Add
proptest harnesses for every parser, cargo-fuzz targets, testcontainers
fixtures with MinIO, playwright E2E suite, ffprobe validation in CI, comparison
harness against MediaMTX.

Work deliberately. Verify each fix with a real test before moving on. Update
the plan file if scope changes. Do not mark a tier complete until verification
passes. Be brutally honest if a test is theatrical or a fix is in dead code;
the prior session lost credibility by not being honest about both.
```

## Recommended repo location

After exiting plan mode, consider copying this roadmap into the repo at
`tracking/ROADMAP.md` so it is version-controlled alongside the code. That
gives future contributors the same context as the humans working on the
project today.

# Tier 4 Plan -- Differentiation Moats (MVP-capped)

**Status**: planning. Proposed for execution starting session 85
(immediately after Tier 3 closed at session 83).
**Source of truth**: `tracking/ROADMAP.md` is the project-level
plan; this document refines Tier 4 only.

## Mission

Tier 4 is where LVQR stops being "MediaMTX-grade single binary
+ Kinesis-grade archive + cluster" and starts being a product
no competitor can match on a feature-for-feature checklist.
Eight MVP-bounded items, each 1-3 weeks, deliver the moat:

* WASM per-fragment filters (developer loyalty)
* io_uring archive writes (performance proof)
* C2PA signed media (provenance moat)
* One-token-all-protocols (cross-protocol auth normalisation)
* In-process AI agents (live transcription demo)
* Cross-cluster federation (multi-region story)
* Server-side transcoding (ABR ladders)
* Latency SLO scheduling (ops dashboards with teeth)

The **hard rule** inherited from ROADMAP load-bearing decision
#10: every item gets a 1-page MVP spec in this document before
any code lands. If a proposed scope does not fit on one page
here, it is research and does not ship in v1. This rule exists
because the prior roadmap rounds drifted into unbounded
generalisation (an "AI agents framework" that could do
anything, for example); every such drift is a de facto
commitment to ship two features where we only had capacity
for one.

None of Tier 4 is blocked on a design unknown. Every item
slots into a Tier 2 / Tier 3 surface that already exists:
`FragmentObserver` for WASM filters and AI agents, the redb
`SegmentIndex` for C2PA signing and io_uring writes,
`lvqr-auth` for one-token-all-protocols, `lvqr-moq` for
federation, `lvqr-cmaf` for transcoding output, the OTLP
metric exporter for SLO scheduling.

## Load-bearing decisions this tier must preserve

Pulled verbatim from ROADMAP.md section "The 10 Load-Bearing
Architectural Decisions":

* **LBD #1** (Unified Fragment Model): every Tier 4 item that
  touches media operates on `lvqr_fragment::Fragment`. No item
  introduces a parallel data model, a bespoke frame type, or a
  side channel that bypasses the segmenter.
* **LBD #3** (control vs hot path): the WASM filter host, AI
  agent trait, and transcoding bridge all run on the
  per-fragment hot path and therefore use concrete types or
  enum dispatch. Only the control-plane surfaces
  (filter-management API, agent lifecycle, federation link
  setup) use `async-trait`.
* **LBD #4** (EventBus split): Tier 4 telemetry goes through
  `metrics` and `tracing` directly, never on the EventBus.
  Latency SLO scheduling reads from the OTLP metric path
  landed in session 82; it does not re-home counters onto
  `tokio::sync::broadcast`.
* **LBD #6** (CMAF segmenter is the data plane root): server-
  side transcoding publishes a new broadcast back into the
  segmenter; it does not produce its own wire format.
  Federation forwards fragments over MoQ; it does not produce
  RTMP.
* **LBD #9** (5-artifact test contract): every Tier 4 item
  ships proptest + fuzz + integration + E2E + conformance.
  Gaps are tracked in the per-item sections below.
* **LBD #10** (MVP cap): every section below fits on one page
  in this document. If a proposed extension would not fit,
  the answer is defer, not reformat.

Any change proposed below that violates one of these is a red
flag and must be re-scoped before implementation starts.

## Execution order

The ROADMAP's Tier 4 section lists items by number (4.1
through 4.8) in chapter order, not in implementation order.
The execution order below prioritises moat value per week of
work, respects dependency ordering, and surfaces the "public
demo" items first so marketing milestones land on schedule.

| # | Item | Weeks | Prereqs | Rationale for position |
|---|---|---|---|---|
| 1 | 4.2 WASM filters | 3 | FragmentObserver (Tier 2) | Highest moat; first session |
| 2 | 4.1 io_uring archive | 2 | lvqr-archive (Tier 2.4) | Orthogonal, easy to parallelise if a second engineer joins |
| 3 | 4.3 C2PA | 1 | lvqr-archive finalize hook | Short; unlocks "provenance" marketing story |
| 4 | 4.8 One-token-all-protocols | 1 | lvqr-auth (shipped) | Short; unblocks enterprise POCs |
| 5 | 4.5 AI agents (whisper.cpp) | 3 | FragmentObserver + Agent trait | Public demo for M4 milestone |
| 6 | 4.4 Cross-cluster federation | 2 | lvqr-cluster (Tier 3) + lvqr-moq | Unlocks multi-region |
| 7 | 4.6 Server-side transcoding | 2 | lvqr-cmaf + gstreamer host | ABR ladders for production deploys |
| 8 | 4.7 Latency SLO scheduling | 1 | OTLP metrics (Tier 3 session I) | Last; polish |

Cumulative weeks: 15. ROADMAP targets 10-12 calendar weeks
because items 2-4 and 7-8 are parallelisable with a second
engineer, and 4.6 may defer to v1.1 if 4.5 blows its budget.

At the observed 10-15 sessions per calendar week velocity and
2-4 sessions per item, **~24 working sessions** for Tier 4 end
to end. Buffer for integration + carry-over: 3 sessions. Total
budget: **27 sessions** (85 through 111).

## 4.2 -- WASM per-fragment filters (3 weeks, 6-9 sessions) -- **COMPLETE (sessions 85-87)**

### Scope (what lands)

1. New crate `crates/lvqr-wasm/` (fresh; unrelated to the
   deleted browser-facing `lvqr-wasm` mentioned in the
   post-0.4 removal). Hosts a `wasmtime::Engine` plus a
   `FragmentFilter` trait whose sole concrete impl loads a
   WASM module from disk and exposes one host function:

   ```wit
   package lvqr:filter@0.1.0
   interface fragment-filter {
       record fragment {
           track-id: u32,
           group-id: u64,
           object-id: u64,
           priority: u8,
           dts: s64,
           pts: s64,
           duration: u32,
           flags: u32,
           payload: list<u8>,
       }
       on-fragment: func(f: fragment) -> option<fragment>;
   }
   ```

2. A `WasmFragmentObserver` that plugs into the
   `FragmentBroadcasterRegistry` observer fan-out. On every
   fragment, it calls the module's `on-fragment` export;
   `Some(f)` replaces the fragment, `None` drops it.
3. Hot-reload: the filter file is watched via `notify-rs`;
   on change, the engine re-instantiates the module with a
   fresh `Store` and swaps the pointer atomically. In-flight
   fragments finish on the old instance.
4. Two demo filters shipped under
   `crates/lvqr-wasm/examples/`:
   * `frame-counter`: prints a counter to stderr via a WASI
     stderr import.
   * `redact`: drops every second keyframe (a deliberately
     visible behaviour for demo purposes).

### Anti-scope

* **No multi-filter pipeline.** v1 runs exactly one filter
  per broadcast. A `Vec<WasmFilter>` is research.
* **No stateful filters.** Each `on-fragment` call is
  stateless; the module gets a fresh `Store` per invocation
  (or a scoped `Store` reset between calls). Aggregating
  across fragments is research.
* **No GPU access.** wasmtime's GPU proposals are not
  stable enough for production.
* **No browser target.** The WASM component runs server-
  side under `wasmtime`. Browser-side fragment processing
  is a different problem.

### API sketch

```rust
// crates/lvqr-wasm/src/lib.rs
pub struct WasmFilter {
    engine: wasmtime::Engine,
    module_path: PathBuf,
    instance: parking_lot::Mutex<Arc<WasmInstance>>,
    _watcher: notify::RecommendedWatcher,
}

impl WasmFilter {
    pub fn load(path: impl AsRef<Path>) -> Result<Self>;
    pub fn apply(&self, fragment: Fragment) -> Option<Fragment>;
}

// Wired through lvqr-cli via --wasm-filter <path>
// or LVQR_WASM_FILTER env var.
```

### Session decomposition

| # | Session | Deliverable | Verification | Status |
|---|---|---|---|---|
| 85 | A | Scaffold `lvqr-wasm`; pin `wasmtime = "25"` at workspace; `FragmentFilter` trait + `WasmFilter` core-WASM impl (on_fragment(ptr,len)->i32; negative=drop, non-negative N=keep first N bytes); fail-open runtime; SharedFilter wrapper with replace() for session-C hot reload; 9 unit tests + 1 proptest (256 cases). Core WASM chosen over component model for scope narrowing; trait surface is stable either way. | `cargo test -p lvqr-wasm` | **DONE (session 85)** |
| 86 | B | `WasmFilterBridgeHandle` + `install_wasm_filter_bridge(registry, filter)` registered as on_entry_created callback; per-(broadcast, track) atomic counters (seen/kept/dropped) + `lvqr_wasm_fragments_total{outcome=keep|drop}` metric. `--wasm-filter <path>` (env `LVQR_WASM_FILTER`) on lvqr-cli; 82-byte pre-compiled `frame-counter.wasm` example; RTMP E2E via TestServer asserts seen > 0 and dropped == 0. **Read-only tap in v1**: drop returns update counters but downstream subscribers see the original fragment unchanged. Full stream-modifying pipeline deferred to v1.1. | `cargo test -p lvqr-cli --test wasm_frame_counter`; 4 observer unit tests; cargo test --workspace | **DONE (session 86)** |
| 87 | C | Hot-reload via `notify::RecommendedWatcher` watching the parent directory (portable across macOS FSEvents + Linux inotify); `WasmFilterReloader` debounces for 50 ms then calls `SharedFilter::replace(new_filter)` on file change. Atomicity documented at the module level: in-flight `apply` calls finish on the OLD module because both `apply` and `replace` take the same `Mutex`. `redact-keyframes.wasm` example filter (82 bytes, returns -1 always). Committed `cargo run -p lvqr-wasm --example build_fixtures` helper regenerates both `.wat -> .wasm` fixtures deterministically. RTMP E2E at `crates/lvqr-cli/tests/wasm_hot_reload.rs` publishes phase-1 with frame-counter, asserts dropped==0, atomically swaps in redact-keyframes, sleeps 500 ms, publishes phase-2 with a different broadcast name, polls for fragments_dropped > 0 within 10 s. Total wall-clock ~1 s on a warm-cache Apple Silicon run. | `cargo test -p lvqr-cli --test wasm_hot_reload` | **DONE (session 87)** |

### Risks + mitigations

* **wasmtime component-model churn.** Pin to a specific
  0.x version behind the workspace dep. Revisit when 1.0
  ships.
* **Performance budget.** A naive bind costs ~50us per
  fragment from Wasmtime's host-to-guest call overhead. If
  the benchmark shows >100us per fragment at 30 fps video,
  the host function surface moves to a batch API
  (`on-group(list<fragment>)`) in a follow-up session.

## 4.1 -- io_uring archive writes (2 weeks, 3-4 sessions)

### Scope (what lands)

1. `lvqr-archive` gains a compile-time feature flag
   `io-uring` (default off; Linux-only). When on, the
   `lvqr_archive::writer::write_segment` helper uses
   `tokio-uring::fs` for the `create_dir_all` +
   `write_all` sequence on archive segments instead of
   `std::fs`. The caller
   (`lvqr_cli::archive::BroadcasterArchiveIndexer::drain`)
   still wraps the call in `tokio::task::spawn_blocking`,
   so enabling io-uring does not change the call-site
   contract.
2. Bench under `crates/lvqr-archive/benches/io_uring_vs_std.rs`
   comparing throughput (MB/s) and p99 latency on a
   1-hour synthetic broadcast. Published in
   `docs/deployment.md` as the
   "when to enable the io_uring backend" section.
3. Graceful fallback: if `tokio_uring::start` fails (kernel
   < 5.6, container sandbox without io_uring syscalls), the
   crate logs a warn and falls back to `std::fs` at
   runtime so a misconfigured deployment never silently
   loses segments.

### Anti-scope

* **Network datapath.** QUIC + RTMP + HLS all stay on
  tokio reactor. io_uring for network is a completely
  different architecture (thread-per-core) that would
  conflict with tokio's scheduler. Revisit as part of a
  potential monoio experiment in a future tier.
* **Archive reader path.** `lvqr-cli::archive::file_handler`
  still serves playback bytes via `tokio::fs::read`;
  io-uring gains accrue almost entirely on the write side
  and adding a reader variant doubles the validation
  surface. Revisit if a latency SLO forces it.

### Session decomposition

| # | Session | Deliverable | Verification | Status |
|---|---|---|---|---|
| 88 | A1 | Lift the archive writer out of `lvqr_cli::archive::BroadcasterArchiveIndexer::drain` into a new `lvqr_archive::writer` module. Public `write_segment(archive_dir, broadcast, track, seq, payload) -> Result<PathBuf, ArchiveError>` wraps `std::fs::create_dir_all` + `std::fs::write` with no behavior change; `segment_path` helper documents the canonical `<dir>/<broadcast>/<track>/<seq:08>.m4s` layout. `ArchiveError::Io` variant added. Session 87-era `BroadcasterArchiveIndexer::segment_path` deleted in favor of the crate-owned helper. Unit tests cover round-trip, mkdir-on-demand, idempotent overwrite, and parent-is-a-file error propagation. Plan text refreshed to reflect the session 59-60 architecture (writer in `lvqr-cli`, not retired `IndexingFragmentObserver`). | `cargo test -p lvqr-archive` green on macOS; `cargo test -p lvqr-cli --test rtmp_archive_e2e` still green (no behavior change). | **DONE (session 88)** |
| 89 | A2 | Feature-gated `tokio-uring` path inside `lvqr_archive::writer::write_segment`. Off by default; Linux-only via `target_os = "linux"` guards alongside the `io-uring` feature. `write_segment`'s outer signature is unchanged; when the feature + target match, the body routes file-create + `write_all_at` + `sync_all` + `close` through a per-call `tokio_uring::start` wrapped in `std::panic::catch_unwind` (tokio-uring 0.5 unwraps `Runtime::new` internally, so `catch_unwind` is the only way to observe a kernel-side setup failure without aborting). `create_dir_all` stays on `std::fs` because tokio-uring 0.5 has no mkdir primitive; the archive tree is amortised across thousands of segments so the extra syscall is noise. Runtime fallback: a process-global `OnceLock<bool>` catches the first `tokio_uring::start` failure, logs a single `tracing::warn!`, and pins `std::fs::write` for the rest of the process. On-path `io::Error`s (create / write / sync / close) do NOT trip the latch -- they surface as `ArchiveError::Io` and the caller retries on the next segment. Option (a) shipped (per-call `tokio_uring::start` inside the existing `spawn_blocking`); option (b) (persistent current-thread runtime on a dedicated writer thread) deferred to session B if the criterion bench shows option (a)'s setup overhead dominates. CI got a new `archive-io-uring` job on `ubuntu-latest` running `cargo clippy -p lvqr-archive --features io-uring` + `cargo test -p lvqr-archive --features io-uring`; kept as a separate job rather than a matrix cell on the existing `test` job so macOS CI time does not grow. | `cargo test -p lvqr-archive` green on macOS; `cargo test -p lvqr-cli --test rtmp_archive_e2e` still green; `cargo test -p lvqr-archive --features io-uring` runs on Linux CI (macOS cannot exercise tokio-uring itself -- the feature is a no-op on non-Linux targets because the dep is target-gated). | **DONE (session 89)** |
| 90 | B | Criterion bench + documentation update. | `cargo bench -p lvqr-archive --features io-uring` produces a report; `docs/deployment.md` cites the numbers. | pending |

### Risks + mitigations

* **CI runs on macOS**, which cannot exercise io_uring. The
  Linux path is covered by a `#[cfg(target_os = "linux")]`
  integration test that runs only on a GitHub Actions
  `ubuntu-latest` job. Added to `.github/workflows/ci.yml`
  as a separate job rather than a matrix cell so macOS CI
  stays fast.
* **tokio-uring runtime integration.** `tokio-uring`
  requires a current-thread runtime. The LVQR server uses
  multi-thread tokio, so the io-uring variant cannot call
  `tokio_uring::fs::File::create(...)` directly from a
  multi-thread task; it has to either (a) spin
  `tokio_uring::start` per segment inside a
  `spawn_blocking` closure, or (b) pin a long-lived
  current-thread runtime to a dedicated writer thread and
  dispatch segment writes to it. Option (a) is simpler and
  matches the current `spawn_blocking`-per-fragment
  cadence; option (b) is faster per write but needs a
  channel + ordering discussion. **Session A2 shipped
  option (a)**; session B's bench decides whether (b) is
  worth the complexity. This nuance was missing from the
  pre-session-88 plan because the plan predated the
  tokio-uring API shape decision.
* **`tokio_uring::start` panics on setup failure.**
  tokio-uring 0.5 calls `.unwrap()` on `Runtime::new`
  internally, with no fallible variant exposed on the
  `Builder`. Session A2 uses `std::panic::catch_unwind` to
  observe the setup failure without aborting the process.
  This is documented in `crates/lvqr-archive/src/writer.rs`;
  if tokio-uring ever ships a fallible `Builder::build` the
  catch_unwind block should be swapped for the explicit
  error. Session B's bench is a good place to revisit.

## 4.3 -- C2PA signed media (1 week, 2 sessions)

### Scope (what lands)

1. `lvqr-archive` gains a `C2paConfig` field:
   `signing_cert_path`, `private_key_path`, `assertion_creator`.
2. On `finalize()` (broadcaster disconnect), the archive
   emits a C2PA manifest asserting authorship + the SHA-256
   of the finalized MP4 bytes. Uses `c2pa-rs` 0.x.
3. A new admin route `GET /playback/verify/{broadcast}`
   that runs `c2pa::Reader::from_file(...)` on the archive
   file and returns the asserted identity + validation
   status as JSON.
4. Integration test: ingest one RTMP broadcast with
   C2PA configured, let it finalize, verify the manifest
   parses.

### Anti-scope

* **Live-signed streams.** Streaming C2PA (sign-as-you-go)
  is research and is deferred; the MVP signs at finalize
  only. File-at-rest signing covers the dominant
  provenance use case (legal discovery, broadcast archive,
  journalism).
* **Custom trust roots.** The MVP uses the Adobe test CA
  roots bundled with `c2pa-rs`. Operator-supplied PKI is
  a follow-up.

### Session decomposition

| # | Session | Deliverable | Verification |
|---|---|---|---|
| 90 | A | `C2paConfig` + finalize-time signing hook in `lvqr-archive`. | `cargo test -p lvqr-archive --test c2pa_sign` |
| 91 | B | `/playback/verify/{broadcast}` admin route + E2E verification test. | `cargo test -p lvqr-cli --test c2pa_verify_e2e` |

### Risks + mitigations

* **c2pa-rs API churn.** Pin to a specific 0.x version
  behind the workspace dep. Revisit on 1.0.

## 4.8 -- One-token-all-protocols (1 week, 2 sessions)

### Scope (what lands)

1. `lvqr-auth` gains a `normalized_auth(request_kind)`
   helper: given a JWT (via `--jwt-secret`), returns the
   same `AuthDecision` regardless of whether the request
   arrived over RTMP, WHIP, SRT, RTSP, MoQ, or WebSocket.
2. All five ingest surfaces (`lvqr-ingest`, `lvqr-whip`,
   `lvqr-srt`, `lvqr-rtsp`, `lvqr-cli` WS ingest) call the
   normalised helper instead of their per-protocol
   one-offs. No behavioural change for operators using
   static tokens; JWT users gain the uniform claim surface.
3. Documented JWT claim shape in `docs/auth.md` (new
   document).
4. Integration test: one JWT accepted by all five ingest
   paths against the same `TestServer` instance.

### Anti-scope

* **OAuth2 / JWKS dynamic key fetching.** Static HS256
  is the supported path. OAuth2 + OIDC discovery is a
  Tier 5 item if a customer needs it.
* **Per-protocol claim differences.** The JWT claims are
  a flat `(sub, broadcast, scope)` tuple regardless of
  protocol. Protocol-specific claims (e.g. RTMP vs WHIP
  metadata) are out of scope.

### Session decomposition

| # | Session | Deliverable | Verification |
|---|---|---|---|
| 92 | A | `normalized_auth` in `lvqr-auth`; plumb through all five ingest crates. | `cargo test -p lvqr-auth --lib` |
| 93 | B | Integration test: one JWT, five protocols, one `TestServer`. | `cargo test -p lvqr-cli --test one_token_all_protocols` |

### Risks + mitigations

* **Protocol-specific auth quirks** (e.g. SRT streamid
  format). Handled by a thin per-protocol extractor that
  feeds the normalised verifier with the raw token bytes;
  the verifier itself stays protocol-agnostic.

## 4.5 -- In-process AI agents framework (3 weeks, 6-8 sessions)

### Scope (what lands)

1. New crate `crates/lvqr-agent/`. Defines a trait
   `Agent { fn on_fragment(&mut self, f: &Fragment); }`
   for in-process zero-RTT access to the fragment stream.
2. First concrete agent: `WhisperCaptionsAgent` uses
   `whisper-rs` FFI to transcribe AAC audio fragments and
   publish a captions track (`<broadcast>/captions`) back
   through the MoQ egress.
3. Public demo: `lvqr serve --whisper-model ggml-small.bin
   --enable-captions` transcribes an English broadcast
   with <1 s latency; hls.js clients can subscribe to the
   captions track via HLS's subtitle rendition group.
4. Agent lifecycle: agents spawn on `BroadcastStarted`,
   stop on `BroadcastStopped`. Failures in an agent do
   NOT propagate to the broadcast; a panic in an agent
   thread is caught and logged.

### Anti-scope

* **Multi-language transcription.** English only. Other
  models load but are not validated.
* **Function calling / agent frameworks.** No LangChain,
  no tool use, no multi-agent conversation. The Agent
  trait is a `fn(&Fragment) -> ()` stream processor, not
  a goal-directed agent in the LLM sense.
* **GPU acceleration.** whisper-rs supports Metal/CUDA via
  feature flags; default is CPU-only. GPU is gated behind
  a `whisper-metal` / `whisper-cuda` feature for later
  validation.

### Session decomposition

| # | Session | Deliverable | Verification |
|---|---|---|---|
| 94 | A | `lvqr-agent` scaffold + `Agent` trait + test harness. | `cargo test -p lvqr-agent --lib` |
| 95 | B | `WhisperCaptionsAgent` reading AAC audio, feeding whisper-rs. | `cargo test -p lvqr-agent --test whisper_basic` |
| 96 | C | Captions track publish via `lvqr-moq`; HLS subtitle rendition wiring in `lvqr-hls`. | `cargo test -p lvqr-cli --test captions_hls_e2e` |
| 97 | D | `--whisper-model` CLI + lifecycle + E2E demo. | Manual: ffmpeg publish + browser hls.js playback with captions visible. |

### Risks + mitigations

* **whisper.cpp binding stability.** `whisper-rs` is the
  accepted binding; pin the version and bundle a small
  test model (ggml-tiny) in `lvqr-conformance` fixtures.
* **AAC decode dependency.** whisper eats raw PCM; we
  need an AAC decoder. Use `symphonia` for decode-only
  (no encode side needed since we are not transcoding
  audio, only captioning it).
* **3-week cap slippage.** If session 97 D still blows
  the budget, cut multi-language validation and
  `--whisper-language` flag; ship English-only.

## 4.4 -- Cross-cluster federation (2 weeks, 4 sessions)

### Scope (what lands)

1. `lvqr-cluster` gains a `FederationLink { remote_url,
   auth_token, forwarded_broadcasts: Vec<String> }`
   config.
2. On startup, each federation link opens a single
   authenticated MoQ session to the remote cluster's MoQ
   relay endpoint. For every broadcast name in
   `forwarded_broadcasts`, the local cluster subscribes to
   the remote's MoQ origin and re-publishes into the local
   origin.
3. Direction is unidirectional per link. Bidirectional
   federation is two separate links.
4. Config-driven (TOML or CLI flag); no auto-discovery.
5. Demo: cluster A in us-east, cluster B in us-west; a
   publisher on A produces a broadcast visible on both
   clusters via the federation link.

### Anti-scope

* **Conflict resolution.** The same broadcast name on
  both clusters produces two broadcasts; the downstream
  subscriber sees the local one. Reconciliation is
  research.
* **Distributed broadcast catalog.** No cluster-of-clusters
  metadata layer. Operators curate the forwarded list.
* **Auto-discovery.** No DNS-SD, no chitchat federation.
  Federation links are explicit.

### Session decomposition

| # | Session | Deliverable | Verification |
|---|---|---|---|
| 98 | A | `FederationLink` config + MoQ subscribe loop. | `cargo test -p lvqr-cluster --test federation_unit` |
| 99 | B | Two-cluster integration test: A publishes, B subscribes via link. | `cargo test -p lvqr-cli --test federation_two_cluster` |
| 100 | C | Admin route `/api/v1/cluster/federation` + reconnect on link failure. | `cargo test -p lvqr-cli --test federation_reconnect` |

### Risks + mitigations

* **MoQ relay-of-relay bugs.** Proven out in the Tier 3
  cluster plane via in-process e2e; the federation link
  is structurally the same pattern at a longer network
  distance.
* **Authentication between clusters.** Use the existing
  JWT path (Tier 4 item 4.8, which lands earlier in the
  priority order). Each link's `auth_token` is a JWT
  minted for the remote cluster's audience.

## 4.6 -- Server-side transcoding (2 weeks, 4 sessions)

### Scope (what lands)

1. New crate `crates/lvqr-transcode/`. Subscribes to a
   fragment stream, pushes samples through a `gstreamer-rs`
   pipeline, and publishes the output as a new broadcast
   (e.g. `live/foo/720p`).
2. Hardware encoders (NVENC, VAAPI, QSV, VideoToolbox)
   via gstreamer plugins. Feature-gated
   (`hw-nvenc`, `hw-vaapi`, etc).
3. ABR ladder generation: configurable rendition list
   (`720p, 480p, 240p` as defaults), per-broadcast
   override via a config file or admin API.
4. Demo: ingest a single 1080p stream, automatically
   generate a 720p/480p/240p ladder, LL-HLS master
   playlist references all four renditions.

### Anti-scope

* **Audio transcoding.** MVP is video only. AAC
  passthrough to every rendition.
* **Custom filter graphs.** Operators pick from the
  gstreamer preset list; arbitrary pipelines are not
  configurable via admin API.
* **Per-subscriber transcoding.** ABR renditions are
  produced once per broadcast, not per subscriber.

### Session decomposition

| # | Session | Deliverable | Verification |
|---|---|---|---|
| 101 | A | Scaffold `lvqr-transcode`; gstreamer-rs pipeline for one 720p rendition. | `cargo test -p lvqr-transcode --test basic_720p` |
| 102 | B | Ladder generation; multi-rendition publish. | `cargo test -p lvqr-cli --test transcode_ladder` |
| 103 | C | Hardware encoder feature flags; benchmark NVENC vs x264. | Documented in `docs/deployment.md`. |

### Risks + mitigations

* **gstreamer plugin availability.** The `gst-plugins-bad`
  and `gst-plugins-ugly` packages are required for NVENC;
  document in deployment guide. macOS CI gets the
  VideoToolbox path.
* **Pipeline deadlocks.** gstreamer's appsrc/appsink can
  deadlock under backpressure. Mitigation: bounded queue
  with drop-oldest policy on the fragment-in side.

## 4.7 -- Latency SLO scheduling (1 week, 2 sessions)

### Scope (what lands)

1. End-to-end latency histogram per subscriber via OTel
   (`lvqr_subscriber_glass_to_glass_ms` histogram with
   `transport` and `broadcast` labels).
2. SLO alert rule pack shipped in `deploy/grafana/` that
   fires when the p99 of the histogram exceeds a
   configurable threshold.
3. `/api/v1/slo` admin route returning the current p50/p95/p99
   latency per active broadcast.

### Anti-scope

* **Admission control.** "Refuse subscribers that would
  blow the budget" is research. Ship the measurement
  first.
* **Per-subscriber latency shaping.** Operators react via
  alerts; the server does not throttle.

### Session decomposition

| # | Session | Deliverable | Verification |
|---|---|---|---|
| 104 | A | Histogram wiring + `/api/v1/slo` route. | `cargo test -p lvqr-admin --test slo_route` |
| 105 | B | Grafana alert pack + documentation. | Manual: Grafana imports the JSON. |

### Risks + mitigations

* **Clock sync between publisher and subscriber.** The
  metric is emitted server-side only, based on wall-clock
  time between fragment receipt on ingest and fragment
  emit on egress. Not true glass-to-glass; client-side
  measurement is a Tier 5 SDK item.

## Total Tier 4 ETA

* 8 items, ~27 sessions (including buffer).
* At 10-15 sessions per calendar week: **2-3 calendar
  weeks of wall time**, ~10-12 focused calendar weeks.
* Item 4.6 (server-side transcoding) is the most likely
  to slip; if it blows its 2-week cap, cut it to v1.1.

## Verification plan

Per tier:

1. Every item has its own 5-artifact test coverage per
   `tests/CONTRACT.md`. Session close blocks document
   which artifacts are in scope vs which are soft-skipped
   pending external validators.
2. `cargo test --workspace` remains green at every
   session close.
3. Each item that ships a public demo (4.2, 4.3, 4.5,
   4.4) gets a shell script under
   `examples/tier4-demos/` that a fresh user can run
   end-to-end.

## Exit criteria (when is Tier 4 done)

* The M4 marketing milestone lands: MoQ demo with
  sub-200ms glass-to-glass, WASM filter showcase, C2PA
  signed broadcast, live captions, ABR ladder, federation
  demo.
* `cargo test --workspace` remains green.
* `HANDOFF.md` documents every Tier 4 surface as feature-
  complete or explicitly deferred to v1.1.
* At least one Tier 4 item has a working public demo
  script under `examples/tier4-demos/`.

## What Tier 5 unlocks once Tier 4 ships

Tier 5 (ecosystem) depends on Tier 4 being stable enough
to demo:

* **Kubernetes operator** orchestrates clusters that use
  the federation link (4.4).
* **Helm chart** installs a cluster with ABR transcoding
  (4.6) enabled by default.
* **SDK parity** with LiveKit depends on the one-token-
  all-protocols (4.8) normalisation being done.
* **Docs site tutorials** exercise WASM filters (4.2) and
  AI captions (4.5) in end-to-end recipes.

None of Tier 5 is blocked on a design decision; everything
hinges on Tier 4 surfaces being stable.

## Dependencies to pin

Target versions (to verify before each item starts):

| Crate | Version | Item | Notes |
|---|---|---|---|
| `wasmtime` | 25.0 | 4.2 | Component model + WASI 0.2 stable as of 25.0. |
| `tokio-uring` | 0.5 | 4.1 | Linux-only; feature-gated. |
| `notify` | 6 | 4.2 | Cross-platform file watcher for hot-reload. |
| `c2pa-rs` | 0.x | 4.3 | Pin exact version; API churns. |
| `whisper-rs` | 0.12 | 4.5 | Matches whisper.cpp 1.6+. |
| `symphonia` | 0.5 | 4.5 | AAC decode for captions input. |
| `gstreamer-rs` | 0.23 | 4.6 | gst-plugins-bad at 1.22+. |

Pin exact versions in `Cargo.toml` workspace deps. Any
upgrade gets its own session.

## Non-goals (explicit)

* **Browser WASM.** v1.0 of `lvqr-wasm` is server-side
  wasmtime only. A browser WASM target is a Tier 5 item
  if a customer asks.
* **Multi-filter pipelines.** Single WASM filter per
  broadcast. Chaining is research.
* **SIP.** Confirmed out of scope for v1 per the ROADMAP
  risk table.
* **Room-composite egress** (a la LiveKit's egress
  service). Not in scope for v1; revisit when comparing
  to LiveKit-enterprise.
* **Live-signed streams.** C2PA on live streams is
  research.
* **GPU WASM filters.** WASM + GPU does not have a stable
  story in wasmtime as of 0.27. Research for v2.
* **Admission control.** SLO scheduling measures but does
  not refuse subscribers.
* **OAuth2 / JWKS discovery.** HS256 static JWT only.

## Resolved questions (session 84, before code)

1. **WASM runtime = wasmtime**, not wasmer or wasmi.
   wasmtime has the component-model implementation that
   lets us bind the `fragment-filter` WIT interface
   directly; wasmer's component model support lags.
2. **AI agent trait runs synchronously on the fragment
   hot path.** The `Agent::on_fragment` signature is
   `&mut self` + `&Fragment` returning `()`. Agents that
   do expensive work (transcription) buffer internally
   and emit outputs on their own background task; the
   fragment-stream thread is never blocked.
3. **Federation auth = JWT via item 4.8.** Do not invent
   a new auth layer; reuse the one-token-all-protocols
   normaliser.
4. **Transcoding output is a new broadcast**, not a new
   track on the source broadcast. The master HLS playlist
   references all renditions via the existing audio-
   rendition-group mechanism, extended to video
   renditions.
5. **SLO metric is server-side only in v1.** True
   glass-to-glass (client-measured) lands in the SDK
   work in Tier 5.

## Open questions (defer to per-item session entry)

1. **C2PA assertion creator identity.** Operators supply
   the signing cert + key; what is the default
   `assertion_creator` string? Proposal: `urn:lvqr:node/<node_id>`.
2. **Federation link encryption.** MoQ already uses
   QUIC (encrypted). Do we add an additional auth bearer
   in the MoQ session SETUP, or rely on the QUIC peer
   cert? Proposal: JWT bearer, matching the 4.8
   normalisation path.
3. **WASM filter output audio preservation.** The MVP
   is video-only for filters; does audio passthrough
   unchanged or also run through the filter's
   `on-fragment`? Proposal: audio passes through
   untouched in v1; audio filters are research.

These are non-blocking for session 85 (4.2 scaffold). They
get answered in the session that lands the affected item.

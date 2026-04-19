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

## 4.1 -- io_uring archive writes (3 sessions, 88-90) -- **COMPLETE (sessions 88-90)**

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
| 90 | B | Criterion bench + operator documentation. `crates/lvqr-archive/benches/io_uring_vs_std.rs` parameterises `write_segment` across `[4 KiB, 64 KiB, 256 KiB, 1 MiB]` segment sizes with criterion 0.5's `BenchmarkId::from_parameter` + `Throughput::Bytes` so per-variant throughput + latency are both reported; `measurement_time = 2s` + `sample_size = 30` caps a full run to ~8 s wall time + ~1 GB of tempdir writes at the top segment size. The harness does not cfg-gate itself: on macOS + Windows `cargo bench -p lvqr-archive` exercises the std path as a smoke test; on Linux + `--features io-uring` it exercises the tokio-uring path. Operators capture the std vs io-uring comparison on their own host via criterion's saved-baseline workflow (`--save-baseline std` + `--baseline std`); numbers are per-host and not portable, so the docs section drives the workflow rather than citing fixed numbers. `docs/deployment.md` gains an "Archive: `io_uring` write backend (Linux-only)" section covering when to enable, how to build + enable, how to run the bench, how to interpret the output, the `OnceLock` cold-start warn operator runbook, and caveats (create_dir_all stays on std::fs, reader path still uses tokio::fs). | `cargo bench -p lvqr-archive --no-run` + a smoke run on macOS (std path, 10-sample variant) green; `cargo clippy --workspace --all-targets --benches -- -D warnings` clean with the new bench in scope. | **DONE (session 90)** |

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

## 4.3 -- C2PA signed media (4 sessions, 91-94) -- COMPLETE

### Plan-vs-code status (refreshed session 91 A, item COMPLETE as of session 94)

The session-84 plan said "on `finalize()` (broadcaster
disconnect), the archive emits a C2PA manifest ... of
the finalized MP4 bytes." The actual archive
architecture (confirmed session 91 A via
`lvqr_cli::archive::BroadcasterArchiveIndexer::drain`
+ `lvqr_fragment::FragmentBroadcasterRegistry`) is:

* **No finalize event.** The drain task exits silently
  when `FragmentStream::next_fragment` returns `None`;
  the registry exposes `on_entry_created` but no
  `on_entry_removed` or broadcast-end hook.
* **No init.mp4 on disk.** Init bytes live in memory
  in `FragmentBroadcaster::meta()`; subscribers
  reconstruct init from their subscription snapshot.
  Only the `.m4s` media segments exist under
  `<archive_dir>/<broadcast>/<track>/`.
* **No single finalized MP4.** The archive is a stream
  of fragments keyed by `(broadcast, track, start_dts)`
  in the redb index, not a concatenated file.

So "sign the finalized MP4" has no referent today.
Session A ships the signing primitive that is
independent of this mismatch; session B owns the
finalize-asset construction (persist init bytes +
register a broadcast-end lifecycle hook + concatenate
segments by dts) and the admin verify route together.
This re-scope is the session-88 A1 precedent: when
plan and code disagree, refresh the plan alongside
the code change.

**Session 94 close update.** B3 landed the missing
pieces: `FragmentBroadcasterRegistry::on_entry_removed`
hook fires from explicit `remove()` calls the RTMP
bridge now issues on unpublish; init bytes persist to
flat `<archive>/<broadcast>/<track>/init.mp4` at
first-fragment time; the drain task terminates per-
broadcast (was per-server-shutdown) and runs
`finalize_broadcast_signed` inside spawn_blocking;
`/playback/verify/{broadcast}` reads the finalize pair
back via `c2pa::Reader::with_manifest_data_and_stream`.
The architecture now matches the session-84 plan's
intent even though the path got there via a different
route than originally specified.

### Scope (what lands)

1. `c2pa` compile-time feature on `lvqr-archive`
   (default off). Pulls `c2pa = "0.80"` with
   `default-features = false, features =
   ["rust_native_crypto"]` so the crypto closure stays
   pure-Rust (no vendored OpenSSL C build) and the
   remote-manifest HTTP stacks (reqwest + ureq) are
   absent.
2. `lvqr_archive::provenance::C2paConfig` operator
   configuration (signing cert path, private key path,
   creator-assertion name, signing algorithm, optional
   RFC 3161 TSA URL). `C2paSigningAlg` 1:1 with
   `c2pa::SigningAlg` so downstream consumers do not
   need a direct `c2pa-rs` dep to construct a
   `C2paConfig`. `ArchiveError::C2pa(String)`
   error variant, feature-gated.
3. `sign_asset_bytes(&config, format, bytes) ->
   Result<SignedAsset, ArchiveError>` primitive.
   Wraps `c2pa::Builder::from_context(Context::new())
   .with_definition(manifest_json)`, `set_no_embed(
   true)`, `set_intent(BuilderIntent::Edit)`, and
   `sign(&signer, format, &mut src, &mut dst)`
   against in-memory cursors. Returns the (unchanged)
   asset bytes + the sidecar manifest bytes so the
   caller chooses on-disk layout.
4. Finalize-asset construction: given a
   `(broadcast, track)` pair, build the bytes to
   sign by persisting init bytes at first-write time
   + concatenating `init + segments ordered by dts`
   when the drain task terminates; hook a
   broadcast-end lifecycle callback onto
   `FragmentBroadcasterRegistry` so the finalize
   task fires at the right moment.
5. `GET /playback/verify/{broadcast}` admin route
   that runs `c2pa::Reader::from_manifest_data_and_stream`
   against the signed asset + sidecar manifest and
   returns the asserted identity + validation status
   as JSON.
6. End-to-end test: ingest one RTMP broadcast with
   C2PA configured, let the broadcaster disconnect,
   verify the manifest parses via the admin route.

### Anti-scope

* **Live-signed streams.** Streaming C2PA (sign-as-
  you-go) is research and is deferred; the MVP signs
  at finalize only. File-at-rest signing covers the
  dominant provenance use case (legal discovery,
  broadcast archive, journalism).
* **Custom trust roots.** The MVP uses c2pa-rs's
  default `CertificateTrustPolicy`. Operator-supplied
  PKI is a follow-up.
* **Reader-side c2pa outside the admin route.** The
  playback file-handler stays unaware of C2PA; only
  the admin verify route reads manifests.

### Session decomposition

| # | Session | Deliverable | Verification | Status |
|---|---|---|---|---|
| 91 | A | `c2pa` feature on `lvqr-archive` + `C2paConfig` / `C2paSigningAlg` / `SignedAsset` / `sign_asset_bytes` primitive in new `provenance` module. `ArchiveError::C2pa(String)` feature-gated variant. Integration test `tests/c2pa_sign.rs` with error-path coverage live + happy-path `#[ignore]`'d pending a C2PA-spec-compliant cert-chain fixture. New `archive-c2pa` CI job on `ubuntu-latest`. Plan refresh above to reflect the archive-is-a-stream reality the session-84 plan was silent about. | `cargo test -p lvqr-archive --features c2pa --test c2pa_sign` green (1 passed, 1 ignored); `cargo clippy --workspace --all-targets --benches -- -D warnings` clean; `cargo test --workspace` unchanged at 739 passed. | **DONE (session 91)** |
| 92 | B1 | Two composition primitives in `provenance` that session B2 wires into the drain-terminated finalize flow: (i) `concat_assets(&[impl AsRef<Path>]) -> Result<Vec<u8>, ArchiveError>` reads a caller-supplied ordered list of paths into one buffer (caller walks the redb index + collects paths in `start_dts` order; this primitive stays redb-free so it tests cleanly without spinning up a DB); (ii) `write_signed_pair(asset_path, manifest_path, &SignedAsset) -> Result<(), ArchiveError>` writes both files with on-demand parent-dir creation. One new config field `C2paConfig.trust_anchor_pem: Option<String>` wired through `c2pa::Context::with_settings({"trust": {"user_anchors": ...}})` so operators with a private CA have a first-class path (this is also the production workflow). Cert-fixture debug attempt this session: confirmed that `Settings.trust.user_anchors` addresses trust-chain validation only, not the structural-profile validation that is failing; re-scoped to B2 with updated test docblock listing three fixture options. No finalize-asset orchestration yet -- that is B2 because it crosses the `lvqr-fragment` / `lvqr-archive` / `lvqr-cli` surface and needs a broadcast-end lifecycle hook on `FragmentBroadcasterRegistry` that is a load-bearing primitive for 4.4 + 4.5 too. | `cargo test -p lvqr-archive --features c2pa` green (31 lib tests passing incl. 5 new provenance helpers + 1 integration + 1 ignored); `cargo clippy --workspace --all-targets --benches -- -D warnings` clean; `cargo clippy -p lvqr-archive --features c2pa --all-targets -- -D warnings` clean; `cargo test --workspace` unchanged at 739. | **DONE (session 92)** |
| 93 | B2 | Cert-fixture breakthrough + sign-side composability refactor + finalize orchestration helpers. Discovery: `c2pa::EphemeralSigner` (publicly re-exported from c2pa 0.80) generates C2PA-spec-compliant Ed25519 chains in memory using c2pa-rs's own `ephemeral_cert` module + rasn_pkix -- exactly the extension layout the structural-profile check wants. The session-91 happy-path test, `#[ignore]`'d through sessions 91-92 because rcgen-generated chains kept tripping `CertificateProfileError::InvalidCertificate`, unignores via this signer with zero PEM-fixture maintenance. Refactor: extract `sign_asset_bytes` body into a new `sign_asset_with_signer(&dyn c2pa::Signer, &SignOptions, format, bytes)` primitive; `sign_asset_bytes` now reads PEMs + delegates. New `SignOptions` carries the subset of `C2paConfig` independent of PEM paths + alg (creator + trust anchor) so any-signer callers do not need a fake config. Two new finalize orchestration helpers: `finalize_broadcast_signed(&C2paConfig, init_bytes, segment_paths, format, asset_path, manifest_path) -> Result<SignedAsset, ArchiveError>` + `_with_signer` variant compose `concat_assets` + `sign_asset_with_signer` + `write_signed_pair` so session 94's drain integration is a one-call site. `init_bytes` is taken as a parameter so the orchestrator stays agnostic to where init persistence lives (flat `init.mp4` vs. `metadata.json` sidecar -- session 94's call). Test suite migrated from rcgen-based `#[ignore]` to EphemeralSigner-based: 3 tests, 0 ignored (`sign_asset_with_signer_emits_non_empty_c2pa_manifest_for_minimal_jpeg`, `finalize_broadcast_signed_with_signer_writes_asset_and_manifest_pair_to_disk`, `sign_asset_bytes_reports_c2pa_error_on_missing_cert_file`). rcgen dropped from dev-deps as the only consumer was the deleted fixture builder. | `cargo test -p lvqr-archive --features c2pa --test c2pa_sign` has 3 passed 0 ignored; `cargo clippy --workspace --all-targets --benches -- -D warnings` clean; `cargo clippy -p lvqr-archive --features c2pa --all-targets -- -D warnings` clean; `cargo test --workspace` unchanged at 739. | **DONE (session 93)** |
| 94 | B3 | Drain-task integration + admin verify route + E2E. Five deliverables, all landed: (a) `FragmentBroadcasterRegistry::on_entry_removed` hook mirrors `on_entry_created` with identical `(broadcast, track, &Arc<FragmentBroadcaster>)` signature. Fires synchronously on successful `remove()` after the map write lock is released (so callbacks may freely re-enter the registry) and NEVER from Drop. Panics propagate to the `remove()` caller consistent with `on_entry_created`. Shape picked so Tier 4 items 4.4 / 4.5 can compose closures that handle both broadcast-start + broadcast-end with the same primitive. Wired through `RtmpMoqBridge::on_unpublish` which now calls `registry.remove(stream_name, "0.mp4")` + audio track so drain tasks see `next_fragment() -> None` per-broadcast (before this change, registry entries lived until server shutdown and per-broadcast finalize could not fire). (b) Flat `<archive>/<broadcast>/<track>/init.mp4` layout picked over `metadata.json` sidecar: parallels the `<seq>.m4s` segment layout for non-c2pa consumers, bytes already MP4 so concat is literal, no JSON metadata surface needed today. New `lvqr_archive::writer::write_init` + `init_segment_path` + `INIT_SEGMENT_FILENAME`. Drain task refreshes meta each loop iteration and persists on first fragment where init is available. (c) `BroadcasterArchiveIndexer::drain` takes `Option<C2paConfig>` (feature-gated) and, on while-loop exit, spawn_blocking's `finalize_broadcast_signed` which reads `init.mp4`, walks the redb segment index in `start_dts` order, concats, signs, and writes `finalized.mp4` + `finalized.c2pa` next to the segment files. Errors log at `warn!`; no retry. (d) `GET /playback/verify/{*broadcast}` admin route reading the finalize pair via `c2pa::Reader::from_context(Context::new()).with_manifest_data_and_stream(..)`, returning `{ signer, signed_at, valid, validation_state, errors }` JSON. `validation_state` is the stable string form of `c2pa::ValidationState` (`"Invalid"` / `"Valid"` / `"Trusted"`); `valid` is true for Valid + Trusted. `errors` filters out `signingCredential.untrusted` (c2pa-rs treats it as non-fatal). Auth runs the same subscribe-token gate the sister `/playback/*` routes use. (e) `crates/lvqr-cli/tests/c2pa_verify_e2e.rs`: real RTMP publish, drop publisher, poll for `finalized.c2pa` on disk with a 10 s budget, hit `/playback/verify/live/dvr`, assert `valid=true`, `validation_state="Valid"` (EphemeralSigner's CA is not in c2pa-rs's default trust list so not Trusted), non-empty signer, empty errors; also asserts 404 on an unknown broadcast. Breaking API change on `C2paConfig`: new `C2paSignerSource` enum with `CertKeyFiles { signing_cert_path, private_key_path, signing_alg, timestamp_authority_url }` + `Custom(Arc<dyn c2pa::Signer + Send + Sync>)` variants replaces the previous inline PEM path fields. Migration: old `C2paConfig { signing_cert_path, private_key_path, signing_alg, timestamp_authority_url, assertion_creator, trust_anchor_pem }` becomes `C2paConfig { signer_source: C2paSignerSource::CertKeyFiles { .. }, assertion_creator, trust_anchor_pem }`. Two new Custom-source unit tests (`sign_asset_bytes_with_custom_signer_source_delegates_to_ephemeral_signer`, `finalize_broadcast_signed_with_custom_signer_source_writes_pair_to_disk`) lock the enum-branching behaviour. Feature plumbing: `lvqr-cli` gains a `c2pa` feature enabling `lvqr-archive/c2pa` + `dep:c2pa`; `ServeConfig.c2pa: Option<C2paConfig>` is feature-gated so the struct stays ABI-stable across feature flips; `lvqr-test-utils` gains `c2pa` + `TestServerConfig::with_c2pa(..)` builder. | `cargo test -p lvqr-cli --features c2pa --test c2pa_verify_e2e` green; `cargo test -p lvqr-archive --features c2pa` 35 lib + 5 integration green; `cargo test -p lvqr-cli --test rtmp_archive_e2e` green (no regression after `registry.remove` wiring); `cargo clippy --workspace --all-targets --benches -- -D warnings` clean; `cargo clippy -p lvqr-archive --features c2pa --all-targets -- -D warnings` clean; `cargo clippy -p lvqr-cli --features c2pa --all-targets -- -D warnings` clean; `cargo test --workspace` 758 passed 0 failed 1 ignored (previously 739 / 0 / 1; +4 registry tests, +4 writer tests, +2 c2pa_sign Custom-source tests, +1 c2pa_verify_e2e test, +5 lvqr-archive lib-tests-with-c2pa-feature-on-in-workspace-build, +3 misc; the 1 remaining ignored test is the pre-existing moq_sink doctest unrelated to 4.3). | **DONE (session 94)** |

### Risks + mitigations

* **c2pa-rs API churn.** Pin to 0.80 behind the
  workspace dep. The `Builder::from_json` → `from_context
  + with_definition` migration (deprecation landed in
  0.80) is already absorbed into session A's primitive;
  any further API shape change gets its own session.
* **Certificate-profile strictness.** c2pa-rs validates
  against C2PA spec §14.5.1 at sign time: approved EKU
  from the crate's `valid_eku_oids.cfg` allow-list,
  digitalSignature KU, AKI extension present, not
  self-signed, within validity window, supported
  signature algorithm. rcgen's default output does not
  satisfy every branch and the error surface collapses
  to a single `InvalidCertificate` variant so
  distinguishing which check failed requires either
  enabling c2pa's `validation_log` or vendoring a
  spec-compliant fixture. Session B owns this work.
* **Dep bloat.** The `c2pa` feature adds ~20 transitive
  crates (img-parts, quick-xml, rasn-*, ring,
  ed25519-dalek, p256/p384/p521, etc.). Operators who
  do not need provenance should not have to link
  against them; that is why the feature is default off.
  `default-features = false` on the workspace pin
  keeps the closure to rust_native_crypto only -- no
  vendored OpenSSL C build, no reqwest/ureq.

## 4.8 -- One-token-all-protocols (1 week, 2 sessions, 95-96; **COMPLETE**)

### Plan-vs-code status (refreshed session 94 close)

Session 94 close scouted the current auth surface
before session 95 starts. Key findings that shift the
scope of what A lands:

* **`lvqr-auth::AuthProvider::check(&AuthContext)` is
  already the normalised decision surface.** Decision
  shape (`AuthDecision::{Allow, Deny{reason}}`) does
  not vary by protocol. `JwtAuthProvider` already
  handles all three `AuthContext` variants (Publish,
  Subscribe, Admin) with the `scope` / `broadcast`
  claims the session-84 plan calls for. So the
  `normalized_auth(request_kind)` helper the plan
  names is really an EXTRACTOR layer, not a verifier
  layer -- its job is to turn each protocol's
  idiosyncratic token carrier into a uniform
  `AuthContext` that the existing `AuthProvider::
  check` consumes.

* **Three of the five ingest surfaces have NO auth
  call-site today.** `lvqr-whip`, `lvqr-srt`, and
  `lvqr-rtsp` contain zero `AuthContext` / `auth.
  check` references. The session-84 "plumb through
  all five ingest crates ... instead of their per-
  protocol one-offs" phrasing assumed existing
  extractors that do not exist. Session 95 A must
  ADD auth call-sites to these three AND unify the
  extractor layer for all five. This is a scope-up
  from the original plan, similar in spirit to the
  session-91 "archive-is-a-stream" re-scope of 4.3.

* **Two ingest surfaces already call
  `AuthContext::Publish` today**:
  - `lvqr_ingest::bridge` (RTMP): `bridge.rs:456`
    inside `on_publish`. Pulls from the RTMP
    connection's (app, key) pair. JWT is carried as
    the RTMP stream key per `JwtAuthProvider`'s
    existing convention.
  - `lvqr_cli` WS ingest: `lib.rs:1415`. Token
    extracted from the WS upgrade URL's `?token=`
    query or Authorization header.

* **Three subscribe call-sites already exist**: the
  MoQ relay (`lvqr_relay::server:155`), the WS
  relay (`lvqr_cli::lib:1289`), and the playback
  router (`lvqr_cli::archive::playback_auth_gate`).
  All use `AuthContext::Subscribe` with the token
  extracted via a small protocol-specific helper.

* **Token-carrier inventory (per protocol, for the
  extractor layer)**:
  - RTMP: stream key IS the JWT (existing
    `JwtAuthProvider::Publish` shape).
  - WHIP: HTTP `Authorization: Bearer <jwt>` on the
    POST /whip/{broadcast} SDP offer. Standard.
  - SRT: `streamid` handshake parameter. No bearer
    convention. De facto format: `#!::r=<resource>,m=
    request,t=<token>` or similar `,`-separated KV
    pairs. Session 95 picks a shape and documents.
  - RTSP: `Authorization: Bearer <jwt>` on ANNOUNCE +
    RECORD (for publish) or DESCRIBE + PLAY (for
    subscribe -- though LVQR's RTSP surface is
    publish-only today). RTSP headers work the same
    as HTTP.
  - MoQ / WebSocket: existing `?token=<jwt>` query
    fallback + `Authorization: Bearer` on the
    initial upgrade. Already handled.

Net re-scope for session 95 A: ship a new module
`lvqr_auth::extract` (or similar) with five per-
protocol `fn extract_<proto>(...) -> AuthContext`
helpers, wire them into three new call-sites
(whip/srt/rtsp) and migrate two existing call-sites
(rtmp/ws-ingest) onto the shared helpers. The
decision surface (`AuthProvider::check`) does not
change.

### Scope (what lands)

1. `lvqr-auth` gains per-protocol extractor helpers
   (likely a new `extract` module or small family
   of free functions) that convert each protocol's
   token carrier (RTMP stream key, WHIP
   `Authorization` header, SRT streamid KV pairs,
   RTSP header, WS token query/header) into a
   uniform `AuthContext`. `AuthProvider::check` is
   unchanged. Given a JWT (via `--jwt-secret`), the
   same token produces the same `AuthDecision`
   regardless of protocol.
2. The three ingest surfaces that have no auth call-
   site today (`lvqr-whip`, `lvqr-srt`, `lvqr-rtsp`)
   gain one. The two ingest surfaces that already
   have a call-site (`lvqr-ingest`, `lvqr-cli` WS
   ingest) migrate to the shared extractor. No
   behavioural change for operators running
   `NoopAuthProvider` (every protocol stays open
   access). Static-token operators get a uniform
   call-site but no claim-surface change. JWT users
   gain the uniform claim surface across five
   protocols.
3. `docs/auth.md` documents the JWT claim shape
   (`sub`, `exp`, `scope`, optional `iss`, `aud`,
   `broadcast`) + the per-protocol carrier
   conventions (header vs streamid vs query vs
   stream-key) + one worked example per protocol.
4. Integration test: one JWT accepted by all five
   ingest paths against the same `TestServer`
   instance. Requires `TestServer` to bind RTMP +
   WHIP + SRT + RTSP + WS ingest simultaneously
   (`TestServerConfig::with_srt()` + `with_rtsp()`
   already exist; WHIP wiring needs a
   `TestServerConfig::with_whip()` added if absent).

### Anti-scope

* **OAuth2 / JWKS dynamic key fetching.** Static HS256
  is the supported path. OAuth2 + OIDC discovery is a
  Tier 5 item if a customer needs it.
* **Per-protocol claim differences.** The JWT claims are
  a flat `(sub, broadcast, scope)` tuple regardless of
  protocol. Protocol-specific claims (e.g. RTMP vs WHIP
  metadata) are out of scope.
* **Revocation / token introspection.** A revoked
  token's validity depends on `exp`; there is no
  revocation list.

### Session decomposition

| # | Session | Deliverable | Verification | Status |
|---|---|---|---|---|
| 95 | A | `lvqr-auth` extractor helpers (5 protocols) under `lvqr_auth::extract`; wired into `lvqr-whip` (new 401 gate on POST offer), `lvqr-srt` (new `ServerRejectReason::Unauthorized` on streamid-parse), `lvqr-rtsp` (new `401 Unauthorized` on ANNOUNCE + RECORD); migrated `lvqr-ingest` RTMP bridge + `lvqr-cli` WS ingest onto shared helpers. `AuthContext::Publish` gains `broadcast: Option<String>` for per-broadcast JWT binding on publish; `JwtAuthProvider::check`'s Publish branch enforces it when Some. `TestServerConfig::with_whip()` added. `docs/auth.md` ships. | `cargo test -p lvqr-auth --lib` 29 passed; `cargo clippy --workspace --all-targets --benches -- -D warnings` clean; `cargo test --workspace` 783 passed / 0 failed / 1 ignored (up from 758; +16 extract, +1 jwt publish-bind, +3 whip router, +5 rtsp gate). | **DONE (session 95)** |
| 96 | B | Cross-protocol auth E2E at `crates/lvqr-cli/tests/one_token_all_protocols.rs`: one `TestServer` with RTMP + WHIP + SRT + RTSP + `JwtAuthProvider`; one publish-scoped JWT bound to `live/cam1` is admitted by every surface, a wrong-secret JWT is denied by every surface (RTMP drop / WHIP 401 / SRT `ConnectionRefused` from 2401 / RTSP 401), and a JWT bound to `live/other` published against `live/cam1` is denied on WHIP/SRT/RTSP but **admitted** on RTMP (the documented anti-scope: RTMP carries the JWT as the stream key so `extract_rtmp` passes `broadcast: None` and `JwtAuthProvider` skips the per-broadcast binding). Three `#[tokio::test]` cases. No new production code -- session 95 A shipped every building block. | `cargo test -p lvqr-cli --test one_token_all_protocols` 3 passed; `cargo clippy --workspace --all-targets --benches -- -D warnings` clean; `cargo test --workspace` 786 passed / 0 failed / 1 ignored (up from 783; +3 cross-protocol). | **DONE (session 96)** |

### Risks + mitigations

* **Protocol-specific auth quirks** (e.g. SRT streamid
  format). Handled by a thin per-protocol extractor that
  feeds the normalised verifier with the raw token bytes;
  the verifier itself stays protocol-agnostic.
* **SRT streamid format choice.** No industry
  standard. Session 95 A picks one (likely `m=publish,
  r=<broadcast>,t=<jwt>`) and documents; operators
  using other ingestors (ffmpeg, OBS SRT) get the
  expected shape in docs. If a later integration
  conflicts, the extractor is a single-file diff.
* **RTSP DIGEST vs Bearer.** RTSP 2.0 supports
  Authorization: Bearer. LVQR's `rtsp-types`-based
  server should pass the header through. If not,
  session 95 extends the server's header
  handling -- small isolated change.

## 4.5 -- In-process AI agents framework (3 weeks, 4 sessions, 97-100; A DONE, B-D pending)

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

| # | Session | Deliverable | Verification | Status |
|---|---|---|---|---|
| 97 | A | New `crates/lvqr-agent` (workspace member, AGPL-3.0-or-later, edition 2024). Surface: `Agent` sync trait (`on_start(&AgentContext)` + `on_fragment(&Fragment)` + `on_stop()` lifecycle, all default-no-op except `on_fragment`); `AgentContext { broadcast, track, meta: FragmentMeta }`; `AgentFactory { name, build(&AgentContext) -> Option<Box<dyn Agent>> }` (per-stream opt-in via `None`); `AgentRunner` builder + `install(&FragmentBroadcasterRegistry) -> AgentRunnerHandle`. The runner wires one `on_entry_created` callback that subscribes synchronously inside the callback and spawns one tokio drain task per agent factory opts in for; the natural `BroadcasterStream::Closed` termination IS the broadcast-stop signal (no separate `on_entry_removed` wiring -- it would race the drain loop). Every `on_start`/`on_fragment`/`on_stop` call wrapped in `std::panic::catch_unwind(AssertUnwindSafe(..))`; panics are logged + counted on `AgentStats::panics` + bumped on `lvqr_agent_panics_total{agent,phase=start\|fragment\|stop}`; `on_fragment` panics do NOT terminate the drain loop (one bad frame must not kill the agent), `on_start` panics DO skip the drain entirely. `AgentRunnerHandle` exposes per-`(agent, broadcast, track)` `fragments_seen` + `panics` + `tracked()` -- mirror of `WasmFilterBridgeHandle`. `lvqr_agent_fragments_total{agent}` counter bumps once per fragment. No CLI wiring this session (session 98 threads it through `lvqr_cli::start` with the WhisperCaptionsFactory). No concrete agent. | `cargo fmt --all` clean; `cargo clippy --workspace --all-targets --benches -- -D warnings` clean; `cargo test -p lvqr-agent` 8 lib + 1 integration + 1 doctest = 10 passed; `cargo test --workspace` 796 passed / 0 failed / 1 ignored (up from 786; +8 lib runner tests, +1 integration, +1 doctest). | **DONE (session 97)** |
| 98 | B | `WhisperCaptionsAgent` reading AAC audio, feeding whisper-rs. Drops in as a `Box<dyn Agent>` from a `WhisperCaptionsFactory` registered on `AgentRunner` -- no changes to `lvqr-agent` itself. | `cargo test -p lvqr-agent --test whisper_basic` | pending |
| 99 | C | Captions track publish via `lvqr-moq`; HLS subtitle rendition wiring in `lvqr-hls`. | `cargo test -p lvqr-cli --test captions_hls_e2e` | pending |
| 100 | D | `--whisper-model` CLI + lifecycle + E2E demo. CLI threads `AgentRunner::install` through `lvqr_cli::start`. | Manual: ffmpeg publish + browser hls.js playback with captions visible. | pending |

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

## 4.4 -- Cross-cluster federation (2 weeks, 3 sessions, 101-103)

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
| 101 | A | `FederationLink` config + MoQ subscribe loop. | `cargo test -p lvqr-cluster --test federation_unit` |
| 102 | B | Two-cluster integration test: A publishes, B subscribes via link. | `cargo test -p lvqr-cli --test federation_two_cluster` |
| 103 | C | Admin route `/api/v1/cluster/federation` + reconnect on link failure. | `cargo test -p lvqr-cli --test federation_reconnect` |

### Risks + mitigations

* **MoQ relay-of-relay bugs.** Proven out in the Tier 3
  cluster plane via in-process e2e; the federation link
  is structurally the same pattern at a longer network
  distance.
* **Authentication between clusters.** Use the existing
  JWT path (Tier 4 item 4.8, which lands earlier in the
  priority order). Each link's `auth_token` is a JWT
  minted for the remote cluster's audience.

## 4.6 -- Server-side transcoding (2 weeks, 3 sessions, 104-106)

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
| 104 | A | Scaffold `lvqr-transcode`; gstreamer-rs pipeline for one 720p rendition. | `cargo test -p lvqr-transcode --test basic_720p` |
| 105 | B | Ladder generation; multi-rendition publish. | `cargo test -p lvqr-cli --test transcode_ladder` |
| 106 | C | Hardware encoder feature flags; benchmark NVENC vs x264. | Documented in `docs/deployment.md`. |

### Risks + mitigations

* **gstreamer plugin availability.** The `gst-plugins-bad`
  and `gst-plugins-ugly` packages are required for NVENC;
  document in deployment guide. macOS CI gets the
  VideoToolbox path.
* **Pipeline deadlocks.** gstreamer's appsrc/appsink can
  deadlock under backpressure. Mitigation: bounded queue
  with drop-oldest policy on the fragment-in side.

## 4.7 -- Latency SLO scheduling (1 week, 2 sessions, 107-108)

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
| 107 | A | Histogram wiring + `/api/v1/slo` route. | `cargo test -p lvqr-admin --test slo_route` |
| 108 | B | Grafana alert pack + documentation. | Manual: Grafana imports the JSON. |

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

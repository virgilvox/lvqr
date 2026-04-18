# LVQR Handoff Document

## Project Status: v0.4.0 -- Tier 3 COMPLETE against TIER_3_PLAN; Tier 4 item 4.2 COMPLETE; 733 tests, 26 crates

**Last Updated**: 2026-04-18 (session 87 close -- Tier 4 item 4.2 session C landed: `WasmFilterReloader` via `notify::RecommendedWatcher` on the parent directory, atomic `SharedFilter::replace` swap, 82-byte `redact-keyframes.wasm` drop-all example, committed `cargo run --example build_fixtures` helper that rebuilds both `.wat -> .wasm` fixtures deterministically, and an RTMP hot-reload E2E at `crates/lvqr-cli/tests/wasm_hot_reload.rs`. Item 4.2 overall is DONE; next session is Tier 4 item 4.1 session A (io_uring archive writes). Workspace tests 733 passing, 0 failed, 1 ignored.

## Session 87 close (2026-04-18)

### What shipped (1 feat commit + 1 close doc commit expected)

1. **Tier 4 item 4.2 session C: WASM filter hot-reload**
   (pending commit). Full writeup in the feat commit
   message; synopsis here.

   New module `crates/lvqr-wasm/src/reloader.rs` (~250
   LOC including 3 unit tests). `WasmFilterReloader::spawn(path,
   filter)` canonicalises `path`, watches the **parent
   directory** via `notify::recommended_watcher` (parent-dir
   watch is the portable best practice: macOS FSEvents and
   Linux inotify both deliver rename-into-place events cleanly
   when the target file is replaced atomically; watching the
   file itself loses events on atomic saves). A background
   worker thread drains the `notify::Event` mpsc, filters by
   canonicalised target path + `EventKind::Create|Modify|Any`,
   debounces for 50 ms (`DEFAULT_DEBOUNCE`), re-runs
   `WasmFilter::load` on the path, and calls
   `SharedFilter::replace` on success. A compile failure logs
   a `tracing::warn` and keeps the previous module live.

   Atomic semantics documented at the top of `reloader.rs`:
   `SharedFilter::replace` takes the same `Mutex` that every
   `FragmentFilter::apply` call holds, so in-flight applies
   finish on the OLD module and the very next apply observes
   the NEW module. No partial-state visibility.

   `Drop` ordering matters: sends the shutdown signal, **then
   drops the watcher** (which closes the `mpsc::Sender` in the
   notify callback and wakes the worker out of its blocking
   `recv()`), **then** `join()`s the worker. Without that
   ordering the join deadlocks. One design iteration: the
   first draft stored the watcher as a plain (non-`Option`)
   field and hung every reloader-bearing test's teardown for
   60+ seconds until the fix landed.

   Example filter:
   `crates/lvqr-wasm/examples/redact-keyframes.{wat,wasm}`.
   The `.wasm` is 82 bytes, byte-identical across rebuilds,
   returns `-1` from `on_fragment` so every fragment is
   dropped. Paired with a new
   `cargo run -p lvqr-wasm --example build_fixtures`
   helper that walks `examples/*.wat` and regenerates
   the sibling `.wasm` files via `wat::parse_str`, so future
   sessions do not need `wat2wasm` or `wasm-tools` on PATH to
   rebuild either fixture. `frame-counter.wasm` round-trips
   byte-identical through the new helper so the session-86
   fixture is unchanged on disk.

   CLI integration: `lvqr-cli::start` now spawns a
   `WasmFilterReloader` alongside
   `install_wasm_filter_bridge` whenever `--wasm-filter` is
   set. `ServerHandle` gets a new
   `_wasm_reloader: Option<WasmFilterReloader>` field held
   solely for its `Drop` side effect. No new public API on
   `ServerHandle`; the reloader surfaces indirectly through
   the existing `WasmFilterBridgeHandle` counters which
   operators already watch to verify a deployed filter.

   Integration test at
   `crates/lvqr-cli/tests/wasm_hot_reload.rs` (~350 LOC).
   Seeds a tempdir `filter.wasm` with a copy of
   `frame-counter.wasm`, starts a `TestServer` pointed at
   it, publishes a real RTMP broadcast (`live/hot-reload-
   before`) via the proven `rml_rtmp` handshake +
   `ClientSession` pattern, asserts tap observed at least
   one fragment with `dropped == 0`, drops the RTMP session,
   atomically-renames `redact-keyframes.wasm` over
   `filter.wasm`, sleeps 500 ms for the watcher, publishes a
   second broadcast (`live/hot-reload-after`), and polls for
   `fragments_dropped > 0` on the new broadcast with a 10 s
   budget. Total wall-clock: ~1 s on a warm-cache Apple
   Silicon run.

2. **Test-contract script comment refresh**
   (pending commit). `scripts/check_test_contract.sh` still
   reports `lvqr-wasm` integration + E2E slots as missing
   because the tests live cross-crate in
   `lvqr-cli/tests/wasm_{frame_counter,hot_reload}.rs`
   (accepted case-by-case per `tests/CONTRACT.md`). Updated
   the inline comment to reflect session-87 reality: both
   integration tests now exist, and the educational warnings
   will remain until a future session either moves the tests
   in-tree or extends the script with a per-crate integration
   exemption mechanism. Fuzz + conformance slots stay open
   pending a WASM trap-surface fuzzer.

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 3 | `reloader::tests::*` in `lvqr-wasm/src/reloader.rs` | ok |
| 1 | `wasm_filter_hot_reload_flips_drop_behavior_mid_stream` in `lvqr-cli/tests/wasm_hot_reload.rs` | ok |

Total workspace tests: **733** (+4 from session 86's 729).

### Ground truth (session 87 close, pre-close-doc)

* **Head**: session-87 feat commit pending on `main` before
  this close-doc commit lands. Local main is 1 commit ahead
  of origin/main at the end of session 86 close; after the
  feat + this close-doc lands locally, main will be 3 ahead.
  Do NOT push without direct user instruction.
* **Tests**: **733** passed, 0 failed, 1 ignored.
* **CI gates locally clean**: fmt, clippy workspace
  --all-targets --benches -- -D warnings, test --workspace
  all green.
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** (A + B + C DONE) | 85 (A) / 86 (B) / 87 (C) |
| 4.1 | io_uring archive writes | PLANNED | 88-89 |
| 4.3 | C2PA signed media | PLANNED | 90-91 |
| 4.8 | One-token-all-protocols | PLANNED | 92-93 |
| 4.5 | In-process AI agents | PLANNED | 94-97 |
| 4.4 | Cross-cluster federation | PLANNED | 98-100 |
| 4.6 | Server-side transcoding | PLANNED | 101-103 |
| 4.7 | Latency SLO scheduling | PLANNED | 104-105 |

### Session 88 entry point

**Tier 4 item 4.1 session A: io_uring archive writes.**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.1 session A:

1. Feature-gated `tokio-uring` path for init + media segment
   writes in `lvqr-archive`. Feature `io-uring` off by
   default; Linux-only. Wire through
   `IndexingFragmentObserver::write_all` so archive segments
   go through `tokio-uring::fs` when the feature is on and
   `tokio::fs` otherwise.
2. Graceful runtime fallback: if `tokio_uring::start` fails
   (kernel < 5.6, container without io_uring syscalls), log
   a `warn` and drop back to `tokio::fs` without propagating
   the error.
3. Gate macOS CI on the non-feature path; add a Linux-only
   `cargo test --features io-uring` job to
   `.github/workflows/ci.yml` as a separate cell, not a
   matrix.

Expected scope: ~250-400 lines; no new crate. Risk: tokio-
uring requires a current-thread runtime. The archive writer
already runs on its own per-broadcast task so this should be
compatible with `lvqr-cli::start`'s multi-thread runtime
without any flavor change, but verify at first attempt.

## Session 86 close (2026-04-17)

### What shipped (3 commits total)

1. **Hygiene sweep** (`67763d1`). HANDOFF.md rotated from
   11,734 lines (564 KB) down to 345 lines; sessions 83 back
   to 1 archived verbatim to
   `tracking/archive/HANDOFF-tier0-3.md`. Five legacy AUDIT
   docs (`AUDIT-2026-04-10.md`,
   `AUDIT-2026-04-13.md`, `AUDIT-INTERNAL-*`,
   `AUDIT-READINESS-*`, `notes-2026-04-10.md`) moved via `git
   mv` to `tracking/archive/` with a new
   `tracking/archive/README.md` mapping each file to its
   role. `lvqr-wasm` added to the 5-artifact contract
   IN_SCOPE list so the educational warnings for its missing
   fuzz + integration + conformance slots surface as the
   forcing function for sessions 86/87. README gets a "what
   is NOT shipped yet" block so a casual reader cannot miss
   the ROADMAP Tier 3 items TIER_3_PLAN scoped out
   (webhooks, DVR scrub UI, hot reload, captions + SCTE-35,
   stream key CRUD) plus all pending Tier 4 items. No code
   changes; test count unchanged at 724.

2. **Tier 4 item 4.2 session B: WASM observer + CLI + E2E**
   (`efca5ce`). Full writeup in the feat commit message;
   synopsis here.

   New module `crates/lvqr-wasm/src/observer.rs` (~230
   LOC). `WasmFilterBridgeHandle` is clonable, holds
   per-`(broadcast, track)` atomic counters (fragments_seen
   / kept / dropped) in a `DashMap`, and holds the per-
   broadcaster tokio tasks alive for the server lifetime.
   `install_wasm_filter_bridge(registry, filter) -> handle`
   registers an `on_entry_created` callback on the shared
   `FragmentBroadcasterRegistry`; each fresh broadcaster
   spawns one tokio task that subscribes, runs every
   fragment through `filter.apply`, increments counters, and
   fires `lvqr_wasm_fragments_total{outcome=keep|drop}`
   metrics.

   The tap is **read-only** in v1 (session-B scope
   narrowing). Drop returns update counters but the original
   fragment still flows to HLS / DASH / WHEP / MoQ / archive
   unchanged. Full stream-modifying pipelines are deferred
   to v1.1; the two clean design options (ingest-side filter
   wiring per protocol, or broadcaster-side interceptor
   inside `lvqr-fragment`) are documented at the top of
   `observer.rs` for whichever session picks it up.

   CLI + config surfaces:

   * `ServeConfig.wasm_filter: Option<PathBuf>` (loopback
     default `None`).
   * `--wasm-filter <path>` / `LVQR_WASM_FILTER` clap arg in
     `lvqr-cli`.
   * `ServerHandle.wasm_filter() -> Option<&WasmFilterBridgeHandle>`.
   * `TestServerConfig::with_wasm_filter(path)` +
     `TestServer::wasm_filter()` passthrough.

   `start()` loads + compiles the module via
   `WasmFilter::load` and installs the bridge BEFORE any
   ingest listener accepts traffic, so the very first
   fragment of the very first broadcast flows through the
   filter.

   Example filter: `crates/lvqr-wasm/examples/frame-counter.
   wat` + an 82-byte pre-compiled `frame-counter.wasm`. The
   filter is a no-op that returns the input length
   unchanged; the interesting behaviour is host-side
   counting.

   Integration test
   `crates/lvqr-cli/tests/wasm_frame_counter.rs` (~260
   LOC) publishes a real two-keyframe RTMP broadcast through
   a TestServer pointed at the committed .wasm and asserts
   the tap observed fragments on `live/frame-counter`, with
   zero drops and kept == seen > 0. No mocks, no stdout
   capture; reads straight off the bridge handle.

3. **Session 86 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 4 | `observer::tests::*` in `lvqr-wasm/src/observer.rs` | ok |
| 1 | `wasm_frame_counter_sees_every_ingested_fragment` in `lvqr-cli/tests/wasm_frame_counter.rs` | ok |

Total workspace tests: **729** (+5 from session 85's 724).

### Ground truth (session 86 close)

* **Head**: `efca5ce` on `main` (feat) before this close-doc
  commit lands. Local main was even with origin/main after
  the hygiene-sweep push (`67763d1`); this session adds two
  more commits on top. Do NOT push without direct user
  instruction.
* **Tests**: 729 passed, 0 failed, 1 ignored.
* **CI gates locally clean**: fmt, clippy workspace --all-
  targets --benches -- -D warnings, test --workspace all
  green.
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **A + B DONE**, C pending | 85 (A) / 86 (B) / 87 (C) |
| 4.1 | io_uring archive writes | PLANNED | 88-89 |
| 4.3 | C2PA signed media | PLANNED | 90-91 |
| 4.8 | One-token-all-protocols | PLANNED | 92-93 |
| 4.5 | In-process AI agents | PLANNED | 94-97 |
| 4.4 | Cross-cluster federation | PLANNED | 98-100 |
| 4.6 | Server-side transcoding | PLANNED | 101-103 |
| 4.7 | Latency SLO scheduling | PLANNED | 104-105 |

### Session 87 entry point

**Tier 4 item 4.2 session C: hot reload + a second example
filter that actually drops.**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.2
session C:

1. `WasmFilter::load` keeps its current shape; add a new
   `WasmFilterReloader` that watches the .wasm path via
   `notify::RecommendedWatcher`, compiles the new module on
   change, and calls `SharedFilter::replace(new_filter)`
   (the replace method shipped in session A).
2. In-flight `apply` calls finish on the OLD module; the
   next fragment uses the new one. Document atomicity at
   the call boundary.
3. Second example filter at
   `crates/lvqr-wasm/examples/redact-keyframes.{wat,wasm}`
   that returns -1 on every call (drops every fragment).
   Committed pre-compiled alongside the existing
   frame-counter.
4. Integration test
   `crates/lvqr-cli/tests/wasm_hot_reload.rs` at
   ~200 LOC. Publishes RTMP, asserts the frame-counter
   tap sees fragments with dropped=0. Then copies
   redact-keyframes.wasm over the configured filter path.
   Gives the watcher a beat to notice. Publishes more
   RTMP. Asserts subsequent fragments increment the
   dropped counter.

Expected scope: ~300-400 lines. Risk: notify's file-watch
semantics differ across macOS (FSEvents) vs Linux
(inotify). The existing lvqr-archive recorder has similar
exposure and landed green; worst case we use polling mode
which costs a 100 ms latency.

Also bring session C: update
`scripts/check_test_contract.sh` if needed -- the
lvqr-wasm integration slot is now met by
`tests/wasm_frame_counter.rs` (via `lvqr-cli`); the
fuzz + conformance slots remain open until a future
session.

## Session 85 close (2026-04-17)

### What shipped (1 feat commit, +1414 / -14 lines)

### Plan-faithful vs roadmap-complete

Tier 3 closed against `tracking/TIER_3_PLAN.md`'s scope
(cluster plane + observability plane). It did NOT close every
item in `tracking/ROADMAP.md`'s broader Tier 3 list. The
deferred items are tracked here explicitly so nobody reading
"Tier 3 COMPLETE" expects surfaces that were scoped out:

* **3.2 DVR scrub UI** -- `/playback/*` admin routes ship the
  JSON + byte-serving data surface. A dedicated web UI is
  Tier 5 ecosystem scope.
* **3.3 Webhook + OAuth + HMAC signed URLs** -- not shipped.
  HS256 static JWT is the only dynamic auth today.
* **3.5 Hot config reload** -- not shipped.
* **3.6 Captions + SCTE-35** -- Tier 4 item 4.5 (whisper.cpp
  captions) lands the transcription path, but SCTE-35 ad
  insertion and a full WebVTT segmenter are not scoped for
  v1.
* **3.7 Stream-key lifecycle CRUD** -- not shipped; static
  keys only.

These would add ~7 calendar weeks if a deployment needs them.
None is blocked by a design unknown.

## Session 85 close (2026-04-17)

### What shipped (1 feat commit, +1414 / -14 lines)

1. **Tier 4 item 4.2 session A: lvqr-wasm scaffold** (`727151f`).
   First Tier 4 code landing per
   `tracking/TIER_4_PLAN.md` section 4.2.

   New workspace crate `crates/lvqr-wasm/` (workspace member
   #26, NOT the browser-facing `lvqr-wasm` deleted in
   0.4-session-44; this is a fresh server-side host).

   Surface:

   * `FragmentFilter` trait. One synchronous method:
     `apply(Fragment) -> Option<Fragment>`. `Some` keeps
     (possibly with a replaced payload), `None` drops.
   * `WasmFilter` concrete impl. Compiles a WASM module via
     `WasmFilter::load(path)` or `WasmFilter::from_bytes(&[u8])`.
     Creates a fresh `wasmtime::Store` per `apply` call so
     filters cannot accumulate state across fragments (LBD
     #10 anti-scope from the plan).
   * `SharedFilter` wrapper (`Arc<Mutex<Box<dyn
     FragmentFilter>>>`) for thread-safe observer installs;
     includes `replace()` so session C's hot-reload path can
     swap modules atomically.

   Host-to-guest ABI (intentionally minimal -- core WASM, not
   the component model):

   * Guest exports `memory` (1-page initial) and
     `on_fragment(ptr: i32, len: i32) -> i32`.
   * Host writes payload to offset 0 of memory, calls
     `on_fragment(0, payload_len)`.
   * Return value: negative -> drop; non-negative N -> keep
     the fragment, use the first N bytes of memory as the
     replacement payload. N = 0 is a legal keep-with-empty-
     payload, semantically distinct from drop.
   * One substantive design cycle: original draft used `0`
     for drop, which collided with the legitimate empty-
     payload case (the `empty_payload_roundtrips_unchanged`
     unit test caught it on first run). Switched to
     negative-means-drop before commit.

   Fail-open semantics: a module that fails to instantiate
   or traps at runtime logs a `tracing::warn` and passes the
   fragment through unchanged. A single misbehaving filter
   cannot take down the server.

   Metadata pass-through: `track_id`, `group_id`,
   `object_id`, `priority`, `dts`, `pts`, `duration`, `flags`
   pass through unchanged regardless of filter output.
   Session B / C broaden the host-function surface to cover
   metadata mutation; session A ships the simplest useful
   shape so the runtime, trait, test harness, and CLI wiring
   path can land without scope entanglement.

   Workspace deps pinned (new):

   * `wasmtime = "25", default-features = false,
     features = ["runtime", "cranelift"]` -- per
     TIER_4_PLAN's dependency-pin table. Component model +
     WASI 0.2 stable as of 25.0 but we use core WASM for
     now; the dep still covers session B+ needs.
   * `notify = "6"` -- pulled in now so session 87's
     hot-reload path has the import available without a
     second Cargo edit. The watcher is stubbed in session A.

   Tests:

   * 9 unit tests in `crates/lvqr-wasm/src/lib.rs` cover
     no-op passthrough, drop, truncate, missing-memory
     fallback, empty-payload roundtrip, `SharedFilter`
     clone + `replace`, invalid-bytes rejection, and the
     `path()` accessor.
   * 1 proptest at `tests/proptest_roundtrip.rs` (256 cases)
     asserts arbitrary `Fragment` (any metadata, 0-16 KiB
     payload) roundtrips through a no-op WASM module
     byte-for-byte. 16 KiB cap is deliberate for session A
     (full bound lands with session B's `FragmentObserver`
     wiring once linear-memory growth is exercised under
     production payload sizes).
   * Test fixtures are WAT snippets assembled via the `wat`
     dev-dep at test time; no pre-compiled `.wasm` fixtures
     in the repo, no external toolchain dependency.

### Why core WASM and not the component model

Scope narrowing, not a design pivot. The
single-export `on_fragment(ptr, len) -> i32` surface binds
with `wasmtime::TypedFunc` directly and lets session A ship
the trait + harness without dragging in `cargo-component` or
a wit-bindgen build step for test fixtures. Session B is the
right place to decide whether the component-model binding is
worth its boilerplate for a broader host surface (e.g. if we
want full metadata mutation, or a richer error channel).
`FragmentFilter` is the stable surface the rest of the
workspace depends on; the transport between `WasmFilter` and
the guest module is an implementation detail that can change
without churning `FragmentBroadcasterRegistry` call sites.

### Ground truth (session 85 close, pre-session-close-doc commit)

* **Head**: `727151f` on `main`. v0.4.0. Local main is **1
  commit ahead of origin/main**; after this session-close
  doc lands it will be 2 ahead. 3 other commits from
  sessions 82-84 that were already queued had been pushed
  at session 82's close (see `6d99bef`); only sessions 83-84
  commits were held. Post-session-83 the 2 unpushed
  (session-83 feat + session-83 doc) + session-84 doc were
  all still local; this session adds session-85 feat. After
  the session-close doc commit lands: **5 commits queued**
  (9666cd1, 755d320, 7fb8dfe, 727151f, and this close doc).
  Do NOT push without direct user instruction.
* **Tests**: 724 passed, 0 failed, 1 ignored. Delta from
  session 84 (which was planning-only): +10 (9 lib unit +
  1 proptest harness with 256 cases). Delta from session 83:
  +10.
* **Code**: +1414 / -14 net. Workspace `Cargo.toml` + `Cargo.lock`
  (wasmtime 25.0.3 + notify 6.1.1 + their transitives),
  `crates/lvqr-wasm/Cargo.toml`, `crates/lvqr-wasm/src/lib.rs`
  (441 lines), `crates/lvqr-wasm/tests/proptest_roundtrip.rs`
  (90 lines).
* **Workspace**: **26 crates** (+1: `lvqr-wasm`).
* **CI gates locally clean**: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets --benches -- -D
  warnings`, `cargo test --workspace` all green.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **A DONE**, B/C pending | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | PLANNED | 88-89 |
| 4.3 | C2PA signed media | PLANNED | 90-91 |
| 4.8 | One-token-all-protocols | PLANNED | 92-93 |
| 4.5 | In-process AI agents | PLANNED | 94-97 |
| 4.4 | Cross-cluster federation | PLANNED | 98-100 |
| 4.6 | Server-side transcoding | PLANNED | 101-103 |
| 4.7 | Latency SLO scheduling | PLANNED | 104-105 |

### Session 86 entry point

**Tier 4 item 4.2 session B: WasmFragmentObserver + CLI
wiring + RTMP E2E.**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.2 session B:

1. New `WasmFragmentObserver` in `lvqr-wasm` that
   implements `lvqr_fragment::broadcaster::FragmentObserver`
   (or the equivalent observer trait used by
   `FragmentBroadcasterRegistry`). On each fragment it calls
   the `SharedFilter::apply` path and forwards the result;
   drops are sinks, not errors.
2. `lvqr-cli` gains `--wasm-filter <path>` (env
   `LVQR_WASM_FILTER`). When set, `start()` loads the
   module via `WasmFilter::load`, wraps in `SharedFilter`,
   and installs the observer on the shared
   `FragmentBroadcasterRegistry` before any ingest listener
   starts accepting traffic.
3. First example filter at
   `crates/lvqr-wasm/examples/frame-counter/`. A hand-rolled
   WAT (or a minimal Rust WASM crate if simpler) that counts
   invocations and writes to WASI stderr every 100th call.
   Committed as source + pre-compiled `.wasm` under
   `examples/frame-counter.wasm`.
4. Integration test at
   `crates/lvqr-cli/tests/wasm_frame_counter.rs`. Publishes
   real RTMP through `TestServer` with `--wasm-filter=<path>`,
   asserts stderr (or a capture hook) contains the counter
   log, asserts the fragment pipeline still reaches
   downstream egress (i.e. HLS playlist shows up with the
   expected segments).

Expected scope: ~400-600 lines. Biggest risk is WASI stderr
capture in the test harness; if that proves flaky, the
example filter writes to a host-call side channel and the
test observes the count directly.

## Session 84 close (2026-04-17)

### What shipped (1 docs commit, +620 / -1 lines across HANDOFF + TIER_4_PLAN)

Planning session only; no code changes. Wrote
`tracking/TIER_4_PLAN.md` to bound Tier 4 scope before the
first implementation session, per ROADMAP load-bearing
decision #10 (\"every Tier 4 item gets a one-page MVP spec
before work starts\").

The plan covers all 8 Tier 4 items from ROADMAP, each with a
1-page section that includes:

1. Scope (what lands)
2. Anti-scope (explicit rejections)
3. API sketch (where relevant)
4. Session decomposition (2-3 sessions per item, numbered 85
   through 105)
5. Risks + mitigations

Execution order prioritises moat value per week of work,
dependency ordering, and "public demo" items first so the M4
marketing milestone lands on schedule:

1. 4.2 WASM per-fragment filters (3 weeks, sessions 85-87)
2. 4.1 io_uring archive writes (2 weeks, sessions 88-89)
3. 4.3 C2PA signed media (1 week, sessions 90-91)
4. 4.8 One-token-all-protocols (1 week, sessions 92-93)
5. 4.5 In-process AI agents / whisper.cpp (3 weeks, 94-97)
6. 4.4 Cross-cluster federation (2 weeks, 98-100)
7. 4.6 Server-side transcoding (2 weeks, 101-103)
8. 4.7 Latency SLO scheduling (1 week, 104-105)

Total: ~27 working sessions including 3-session buffer.
Budget: sessions 85 through 111. At 10-15 sessions / calendar
week, **~10-12 focused calendar weeks** for all of Tier 4.

Plan includes explicit non-goals: no browser WASM target, no
multi-filter pipelines, no SIP, no room-composite egress, no
live-signed C2PA streams, no GPU WASM, no admission control on
SLO breach, no OAuth2 / JWKS.

Three open questions deferred to the session that lands the
affected item:
* C2PA default `assertion_creator` string (proposal:
  `urn:lvqr:node/<node_id>`)
* Federation link auth layer (proposal: JWT bearer via item
  4.8's normaliser, which lands BEFORE federation)
* WASM filter audio handling (proposal: audio passthrough
  untouched in v1)

Five resolved questions (answered in the plan itself):
* WASM runtime = wasmtime (not wasmer, not wasmi)
* AI agent trait runs synchronously on the fragment hot path
  via `&mut self + &Fragment -> ()`; expensive work buffers
  internally
* Federation auth = JWT via item 4.8 (do not invent a new
  layer)
* Transcoding output = new broadcast, not new track on
  source broadcast
* SLO metric is server-side only in v1; true glass-to-glass
  lands in Tier 5 SDKs

### Ground truth (session 84 close, pre-session-close-doc commit)

* **Head**: `755d320` on `main`. v0.4.0. Local main and
  origin/main are **EVEN** at the session-83 close
  (`755d320`) as of this session's start; after this doc
  commit lands, local will be 1 commit ahead. Do NOT push
  without direct user instruction.
* **Tests**: 714 passed, 0 failed, 1 ignored.
  Unchanged from session 83 close (no code landed this
  session).
* **Code**: planning-only. `tracking/TIER_4_PLAN.md` (~620
  lines) and this close block on `tracking/HANDOFF.md`.
* **Workspace**: 25 crates, unchanged.
* **CI gates locally clean**: no rebuild needed; session 83
  close state stands.

### Tier 3 final state (unchanged from session 83 close)

All 13 sessions DONE. Cluster plane (71-79) + observability
plane (80-83) closed. LVQR is a multi-node live video server
with turnkey OTLP telemetry.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | PLANNED, next up | 85-87 |
| 4.1 | io_uring archive writes | PLANNED | 88-89 |
| 4.3 | C2PA signed media | PLANNED | 90-91 |
| 4.8 | One-token-all-protocols | PLANNED | 92-93 |
| 4.5 | In-process AI agents | PLANNED | 94-97 |
| 4.4 | Cross-cluster federation | PLANNED | 98-100 |
| 4.6 | Server-side transcoding | PLANNED | 101-103 |
| 4.7 | Latency SLO scheduling | PLANNED | 104-105 |

### Session 85 entry point

**Tier 4 item 4.2 -- WASM per-fragment filters (session A of
3).**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.2 session A:

1. New crate `crates/lvqr-wasm/`. NOT the deleted
   browser-facing `lvqr-wasm` referenced in the post-0.4.0
   removal block; this is a fresh server-side crate for
   `wasmtime`-hosted fragment filters.
2. Pin `wasmtime = "25"` as a workspace dep. Component
   model + WASI 0.2 are stable in 25.0. Pin `notify = "6"`
   for the session-87 hot-reload path (added now so
   session A has the import available but the code path is
   stubbed).
3. Define a `FragmentFilter` trait plus one concrete impl
   `WasmFilter` that loads a WASM component from disk and
   exposes one host call: `on-fragment(fragment) -> option<fragment>`.
   Matches the `lvqr:filter@0.1.0` WIT interface documented
   in the plan.
4. One proptest under `crates/lvqr-wasm/tests/proptest_roundtrip.rs`
   that pushes arbitrary `lvqr_fragment::Fragment` values
   through a no-op filter (a WASM component that returns
   input unchanged) and asserts bytewise equality on the
   payload.
5. Skeletons for fuzz + integration + E2E + conformance
   slots per the 5-artifact contract. These can be
   educational-warning level in session A; session B closes
   them.

Expected scope: ~300-500 lines split between
`crates/lvqr-wasm/src/lib.rs` (~200 LOC),
`crates/lvqr-wasm/Cargo.toml`, a minimal WASM component
fixture under `crates/lvqr-wasm/tests/fixtures/` (can be
compiled-in-advance WASM bytes committed to the repo), and
the proptest harness. No CLI wiring in session A; that comes
in session B.

Risk to flag on entry: wasmtime 25's component-model host
binding generator has a fair amount of boilerplate. If the
generated code exceeds ~300 LOC per host call, we use the
lower-level `Linker::func_wrap` API instead of the WIT
bindgen macro; session A picks whichever ships green first.

## Archived session blocks

Sessions 83 back to 1 live in
[`tracking/archive/HANDOFF-tier0-3.md`](archive/HANDOFF-tier0-3.md).

Rotation happened at session 86 during the post-Tier-3 hygiene
sweep. Live HANDOFF now holds only Tier 4 session blocks
(session 84 onward); historical context for Tier 0 through
Tier 3 stays on disk but outside the default read path so
fresh sessions do not pay the full ~560 KB context load on
every HANDOFF.md open.

The rotation is lossless. Every session close from 1 through
83 is preserved verbatim in the archive file; this live
HANDOFF is the authoritative source going forward.

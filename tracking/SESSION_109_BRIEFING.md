# Session 109 briefing -- v1.1-A: broaden egress latency SLO instrumentation

**Kick-off prompt (copy-paste into a fresh session):**

---

You are continuing work on LVQR, a Rust live video streaming server.
**Tier 3 and Tier 4 are COMPLETE** (8 of 8 Tier 4 items landed across
sessions 85-108). `origin/main` head is `327b165` with 912 workspace
tests passing on the default gate (0 failed, 1 pre-existing ignored
`moq_sink` doctest), 29 crates. Session 109 is the first post-Tier-4
work session. Scope: broaden the Tier 4 item 4.7 latency SLO tracker's
egress coverage. Session 107 A wired the tracker +
`/api/v1/slo` admin route, 108 B shipped the Prometheus / Grafana
alert pack + operator runbook, but only the LL-HLS drain loop
currently records samples. Session 109 A lands DASH instrumentation so
MPEG-DASH subscribers appear on the dashboard + alert pack too.

## Read first, in this order

1. `CLAUDE.md`. Project rules. AGPL-3.0-or-later + commercial
   dual-license. No Claude attribution in commits. No emojis. No
   em-dashes. Max line width 120.
2. `tracking/HANDOFF.md`. Read from the top through the session 108 B
   close block. The "Session 109 entry point" callout is the same
   content as this briefing, one level denser.
3. `docs/slo.md`. The operator runbook shipped in session 108 B.
   Pay attention to the "Threshold tuning by transport" section;
   DASH's thresholds are already in the table but nothing records
   samples under `transport="dash"` yet.
4. `crates/lvqr-admin/src/slo.rs`. The `LatencyTracker` shape +
   `record(broadcast, transport, latency_ms)` surface. Your DASH
   wiring calls this exactly like the HLS drain does today.
5. `crates/lvqr-cli/src/hls.rs`. The session 107 A HLS
   instrumentation: `BroadcasterHlsBridge::install` takes
   `Option<LatencyTracker>`, `drain` records one sample per
   `push_chunk_bytes` call, `unix_wall_ms()` helper for the
   now-minus-ingest delta. Session 109 A mirrors this exactly on
   the DASH side.
6. `crates/lvqr-dash/src/bridge.rs`. `BroadcasterDashBridge::install`
   today takes `(MultiDashServer, &FragmentBroadcasterRegistry)` --
   you extend it to take an optional `LatencyTracker` the way
   `BroadcasterHlsBridge::install` already does.
7. `crates/lvqr-cli/src/lib.rs` line ~948: the DASH bridge install
   call site. Your new argument threads `Some(slo_tracker.clone())`
   here exactly like the HLS line right above it does today.
8. `crates/lvqr-cli/tests/slo_latency_e2e.rs`. The session 107 A
   integration test pattern. You extend it (or add a sibling
   `slo_latency_dash_e2e.rs`) that publishes fragments via the
   registry + polls `/api/v1/slo` until a `transport="dash"` entry
   appears.
9. Auto-memory at
   `/Users/obsidian/.claude/projects/-Users-obsidian-Projects-ossuary-projects-lvqr/memory/`.
   `project_lvqr_status.md` is refreshed through session 108 B
   close.

## Why only DASH in session 109 A (not WS / MoQ / WHEP)

The 4.7 tracker keys on `Fragment::ingest_time_ms`. DASH's
`BroadcasterDashBridge` subscribes to the shared
`FragmentBroadcasterRegistry` and drains `Fragment` values, so the
ingest stamp is available for free on the egress delivery point --
same shape as the HLS drain.

WS / MoQ / WHEP are different:

* **WS relay** (`lvqr-cli::ws_relay_session` +
  `relay_track`) subscribes to a `moq_lite` `TrackConsumer` and
  reads `Bytes` frames via `track.next_group()` +
  `group.read_frame()`. These frames are NOT `Fragment` values;
  the ingest wall-clock stamp is not propagated through the MoQ
  wire today.
* **MoQ subscribers** drink directly from `OriginProducer`; again,
  no `Fragment` wall-clock on the wire.
* **WHEP** uses `SharedRawSampleObserver` + `str0m` RTP packetizer.
  The packetizer emits per-RTP-packet bytes; mapping those back to
  the original fragment's ingest stamp is a larger refactor.

Each of those requires a v1.1 design decision (carry `ingest_time_ms`
on the MoQ frame header, or have the WS relay subscribe
*additionally* to the fragment registry purely for SLO sampling, or
a sidecar `SloSampler` with its own Hz of wall-clock sampling
heuristics). Don't do that work in 109 A; narrow to DASH + document
the WS / MoQ / WHEP blocker in the session 109 A close block.

## Session 109 A scope -- three deliverables

1. **DASH bridge instrumentation** (`crates/lvqr-dash/src/bridge.rs`).
   * `BroadcasterDashBridge::install` signature grows a fourth arg:
     `slo: Option<lvqr_admin::LatencyTracker>`. Threading pattern is
     identical to `BroadcasterHlsBridge::install` in session 107 A.
   * The per-broadcaster drain loop records
     `tracker.record(&broadcast, "dash", now_ms - fragment.ingest_time_ms)`
     on every fragment delivered to `DashServer`, skipping zero
     `ingest_time_ms` values (federation replays, test synthetics,
     backfill paths).
   * New internal `unix_wall_ms()` helper (or pull the one from
     `crates/lvqr-cli/src/hls.rs` into a shared spot; the HLS + DASH
     helpers are byte-identical, so a tiny `lvqr-core::wall_ms()` is
     a legitimate cleanup candidate -- land as part of 109 A or defer
     to a later tidy session, your call).
   * The `lvqr-admin` dep is new on `lvqr-dash`; add it to
     `crates/lvqr-dash/Cargo.toml`.

2. **CLI composition-root wiring** (`crates/lvqr-cli/src/lib.rs`
   line ~948).
   * `BroadcasterDashBridge::install(dash.clone(), &shared_registry)`
     grows `Some(slo_tracker.clone())`.
   * The shared `slo_tracker` built in `start()` already exists from
     session 107 A; you just pass it through.
   * One existing inline test in `bridge.rs` may need a `None`
     trailing arg. Update the three existing call sites inside the
     `lvqr-dash` test module so the crate keeps building.

3. **Integration test** -- decide either (a) extend
   `crates/lvqr-cli/tests/slo_latency_e2e.rs` to enable DASH on the
   TestServer and assert both `transport="hls"` and `transport="dash"`
   entries appear, or (b) add a new
   `crates/lvqr-cli/tests/slo_latency_dash_e2e.rs` mirroring the 107 A
   test shape. Pick whichever keeps the test scoping clean; if you
   extend the existing test, rename it to something protocol-neutral
   (e.g. `slo_latency_e2e.rs` stays but the test function name
   `slo_route_reports_hls_latency_samples_after_publish` should grow a
   second test function for the DASH assertion rather than conflating
   the two).

### Optional polish (do only if time remains)

* Move `unix_wall_ms()` out of `crates/lvqr-cli/src/hls.rs` +
  `crates/lvqr-ingest/src/dispatch.rs` (and the new DASH helper) into
  `crates/lvqr-core/src/lib.rs` as a `pub fn now_unix_ms()`. Three
  identical copies is the tipping point for a shared helper.
* Refresh `docs/slo.md`'s "HLS-only egress instrumentation" limitation
  bullet to reflect the new DASH coverage. The threshold table is
  already label-generic so no edit there.

## First decisions to lock in-commit (plan-vs-code rule applies)

(a) **Transport label is `"dash"`**. Keep to the unquoted lowercase
    short form; matches the existing `"hls"` convention and the rule
    pack's decision table header.

(b) **Bridge signature**: `install(multi, registry, slo: Option<...>)`.
    Option keeps the public surface backward-compatible for callers
    that don't wire the tracker (tests, external consumers of
    `lvqr-dash`).

(c) **Zero ingest-time is still "unset"**. The DASH drain loop's
    `if fragment.ingest_time_ms > 0` guard mirrors the HLS drain
    exactly -- synthetic test fragments and federation replays that
    don't stamp the field keep flowing without contaminating the
    histogram.

(d) **Asset-hygiene test changes**: probably none. The rule pack +
    dashboard already label-match generically on `transport`, so
    adding `"dash"` samples to the tracker automatically surfaces
    new `transport="dash"` dimensions without any YAML / JSON edits.
    If you decide to add a "known transports" list somewhere for
    validation, add it to `lvqr-admin::slo` (not to the YAML).

(e) **Lvqr-dash Cargo.toml new dep**: `lvqr-admin = { workspace = true
    }` is a regular dep (not optional). `lvqr-admin`'s own
    `cluster` default feature pulls in `lvqr-cluster`; confirm no
    circular dep surfaces. If `lvqr-admin` pulling `lvqr-dash`
    transitively creates a cycle, reconsider: put the `LatencyTracker`
    struct itself (no HTTP concerns) in a new `lvqr-slo` leaf crate
    OR a `lvqr-core::slo` module so `lvqr-dash` + `lvqr-hls` + every
    other egress crate can depend on it without dep-graph thrash.
    Today `lvqr-admin` depends on `lvqr-core` + `lvqr-auth`, not on
    `lvqr-dash` -- so adding `lvqr-admin` to `lvqr-dash`'s deps
    should be clean, but verify with `cargo tree -p lvqr-dash` before
    committing.

(f) **Test harness**: use `TestServerConfig::default().with_dash()`
    on the TestServer so the DASH server spins up. Reuse the synthetic
    `moof + mdat` fragment helpers from `slo_latency_e2e.rs`; they
    work against both HLS and DASH bridges because both drain on the
    same registry.

## Test shape

1. `cargo test -p lvqr-dash` -- existing tests (3 pass currently)
   continue green after the signature bump. The existing call sites
   in the crate's own test module grow a `None` trailing arg.
2. `cargo test -p lvqr-admin` -- unchanged from session 108 B's 25 +
   3 asset-hygiene tests.
3. `cargo test -p lvqr-cli --test slo_latency_e2e` (or the new DASH
   sibling) -- asserts the tracker now has both `hls` and `dash`
   transport entries for the published broadcast.
4. `cargo test --workspace` -- expect ~912 + 1-2 new integration tests
   = ~913-914 on the default gate.

## Verification gates (session 109 A close)

* `cargo fmt --all --check`.
* `cargo clippy --workspace --all-targets --benches -- -D warnings`.
* `cargo test -p lvqr-dash` green.
* `cargo test -p lvqr-admin` green.
* `cargo test -p lvqr-cli --test slo_latency_e2e` green (or the new
  DASH sibling).
* `cargo test --workspace` green.
* `git log -1 --format='%an <%ae>'` reads
  `Moheeb Zara <hackbuildvideo@gmail.com>` alone for every commit.

## Absolute rules (hard fails if violated)

* NEVER add Claude as author or co-author. No `Co-Authored-By`
  trailers. Verify with `git log -1 --format='%an <%ae>'` after
  every commit.
* No emojis in code, commit messages, or documentation.
* No em-dashes or obvious AI language patterns in prose.
* Max line width 120. fmt + clippy + test must be clean before
  committing.
* Integration tests use real ingest + egress (real registry emit,
  real HTTP fetch), not mocks. The 107 A e2e test is the pattern;
  copy it.
* Only edit files within
  `/Users/obsidian/Projects/ossuary-projects/lvqr/`.
* Do NOT push or publish without a direct instruction from the
  user.
* If the plan and the code disagree, refresh the plan in the same
  commit as the code change.

## Expected scope + biggest risks

~80-150 LOC across `crates/lvqr-dash/src/bridge.rs`,
`crates/lvqr-dash/Cargo.toml`, `crates/lvqr-cli/src/lib.rs` (one
line), + the extended integration test. Plus smaller edits on
`docs/slo.md` if you refresh the limitations bullet.

Risks, ranked:

1. **Cargo dep cycle**: `lvqr-admin` currently depends on `lvqr-core`
   + `lvqr-auth` + optionally `lvqr-cluster`. Adding `lvqr-admin` as a
   `lvqr-dash` dep creates a path `lvqr-cli -> lvqr-dash ->
   lvqr-admin`. That's not a cycle (cargo would reject it at resolve
   time), but the workspace gets a second crate pulling `lvqr-admin`
   in. Verify with `cargo tree -p lvqr-dash` before committing. If
   it turns out to be messier than expected, a tidier path is to
   extract `LatencyTracker` + `SloEntry` into a new leaf crate
   `lvqr-slo` (or move into `lvqr-core` as a module) -- that's a
   15-minute refactor with zero behavior change.
2. **DASH fragment timing**: DASH segments are typically ~2-6 s
   each. The `$Number$` segmenter closes a segment on specific
   cadences, and `push_chunk_bytes` runs per-fragment not
   per-segment, so each recorded sample corresponds to one incoming
   `Fragment` not one finalized segment. This is fine for the SLO
   metric but document it in `docs/slo.md` so operators reading the
   "sample rate" panel don't expect 1 sample per DASH segment.
3. **Test flakiness**: the 107 A `slo_latency_e2e.rs` polls for 5 s
   via `tokio::time::sleep`. Keep that budget when extending to
   DASH; DASH drain tasks spawn on the same tokio runtime and will
   register within a few hundred ms of the first fragment emit.

## After session 109 A

* Write a "Session 109 A close" block at the top of `HANDOFF.md`
  below the session 108 B close.
* Update `tracking/HANDOFF.md`'s Session 109 entry point callout --
  flip to either "109 B: MoQ-side ingest-time propagation for
  WS / MoQ / WHEP SLO instrumentation" (the deeper design work) or
  "109 B: examples/tier4-demos/ public demo script" (the Tier 4
  exit-criterion gap), whichever the user picks.
* Refresh `project_lvqr_status.md` memory.
* The post-Tier-4 follow-up list stays: WS / MoQ / WHEP SLO
  instrumentation (blocked on MoQ ingest-time propagation), hardware-
  encoder backends (NVENC / VAAPI / VideoToolbox / QSV), stream-
  modifying WASM filter pipelines, WHEP audio transcoder
  (AAC -> Opus), at least one public demo script under
  `examples/tier4-demos/`, Tier 5 client SDK work.
* Commit feat + docs as two commits. Do NOT push without a direct
  user instruction; if pushed, follow up with a `docs: session 109 A
  push event` commit that refreshes the HANDOFF status header to
  `origin/main synced (head <new>)` and updates the README Tier 4
  progress hook's "HLS-only" caveat.

## Post-Tier-4 follow-up candidates (prioritized)

Each maintainer prioritization should slot one of these into the
Session 109 entry point as the next session's focus.

| # | Candidate | Rough scope | Risk | Unblocks |
|---|---|---|---|---|
| 1 | **DASH SLO instrumentation** (THIS session, 109 A) | 1 session, ~100 LOC | low | dashboard coverage for DASH subscribers |
| 2 | MoQ frame-carried ingest-time propagation | 1-2 sessions, design-heavy | medium | WS / MoQ / WHEP SLO instrumentation |
| 3 | WS / MoQ / WHEP SLO instrumentation | 1 session, atop #2 | low | full SLO surface coverage |
| 4 | `examples/tier4-demos/` first demo script | 1 session, polish-heavy | low | Tier 4 exit criterion |
| 5 | Hardware encoder feature flags (NVENC / VideoToolbox / VAAPI / QSV) | 3-5 sessions per backend | high (platform-specific CI) | ABR ladder on GPU |
| 6 | Stream-modifying WASM filter pipelines | 2-3 sessions, contract rewrite | high | v1.1 marquee feature |
| 7 | WHEP audio transcoder (AAC -> Opus) | 2-3 sessions | medium | true WebRTC browser audio |
| 8 | Tier 5 client SDK (browser, Rust, Python) | ~20 sessions, product decisions | high | distribution story |

Work deliberately. Each commit should tell a future session exactly
what changed and why. Do not mark anything DONE until verification
passes.

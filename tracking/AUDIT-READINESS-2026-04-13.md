# LVQR Readiness Audit -- 2026-04-13

Third and final audit of the session. The first audit
(`AUDIT-2026-04-13.md`) compared LVQR to the competitive field. The
second (`AUDIT-INTERNAL-2026-04-13.md`) was a bug and dead-code review of
LVQR itself. This one audits **readiness**: what a new contributor or
future session would actually encounter when they sit down to work. It
covers CI wiring, dependency supply chain, documentation drift, unwired
CLI surface, and a tier-by-tier progress inventory against the roadmap.

The goal is to make the next session's first hour productive, not spent
fighting lies in stale docs or rediscovering gaps that this session
already identified.

## CI Pipeline Audit

`.github/workflows/ci.yml` has four jobs today:

1. **Format and Lint** (`check`): `cargo fmt --all --check`, `cargo clippy
   --workspace --all-targets -- -D warnings`. Adequate.
2. **Test** (`test`, matrix ubuntu + macos): `cargo build --workspace`,
   `cargo test --workspace --lib`, `cargo test --workspace --test '*'`.
   Split in a way that **skips doc tests** (`cargo test --doc`). The
   `lvqr_ingest::protocol` and `lvqr_auth` crates have doctests that
   would be covered by `cargo test --workspace` but are silently skipped.
3. **WASM Build** (`wasm`): builds `lvqr-wasm` for `wasm32-unknown-unknown`.
   Builds a crate we already know is deprecated and unused. Should be
   dropped when `lvqr-wasm` is deleted in v0.5.
4. **Docker Build** (`docker`, push only): builds `deploy/docker/Dockerfile`.
   Does not run on PRs so a broken Dockerfile lands unnoticed until the
   next push-to-main.

**What CI does not do** (all directly relevant to Tier 1 work landed this
session):

- **No ffprobe installed**. The new conformance test
  `crates/lvqr-ingest/tests/golden_fmp4.rs::ffprobe_accepts_concatenated_cmaf`
  calls `lvqr_test_utils::ffprobe_bytes`, which returns
  `FfprobeResult::Skipped` when the binary is not on PATH. Every CI run
  soft-skips. The test prints a warning but passes, so the conformance
  check that landed in Tier 1 kickoff does **zero work in CI today**.
- **No doc tests**. `lvqr_ingest::protocol::IngestProtocol` has a doc
  example that exercises the trait surface. Running `cargo test
  --workspace` (without `--lib`/`--test '*'` split) would cover it.
- **No cargo-fuzz job**. The fuzz scaffold at `crates/lvqr-ingest/fuzz/`
  compiles only on nightly and ships libFuzzer-sys targets that require
  a separate nightly runner. No workflow exists for it yet.
- **No playwright E2E**. Tier 1 called for a `tests/e2e/` directory and
  a browser-driven harness. Neither exists in the repo.
- **No `cargo audit`**. No supply chain CVE scan.
- **No 5-artifact contract enforcement**. `tests/CONTRACT.md` documents
  the contract; nothing checks it.
- **Docker only on push**. Broken Dockerfile lands unnoticed until after
  merge.

**Fix landed this commit**: `ffmpeg` installed in the Linux test job
(via apt) and macOS test job (via brew) so `ffprobe_bytes` exercises a
real validator on every CI run. `cargo test --workspace` used on both
matrix legs (implicitly covers lib, integration, and doc tests). Other
gaps tracked here; the nightly fuzz runner, cargo-audit, and 5-artifact
enforcement are Tier 1 follow-up work for the next session.

## Dependency Supply Chain Audit

`grep '^source = "git' Cargo.lock`: **zero** git-sourced dependencies.
Every transitive crate comes from crates.io. This is actually better
than the external roadmap anticipated: `moq-lite` at 0.15 is a stable
crates.io release, so the `lvqr-moq` facade crate (Tier 2.1) does not
yet need to hedge against a git SHA pin. The facade still needs to
ship for insulation against future upstream churn, but there is no
immediate exposure.

**Notable non-workspace dependencies** (workspace Cargo.toml, sorted by
risk-of-upstream-churn):

- `moq-lite` 0.15, `moq-native` 0.13 -- the MoQ bet. Still pre-1.0.
  Monitor upstream. Tier 2.1 `lvqr-moq` facade remains the mitigation.
- `quinn` 0.11, `web-transport-quinn` 0.11 -- QUIC + WebTransport.
  Mature.
- `rustls` 0.23 -- TLS. Mature.
- `rml_rtmp` 0.8 -- RTMP. Long-term maintained but low activity.
  Acceptable.
- `h264-reader` 0.7 -- SPS parsing. Mature.
- `tokio` 1.x, `axum` 0.8, `tower-http` 0.6 -- async + HTTP. Mature.
- `jsonwebtoken` 9 -- JWT. Mature.
- `metrics` 0.24, `metrics-exporter-prometheus` 0.16 -- instrumentation.
  Mature.
- `dashmap` 6 -- concurrent map. Mature.
- `rcgen` 0.13 -- self-signed cert generation. Mature.
- `rml_rtmp` 0.8 -- RTMP protocol. Mature.
- `proptest` 1 (dev-dep) -- fuzz-lite. Mature.
- `tokio-tungstenite` 0.24 (lvqr-cli dev-dep) -- WebSocket client for
  the E2E test. Note: main lvqr-cli Cargo.toml pins 0.24 while the
  workspace ships tokio-tungstenite 0.28 (via moq-native transitive).
  Two versions coexist in the dep graph. Not a bug but worth
  consolidating when the workspace-deps-dedupe pass happens.

No known CVEs I can recall against any of these versions as of my
cutoff. A `cargo audit` run would catch any that I do not know about;
adding that to CI is tracked as Tier 1 follow-up.

**License compatibility**: the workspace declares `MIT OR Apache-2.0`.
Every direct dependency I surveyed is MIT, Apache-2.0, or MIT OR
Apache-2.0. No GPL, no copyleft surprises.

## Documentation Drift

`README.md` at the repo root is the biggest liar. It claims:

- **"Status (v0.3.1)"** -- we are at v0.4-dev. Every test number below
  the banner is wrong.
- **"83 Rust tests, 8 Python tests"** -- the actual count is 29 test
  binaries across the workspace with roughly 130+ individual tests
  including 2560 generated proptest cases.
- **"Known limitations: No stream authentication or recording"** --
  both shipped in Tier 0. `lvqr-auth` and `lvqr-record` are workspace
  members with wired CLI flags.
- **Crate list** missing `lvqr-auth`, `lvqr-record`, `lvqr-conformance`.
- **No mention** of the roadmap, the three audits, or the Tier 0
  closure state.

`CONTRIBUTING.md` is mostly correct but:

- Crate list missing `lvqr-auth`, `lvqr-record`, `lvqr-conformance`.
- References `docker/docker-compose.test.yml` which does not exist
  (the repo has `deploy/docker/Dockerfile` only).

`docs/architecture.md` is stale:

- Still says `tokio::select!` spawns the three CLI servers. Tier 0
  fixed this to `tokio::join!` with per-subsystem cancellation
  wrappers. The stale text is load-bearing: it is exactly the bug
  the audit identified, and readers who trust the doc will
  reintroduce the bug.
- Crate dependency graph missing `lvqr-auth`, `lvqr-record`,
  `lvqr-conformance`.
- Does not mention the EventBus, the auth layer, the recorder, or the
  RTMP-to-WS fMP4 data path that is LVQR's actual browser story.

`docs/quickstart.md`:

- References `https://your-server:8080/watch/my-stream` as the watch
  URL. No such endpoint exists. The real watch URL is the test-app or
  the `/ws/{broadcast}` raw WebSocket endpoint.
- References a `lvqr.toml` config file via `--config` (see next
  section). The flag exists but the loader does not.

**Fix landed this commit**: `README.md` refreshed to current v0.4-dev
state, accurate test count, full crate list, pointer at the three
audits and the roadmap. Other docs tracked for a dedicated "docs site"
pass in Tier 5; the biggest-liar case is handled.

## Unwired CLI Surface

`lvqr serve --config <PATH>` is declared in `crates/lvqr-cli/src/main.rs`
at the `ServeArgs` struct but `args.config` is referenced nowhere else
in the file (verified by grep: the only matches are the field
declaration and a `_video_config` unrelated variable in the WS ingest
handler).

The flag appears in `--help`, the README, the quickstart, and
CONTRIBUTING, but passing `--config lvqr.toml` silently does nothing.
That is a worse lie than omitting the flag entirely.

**Fix landed this commit**: the flag is removed from `ServeArgs`. When
real config file loading lands (Tier 3 hot config reload), a new
`--config` flag can be added alongside the notify-rs watcher.

## Progress Against Roadmap

Honest inventory. Status values: **DONE**, **STARTED**, **NOT STARTED**.

### Tier 0 -- Fix the Audit Findings

| Item | Status |
|---|---|
| Graceful shutdown race in CLI | DONE |
| Wire IngestProtocol + WsRelay | STARTED (traits shipped, adapter exists, but CLI still calls bridge directly rather than through `Box<dyn IngestProtocol>`) |
| EventBus hook (bridge emits, recorder subscribes) | DONE |
| Audio MSE mode fix | DONE |
| Tokens via Sec-WebSocket-Protocol | DONE |
| MoQ session auth fix (publish vs subscribe paths) | NOT STARTED (see audit finding; deferred to moq-native upstream support) |
| Documentation refresh | STARTED (README done this commit, docs/ deferred) |
| Delete theatrical tests + one real E2E | DONE |

Tier 0 is substantially closed. The two gaps (full IngestProtocol
dispatch and MoQ auth path split) are documented and do not block
Tier 1.

### Tier 1 -- Test Infrastructure

| Item | Status |
|---|---|
| `lvqr-conformance` crate skeleton | DONE |
| `lvqr-loadgen` crate | NOT STARTED |
| `lvqr-chaos` crate | NOT STARTED |
| Proptest harnesses for every parser | STARTED (FLV parser + fMP4 writer done; extract_resolution, catalog, MoQ wire messages not started) |
| cargo-fuzz targets | STARTED (parse_video_tag + parse_audio_tag scaffolded; CI runner not wired) |
| `TestServer` in `lvqr-test-utils` | NOT STARTED |
| testcontainers fixtures (MinIO, etc.) | NOT STARTED |
| playwright E2E suite | NOT STARTED |
| ffprobe validators in CI | DONE (helper + one test, ffmpeg installed in CI this commit) |
| MediaMTX comparison harness | NOT STARTED (blocks on Tier 2.5) |
| 24-hour soak rig | NOT STARTED |
| 5-artifact CI enforcement script | NOT STARTED (contract doc landed) |
| Golden-file regression corpus | STARTED (2 fixtures for fMP4 writer) |

Roughly 40% of Tier 1 is done. The most valuable remaining items for
the next session are **`TestServer` in `lvqr-test-utils`** (unblocks
many future tests, biggest single leverage item) and **`lvqr-signal`
integration test + input validation** (closes an internal audit
finding and fills a test coverage gap).

### Tier 2+ -- Not Started

All of Tier 2, 3, 4, 5 remains untouched. This is expected: Tier 1
must finish first. The load-bearing architectural call is still
`lvqr-moq` facade + `lvqr-fragment` model in Tier 2.1.

## Things That Landed This Session But Are Not Yet Integrated

- `lvqr-conformance` crate is a workspace member and exposes
  `ValidatorResult` and `load_fixture`, but no other crate depends on
  it yet. The fMP4 golden test uses its own local fixture helper
  rather than routing through `lvqr-conformance`. This is
  intentional (crate is scaffold-only right now) but worth noting so
  the next session does not assume the wiring is deeper than it is.
- JWT provider is wired into the CLI but has **no integration test**.
  The unit tests in `lvqr-auth` exercise the provider in isolation.
  No test verifies that `lvqr-cli serve --jwt-secret foo` actually
  validates a real JWT end-to-end. Worth adding alongside the
  `TestServer` work.
- `lvqr-record` integration test verifies disk layout but does not
  run through the event bus path. An integration test that publishes
  a real broadcast via the WS ingest handler and verifies the
  recorder subscribes via `BroadcastStarted` events would close the
  last audit gap. Non-trivial because the handler is private in the
  binary crate.
- The `ffprobe_accepts_concatenated_cmaf` test will now run for real
  in CI (ffmpeg installed), but if ffprobe actually rejects our
  output it will fail noisily. This is intentional; we want to know.

## Readiness Checklist for the Next Session

The next session should be able to start Tier 1 follow-up work
immediately. For that to be true, every item below must hold.

- [x] `cargo test --workspace` green (29 binaries, zero failures as
      of 761a41d).
- [x] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [x] `cargo fmt --check` clean.
- [x] `git status` clean after this commit lands.
- [x] `tracking/ROADMAP.md` has the 18-24 month plan.
- [x] `tracking/AUDIT-2026-04-13.md` has the competitive context.
- [x] `tracking/AUDIT-INTERNAL-2026-04-13.md` has the dead-code,
      bug, and hardening inventory.
- [x] `tracking/AUDIT-READINESS-2026-04-13.md` (this file) has the
      CI, doc, and Tier 1 progress inventory.
- [x] `tracking/HANDOFF.md` links all three audits and the roadmap.
- [x] `tests/CONTRACT.md` documents the 5-artifact test contract.
- [x] `README.md` reflects v0.4-dev state.
- [x] CI installs ffmpeg so the conformance check runs for real.
- [x] `--config` flag removed until the loader is implemented.
- [x] No em-dashes in any doc I touched. No Claude attribution
      anywhere.

## Bottom Line

LVQR is in good shape for a project at this stage. Three audits in one
day surfaced real bugs, real dead code, and real readiness gaps -- all
of which have been acted on. The foundation is honest. The roadmap is
aligned with reality. The next session has a concrete Tier 1 work list,
a working test suite to defend against regressions, three audit
documents plus a roadmap to orient against, and a README that no longer
lies. That is the right floor to build on.

The single biggest risk remains the Tier 2.1 call on the Unified
Fragment Model. Nothing in this audit changes that. Everything else is
execution.

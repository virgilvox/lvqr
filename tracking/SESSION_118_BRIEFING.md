# Session 118 briefing -- phase-C row 117

Authored at the start of session 118 (head `f9ece25`) to lock
scope on `PLAN_V1.1.md` row 117 before opening source files.
Row 117 has two non-trivial design decisions (test-overlap with
`rtmp_archive_e2e.rs`; DASH-IF validator tooling choice) that
warrant a written briefing per the plan's "How to kick off the
next session" convention.

## Context

Phase B v1.1 fully SHIPPED and pushed at `f9ece25` (session 117
close). Local `main` = `origin/main`. 941 workspace tests on the
default gate, 29 crates, Tier 4 COMPLETE (exit criterion closed
by `examples/tier4-demos/demo-01.sh`). Row 117 in
`PLAN_V1.1.md`:

> 117 | Archive READ DVR E2E + DASH-IF conformance validator in CI.

Two separable deliverables:

1. `crates/lvqr-cli/tests/archive_dvr_read_e2e.rs` -- new
   Rust integration test exercising the `/playback/*` scrub
   routes.
2. `.github/workflows/dash-conformance.yml` -- new CI workflow
   validating the live DASH egress surface against an external
   reference tool.

## Finding #1: "the read side has zero E2E" is stale

The session 117 entry-point block claims the `/playback/*`
read surface has zero E2E coverage. In practice
`crates/lvqr-cli/tests/rtmp_archive_e2e.rs` already covers
(verified 2026-04-22 on head `f9ece25`):

* `GET /playback/{broadcast}` happy-path + status + JSON shape
* `GET /playback/{broadcast}?from=X&to=Y` future-window empty
* `GET /playback/{broadcast}` unknown-broadcast empty array
* `GET /playback/latest/{broadcast}` status + JSON shape
* `GET /playback/latest/{broadcast}` unknown-broadcast 404
* `GET /playback/file/{rel}` bytes + `moof` prefix
* `GET /playback/file/{rel}` missing 404
* `GET /playback/file/{rel}` path-traversal rejection (400 or 404)
* Subscribe-auth gate on every route via both
  `Authorization: Bearer` header and `?token=` query

What remains genuinely uncovered:

* **Multi-keyframe scrub window arithmetic.** The existing
  test publishes only two keyframes, both at low DTS. There is
  no scrub scenario where `from` / `to` windows select a real
  subset of segments out of a multi-segment stream.
* **Live-DVR scrub.** Every existing assertion runs after
  `publish_two_keyframes` returns, so redb is quiescent when
  the HTTP scan runs. An operator scrubbing a DVR of a still-
  active broadcast is the actual production scenario; the
  existing test does not prove that the reader does not race
  the writer's exclusive redb lock.
* **Content-Type assertions.** Every handler hard-codes
  `application/json` or `application/octet-stream`, but
  nothing in the test suite verifies the header, so a drop-in
  swap that returned plain text would pass CI.

## Finding #2: Scope-shape for the archive read test

`crates/lvqr-cli/tests/archive_dvr_read_e2e.rs` ships three
new `#[tokio::test]` functions, each targeting an uncovered
scenario. Every helper (RTMP handshake, FLV tag builders, raw
HTTP client) is copy-paste from `rtmp_archive_e2e.rs`; shared
helper extraction into `lvqr-test-utils` is scope creep (the
pattern is duplicated across six tests already and has worked
fine, the refactor is a separate hygiene session).

### Test 1: `playback_scrub_window_arithmetic`

1. Publish five keyframes at RTMP timestamps 0, 2000, 4000,
   6000, 8000 ms. RTMP timescale is 1000; the bridge maps
   into the CMAF 90 kHz timescale, so segment `start_dts`
   spans [0, 90_000 * (kf_ts_ms / 1000)).
2. Wait 500 ms for every segment to land in redb.
3. Assert `/playback/live/dvr?from=0&to=u64::MAX` returns all
   rows; record the row count N.
4. Assert `/playback/live/dvr?from=0&to=<midpoint>` returns
   strictly fewer than N rows and every row's `end_dts <=
   midpoint` OR `start_dts < midpoint`.
5. Assert `/playback/live/dvr?from=<midpoint>&to=u64::MAX`
   returns the complement: row count + midpoint-window count
   sums to the full-window count (no overlap, no missing).
6. Assert `/playback/latest/live/dvr`'s `start_dts` matches
   the last row of the full-window response.

### Test 2: `live_dvr_scrub_while_publisher_is_active`

1. Spawn an RTMP publisher that writes a keyframe every 500 ms
   for 3 s, without closing.
2. After ~1 s, hit `/playback/live/dvr` and assert the scan
   succeeds (200 + non-empty array). Proves the reader does
   not block on the writer's redb lock.
3. Keep the publisher running; wait another 1 s; hit
   `/playback/latest/live/dvr`; assert its `start_dts` is
   greater than the initial scrub's last row.
4. Stop the publisher and assert no mid-test panic fired.

Uses `tokio::spawn` to run the publisher, matching the session
115 `rtmp_whep_audio_e2e.rs` publisher-as-background-task
pattern. The publisher must NOT hold the test task's only
thread or the HTTP read cannot interleave; `flavor =
"multi_thread", worker_threads = 2` on the test attribute.

### Test 3: `playback_routes_emit_expected_content_types`

1. Publish two keyframes via the existing helper.
2. Assert `/playback/live/dvr` response `Content-Type`
   contains `application/json`.
3. Assert `/playback/latest/live/dvr` response `Content-Type`
   contains `application/json`.
4. Assert `/playback/file/live/dvr/0.mp4/<seq>.m4s` response
   `Content-Type` contains `application/octet-stream`.

The raw TCP HTTP client the existing test uses already parses
headers; extend the `HttpResponse` struct with a `headers:
Vec<(String, String)>` field so the new test can assert on
them.

### Scope guard

NO new assertions on segment payload fMP4 correctness (that is
`lvqr-cmaf` / `lvqr-record`'s contract, exercised by the
conformance fixtures). NO range-request (`Range: bytes=`)
tests -- the file handler does not implement them today and
that is a documented gap, not a regression.

## Finding #3: DASH-IF validator tooling choice

`.github/workflows/hls-conformance.yml` is the load-bearing
precedent. Its design: prefer the authoritative validator
(`mediastreamvalidator`), soft-skip when the runner image does
not ship it, and always run an `ffmpeg` client-pull as a
second (weaker) signal so the artifact never comes back empty.

The DASH validator options:

| Tool | Install cost | CLI-friendly | Authoritative |
|---|---|---|---|
| DASH-IF Conformance Tool (official) | Heavy: Docker image `dashif/conformance`; web UI, has a REST API | Marginal -- needs an HTTP POST dance against the running container | Yes |
| GPAC `MP4Box -dash-check` | `apt install gpac` on ubuntu-latest, ~40 MB | Yes | Partial (structural, not full DASH-IF rules) |
| `ffmpeg -i <mpd>` as client | Already installed for the HLS workflow pattern | Yes | No (weaker structural check only) |
| Shaka packager `packager --validate` | Static binary download | Yes | Partial |

**Decision**: ship `dash-conformance.yml` with GPAC MP4Box as
the primary validator + ffmpeg as the fallback. Defer the
authoritative DASH-IF container to a follow-up row because its
REST API does not match the one-shot validator shape every
other workflow uses and wiring it robustly is a day of work
on its own. This matches the HLS workflow's "prefer the
authoritative tool, fall back to ffmpeg" posture exactly, with
MP4Box in the "authoritative" slot for the first iteration.

The workflow initially carries `continue-on-error: true` to
match `hls-conformance.yml`'s early-days posture; promotion to
a required check waits until we see a clean run on `main`.

### Workflow shape

```yaml
name: MPEG-DASH Conformance

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  dash-validator:
    runs-on: ubuntu-latest
    timeout-minutes: 30
    continue-on-error: true
    steps:
      - checkout + rust-toolchain + rust-cache
      - apt install ffmpeg gpac
      - cargo build -p lvqr-cli --release
      - start lvqr serve with --dash-port 8889 in background
      - ffmpeg push synthetic RTMP for 20 s
      - sleep 3 for segment finalize
      - curl the MPD + one init.mp4 + one media segment
      - MP4Box -dash-check <mpd-url> (primary)
      - ffmpeg -i <mpd-url> -c copy -t 5 out.mp4 (fallback)
      - ffprobe the fallback output
      - kill lvqr
      - upload-artifact with the dash-out directory
```

No surprises relative to hls-conformance.yml. Every difference
is deliberate:

* **Runner**: `ubuntu-latest` instead of `macos-latest`
  because the DASH validator stack is cross-platform and
  `gpac` is an apt package on Ubuntu. HLS validator must be
  macOS because `mediastreamvalidator` ships only with Apple
  HTTP Live Streaming Tools; DASH has no such constraint.
* **Port**: `--dash-port 8889` to keep HLS (`--hls-port 8888`)
  and DASH distinct. Matches the quickstart table in README.
* **Runtime**: `continue-on-error: true` initially.

## Read first, in this order

Regardless of deliverable:

1. `CLAUDE.md` -- absolute rules (no Claude attribution, no
   emojis, no em-dashes, 120-col max, real ingest and egress
   in tests, edit in-repo, no push without instruction).
2. `tracking/HANDOFF.md` -- status header + session 117 close
   block.
3. `tracking/PLAN_V1.1.md` -- row 117 scope line.

For the **archive test**:

4. `crates/lvqr-cli/src/archive.rs` -- route handlers +
   `PlaybackSegment` JSON shape.
5. `crates/lvqr-cli/tests/rtmp_archive_e2e.rs` -- existing
   E2E; copy helpers verbatim.
6. `crates/lvqr-archive/src/index.rs` -- redb layout + the
   `find_range` / `latest` contract the HTTP routes expose.

For the **DASH workflow**:

4. `.github/workflows/hls-conformance.yml` -- precedent.
5. `crates/lvqr-dash/src/` -- MPD shape + segment URI
   layout so the workflow knows what paths to curl.
6. `README.md` "Egress" section -- DASH is on when
   `--dash-port <PORT>` is non-zero; port 8889 is the
   documented default.

## Verification gates

* `cargo fmt --all --check` clean.
* `cargo clippy --workspace --all-targets -- -D warnings` clean.
* `cargo test --workspace` default gate >= 944 / 0 / 1
  (941 baseline + three new tests in `archive_dvr_read_e2e.rs`).
* `cargo test -p lvqr-cli --test archive_dvr_read_e2e`
  passes on macOS dev host (no feature flags, no external
  tooling required).
* `dash-conformance.yml` runs green (or soft-skip green) on a
  GitHub-hosted runner. Not verifiable locally without `act`;
  CI is the authoritative signal.

## After session 118

Write a "Session 118 close" block at the top of
`tracking/HANDOFF.md` immediately after the status header.
Mark `tracking/PLAN_V1.1.md` row 117 SHIPPED with the design
decisions rolled into the row summary. Update the
`project_lvqr_status` auto-memory. Commit as a feat commit
(test + workflow together, or split into two feats if the
diff is easier to review that way) + a close-doc commit. Push
only on user instruction; if pushed, re-verify
`git log --oneline origin/main..main` first so the chain rides
as one batch.

Follow-up rows candidate for phase C after 117:
* Authoritative DASH-IF Conformance Tool (containerized)
  wired as a second workflow step.
* CLI C2PA wiring (`--c2pa-signing-cert` etc.) -- carried
  over from session 117's Known Limitations entry.
* CI coverage for `examples/tier4-demos/demo-01.sh` -- carried
  over from session 117's follow-up list.

## Absolute rules (copied from `CLAUDE.md`)

* Never add Claude as author, co-author, or contributor in git
  commits, files, or any other attribution (no
  `Co-Authored-By` trailers).
* No emojis in code, commit messages, or documentation.
* No em-dashes in prose.
* 120-column max in Rust.
* Real ingest and egress in integration tests (no
  `tower::ServiceExt::oneshot` shortcuts, no mocked sockets).
* Only edit files within this repository.
* Do NOT push to origin without a direct user instruction.
* Plan-vs-code refresh on any design deviation from
  `PLAN_V1.1.md`.
* Never skip git hooks (no `--no-verify`, no `--no-gpg-sign`).
* Never force-push main.

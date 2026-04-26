# Session 155 Briefing -- Session 154 test-coverage close-out (live RTMP marker e2e)

**Date kick-off**: 2026-04-26 (locked the morning after session 154's
push; same-week close).
**Predecessor**: Session 154 (`@lvqr/dvr-player` v0.3.3 -- SCTE-35
ad-break marker rendering on the seek bar; pure consumer of session
152's `#EXT-X-DATERANGE` wire; 28 Vitest unit + 3 Playwright e2e
in `markers.spec.ts`; new `bindings/js/tests/helpers/rtmp-push.ts`
ffmpeg wrapper). Default-gate tests at workspace lib **1111 / 0 / 0**;
admin surface at **12 route trees**; origin/main head `f849c84`. SDK
packages: `@lvqr/core 0.3.2`, `@lvqr/player 0.3.2`,
`@lvqr/dvr-player 0.3.3`; workspace `0.4.1` unchanged. The session
154 HANDOFF "Pending follow-ups" enumerates four loose ends
(README + npm publish are deferred to a release session); the
remaining three are test-coverage gaps this session closes
together.

## Goal

Session 154 shipped the marker-rendering feature with three new
Playwright tests but two of them are routed-stub fixtures and the
third (the live-RTMP test) only asserts that the relay accepts the
ffmpeg publish + serves a master playlist. Three follow-ups remain
open in the session 154 HANDOFF:

1. **CI integration of the live-RTMP path.** The `mesh-e2e.yml`
   workflow does not install ffmpeg today; the live-RTMP test
   `test.skip()`s on every CI run. After this session the workflow
   `apt-get install`s ffmpeg, sets `LVQR_LIVE_RTMP_TESTS=1` for the
   Playwright run, and the live-RTMP test exercises the helper
   end-to-end on every push.
2. **Stronger consumer-side LIVE-pill assertion.** The current
   live-RTMP test asserts the relay accepts the publish and serves
   a master with `#EXT-X-STREAM-INF`; the originally-planned LIVE
   pill `is-live` flip hit a `manifestLoadError` race against
   hls.js's first variant fetch on the dev box. After this session
   the test waits for a non-empty variant playlist BEFORE setting
   `src` on the dvr-player, and asserts the LIVE pill reaches
   `is-live` within 30 s of the publish.
3. **Real RTMP `onCuePoint` -> `#EXT-X-DATERANGE` -> marker render
   Playwright e2e.** Session 154 explicitly descoped this on the
   "no Rust crate touched besides the new bin" anti-scope, then
   descoped the bin itself because the vendored `rml_rtmp` v0.8
   client lacks a generic AMF0-data sender (its only AMF0 surface,
   `publish_metadata`, hard-codes `@setDataFrame` + `onMetaData`).
   Session 155 explicitly authorizes patching the vendored fork
   (symmetric to session 152's server-side `Amf0DataReceived`
   addition: that session patched the receive side, this one
   patches the publish side). After this session a new
   `[[bin]] scte35-rtmp-push` on `lvqr-test-utils` opens a real
   RTMP publisher session, sends a few seconds of synthetic H.264
   plus one `onCuePoint scte35-bin64` AMF0 Data message, and a new
   gated Playwright test drives the bin end-to-end into the
   dvr-player webServer profile + asserts the marker tick + span
   render at the expected fractions.

The wire is unchanged. SDK package versions are unchanged.
Workspace stays at v0.4.1. The `@lvqr/dvr-player` package stays at
v0.3.3 with no source change to its production code path -- this
session is a test + CI + tooling close-out of session 154's feature.

## Decisions (locked at brief read-back, 2026-04-26)

The seven decisions below are drafted; the read-back fixes them.
The largest design lever is decision 3 -- the rml_rtmp client patch
shape -- because it re-opens the vendored fork that session 152
already touched. The patch surface is small (one method, mirroring
`publish_metadata`'s shape minus the `@setDataFrame` + `onMetaData`
hardcoding) and the precedent is in-tree; the read-back locks the
exact method name and signature.

### 1. CI workflow shape (`mesh-e2e.yml`)

Two-line addition to install ffmpeg + one new env block to gate the
opt-in live-RTMP test on:

```yaml
- name: Install ffmpeg
  run: sudo apt-get update && sudo apt-get install -y ffmpeg

- name: Run mesh Playwright tests
  working-directory: bindings/js
  env:
    LVQR_LIVE_RTMP_TESTS: "1"
  run: npx playwright test
```

Rejected alternatives:

* `actions/setup-ffmpeg@v3`. Third-party action; resolves to a
  prebuilt binary that occasionally drifts. The workspace already
  has `apt-get install ffmpeg` precedent in its CI cluster (the
  captions tests' workflow uses the same pattern); reuse the
  pattern rather than introduce a new action.
* Step-level `env:` (only on the Run step). Workflow-level on the
  job is fine and lets either Playwright project see the env var,
  matching the spec's `process.env.LVQR_LIVE_RTMP_TESTS !== '1'`
  gate; mesh tests don't read the var so the broader scope is
  harmless.
* Conditional install (`if: hashFiles(...)` or path-filter-driven).
  ffmpeg adds ~30 s to a workflow that already takes minutes;
  unconditional install is simpler, cache-friendly across runs of
  the same workflow, and removes a class of "live test silently
  skipped" failure mode.

The `cargo build -p lvqr-cli` step grows to
`cargo build -p lvqr-cli -p lvqr-test-utils --bins` so the new
`target/debug/scte35-rtmp-push` is on disk by the time Playwright
spawns it. The path filters under `on.pull_request.paths` and
`on.push.paths` add `vendor/rml_rtmp/**` and
`crates/lvqr-test-utils/**` so a vendor or test-utils touch
retriggers the workflow.

The workflow header comment grows two lines explaining the env
gate's purpose (opt-in live RTMP path, ffmpeg-driven publish + bin-
driven `onCuePoint` injection).

**Locked: workflow-level `env: LVQR_LIVE_RTMP_TESTS: "1"`,
unconditional `apt-get install -y ffmpeg`, build step extended to
include `-p lvqr-test-utils --bins`, path filters extended to cover
`vendor/rml_rtmp/**` + `crates/lvqr-test-utils/**`.**

### 2. LIVE-pill wait strategy

Option (b) per the kickoff prompt: variant-playlist-non-empty
pre-check before setting `src` on the dvr-player. The test loop
becomes:

1. ffmpeg starts publishing via the existing `rtmpPush` helper.
2. Node-side poll fetches `master.m3u8` until it contains
   `#EXT-X-STREAM-INF` and a variant URI (existing logic). Budget
   30 s.
3. **NEW**: Node-side poll fetches the variant playlist URI from
   the master and waits until it contains at least 2 `#EXTINF`
   entries (so hls.js's first variant fetch finds segments rather
   than the brief empty-playlist window). Budget 60 s (CI runners
   are slower than dev boxes; the dev-box race window was sub-
   second but CI's rust-cache warm-up + ffmpeg startup add tail
   latency).
4. Test sets `src` on the dvr-player. The component immediately
   loads the master + first variant + first segment.
5. **NEW**: Wait for `lvqr-dvr-live-edge-changed` with
   `isLiveEdge: true`. Budget 30 s. The LIVE pill text reads "LIVE"
   when `isLiveEdge` is true and the host is at the live edge.
   Existing public event from session 153 (debounced 250 ms; fires
   on threshold crossing of `seekable.end - currentTime` against
   `live-edge-threshold-secs` default 6 s).

Total wait budget: ~120 s for the live-pill assertion path, vs the
current ~30 s for the master-only assertion. Fits within the
default Playwright 30-second per-test timeout if we bump it on the
new test specifically; the live tests are wrapped in a
`test.setTimeout(180_000)` for headroom.

Rejected alternatives:

* (a) Tune hls.js retry config (`manifestLoadingMaxRetry`,
  `levelLoadingMaxRetry`). Overrides hls.js defaults from session
  153's component bootstrap, which has knock-on effects for any
  consumer who tunes their own retry budgets via `getHlsInstance()`.
  Pre-check is cleaner because it does not touch component code
  at all.
* Increase `--hls-dvr-window-secs` from 300 (session 153's default)
  so segments accumulate faster. The window is a duration cap, not
  a fill rate; this does not fix the variant-fetch race.
* Wait fixed seconds (`page.waitForTimeout(20_000)`) before setting
  `src`. Anti-pattern; the new pre-check waits for the actual
  precondition.

A small `waitForLiveVariantPlaylist(masterUrl, options)` helper
factors into `bindings/js/tests/helpers/hls-poll.ts` (NEW) so the
"poll master, follow first variant URI, wait for >=2 EXTINF" loop
is reusable across future live-RTMP tests. Pure Node-side `fetch`
+ regex; no browser context.

**Locked: variant-playlist-non-empty pre-check (option b);
30/60/30-second budgets; new `waitForLiveVariantPlaylist` helper
in `bindings/js/tests/helpers/hls-poll.ts`; `test.setTimeout(180_000)`
on the live-RTMP describe block.**

### 3. rml_rtmp client patch shape (`publish_amf0_data`)

The patch mirrors session 152's server-side `Amf0DataReceived`
addition in shape and size. New top-level method on `ClientSession`
in `vendor/rml_rtmp/src/sessions/client/mod.rs`:

```rust
/// If publishing, this allows us to send arbitrary AMF0 data
/// messages on the publishing stream. Unlike `publish_metadata`,
/// which hard-codes `@setDataFrame` + `onMetaData`, this method
/// emits the supplied `Amf0Value` vector verbatim. Use cases
/// include `onCuePoint` / `onTextData` / `onFI` and other AMF0
/// data carriages that flash-era publishers adopted for in-band
/// signalling.
///
/// The caller owns the wire shape: typically the first value is
/// a `Utf8String` carrying the method name (e.g. "onCuePoint")
/// and the second is an `Object` carrying the payload.
pub fn publish_amf0_data(
    &mut self,
    values: Vec<Amf0Value>,
) -> Result<ClientSessionResult, ClientSessionError> {
    match self.current_state {
        ClientState::Publishing => (),
        _ => {
            return Err(ClientSessionError::SessionInInvalidState {
                current_state: self.current_state.clone(),
            });
        }
    }
    let active_stream_id = match self.active_stream_id {
        Some(x) => x,
        None => return Err(ClientSessionError::NoKnownActiveStreamIdWhenRequired),
    };
    let message = RtmpMessage::Amf0Data { values };
    let payload = message.into_message_payload(self.get_epoch(), active_stream_id)?;
    let packet = self.serializer.serialize(&payload, false, false)?;
    Ok(ClientSessionResult::OutboundResponse(packet))
}
```

Signature notes:

* Returns `ClientSessionResult` (single, not `Vec<...>`) -- mirrors
  `publish_metadata` exactly. Caller wraps in a single-element vec
  if it wants to feed it through the existing `send_results`
  helper from `lvqr_test_utils::rtmp`, or feeds it through
  `send_result` directly.
* Takes `Vec<Amf0Value>` by value -- mirrors `publish_metadata`'s
  internal `RtmpMessage::Amf0Data { values: vec![...] }`
  construction.
* No convenience wrapper inside `rml_rtmp` for the SCTE-35 shape.
  `rml_rtmp` is a low-level RTMP crate; SCTE-35 is a payload
  carriage convention layered on top. The bin (LVQR-side, not
  vendored) builds the AMF0 onCuePoint shape via a small helper
  in `lvqr-test-utils`. A `publish_oncuepoint_scte35` method
  inside `rml_rtmp` would mix concerns and make future sync with
  upstream harder.

Patch size estimate: ~25 lines (the method body) + ~50 lines of
test (one new `#[test]` in
`vendor/rml_rtmp/src/sessions/client/tests.rs` that drives a full
publish setup, calls `publish_amf0_data` with a known
`Vec<Amf0Value>`, deserializes the produced packet via the existing
`split_results` helper, and asserts the `RtmpMessage::Amf0Data`
arm round-trips with the same values). Total fork delta from
session 152's 25-line patch + this session's ~75-line patch =
~100 lines.

The patch lands BEFORE the bin lands so the bin can call the
method straight away. The vendored fork's `Cargo.toml` does not
need to change (the new method is additive on a public type and
is reachable via the existing `pub use ClientSession` re-export).

Rejected alternatives:

* `send_amf0_data` -- ambiguous naming (sounds like a generic
  send, doesn't make the publish-state precondition clear).
* `publish_data` -- too vague (publish_metadata is also "publish
  data").
* `publish_oncuepoint_scte35` convenience wrapper inside
  `rml_rtmp`. Mixing layers; the bin owns the SCTE-35 wire shape
  not the RTMP crate.
* Generic `(name: &str, payload: &[u8])` higher-level helper. Loses
  AMF0 type fidelity; the relay's parser at
  `crates/lvqr-ingest/src/rtmp.rs:470` reads structured AMF0 fields
  off an `Object` (`name`, `data`, `time`, `type`) so the publisher
  needs to construct an `Amf0Value::Object` not pass raw bytes.

**Locked: `publish_amf0_data(&mut self, values: Vec<Amf0Value>) -> Result<ClientSessionResult, ClientSessionError>`,
no convenience wrapper inside `rml_rtmp`.**

### 4. `scte35-rtmp-push` bin CLI

New `[[bin]]` declaration in `crates/lvqr-test-utils/Cargo.toml`:

```toml
[[bin]]
name = "scte35-rtmp-push"
path = "src/bin/scte35_rtmp_push.rs"
```

CLI shape (clap derive, mirroring the workspace's existing CLI
style):

```
scte35-rtmp-push \
    --rtmp-url rtmp://127.0.0.1:11936/live/dvr-test \
    --duration-secs 8 \
    --inject-at-secs 3.0 \
    [--scte35-hex 0xFC301100...]
```

Flag table:

| flag | type | default | semantics |
| --- | --- | --- | --- |
| `--rtmp-url` | `String` | (required) | full RTMP URL incl. app + stream key |
| `--duration-secs` | `f64` | `8.0` | total publish runtime (after first IDR) |
| `--inject-at-secs` | comma-sep `Vec<f64>` | `3.0` | offsets at which to send `onCuePoint` |
| `--scte35-hex` | `String` | (auto) | hex-encoded splice_info_section |
| `--video-size` | `String` | `320x180` | reserved (synthetic NAL ignores) |
| `--video-fps` | `u32` | `30` | frames-per-second |
| `--keyframe-interval-frames` | `u32` | `60` | GOP cadence (60 = 2 s @ 30 fps) |

Default `--scte35-hex`: when omitted, the bin uses
`build_splice_insert_section(event_id=0xCAFEBABE, pts=8_100_000, duration_90k=2_700_000)`
(the same fixture as `scte35_hls_dash_e2e.rs`). Smoke runs without
arguments work out of the box.

Multi-injection: `--inject-at-secs 2.5,5.0,7.5` sends three
`onCuePoint` messages over the same publish. Each uses the same
`--scte35-hex` payload but with a fresh `event_id` (incremented
per emission so the relay-side daterange ID is unique; the bin
patches the section's event_id field in-place per send + recomputes
CRC). The smoke and the e2e specs both stick to single-emission for
v1 to keep assertions tractable; multi-emission is supported for
future ad-pod tests.

Exit semantics:

* `0` on clean publish-end + disconnect.
* Non-zero on RTMP wire error (handshake failure, publish-rejected,
  socket reset). The Playwright spec sees a hard failure rather
  than a silent success.

stderr log lines (the `tracing` `info` level by default; the bin
calls `init_test_tracing()` at startup so its envfilter matches the
workspace style). stdout is reserved for a single JSON line at exit
reporting `{"events_sent": N, "frames_sent": M, "duration_secs": D}`
so the Playwright spec can capture it via `child.stdout` for
debugging when an assertion fails.

Rejected alternatives:

* Take splice_info_section as a path to a binary file. Hex-on-CLI
  is more diff-friendly + works around quoting nuances; binary
  files would also need to live somewhere in-tree.
* Hard-code a single inject offset (no `--inject-at-secs`).
  Multi-emission is cheap and covers the future ad-pod test.
* Make the bin a Cargo example instead of a `[[bin]]`. Examples
  don't compile under `cargo build --bins` and the Playwright
  test-runner spawns from `target/debug/`, not
  `target/debug/examples/`.

**Locked: the flag table above, default --scte35-hex from
build_splice_insert_section, single-emission default, exit codes
0 / non-zero, stdout JSON line at exit.**

### 5. Synthetic H.264 NAL shape

Minimum viable for the relay's RTMP -> HLS bridge:

* SPS (sequence_parameter_set), one NAL.
* PPS (picture_parameter_set), one NAL.
* IDR slice (NAL type 5), one NAL per keyframe.
* Non-IDR slice (NAL type 1), one NAL per P-frame.

Each NAL is preceded by the Annex-B start code `0x00000001`. The
relay's RTMP bridge does NOT decode; it muxes NAL units into fMP4
and emits HLS segments at IDR boundaries. The synthetic SPS / PPS
must parse to resolve `width=320, height=180, profile=Baseline,
level=3.0, frame_rate=30`; the existing precedent for synthetic
H.264 in the workspace is the 16-byte
`synthetic_video_fragment` in `scte35_hls_dash_e2e.rs:44`, but
that test publishes directly onto the FragmentBroadcasterRegistry
(skipping RTMP -> HLS NAL parsing). The bin goes through the real
RTMP wire, so SPS / PPS must be a parseable byte sequence.

The bin pulls SPS / PPS from a small constant table compiled into
`crates/lvqr-test-utils/src/h264.rs` (NEW module). The table is a
hand-rolled minimal Baseline 320x180 30fps SPS + PPS captured from
ffmpeg `lavfi -i testsrc` output (lifted once at brief-write
time, validated via `ffprobe -show_streams` on a recorded sample).
Treat the table as opaque hex; comment block above documents the
parameters it encodes.

IDR + P-slice payloads carry small fixed bit patterns -- the
relay does not decode the macroblock data, only the slice header
(`first_mb_in_slice`, `slice_type`, `pic_parameter_set_id`,
`frame_num`). The synthetic slice header satisfies the parse but
the macroblock data is just zero-filled.

`crates/lvqr-test-utils/src/h264.rs` exports:

```rust
pub const SPS_320X180_30FPS: &[u8];
pub const PPS_BASELINE: &[u8];
pub fn synthetic_idr_nal() -> Bytes;
pub fn synthetic_p_slice_nal(frame_num: u32) -> Bytes;
```

The existing `synthetic_keyframe(size)` and
`synthetic_delta_frame(size)` helpers at `lib.rs:17-44` predate the
RTMP-wire path; the new helpers wrap them with the SPS / PPS prefix
and a valid IDR slice header. The old helpers are kept for the
direct-Fragment test paths that don't go through RTMP parsing.

Rejected alternatives:

* Run ffmpeg as a child + use it to encode synthetic frames. Adds
  a runtime dep on ffmpeg for what is meant to be a deterministic
  Rust-side bin. The dvr-player's existing `rtmpPush` helper does
  call ffmpeg, but for that helper ffmpeg is the publisher; this
  bin is a custom-protocol publisher precisely so it can inject
  AMF0 data ffmpeg cannot emit.
* Lift the `lvqr-record` H.264 emission code. Out of scope; that
  crate emits real encoded H.264 from raw YUV, which is much more
  surface than this test bin needs.
* Extract from a pre-recorded fMP4 fixture. fMP4 -> Annex-B
  conversion is non-trivial; in-tree const tables are simpler.

**Locked: `crates/lvqr-test-utils/src/h264.rs` module with const
SPS / PPS for Baseline 320x180 30fps + `synthetic_idr_nal()` +
`synthetic_p_slice_nal(frame_num)` helpers; bin emits 1 IDR +
59 P-slices per GOP at 30 fps with GOP=60 frames (= 2 s segments).**

### 6. Test scope

Per CLAUDE.md, integration tests use real network connections not
mocks. Five test additions across three tiers:

#### Unit (rml_rtmp client patch)

* `vendor/rml_rtmp/src/sessions/client/tests.rs` -- one new test
  `can_send_publish_amf0_data`. Drives the full publish setup
  (`perform_successful_connect` + `perform_successful_publish`),
  calls `publish_amf0_data` with a stub vector of three values
  (Utf8String, Object with two number / string fields, Number),
  feeds the produced packet back through `split_results`, and
  asserts the deserialized `RtmpMessage::Amf0Data { values }` arm
  round-trips with the same values. Mirrors the existing
  `can_send_publish_video_data` test shape.
* The fork's existing 168 upstream + 2 LVQR session-152 tests
  remain green (170/0/0). New count: 171/0/0.

#### Unit (lvqr-test-utils `splice_insert_section_bytes` extraction)

* The existing `build_splice_insert_section` helper at
  `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs:61` is a 90-line
  function that hand-rolls the SCTE-35 splice_info_section bytes
  including a CRC-32/MPEG-2 checksum. Move it into a public
  `crates/lvqr-test-utils/src/scte35.rs` module so both the
  existing e2e test AND the new bin / smoke test share a single
  source of truth.
* New `#[cfg(test)]` test in `lvqr-test-utils/src/scte35.rs`
  pinning the existing fixture's hex output for
  `(event_id=0xCAFEBABE, pts=8_100_000, duration_90k=2_700_000)`.
  Regression safety for the move; cross-checked through the full
  pipeline by the existing scte35_hls_dash_e2e.rs assertions
  (DATERANGE ID 3405691582, DURATION=30.000).
* The existing scte35_hls_dash_e2e.rs test re-exports the helper
  via `use lvqr_test_utils::scte35::splice_insert_section_bytes`;
  no behavior change.
* `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs:61-147` deletes the
  private `build_splice_insert_section` (the moved version is now
  the single source of truth). The local helper signature matches
  exactly so call sites at lines 150-160 are unchanged.

#### Integration (Rust, lvqr-test-utils smoke for the bin)

* `crates/lvqr-test-utils/tests/scte35_rtmp_push_smoke.rs` (NEW).
  Spawns a `TestServer` configured with RTMP enabled (on a random
  ephemeral port via the existing `find_available_port()` helper),
  spawns the `target/debug/scte35-rtmp-push` bin via
  `std::process::Command`, waits for the bin's `events_sent: 1`
  JSON line, polls the relay's variant playlist for
  `#EXT-X-DATERANGE`, asserts the daterange ID matches
  `splice-3405691582` (matching the default --scte35-hex's
  event_id 0xCAFEBABE), drops references + shuts down the server.
  Real RTMP wire end-to-end without browser; load-bearing
  coverage for the new bin.
* The smoke pulls the bin path from `env!("CARGO_BIN_EXE_scte35-rtmp-push")`
  (Cargo's standard mechanism for naming a sibling bin from an
  integration test). No PATH lookup, no manual cargo build inside
  the test, no env-var configuration.
* This test is NOT gated on `LVQR_LIVE_RTMP_TESTS`. It is the
  default-gate Rust integration test for the bin, runs on every
  `cargo test --workspace`, ships in CI's existing test-contract
  workflow.

#### E2E (Playwright, dvr-player gated on LVQR_LIVE_RTMP_TESTS=1)

* `bindings/js/tests/e2e/dvr-player/markers.spec.ts` -- ONE new
  test in the existing live-RTMP describe block, joining (and
  strengthening) the existing live-RTMP test:
  * **EXISTING test ("rtmpPush helper publishes a real RTMP feed
    the relay accepts")**: tighten the assertion -- after the
    master is ready, follow the variant URI, wait for >=2 EXTINF
    entries via the new `waitForLiveVariantPlaylist` helper, set
    `src` on the dvr-player, wait for
    `lvqr-dvr-live-edge-changed` with `isLiveEdge: true` within
    30 s, assert the LIVE pill shadow-DOM element carries
    `data-state="live"` (or whatever attr session 153 set; verified
    at brief-write time the part is `<span part="live-badge"
    data-state="live">LIVE</span>`).
  * **NEW test ("scte35-rtmp-push injects onCuePoint -> dvr-player
    renders DATERANGE marker")**: spawn the bin via
    `child_process.spawn` with --inject-at-secs=3, wait for
    variant playlist + DATERANGE line, mount the dvr-player,
    assert the marker pipeline emits `lvqr-dvr-markers-changed`
    with at least one OUT marker, assert the shadow-DOM
    `.marker-layer` carries one `.marker[data-kind="out"]` tick at
    `left: ~25%` (3 s OUT in a ~12 s window), assert the duration-
    derived IN tick + the paired span if --scte35-hex's break
    duration is non-zero.
* The new test re-uses the existing `test.beforeAll`'s
  `process.env.LVQR_LIVE_RTMP_TESTS !== '1'` skip + the
  `rtmpPushAvailable()` ffmpeg gate (extended to also check that
  `target/debug/scte35-rtmp-push` exists; if absent, skip with a
  clear "rebuild lvqr-test-utils bins" message).
* Both live tests share a `test.setTimeout(180_000)` block to
  accommodate the longer publish + variant-fill + LIVE-pill +
  marker-render budget.

Total test additions: 1 rml_rtmp + 1 lvqr-test-utils unit + 1 Rust
integration + 1 strengthened + 1 new Playwright = 5 net additions.
The existing 28 markers Vitest, the existing 32 dvr-player
attrs/seekbar/dispatch Vitest, and the existing 17 dvr-player
Playwright are unchanged.

#### Test counts (post-session, default-gate)

* Rust workspace lib slice: **1112 / 0 / 0** (+1 for the rml_rtmp
  client unit). The rml_rtmp tests are workspace lib because the
  vendored crate participates in `cargo test --workspace`.
* Rust workspace integration: +1 new test
  (`scte35_rtmp_push_smoke`). Existing scte35_hls_dash_e2e.rs
  retains its 3 tests (no behavior change from the helper extract).
* SDK Vitest: unchanged at 60 dvr-player + N other.
* Playwright: 1 strengthened (`live RTMP publish` -> `... + LIVE
  pill flips`) + 1 new (`scte35-rtmp-push -> marker render`),
  total dvr-player project goes from 18 to 19 tests with 17 of
  them runnable on default invocation + 2 gated.

**Locked: 5 test additions across the four tiers above; no
behavior change to existing tests.**

### 7. Anti-scope (locked)

Hard exclusions for this session, distinct from rejected
alternatives:

* **No npm publish.** `@lvqr/dvr-player` stays at 0.3.3 on main
  but is NOT published. Release happens in a separate session.
* **No SDK package version bump.** `@lvqr/dvr-player` stays at
  0.3.3; `@lvqr/player` and `@lvqr/core` stay at 0.3.2. The
  package source files are not touched by this session
  (`bindings/js/packages/dvr-player/src/*.ts` are unchanged); only
  test files + the Playwright config are touched.
* **No DASH-side rendering work.** dvr-player remains HLS-only.
* **No new HLS tag, no new admin endpoint, no relay route change.**
  Wire is session 152's `#EXT-X-DATERANGE` as-is.
* **No semantic interpretation of splice events.** The bin emits
  the bytes; the relay re-emits the bytes; the component renders
  a tick + a span. No ad-substitution, no blackout, no SSAI.
* **No v1.2 candidates** (engine="dash", server-side WEBVTT
  spritesheet, mobile-touch-optimised seek bar, native HLS Safari
  marker parity).
* **No additional changes to vendored rml_rtmp** beyond the
  `publish_amf0_data` patch + its one test. The session 152 patch
  (server-side `Amf0DataReceived`) is unchanged.
* **No CHANGELOG entry beyond the SDK package list line.** This
  session ships test + tooling deltas, not an SDK feature.
* **No `cargo publish`.** No Rust crate version bumps.
* **No production code path change in `@lvqr/dvr-player`.** Only
  test additions + helper additions (the bindings/js helpers
  module is dev-only and is not part of the published package).
* **No mesh-e2e CI surface change beyond the ffmpeg install + env
  var + path filter additions.** The `mesh` Playwright project's
  webServer + tests are unchanged.

## Execution order

1. **Author this briefing.** Step 0; this file.

2. **Read-back confirmation.** One-paragraph summary plus answers
   to the seven decisions above. Wait for explicit user OK before
   opening source.

3. **Pre-touch reading list (after confirmation):**
   * `tracking/SESSION_154_BRIEFING.md` -- the brief shape this
     mirrors.
   * `tracking/HANDOFF.md` "Session 154 close" + "Pending
     follow-ups" -- ground truth on what's deferred.
   * `bindings/js/tests/e2e/dvr-player/markers.spec.ts` -- the
     existing live-RTMP test this session strengthens.
   * `bindings/js/tests/helpers/rtmp-push.ts` -- the helper this
     session leans on; the new bin invocation mirrors its
     subprocess shape.
   * `.github/workflows/mesh-e2e.yml` -- the CI workflow this
     session edits.
   * `vendor/rml_rtmp/src/sessions/client/mod.rs` around
     `publish_metadata` at line 381 -- the patch-precedent for the
     new `publish_amf0_data` API.
   * `vendor/rml_rtmp/src/sessions/server/mod.rs` lines 920-950 --
     the session-152 patch precedent for symmetric
     `Amf0DataReceived` (informs `publish_amf0_data` shape).
   * `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs:61-147` -- the
     `build_splice_insert_section` helper to extract.
   * `crates/lvqr-test-utils/src/lib.rs` -- the existing helper
     surface; the new `scte35` module + `h264` module sit alongside
     `flv` / `http` / `rtmp` / `test_server`.
   * `crates/lvqr-test-utils/Cargo.toml` -- where the new
     `[[bin]]` declaration lands.
   * `crates/lvqr-ingest/src/rtmp.rs:454-545` --
     `parse_oncuepoint_scte35` ground truth on what AMF0 wire
     shape the relay accepts (the bin's wire shape must match).

4. **Land the rml_rtmp `publish_amf0_data` patch.** Add the method
   to `vendor/rml_rtmp/src/sessions/client/mod.rs` after
   `publish_metadata`. Add the unit test to
   `vendor/rml_rtmp/src/sessions/client/tests.rs`. Verify
   `cargo test -p rml_rtmp` passes 171/0/0.

5. **Land the `splice_insert_section_bytes` extraction.** Move the
   helper from `scte35_hls_dash_e2e.rs:61-147` into
   `crates/lvqr-test-utils/src/scte35.rs`; add a hex-pin test on
   the existing fixture; update the e2e test's import. Verify
   `cargo test -p lvqr-cli --test scte35_hls_dash_e2e` passes
   3/0/0 (no behavior change).

6. **Land the `h264` synthetic helpers.** New
   `crates/lvqr-test-utils/src/h264.rs` with const SPS / PPS
   tables + `synthetic_idr_nal()` + `synthetic_p_slice_nal(n)`.
   Validate via a small smoke test (`cfg(test)`) that the produced
   NAL byte sequences carry the expected start codes + NAL type
   bytes (5 for IDR, 1 for non-IDR).

7. **Land the `scte35-rtmp-push` bin.** New
   `crates/lvqr-test-utils/src/bin/scte35_rtmp_push.rs` with a
   clap-derive CLI matching decision 4. Body wires:
   * TCP connect to `--rtmp-url`
   * `rtmp_client_handshake(stream)` from `lvqr_test_utils::rtmp`
   * `ClientSession::new()` -> `request_connection(app)` -> wait
     `ConnectionRequestAccepted` via `read_until`
   * `request_publishing(stream_key, Live)` -> wait
     `PublishRequestAccepted`
   * `publish_metadata` (basic StreamMetadata: width, height,
     framerate, video_codec_id=7 / AVC)
   * Loop: emit one IDR + 59 P-slices per GOP at the configured
     fps (using `tokio::time::sleep` for paced sending). At each
     `--inject-at-secs` offset, emit a `publish_amf0_data` with
     the AMF0 onCuePoint shape (`Utf8String("onCuePoint"),
     Object{"name":"scte35-bin64", "data": base64(scte35_hex),
     "time": Number(now), "type": "event"}`).
   * `stop_publishing()` + close socket.
   Verify `cargo build -p lvqr-test-utils --bins` produces
   `target/debug/scte35-rtmp-push`. Manual smoke: run against a
   `lvqr serve` instance, observe `#EXT-X-DATERANGE` in the
   variant playlist.

8. **Land the Rust integration smoke.**
   `crates/lvqr-test-utils/tests/scte35_rtmp_push_smoke.rs` per
   decision 6. Verify `cargo test -p lvqr-test-utils --test
   scte35_rtmp_push_smoke` passes 1/0/0.

9. **Land the `waitForLiveVariantPlaylist` helper.** New
   `bindings/js/tests/helpers/hls-poll.ts` with the variant-
   playlist-non-empty pre-check function. Pure Node-side fetch +
   regex; no browser context. JSDoc covers the budget contract.

10. **Strengthen the existing live-RTMP test in markers.spec.ts.**
    Add the variant-pre-check + LIVE-pill assertion per decision 2.
    Verify locally with `LVQR_LIVE_RTMP_TESTS=1
    npx playwright test --project=dvr-player markers.spec.ts`.

11. **Land the new scte35-rtmp-push e2e test in markers.spec.ts.**
    Per decision 6 e2e tier. Verify locally with
    `LVQR_LIVE_RTMP_TESTS=1 npx playwright test --project=dvr-player
    markers.spec.ts -g "scte35-rtmp-push"`.

12. **Update mesh-e2e.yml.** Add the ffmpeg install step, the
    env var, the build flag extension, the path filters per
    decision 1.

13. **Update docs.**
    * `docs/scte35.md` or `docs/dvr-scrub.md` -- gain a small
      "Running the live-RTMP marker test locally" recipe (env var,
      ffmpeg install, cargo build, npx playwright test).
    * `tracking/HANDOFF.md` -- session 155 close block per
      existing shape (Project Status lead + What landed + What is
      NOT touched + Verification status + Pending follow-ups).
    * Root `README.md` -- "Recently shipped" gains a session-155
      bullet above the session-154 bullet.

14. **Verify CI green.** Push to a working branch first; verify
    the mesh-e2e workflow goes green with the live-RTMP test
    actually running (not skipped). Wait for explicit user OK
    before pushing to main.

## Risks + mitigations

* **rml_rtmp client patch breaks an upstream-flavored test.**
  The fork's 168 upstream tests have been green since session 152;
  the new method is additive. Mitigation: the patch lands with one
  new test (no modification to existing tests); if upstream has a
  future "all public methods listed" reflection test, the new
  method appears as a new entry, not a removal.

* **Synthetic SPS / PPS does not parse on the relay's RTMP -> HLS
  bridge.** The relay's bridge uses the `rml_rtmp` chunked stream
  parser + a downstream H.264 NAL splitter; SPS / PPS that satisfy
  the splitter but not a real decoder still pass through. The
  smoke test catches this (it asserts the variant playlist
  becomes non-empty + carries DATERANGE; if SPS / PPS fails to
  parse, no segment is emitted + the smoke times out). Mitigation:
  validate the SPS / PPS table hex against `ffprobe -show_streams`
  on a recorded sample at brief-write time + cite the specific
  fixture in `h264.rs`.

* **CI runner ffmpeg install adds wall time.** ~30 s on
  ubuntu-latest with apt cache cold, sub-5 s warm.
  `Swatinem/rust-cache@v2` caches the cargo target directory but
  not apt; the workflow already amortizes apt across the
  workflow's other steps. Net cost: <1 minute per workflow run.
  Mitigation: accept the cost; the live-RTMP test is the only
  CI-side coverage of the relay's RTMP onCuePoint -> HLS DATERANGE
  -> dvr-player marker pipeline end-to-end.

* **LIVE-pill assertion timeout under CI variance.** 30 s for the
  is-live flip is generous on local but CI runners can stutter.
  Mitigation: the variant-non-empty pre-check (option b in
  decision 2) ensures hls.js's first variant fetch finds segments;
  the LIVE pill threshold is `live-edge-threshold-secs` default
  6 s, and as soon as `seekable.end - currentTime` is below 6 s
  the pill flips. Time-from-src-set to is-live is sub-second on
  local; 30 s is 50x headroom. If a CI run does fail on this,
  bump to 60 s on the CI side via `LVQR_LIVE_RTMP_TIMEOUT_MS=60000`
  read by the spec.

* **scte35-rtmp-push smoke flakes on TIME_WAIT loopback
  collision.** Same shape as the existing markers.spec.ts dev-
  box flake noted in session 154's HANDOFF. Mitigation: the smoke
  uses ephemeral ports via `find_available_port()` so back-to-
  back test runs do not collide; the test serializes via a
  `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`
  attribute (matching scte35_hls_dash_e2e.rs's shape) so the
  TestServer's Tokio runtime is isolated from any upstream
  process state.

* **The Playwright test's `child_process.spawn` doesn't surface
  the bin's exit code if the bin crashes mid-publish.** The
  `rtmpPush` helper from session 154 caps the captured stderr at
  2 KB; the new test should mirror that cap so a misbehaving bin
  cannot bloat memory. Mitigation: re-use the existing
  `onStderr: (chunk) => { stderrTail = (stderrTail + chunk).slice(-2048); }`
  pattern from rtmp-push.ts:354-359.

* **`build_splice_insert_section` extraction breaks the existing
  e2e test.** The function is move-only (no signature change); the
  e2e test's call sites at lines 150-160 just need the import
  swapped. Mitigation: run the e2e test before+after the move and
  assert byte-for-byte equality of the produced section bytes.
  The `lvqr-test-utils` hex-pin test covers regression against
  a future rewrite.

* **The `[[bin]]` is built only when the test-utils crate is
  built `--bins`.** A developer running
  `cargo test -p lvqr-cli --test scte35_hls_dash_e2e` won't
  build the bin; the smoke at
  `lvqr-test-utils/tests/scte35_rtmp_push_smoke.rs` uses
  `env!("CARGO_BIN_EXE_scte35-rtmp-push")` which forces the build
  via Cargo's standard cross-test-target dependency. Mitigation:
  the smoke is the load-bearing default-gate test; no manual
  build step needed.

## Ground truth (session 155 brief-write)

* **Head**: `f849c84` on `main` (post-154 README polish).
  Workspace `0.4.1`. SDK packages `@lvqr/core 0.3.2`,
  `@lvqr/player 0.3.2`, `@lvqr/dvr-player 0.3.3`. After session
  155: all unchanged (test + tooling close-out, not a feature
  release).
* **Default-gate Rust workspace lib slice**: 1111 / 0 / 0. After
  session 155: 1112 / 0 / 0 (+1 rml_rtmp client unit).
* **Default-gate Rust integration tests**: scte35_hls_dash_e2e
  (3 tests) + others. After session 155: scte35_rtmp_push_smoke
  (1 test) joins the lvqr-test-utils integration target.
* **bindings/js shape**: the workspace at `bindings/js/`,
  `packages/{core,player,dvr-player}/`, tests at
  `bindings/js/tests/{e2e/{mesh,dvr-player},sdk,helpers}/`.
  Playwright runs two webServer profiles on ports 18088 (mesh)
  and 18089 (dvr-player); the dvr-player profile already passes
  `--no-auth-live-playback --hls-dvr-window-secs 300 --archive-dir
  ...` so the test does not need to mint signed URLs.
* **`@lvqr/dvr-player` shape**: vanilla `HTMLElement`, shadow DOM,
  `customElements.define`, ESM-only via tsc, dependent on
  `hls.js@^1.5.0`. Session 154 added the marker store + render +
  events + `getMarkers()` + `markers="visible|hidden"` + the two
  new public events (`lvqr-dvr-markers-changed`,
  `lvqr-dvr-marker-crossed`). Session 153's existing public event
  `lvqr-dvr-live-edge-changed` is the load-bearing signal for the
  strengthened LIVE-pill assertion.
* **vendored rml_rtmp**: at `vendor/rml_rtmp/`; loads via
  `[patch.crates-io]` from the workspace `Cargo.toml`. Session 152
  added `ServerSessionEvent::Amf0DataReceived` (server-side, ~25
  lines) + 2 LVQR defense tests. Total fork delta after session
  155: ~25 lines server + ~25 lines client method + ~50 lines
  client test = ~100 lines.
* **lvqr-test-utils**: `publish = false`; `[[bin]]` declarations
  appear in workspace test runs only. Existing modules: `flv`,
  `http`, `rtmp`, `test_server` + free helpers
  (`find_available_port`, `synthetic_keyframe`,
  `synthetic_delta_frame`, `init_test_tracing`,
  `generate_test_certs`, `ffprobe_bytes`,
  `mediastreamvalidator_playlist`, `is_on_path`). New modules:
  `scte35` (extracted helper), `h264` (synthetic NAL helpers).
  New bin: `scte35-rtmp-push`.
* **Relay HLS DATERANGE surface**: `#EXT-X-DATERANGE` rendered by
  `crates/lvqr-hls/src/manifest.rs:290` after `#EXT-X-MAP`; ID is
  `splice-<event_id>` (e.g. `splice-3405691582` for event_id
  0xCAFEBABE). The bin's default --scte35-hex emits event_id
  0xCAFEBABE so the e2e + smoke tests assert on that ID.
* **Relay RTMP onCuePoint parser**: at
  `crates/lvqr-ingest/src/rtmp.rs:470`. Wire shape:
  `vec![Amf0Value::Utf8String("onCuePoint"), Amf0Value::Object({
    "name": "scte35-bin64",
    "data": <base64-encoded splice_info_section>,
    "time": <seconds f64>,
    "type": "event",
  })]`. The bin emits exactly this shape.
* **CI workflows GREEN on session 154 head**: 8 GitHub Actions
  workflows (LL-HLS Conformance + MPEG-DASH Conformance + Feature
  matrix + Supply-chain audit + Tier 4 demos + SDK tests + Test
  Contract + CI). Mesh E2E continues to be `continue-on-error`;
  this session's additions to it are additive (the live-RTMP
  test going from skipped to running).

## Step 0 deliverable -- this briefing

Author at `tracking/SESSION_155_BRIEFING.md`. Read decisions 1
through 7 in order; the actual implementation order is in
"Execution order". The author of session 155 should re-read
`vendor/rml_rtmp/src/sessions/client/mod.rs` first (the
`publish_metadata` method this session mirrors), then
`vendor/rml_rtmp/src/sessions/server/mod.rs` lines 920-950 (the
session-152 patch precedent that locks the symmetric API shape),
then `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs:61-147` (the
`build_splice_insert_section` helper that the new bin reuses
through the extracted `lvqr-test-utils::scte35` module), then
`crates/lvqr-ingest/src/rtmp.rs:454-545` (the relay's
`parse_oncuepoint_scte35` shape the bin must match on the wire).

Decision 3 -- the `publish_amf0_data` API on the rml_rtmp client
fork -- is the design lever that unblocks session 154's "pending
follow-up" #3 (the real `onCuePoint` -> DATERANGE -> marker render
e2e). Without it, the bin would either need to roll its own
chunk-level RTMP publisher (high risk, big surface) or stick to
ffmpeg (which cannot natively emit AMF0 onCuePoint). With the
patch -- one new public method, ~25 LOC, mirroring the existing
`publish_metadata` shape minus the `@setDataFrame` + `onMetaData`
hardcoding -- the bin is straightforward and the e2e closes the
loop on session 154's deferred coverage gap.

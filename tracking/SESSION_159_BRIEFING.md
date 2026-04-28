# Session 159 Briefing -- PATH-X-MOQ-TIMING (Phase A v1.1 #5 close-out)

**Date kick-off**: 2026-04-27 (immediately after session 158's audit
+ DOC-DRIFT-A close-out). **Predecessor**: Session 158 follow-up
(DOC-DRIFT-A; commit `3192789` on `main`). Origin/main is `3192789`.
Workspace `0.4.1` unchanged. SDK packages `@lvqr/core 0.3.2`,
`@lvqr/player 0.3.2`, `@lvqr/dvr-player 0.3.3`. Default-gate
workspace lib **839 / 0 / 0**. Eight GitHub Actions workflows
GREEN; 14 of 15 are `continue-on-error: true` (only
`hls-conformance.yml` was promoted to required, per session 33).

The session-157 audit fired scenario (c) -- the MoQ wire carries
no per-frame wall-clock anchor, so a pure-MoQ subscriber cannot
compute glass-to-glass latency without an out-of-band timing
channel. The session-157 briefing
(`tracking/SESSION_157_BRIEFING.md:124-157`) sketched the v1.2
close-out (sibling `<broadcast>/0.timing` MoQ track) and locked
the strategy decisions; this session locks the engineering
decisions and ships the implementation.

After this session, Phase A v1.1 #5 (MoQ egress latency SLO) is
fully closed: the server-side endpoint shipped in the session 156
follow-up, the HLS-side first client (`@lvqr/dvr-player` PDT
sampler) shipped in the same wave, and this session ships the
pure-MoQ sample-pusher.

## Goal

Ship three artefacts in one commit, ~800-1200 LOC total:

1. **Producer side**: a new `MoqTimingTrackSink` type in
   `lvqr-fragment` that consumes `(group_id, ingest_time_ms)`
   pairs and writes 16-byte timing anchors onto a sibling MoQ
   track. Wired into `crates/lvqr-ingest/src/bridge.rs` so the
   RTMP path emits a timing anchor every keyframe.
2. **Subscriber side**: a new `[[bin]] lvqr-moq-sample-pusher` on
   `lvqr-test-utils` that subscribes to both `<broadcast>/0.mp4`
   and `<broadcast>/0.timing`, joins frames against anchors by
   `group_id`, computes `latency_ms = now_unix_ms() -
   anchor.ingest_time_ms`, and POSTs samples to
   `POST /api/v1/slo/client-sample`.
3. **Integration test**: a new `tests/moq_timing_e2e.rs` on
   `lvqr-test-utils` that drives the full RTMP -> relay -> bin
   -> SLO endpoint loop and asserts `GET /api/v1/slo` exposes a
   non-empty entry under `transport="moq"`.

After this session:

* `lvqr-fragment` exports `MoqTimingTrackSink` + a
  `TimingAnchor` value type.
* `crates/lvqr-ingest/src/bridge.rs` creates a
  `<broadcast>/0.timing` track at broadcast-start and pushes one
  16-byte anchor per keyframe.
* `lvqr-test-utils` ships the `[[bin]] lvqr-moq-sample-pusher`
  with a `clap` CLI, configurable push interval, and graceful
  shutdown on `SIGINT` / `SIGTERM`.
* The integration test runs default-gate (no feature flag,
  ubuntu-latest CI) and asserts the SLO histogram receives a
  pure-MoQ sample.
* README "Next up" #5 flips from open to closed; Phase A v1.1
  row "MoQ egress latency SLO" gets `[ ] -> [x]`.

## Decisions (locked)

The eight decisions below are drafted; per the user's "ultrathink
do whatever you should do next" instruction the session proceeds
without an explicit read-back gate, mirroring the session-156
pattern at briefing line 354.

### 1. Sibling `<broadcast>/0.timing` track shape

**Locked: 16-byte little-endian payload, one frame per group,
one group per video keyframe, sequence numbers align with the
video track by construction.**

Wire shape per timing-track frame:

```
+----------------+----------------------+
| group_id       | ingest_time_ms       |
| (8 bytes LE)   | (8 bytes LE)         |
+----------------+----------------------+
```

Each frame is the only frame in its MoQ group; the timing track
has no init-segment prefix (so `MoqGroupStream::without_init_prefix`
is the right adapter on the subscriber side). `group_id` is the
wire-side group sequence the *video* track assigned -- both tracks
call `track.append_group()` once per keyframe in lockstep, so
`moq-lite`'s auto-incrementing sequence numbers match.

`ingest_time_ms` is the same `Fragment::ingest_time_ms` value
(`crates/lvqr-fragment/src/fragment.rs:70`) the server-side
`LatencyTracker` already records for HLS / DASH / WS / WHEP
subscribers.

**Why little-endian**: matches every other 16-byte LVQR wire
shape (e.g. the 8-byte BE `object_id` prefix on the mesh
DataChannel framing is BE, but that's an intentional outlier
because mesh shares wire shape with the moq-js catalog convention;
LVQR's own choice for new internal wire shapes is LE per
[Rust's `to_le_bytes()` ergonomics]).

**Why one frame per group**: the MoQ group is the addressable
unit on the wire; subscribers with `live=true` jump in at the
latest group boundary. Putting the anchor in its own group
means a freshly-joined subscriber gets the most recent anchor
on the first frame, not after waiting for the next anchor write.
Cost: one extra group per keyframe (~one per ~2s GoP). On a
well-tuned MoQ relay this is sub-microsecond per anchor.

**Rejected: bundle anchors with video frames.** Putting
timing data inside the `0.mp4` track would re-litigate the
v1.1-B in-band wire-change rejection. Stays anti-scope.

### 2. Producer-side wiring location

**Locked: in `crates/lvqr-ingest/src/bridge.rs`, alongside the
existing `.catalog` track creation at `:177` and the keyframe
dispatch at `:317-348`.**

The bridge already:

* Creates `0.mp4` (video, `:161`), `1.mp4` (audio, `:169`), and
  `.catalog` (sibling track, `:177`) on `broadcast.create_track`.
* Holds `video_sink: MoqTrackSink` and `audio_sink: MoqTrackSink`
  on the per-broadcast state struct (`:32-33`).
* Uses `stream.catalog_track.append_group()` at `:548` for the
  catalog channel.

This session adds a fourth track via the same pattern:

```rust
// New, at :177-179 region:
let timing_track = match broadcast.create_track(Track::new("0.timing")) {
    Ok(t) => t,
    Err(e) => { warn!(error = %e, "failed to create 0.timing track"); /* skip */ }
};
```

The bridge's per-broadcast state grows a new field
`timing_sink: Option<MoqTimingTrackSink>` (option-typed so
broadcasts that fail the `create_track` call still ingest video).

In the keyframe handler at `:317-348`, after the existing
`publish_fragment(&registry_video, ...)` call, the bridge pushes
one timing anchor:

```rust
if frag.flags.keyframe && frag.ingest_time_ms != 0 {
    if let Some(timing) = stream.timing_sink.as_mut() {
        let _ = timing.push_anchor(group_id_used, frag.ingest_time_ms);
    }
}
```

Where `group_id_used` is the wire-side sequence the video sink
just allocated (see decision 4 for the API change on
`MoqTrackSink::push`).

**Rejected: install as a `FragmentBroadcaster` observer.** The
broadcaster pattern is for cross-cutting consumers (HLS bridge,
archive indexer, WASM filter tap, agents). The timing track is
tightly coupled to one specific `MoqTrackSink` instance (it must
align group sequences with that exact track), so co-locating
with the bridge is correct.

**Rejected: tap inside `MoqTrackSink::push`.** Would force every
caller of `MoqTrackSink` to opt out (str0m bridges in lvqr-whip,
test fixtures, etc.) and would mix two responsibilities into one
type. Keeping the timing sink separate keeps the test surface
small.

### 3. Wire shape for `MoqTimingTrackSink::push_anchor`

**Locked: pure 16-byte payload, no varint, no length prefix.**

The receiver knows by construction that every frame on the
`0.timing` track is exactly 16 bytes. Adding a length prefix or
versioning header is YAGNI -- if the wire shape ever needs to
change (extra fields, tagged enum), adding a sibling track named
e.g. `0.timing.v2` is the additive-evolution path the design is
already optimised for.

### 4. `MoqTrackSink::push` API change: return current group sequence

**Locked: `MoqTrackSink::push` return type changes from
`Result<(), MoqSinkError>` to `Result<Option<u64>, MoqSinkError>`,
where `Some(seq)` is the wire-side group sequence the call just
opened (only on keyframe paths) and `None` is the
non-keyframe / dropped-delta path.**

This is the cleanest way for the bridge to know the wire-side
sequence number to encode into the timing anchor. The alternative
(have the bridge call `track.append_group()` separately on the
timing track and assume sequences align) works but couples the
bridge to a load-bearing assumption about moq-lite's
sequence-number contract; explicit return is robust.

**API impact**: every caller of `MoqTrackSink::push` updates from
`sink.push(&frag)?;` to `let _ = sink.push(&frag)?;` or
`let group_seq = sink.push(&frag)?;`. The four call sites in the
workspace (per audit grep):

* `crates/lvqr-whip/src/bridge.rs:268` (video path) -- keep result
  in a `let _`.
* `crates/lvqr-whip/src/bridge.rs:341` (audio path) -- keep result
  in a `let _`.
* `crates/lvqr-fragment/tests/integration_sink.rs:25,130` -- tests,
  ignore the new return.
* `crates/lvqr-fragment/tests/proptest_fragment.rs:93,171` --
  proptest, ignore.
* `crates/lvqr-fragment/tests/moq_stream_roundtrip.rs:24` --
  ignore.

This is an internal API change; no SDK wire shape changes.

### 5. Subscriber-side bin shape

**Locked: `[[bin]] lvqr-moq-sample-pusher` on `lvqr-test-utils`,
mirroring the session 155 `scte35-rtmp-push` bin pattern.**

The bin lives at
`crates/lvqr-test-utils/src/bin/moq_sample_pusher.rs` and is
declared in `crates/lvqr-test-utils/Cargo.toml` as a second
`[[bin]]`. CLI shape:

```
lvqr-moq-sample-pusher [OPTIONS]

  --relay-url <URL>                 MoQ relay endpoint
                                    (e.g. https://localhost:4443)
  --broadcast <NAME>                Broadcast name (e.g. live/demo)
  --slo-endpoint <URL>              POST target
                                    (default: derives from --relay-url
                                     by swapping :4443 for :8080)
  --token <TOKEN>                   Subscribe-token bearer
                                    (rides the dual-auth path)
  --push-interval-secs <N>          Min seconds between pushes
                                    [default: 5]
  --max-samples <N>                 Optional sample cap
                                    (test convenience; default unbounded)
  --cert-fingerprint <HEX>          Self-signed cert fingerprint
                                    (dev/test convenience for
                                     `lvqr serve` without `--tls-cert`)
  --transport-label <STR>           Transport label sent in the SLO
                                    push body [default: "moq"]
  --duration-secs <N>               Run for N seconds then exit
                                    (test convenience; default unbounded)
```

`--cert-fingerprint` is necessary because `lvqr serve` defaults
to a self-signed cert; the integration test computes the
fingerprint from the TestServer's cert and passes it through.

The bin does NOT depend on any new feature; it builds on the
default-feature `lvqr-test-utils` graph.

### 6. Group-id matching strategy

**Locked: keep the last 64 timing anchors in a ring buffer; for
each video frame, look up the anchor with `group_id == frame.group_id`
exactly; on miss, fall back to the largest `group_id < frame.group_id`;
on still-miss, skip the sample.**

Rationale:

* Exact match is the common case (timing track ships the anchor
  in the same group sequence as the video keyframe; subscribers
  see them at the same logical instant).
* Largest-`group_id`-less-than fallback handles the case where
  the timing track's group is delayed by network jitter beyond
  the video group's first delta frame.
* Skip-on-miss handles the cold-start case (subscriber jumps in
  mid-broadcast and the video track's first group arrives before
  the timing track's catches up).

64 anchors is enough headroom for ~128 s of GoP at 2 s
(`max-keyframe-interval=60` from the GStreamer pipeline strings)
and the bin's intended push cadence (~5 s); evicting older
anchors keeps the ring buffer constant-memory.

### 7. Test scope

Three tiers, mirroring the session 155 + 156 shape:

#### Unit tests (in-crate)

* `MoqTimingTrackSink::push_anchor`: wire-shape pin (writes
  exactly 16 bytes; little-endian byte order verified by
  reading back the consumer side).
* `MoqTimingTrackSink::push_anchor`: each call opens one MoQ
  group (sequence number monotonic).
* `TimingAnchorJoin` helper (subscriber-side): exact-match,
  largest-less-than fallback, skip-on-miss-only-anchor
  ring-buffer behaviour, 64-entry cap eviction.
* Bin CLI parser: rejects empty broadcast, accepts the env
  variant of every flag, defaults are right.

#### Integration test (gated only on default features)

`crates/lvqr-test-utils/tests/moq_timing_e2e.rs`:

* Boots a `TestServer` with `--no-auth-live-playback` +
  `--no-auth-signal` (NoopAuthProvider). Captures the QUIC
  bind address, the admin bind address, and the self-signed
  cert fingerprint.
* Drives a synthetic RTMP publisher via existing
  `lvqr_test_utils::rtmp` helpers (re-uses the
  `scte35_rtmp_push` h264 builder for the synthetic NAL units).
  Pushes ~5 keyframes over ~3 seconds.
* Spawns `lvqr-moq-sample-pusher` as a subprocess against the
  TestServer's MoQ + admin endpoints, with
  `--push-interval-secs 1 --duration-secs 5
  --transport-label moq`.
* Polls `GET /api/v1/slo` (admin endpoint) every 200 ms for up
  to 10 s, asserts a non-empty entry appears under
  `transport == "moq"` with `count >= 1` and a finite
  `last_latency_ms` < 5_000.
* Cleans up the subprocess + the TestServer.

#### Workspace test counts (expected post-session)

* `cargo test -p lvqr-fragment --lib`: previous + ~3 new unit
  tests on `MoqTimingTrackSink`.
* `cargo test -p lvqr-test-utils --lib`: previous + ~5 new unit
  tests on `TimingAnchorJoin` + CLI.
* `cargo test -p lvqr-test-utils --test moq_timing_e2e`:
  **1 / 0 / 0** (the new integration test).

### 8. Anti-scope (locked)

* **No change to `MoqTrackSink`'s wire output.** Decision 4
  changes only the *return type* of `push`; the bytes written
  to the underlying `TrackProducer` are byte-identical.
* **No change to `0.mp4` wire shape.** The v1.1-B in-band
  wire-change rejection stays.
* **No new feature flag.** The timing track is always-on at the
  ingest bridge; foreign MoQ clients ignore unknown track names
  per the moq-lite contract.
* **No SDK package version bump.** No browser-side change; the
  TypeScript `MoqSubscriber` at
  `bindings/js/packages/core/src/moq.ts` does not need
  `0.timing` awareness today (the session-156 follow-up's
  HLS-side sampler already covers the browser path; pure-MoQ
  *browser* subscribers are not yet a deployment shape).
* **No change to `lvqr-cli`'s composition root.** The bridge
  already owns track creation; lvqr-cli sees no new flag.
* **No new admin route.** The existing
  `POST /api/v1/slo/client-sample` is the target.
* **No producer-side timing-track wiring beyond the RTMP
  bridge.** WHIP / SRT / RTSP / WS bridges stay on the existing
  shape this session; if the SLO surface eventually needs MoQ
  measurement for non-RTMP-ingested broadcasts, a future
  follow-up can mirror the wiring. The README's known limitation
  list explicitly documents that HLS / DASH / WS / WHEP all
  contribute to the SLO histogram via the existing server-side
  stamping; this session adds the MoQ-as-egress branch on top
  of RTMP-as-ingest, which is the dominant deployment shape.
* **No npm publish, no cargo publish, no workspace version
  bump.**

## Execution order

1. **Author this brief.** Step 0; this file.

2. **Lock decisions implicitly via the brief.** Per the user's
   prior "ultrathink do whatever you should do next" instruction,
   no read-back gate; proceed to source.

3. **Pre-touch reading list (before source touch):**
   * `crates/lvqr-fragment/src/moq_sink.rs` -- the existing
     `MoqTrackSink` shape; the new sink mirrors it.
   * `crates/lvqr-fragment/src/moq_stream.rs` -- the inverse
     adapter the subscriber-side will reuse for the video track
     side of the bin.
   * `crates/lvqr-fragment/src/lib.rs` -- where the new type
     re-exports.
   * `crates/lvqr-ingest/src/bridge.rs:30-200` (per-broadcast
     state + track creation), `:300-360` (keyframe dispatch).
   * `crates/lvqr-test-utils/src/bin/scte35_rtmp_push.rs` -- the
     reference shape for the new bin.
   * `crates/lvqr-test-utils/src/test_server.rs` -- existing
     TestServer accessors (`relay_addr()`, `admin_addr()`,
     `cert_der()` / fingerprint helper).
   * `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs` -- the shape
     of an admin-route integration test.
   * `crates/lvqr-admin/src/slo.rs` -- the `LatencyTracker` +
     `SloEntry` shape so the integration test asserts on the
     right field names.

4. **Land `MoqTimingTrackSink` in lvqr-fragment**:
   * New file `crates/lvqr-fragment/src/moq_timing_sink.rs`
     (~80 LOC + ~40 LOC of unit tests).
   * `pub mod moq_timing_sink;` + `pub use
     moq_timing_sink::{MoqTimingTrackSink, TimingAnchor};` in
     `lib.rs`.

5. **Change `MoqTrackSink::push` return type** to
   `Result<Option<u64>, MoqSinkError>`. Update the four
   in-tree call sites + the `MoqTrackSink::push` doc-comment.

6. **Wire the timing sink into the ingest bridge.** Add the
   `0.timing` track creation at `crates/lvqr-ingest/src/bridge.rs:177`
   region; thread the `MoqTimingTrackSink` through the per-
   broadcast state; push one anchor per keyframe in the
   dispatch loop.

7. **Land the `lvqr-moq-sample-pusher` bin.**
   * New file `crates/lvqr-test-utils/src/bin/moq_sample_pusher.rs`
     (~250-350 LOC).
   * `[[bin]]` declaration in `Cargo.toml`.
   * Reuse `lvqr_moq` for the subscriber-side; reuse
     `bindings/js/packages/dvr-player/src/slo-sampler.ts`'s
     POST body shape (the SLO endpoint is shape-agnostic
     between the JS and Rust clients).

8. **Land the `TimingAnchorJoin` helper** in
   `crates/lvqr-test-utils/src/timing_anchor.rs` so the bin and
   its unit tests can share the join logic.

9. **Land the integration test**
   `crates/lvqr-test-utils/tests/moq_timing_e2e.rs` (~150-200
   LOC). Default-feature gated so ubuntu-latest CI exercises it.

10. **Run the test sweep:**
    * `cargo test -p lvqr-fragment --lib`
    * `cargo test -p lvqr-test-utils --lib`
    * `cargo test -p lvqr-test-utils --test moq_timing_e2e`
    * `cargo test -p lvqr-ingest --lib`
    * `cargo test -p lvqr-whip --lib` (catches the
      `MoqTrackSink::push` callsite update)
    * `cargo build --workspace`
    * `cargo fmt --all -- --check`
    * `cargo clippy --workspace --all-targets -- -D warnings`

11. **Update docs**:
    * `docs/slo.md`: add a "Pure-MoQ subscriber sampling"
      section right after the existing HLS-side dvr-player
      section. Cite the new bin + the integration test path.
      Note that the HLS / pure-MoQ histograms merge under the
      existing per-`transport` keying.
    * `docs/architecture.md`: the per-broadcast track listing
      gains `0.timing` as a fourth sibling alongside `0.mp4` /
      `1.mp4` / `.catalog`. One paragraph addition.
    * `README.md`:
      * "Next up" #5 flips from open to checked with a forward
        link to the bin + the brief.
      * Phase A v1.1 row "MoQ egress latency SLO" flips
        `[ ] -> [x]`.
      * "Recently shipped" gains a session-159 bullet.
      * "Known v0.4.0 limitations" -- the "Pure MoQ subscribers
        do not contribute to the latency SLO histogram" bullet
        flips to past-tense (closed in session 159; bin ships
        on `main` for builds-from-source, with the v1.2
        consumer-grade SDK being a separate Tier 5 follow-up).
    * `tracking/HANDOFF.md`: lead paragraph + Last Updated +
      a new `## Session 159 close (2026-04-27)` block above
      the existing session-158 follow-up block.

12. **Single commit** following the session-156 / 158
    conventions:
    `feat(slo): pure-MoQ glass-to-glass sample pusher -- session 159 close`.
    Body: 3-section structure (What landed / What is NOT touched
    / Verification status), modeled on session 156's commit
    message.

13. **Push** when the user gives the OK.

## Risks + mitigations

* **moq-lite group-sequence alignment is not contractual**:
  decision 4's API change (return the wire-side group sequence)
  removes this risk.

* **TestServer cert fingerprint mismatch**: the integration test
  computes the SHA-256 fingerprint of the bound cert and passes
  it via `--cert-fingerprint`. If `TestServer` does not already
  expose the cert DER bytes via an accessor, the brief
  pre-supposes a small additive helper at
  `crates/lvqr-test-utils/src/test_server.rs` returning the
  cert DER. Five-line accessor; not a workspace risk.

* **Subprocess shutdown flake on macOS CI**: the bin runs
  subscribed to the relay until `--duration-secs` elapses or it
  receives `SIGINT` / `SIGTERM`. The integration test uses
  `tokio::process::Command::kill_on_drop(true)` so test panics
  do not leak processes.

* **Auth on the TestServer**: NoopAuthProvider is the test
  default. The bin's `--token` flag accepts an empty string and
  skips the bearer header in that case so the integration test
  exercises the dual-auth route's *anonymous* path. If the
  per-broadcast subscribe-token check rejects the unauth'd
  push, we explicitly use NoopAuthProvider in the test (per
  brief: `with_no_auth_live_playback(true)` already exists, and
  `with_no_auth_signal(true)` for completeness).

* **Integration test flake on a slow runner**: the 10 s polling
  loop is generous (5x the bin's `--duration-secs`); 200 ms
  poll interval keeps the test fast on a healthy runner. If
  the loop times out, the test logs the last `GET /api/v1/slo`
  response body for diagnosability.

* **Producer-side `ingest_time_ms == 0` skip**: the bridge
  guards on `frag.ingest_time_ms != 0` before pushing the
  anchor (per decision 2's snippet) so a zero-stamped fragment
  does not push a `(group_id, 0)` anchor that would compute
  60-year-latency on the subscriber side.

## Verification commands

* `cargo test -p lvqr-fragment --lib` -- expects ~3 new unit
  tests on `MoqTimingTrackSink`.
* `cargo test -p lvqr-test-utils --lib --test moq_timing_e2e`
  -- expects 1 new integration test passing.
* `cargo test -p lvqr-whip --lib` -- expects no change in
  count (callsite update is a single `let _ =` insertion).
* `cargo test --workspace --lib` -- expects ~847 / 0 / 0 (was
  839; +~8 net).
* `cargo build --workspace` -- clean.
* `cargo fmt --all -- --check` -- clean.
* `cargo clippy --workspace --all-targets -- -D warnings` --
  clean.
* Manual smoke: `cargo run -p lvqr-cli -- serve --rtmp-port
  11935 --hls-port 18888` in one terminal; `ffmpeg -re -f lavfi
  -i testsrc -c:v libx264 -f flv rtmp://localhost:11935/live/demo`
  in a second terminal; `cargo run -p lvqr-test-utils --bin
  lvqr-moq-sample-pusher -- --relay-url https://localhost:4443
  --broadcast live/demo --slo-endpoint http://localhost:8080/api/v1/slo/client-sample
  --duration-secs 10` in a third; `curl
  http://localhost:8080/api/v1/slo` should show a `transport:
  "moq"` entry.

## Pending follow-ups (NOT in this session)

* **Tier 5 browser MoQ subscriber sampling.** The `@lvqr/player`
  package's TypeScript `MoqSubscriber` could read the same
  `0.timing` track and push samples via `fetch()` to the SLO
  endpoint -- mirroring `@lvqr/dvr-player`'s session 156
  follow-up shape. Adds ~150 LOC + a Vitest spec; lives in a
  follow-up session because (a) the wire shape is brand new
  and ought to bake on the Rust client first, and (b) the
  TypeScript reader of the 16-byte LE payload needs a
  `DataView` shim that's worth building once the bin's edge
  cases are settled.

* **Per-non-RTMP ingest-bridge timing wiring.** WHIP / SRT /
  RTSP / WS bridges all carry `Fragment::ingest_time_ms`
  already; mirroring the bridge.rs wiring in each of the four
  ingest paths is mechanical (~30-50 LOC each) and ships in a
  follow-up if a non-RTMP deployment shape needs the
  measurement.

* **NVENC / VAAPI / QSV transcode backends** (still v1.2 from
  session 156; unchanged).

* **CI promotion** (audit recommendation #4): 14 of 15 workflows
  remain `continue-on-error: true`; promotion of 1-2 with the
  longest green streak is a separate operational session.

## Ground truth (session 159 brief-write)

* **Head**: `3192789` on `main` (post DOC-DRIFT-A close-out).
* **Workspace lib tests**: 839 / 0 / 0 (default features).
* **MoQ wire ground truth**:
  * `MoqTrackSink::push` writes `frag.payload.clone()` only
    (`crates/lvqr-fragment/src/moq_sink.rs:99-100`).
  * `MoqGroupStream::next_fragment` emits hard-zero
    `dts`/`pts`/`duration`/`priority`
    (`crates/lvqr-fragment/src/moq_stream.rs:142-152`); the
    inverse path on the timing track will use the
    `without_init_prefix` constructor (`:90-98`) so the 16-byte
    payload is treated as a payload, not an init segment.
* **Existing sibling-track precedent**: bridge already creates a
  `.catalog` track (`crates/lvqr-ingest/src/bridge.rs:177`) and
  drains it via `stream.catalog_track.append_group()` at `:548`.
  The new `0.timing` track plugs in at the same spots.
* **MoqTrackSink::push call sites** (4 + 5 in tests):
  * `crates/lvqr-whip/src/bridge.rs:268,341`
  * `crates/lvqr-fragment/tests/{integration_sink,proptest_fragment,moq_stream_roundtrip}.rs`
  * `crates/lvqr-fragment/src/moq_stream.rs:328` (in-test)

## Step 0 deliverable -- this briefing

Author at `tracking/SESSION_159_BRIEFING.md`. The eight locked
decisions are the read-back surface; the largest design lever is
decision 4 (the `MoqTrackSink::push` return-type widening) which
makes the timing-anchor producer side robust against moq-lite's
sequence-number contract. Decision 6 (group-id matching with
exact + largest-less-than fallback + skip-on-miss-only) is the
second-largest lever; the 64-anchor ring buffer is sized
generously so a delayed timing-track group does not silently
drop samples.

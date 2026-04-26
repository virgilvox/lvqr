# Session 156 Briefing -- Hardware encoder backend v1 (VideoToolbox / macOS)

**Date kick-off**: 2026-04-26 (same calendar day as session 155 close;
opportunistic kickoff after the user installed the official
GStreamer 1.28 .framework that previously gated this work).
**Predecessor**: Session 155 (test-coverage close-out for session
154's SCTE-35 marker render). Origin/main head `8301ea7`. Workspace
`0.4.1` unchanged. SDK packages `@lvqr/core 0.3.2`,
`@lvqr/player 0.3.2`, `@lvqr/dvr-player 0.3.3`. Default-gate Rust
workspace lib **1112 / 0 / 0**; admin surface **12 route trees**;
8 GitHub Actions workflows GREEN.

The README "Next up" / Phase A v1.1 roadmap has had two open
checkboxes since session 154 closed: hardware-encoder backend
(`README:797`) and MoQ egress latency SLO (`README:812`). The
latter is blocked behind Tier 5 client-SDK work by an explicit
v1.1-B scoping decision. The former is now unblocked: GStreamer
1.28.2 is installed at `/Library/Frameworks/GStreamer.framework`,
the dev box's `vtenc_h264_hw` element ran a smoke encode in 28 ms
(`videotestsrc num-buffers=60 ! videoconvert ! vtenc_h264_hw
bitrate=500 ! h264parse ! mp4mux` produced a valid MP4), and
`pkg-config --modversion gstreamer-1.0 gstreamer-app-1.0
gstreamer-video-1.0` returns `1.28.2 / 1.28.2 / 1.28.2`.

## Goal

Ship one hardware encoder backend behind a per-encoder Cargo
feature flag, mirroring the design intent already captured in
`crates/lvqr-transcode/src/lib.rs:48-49` ("Optional
hardware-encoder backends behind per-encoder feature flags
(`hw-nvenc`, `hw-vaapi`, `hw-qsv`, `hw-videotoolbox`)"). The pick
is **VideoToolbox on macOS** because:

* The dev box is macOS, so local verification is end-to-end
  testable without a separate CI runner.
* `vtenc_h264_hw` is GStreamer's HW-only path on Apple Silicon /
  Intel macOS, so a successful integration test asserts actual HW
  acceleration (no silent CPU fallback).
* The other three (NVENC, VAAPI, QSV) are deferred to v1.2 per the
  README's existing language; this session does not touch them.

After this session:

* New `[[file]] crates/lvqr-transcode/src/videotoolbox.rs` ships
  a `VideoToolboxTranscoderFactory` + `VideoToolboxTranscoder` that
  mirrors `SoftwareTranscoderFactory` / `SoftwareTranscoder` (105 B
  shape) but swaps `x264enc` for `vtenc_h264_hw`. Same trait
  surface, same lifecycle, same output broadcast naming
  (`<source>/<rendition>`), same fragment flow.
* New `hw-videotoolbox` Cargo feature gates the new module. Implies
  `transcode` so a transcode-feature build alone does not pull in
  the new module; an explicit `--features hw-videotoolbox` is
  required.
* `lvqr-cli` gains `--transcode-encoder software|videotoolbox`
  (default `software`). The `videotoolbox` value is only accepted
  when the binary was built with the new feature; otherwise the
  flag value is rejected at parse time with a clear error.
* The README's `[ ] One hardware encoder backend` checkbox under
  Phase A v1.1 flips to `[x]` with a forward link to the new
  module.

The session is workspace-additive: no Rust crate's API changes,
no relay-side wire changes, no SDK package version bump.
`@lvqr/dvr-player` stays at v0.3.3. Workspace stays at v0.4.1.
The other three HW backends (NVENC, VAAPI, QSV) are documented as
v1.2 candidates and remain deferred.

## Decisions (locked)

The seven decisions below are drafted; the read-back from the user
fixes them. The largest design lever is decision 1 -- whether to
extract a shared `pipeline` module from `software.rs` so the
two backends share the worker scaffolding, or to duplicate.
Locking it now so the patch shape is concrete.

### 1. Shared scaffolding: NEW backend duplicates ~50 lines, NOT a refactor

`software.rs` (916 lines) is built around three encoder-specific
bits:

* `REQUIRED_ELEMENTS` const (line 62-72) -- includes `x264enc`.
* `build_pipeline()` pipeline string (line 466-481) -- includes
  the `x264enc bitrate=... threads=2 tune=zerolatency
  speed-preset=superfast key-int-max=60` snippet.
* Factory `name()` returns `"software"`; `OUTPUT_CODEC` const is
  `avc1.640028`.

Everything else (worker thread, EOS handling, `attach_output_callback`,
`run_worker`, `push_buffer`, `wait_for_drain`,
`missing_required_elements`, `ns_to_ticks`,
`looks_like_rendition_output`, `WorkerHandle`, `BuiltPipeline`,
`WorkerSpawnArgs`, `WorkerSpawnError`) is generic GStreamer
plumbing.

Two design paths were considered:

* **Path A (refactor)**: extract scaffolding into a new internal
  `pipeline.rs` module and have both `software.rs` and
  `videotoolbox.rs` consume it. Long-term cleaner; future NVENC /
  VAAPI / QSV backends drop in trivially. Diff is bigger
  (~600-line move + scaffolding interface) and risks
  regressing the existing 105 B + 106 C tests by changing the
  module structure.
* **Path B (duplicate)**: copy the scaffolding into
  `videotoolbox.rs` verbatim, swapping the encoder bits. Smaller,
  lower-risk diff; existing `software.rs` is untouched; future
  refactor can happen in a session that introduces the third
  backend (when the duplication cost is undeniable).

**Locked: Path B (duplicate).** The CLAUDE.md rule "don't
introduce abstractions beyond what the task requires" pushes
this; the second backend is the wrong moment to refactor
prematurely. When NVENC or VAAPI lands, that session may extract
shared scaffolding as part of the third-backend work. Three
similar files is the threshold for an abstraction; two is fine.

### 2. Feature flag shape: `hw-videotoolbox` implies `transcode`

`Cargo.toml` gains:

```toml
hw-videotoolbox = ["transcode"]
```

`hw-videotoolbox` is purely additive: existing `--features
transcode` builds (the 105 B / 106 C software ladder default) are
unchanged. `--features hw-videotoolbox` builds with both software
and hardware encoders compiled in -- one binary, two factories
selectable at runtime via the new CLI flag (decision 5).

`crates/lvqr-transcode/src/lib.rs` re-exports the new types
behind `#[cfg(feature = "hw-videotoolbox")]` paralleling the
existing `#[cfg(feature = "transcode")]` re-exports of
`SoftwareTranscoder` / `SoftwareTranscoderFactory`.

Rejected:

* Single `transcode` feature that always pulls in both backends.
  Forces every transcode-enabled deployment to ship the
  `gstreamer-bad` plugins for VideoToolbox even if they only run
  software x264. Cargo features should be additive opt-ins; the
  per-backend feature pattern is also already documented as
  design intent in `lvqr-transcode/src/lib.rs:48-49`.
* `transcode-software` + `transcode-hardware` flags that need
  one-of-many semantics. Adds no value over additive features and
  is harder to compose in deployment Dockerfiles.

### 3. Pipeline string: `vtenc_h264_hw` with realtime + no reordering

The new `videotoolbox.rs::build_pipeline()` emits:

```
appsrc name=src caps=video/quicktime is-live=false format=time
  ! qtdemux
  ! h264parse
  ! avdec_h264
  ! videoscale
  ! video/x-raw,width=<W>,height=<H>
  ! videoconvert
  ! vtenc_h264_hw bitrate=<kbps> realtime=true allow-frame-reordering=false max-keyframe-interval=60
  ! h264parse
  ! mp4mux streamable=true fragment-duration=2000
  ! appsink name=sink emit-signals=true sync=false
```

Property mapping vs `software.rs`:

| x264enc (existing) | vtenc_h264_hw (new) |
| --- | --- |
| `bitrate={kbps}` | `bitrate={kbps}` (same units, kbps) |
| `tune=zerolatency` | `realtime=true` |
| `speed-preset=superfast` | (no equivalent; HW path is implicit fastest) |
| `key-int-max=60` | `max-keyframe-interval=60` |
| `threads=2` | (no threads; HW encoder uses VT framework workers) |
| (default frame reordering enabled) | `allow-frame-reordering=false` |

Output `OUTPUT_CODEC` stays `avc1.640028` (High profile per LVQR
convention; vtenc_h264_hw produces compatible bitstream).
`OUTPUT_TRACK = "0.mp4"`, `OUTPUT_TIMESCALE = 90_000` unchanged.
`REQUIRED_ELEMENTS` for the new module is the same list as
`software.rs` minus `x264enc` plus `vtenc_h264_hw`:

```rust
&[
    "appsrc",
    "qtdemux",
    "h264parse",
    "avdec_h264",
    "videoscale",
    "videoconvert",
    "vtenc_h264_hw",
    "mp4mux",
    "appsink",
]
```

### 4. Factory naming: `VideoToolboxTranscoderFactory`

The factory + transcoder pair mirror the `SoftwareTranscoder*`
naming. `name()` returns `"videotoolbox"`, surfacing in the
existing `lvqr_transcode_dropped_fragments_total{transcoder}`
and `lvqr_transcode_output_fragments_total{transcoder}` metric
labels. The metric labels stay unchanged in shape; operators see
new label values when the new factory is in play.

Output broadcast naming convention is unchanged: source
`live/cam1` with rendition `720p` produces output broadcast
`live/cam1/720p` regardless of which encoder factory built the
transcoder. The HLS bridge composer + DASH composer + archive
indexer all see the same shape.

The `looks_like_rendition_output` recursion guard logic is
duplicated verbatim into `videotoolbox.rs` so the two factories
behave identically at the registry-callback layer.

### 5. CLI wiring: new `--transcode-encoder` flag on `lvqr serve`

`lvqr-cli` already exposes `--transcode-rendition 720p,480p,240p`
to pick the ladder (session 106 C). This session adds:

```
lvqr serve --transcode-rendition 720p,480p,240p \
           --transcode-encoder videotoolbox
```

The flag's accepted values are:

* `software` (default) -- builds `SoftwareTranscoderFactory` per
  rendition; existing 106 C behavior.
* `videotoolbox` -- builds `VideoToolboxTranscoderFactory` per
  rendition; only available when the binary was compiled with the
  `lvqr-cli` feature `hw-videotoolbox` (which forwards to
  `lvqr-transcode/hw-videotoolbox`).

When `--transcode-encoder videotoolbox` is passed but the binary
lacks the feature, `lvqr-cli` exits with a clear error message at
config-validation time:

```
error: --transcode-encoder=videotoolbox requires the lvqr-cli
       binary to be built with the `hw-videotoolbox` feature
       (e.g. `cargo build --features hw-videotoolbox`).
```

The flag value is plumbed via a new `transcode_encoder:
TranscodeEncoderKind` enum on `ServeConfig` (default
`Software`). The `lvqr-cli` factory-construction loop in
`crates/lvqr-cli/src/lib.rs` switches on the enum to pick which
factory type to instantiate per rendition.

Per-rendition encoder selection (e.g. `720p:vt,480p:sw`) is
**out of scope** for v1; documented as an explicit anti-scope
below. One global encoder choice covers the deployment shape
operators ask for today.

### 6. Test scope

Five tiers, mirroring session 105 B's coverage shape for
software:

#### Unit (videotoolbox.rs)

Five tests parallel to `software.rs::tests`:

* `pipeline_string_embeds_rendition_geometry_and_bitrate` --
  smoke-test the pipeline string includes the expected `width=`,
  `height=`, `bitrate=` substrings. Same shape as software.
* `factory_opts_out_of_non_video_tracks_when_available` --
  audio track (`1.mp4`) returns `None` from `build`.
* `factory_returns_transcoder_for_video_track_when_available` --
  video track (`0.mp4`) returns `Some(...)` from `build`.
  Requires `vtenc_h264_hw` factory present; skipped on hosts
  without the applemedia plugin (the `is_available` branch).
* `videotoolbox_transcoder_output_broadcast_name_concatenates_source_and_rendition`
  -- pure naming convention check.
* `factory_skip_source_suffixes_builder_opts_out_of_custom_names`
  -- the recursion-guard suffix builder works.

The shared helpers (`ns_to_ticks`, `looks_like_rendition_output`)
already have unit tests in `software.rs`; the duplicated copy in
`videotoolbox.rs` is covered by reference (the tests in software.rs
exercise the shared logic, and the duplicated functions in
videotoolbox.rs are byte-equivalent).

#### Integration (gated)

`crates/lvqr-transcode/tests/videotoolbox_e2e.rs` (NEW):

* `#[cfg(all(target_os = "macos", feature = "hw-videotoolbox"))]`
  -- gated on macOS + the feature so non-mac CI runners and
  software-only builds skip cleanly.
* Drives a synthetic source-side `Fragment` stream into the
  factory (mirroring session 105 B's existing
  `transcode_software_e2e.rs` shape, if one exists; otherwise
  ships a minimal harness that pushes one fragment, drops the
  source side, waits for the output broadcaster to receive an
  init segment + at least one fragment).
* Asserts the output broadcast appears under
  `<source>/<rendition>` and that its first fragment is non-empty.

#### Workspace (existing)

* `cargo test -p lvqr-transcode` (no features) -- unchanged: the
  scaffold + passthrough tests run, the new module is gated out.
* `cargo test -p lvqr-transcode --features transcode` -- unchanged:
  software unit tests run, vtenc tests gated out.
* `cargo test -p lvqr-transcode --features hw-videotoolbox` --
  NEW: software + videotoolbox unit tests run; vtenc integration
  test runs on macOS.
* `cargo test -p lvqr-cli` -- gains coverage of the new
  `--transcode-encoder` flag parser (existing `clap` test pattern;
  unit tests in the CLI's own test module).

#### Manual smoke

Documented in HANDOFF "Verification status" -- manual smoke runs
`lvqr serve --transcode-rendition 720p --transcode-encoder
videotoolbox` against an ffmpeg testsrc publish, observes the
output broadcast in the relay's HLS variant playlist, and
confirms VideoToolbox is actually engaged via macOS Activity
Monitor's "VideoToolbox" GPU usage line.

### 7. Anti-scope (locked)

* **No NVENC / VAAPI / QSV.** v1.2 follow-ups; explicitly out of
  scope per the README's existing language.
* **No per-rendition encoder selection.** One global flag covers
  the v1 deployment shape; per-rendition mixing is v1.2.
* **No refactor of `software.rs`.** Decision 1 above; see the
  rationale.
* **No CI runner changes.** GitHub Actions ubuntu-latest doesn't
  ship VideoToolbox; the integration test gates on
  `target_os = "macos"`. CI continues to run the software-only
  test suite. A future macos-runner CI lane is an unrelated
  workflow change.
* **No public-API change on `lvqr-transcode`.** The crate keeps
  its existing `Transcoder` / `TranscoderFactory` /
  `TranscoderContext` / `RenditionSpec` exports. The new factory
  type is a new pub re-export under the new feature.
* **No SDK package version bump.** JS / TS bindings stay
  unchanged.
* **No relay-side wire change.** No new HLS tag, no new admin
  endpoint, no DASH change.
* **No npm publish, no cargo publish.**
* **No docs deep-dive on "how VideoToolbox works".** The docs
  update is operational: install recipe, CLI flag, feature-flag
  notes. Apple's AVFoundation / VideoToolbox docs are linked but
  not duplicated.

## Execution order

1. **Author this brief.** Step 0; this file.

2. **Lock decisions implicitly via the brief.** Per the user's
   "ultrathink do it" instruction, no read-back gate; proceed
   to source.

3. **Pre-touch reading list (before source touch):**
   * `crates/lvqr-transcode/src/lib.rs` -- the public API surface
     to extend.
   * `crates/lvqr-transcode/src/software.rs:62-72` (REQUIRED_ELEMENTS),
     `:466-481` (pipeline string), `:687-735` (helpers).
     The new module mirrors these.
   * `crates/lvqr-transcode/Cargo.toml` -- the feature stanza.
   * `crates/lvqr-cli/src/lib.rs` -- where `--transcode-rendition`
     parses + factories instantiate; new `--transcode-encoder`
     hooks alongside.
   * `crates/lvqr-cli/Cargo.toml` -- where the new feature
     forwards into `lvqr-transcode/hw-videotoolbox`.

4. **Land the videotoolbox.rs module** (~250 LOC; mostly mirrors
   software.rs).

5. **Land the Cargo.toml + lib.rs feature wiring.**

6. **Land the lvqr-cli flag + ServeConfig field + factory dispatch.**

7. **Land unit tests in videotoolbox.rs** (~150 LOC; mirrors
   software.rs::tests).

8. **Land the integration test** (gated; ~150 LOC).

9. **Run the test sweep:**
   * `PKG_CONFIG_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib/pkgconfig
     cargo test -p lvqr-transcode --features hw-videotoolbox`
   * `cargo test -p lvqr-transcode` (default features; sanity
     check the new gate doesn't leak)
   * `cargo test -p lvqr-cli` (CLI flag tests)
   * `cargo build -p lvqr-cli --features hw-videotoolbox` (so the
     binary supports the new flag end-to-end)

10. **Update docs:**
    * README.md: flip `[ ] One hardware encoder backend` to `[x]`
      with a session-156 footnote linking to the new module +
      installation recipe. Update the v1.1 "Next up" #4 entry.
      Add a session-156 bullet to "Recently shipped".
    * tracking/HANDOFF.md: session 156 close block per the
      session-155 shape (Project Status lead update, What landed,
      Decisions intentionally rejected, What is NOT touched,
      Verification status, Pending follow-ups).

11. **Single commit** following the session-155 shape:
    `feat(transcode): VideoToolbox hardware encoder backend on macOS -- session 156 close`.

12. **Wait for explicit user OK before pushing.**

## Risks + mitigations

* **vtenc_h264_hw caps-negotiation fails on a real ffmpeg
  testsrc input.** The smoke test used `videotestsrc` directly;
  the real path goes `qtdemux ! h264parse ! avdec_h264 !
  videoscale ! videoconvert ! vtenc_h264_hw`, which inserts
  format conversions. Mitigation: the integration test runs the
  full pipeline against a real fragment stream; if it fails, we
  add explicit caps filters between videoconvert and the encoder
  (e.g. `video/x-raw,format=NV12`).

* **VideoToolbox does not produce a `HEADER`-flagged buffer
  through mp4mux's `streamable=true fragment-duration=2000`
  shape.** The `attach_output_callback` in software.rs assumes
  the first buffer with `BufferFlags::HEADER` is the init segment.
  Mitigation: replicate the same logic in videotoolbox.rs; if
  vtenc behaves differently, add a pre-emit normalization pass
  (init segment cached on first non-HEADER fragment that
  resembles a moov / ftyp shape).

* **macOS-only HW path fails on an Intel Mac without VT-supported
  H.264 hardware.** Modern Macs support VT for H.264 universally,
  but very old Intel Macs may not. Mitigation: the
  `is_available()` probe at factory construction returns false
  when `vtenc_h264_hw` is missing; the factory then opts out of
  every build. The CLI flag's `videotoolbox` value at parse time
  errors with a descriptive message in that case.

* **Plugin probing race**: multiple
  `VideoToolboxTranscoderFactory::new()` calls could race on
  `gst::init()`. Mitigation: software.rs already calls `gst::init()`
  per factory construction; gst::init is documented idempotent
  across threads. The new factory follows the same pattern.

* **Integration-test flake on a busy CI macOS lane** (if a future
  session adds one): VT is shared across processes on macOS, so
  high concurrency could starve the encoder. Mitigation: the
  integration test publishes a small fragment count (~5) and
  uses bounded timeouts (~10 s).

* **CLI flag value rejected at runtime instead of parse time.**
  When the binary lacks the `hw-videotoolbox` feature, the flag
  must error at config validation, not at first transcoder
  spawn. Mitigation: the validator in `lvqr-cli` runs synchronously
  after `clap` parsing and before any registry callback fires.

## Ground truth (session 156 brief-write)

* **Head**: `8301ea7` on `main` (post-155 close).
* **GStreamer**: 1.28.2 at `/Library/Frameworks/GStreamer.framework`.
  `vtenc_h264_hw` (HW-only), `vtenc_h264` (HW with SW fallback),
  `x264enc`, `appsrc` / `appsink` all present.
* **gstreamer-rs**: 0.23.x already pinned at workspace level (per
  the existing `transcode` feature on `lvqr-transcode`).
* **Cargo features**: workspace-level `gstreamer = "0.23"` etc.
  Re-used as-is; no new transitive deps.
* **lvqr-transcode shape**: 916-line `software.rs` is the
  reference implementation; encoder-specific bits are isolated
  to ~50 LOC.
* **CI**: 8 GitHub Actions workflows GREEN on session 155 head;
  this session is additive and the new feature is opt-in, so the
  default CI matrix is unchanged.

## Step 0 deliverable -- this briefing

Author at `tracking/SESSION_156_BRIEFING.md`. Read decisions 1
through 7 in order; the actual implementation order is in
"Execution order". The largest design lever is decision 1 -- the
no-refactor rule keeps the diff bounded; decision 5 -- the CLI
flag pattern -- keeps existing software-encoder deployments
working unchanged when the binary is rebuilt with the new
feature.

# Session 105 briefing -- Tier 4 item 4.6 session B

**Kick-off prompt (copy-paste into a fresh session):**

---

You are continuing work on LVQR, a Rust live video streaming server.
Tier 3 is closed. Tier 4 items 4.1 (io_uring archive writes), 4.2
(WASM filters), 4.3 (C2PA signed media), 4.4 (cross-cluster
federation), 4.5 (AI agents + whisper captions), and 4.8
(one-token-all-protocols) are COMPLETE. Tier 4 item 4.6 session A
(`lvqr-transcode` scaffold crate) is DONE. Local `main` is at
`origin/main` (head `777a06c`). Session 105 is Tier 4 item 4.6
session B: real gstreamer-rs software encoder pipeline behind a
default-OFF `transcode` Cargo feature, driving the default
720p/480p/240p ABR ladder into the local
`FragmentBroadcasterRegistry` as `<source>/<rendition>`
broadcasts.

## Prerequisite: gstreamer install

The `transcode` feature pulls `gstreamer-rs` + the plugin set
`gst-plugins-base`, `gst-plugins-good`, `gst-plugins-bad`, and
`gst-plugins-ugly`. On macOS:

```bash
brew install gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly
```

On Debian / Ubuntu:

```bash
apt install libgstreamer1.0-dev gstreamer1.0-plugins-base \
            gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
            gstreamer1.0-plugins-ugly gstreamer1.0-libav
```

Verify with `gst-inspect-1.0 x264enc qtdemux mp4mux` -- all three
must resolve. Also verify the ffmpeg-based decoder plugin
(`avdec_h264`) resolves via `gst-inspect-1.0 avdec_h264`; on
Debian it lives in `gstreamer1.0-libav`.

If any plugin is missing, STOP here and surface the absent plugins
to the user before touching code -- a partial install produces
confusing drain-time errors rather than a clean factory-construction
error.

## Read first, in this order

1. `/Users/obsidian/Projects/ossuary-projects/lvqr/CLAUDE.md`.
   Project rules. AGPL-3.0-or-later + commercial dual-license. No
   Claude attribution in commits. No emojis. No em-dashes. Max
   line width 120.
2. `tracking/HANDOFF.md`. Read from the top through the session
   104 close block. The "Session 105 entry point" callout under
   the session 104 close block is ground truth.
3. `tracking/TIER_4_PLAN.md` section 4.6, specifically row 105 B.
   The plan row is a one-liner; you'll scope it up in-commit per
   CLAUDE.md's plan-vs-code rule.
4. `crates/lvqr-transcode/src/lib.rs`, `src/rendition.rs`,
   `src/transcoder.rs`, `src/passthrough.rs`, `src/runner.rs`.
   Session 104 A's scaffold. The new software encoder module
   slots in behind the `transcode` feature; the existing
   surface does NOT change shape.
5. `crates/lvqr-agent-whisper/src/worker.rs`. Reference pattern
   for "blocking C dependency behind a worker thread driven by
   `std::sync::mpsc` from inside `on_start`". The 105 B software
   encoder follows the same shape because gstreamer's blocking
   APIs don't compose with tokio.
6. `crates/lvqr-relay/tests/relay_integration.rs`. Reference for
   producing + consuming fMP4 fragments on a
   `FragmentBroadcasterRegistry` in a test.
7. Auto-memory at `/Users/obsidian/.claude/projects/-Users-obsidian-Projects-ossuary-projects-lvqr/memory/`.
   `project_lvqr_status.md` is refreshed through session 104
   push event.

## Ground truth (session 104 push event + README refresh, 2026-04-21)

- Head: `777a06c` on `main`, pushed to `origin/main`. Local is 0
  commits ahead. Verify with `git log --oneline origin/main..main`
  (empty).
- Tests: **892** passed, 0 failed, 1 ignored on macOS (default
  features). The 1 ignored is the pre-existing `moq_sink` doctest.
- Workspace: **29 crates** (added `lvqr-transcode` in session
  104 A). 25 published to crates.io at v0.4.0; 3 are
  `publish = false`; `lvqr-transcode` is a pending first-time
  publish. crates.io is unchanged since the post-session-98
  publish event.
- All gates green: `cargo fmt --all --check`;
  `cargo clippy --workspace --all-targets --benches -- -D warnings`;
  `cargo test --workspace` 892 / 0 / 1.
- CARRY-FORWARD reminders:
  - `RenditionSpec { name, width, height, video_bitrate_kbps,
    audio_bitrate_kbps }` is serde-ready; 105 B consumes these
    fields as x264enc + videoscale configuration.
  - `Transcoder` + `TranscoderFactory` + `TranscodeRunner`
    lifecycle (sync trait, panic-isolated, `catch_unwind` on
    `on_start` / `on_fragment` / `on_stop`) already handles the
    drain task spawning + metrics fan-out. The 105 B
    `SoftwareTranscoder` just implements the trait; the runner
    shape does NOT change.
  - `TranscodeRunnerHandle::fragments_seen(transcoder,
    rendition, broadcast, track)` is the existing observability
    surface. 105 B may add `output_fragments_seen` as a sibling
    counter on the handle for the output side; follow the same
    DashMap pattern.
  - `PassthroughTranscoder` stays as the always-available,
    no-gstreamer scaffold. Do NOT delete it; it's the reference
    that keeps the feature-off build green.
  - gstreamer-rs blocks on `element.set_state(Playing)` and on
    `appsrc.push_buffer`; any blocking call from inside
    `on_fragment` stalls the drain task. Use the
    `lvqr-agent-whisper/src/worker.rs` bounded-mpsc pattern:
    the Transcoder's `on_start` spawns a worker thread that
    owns the pipeline; `on_fragment` sends a `PushFragment(bytes)`
    message; the worker pushes buffers to `appsrc` and pulls
    output bytes from `appsink` via a signal callback.

## Session 105 scope: Tier 4 item 4.6 session B

Three deliverables:

1. **`transcode` Cargo feature on `lvqr-transcode`** (default
   OFF). Enables new optional deps: `gstreamer` (+ core), plus
   whichever of `gstreamer-app`, `gstreamer-video`, `glib` the
   pipeline needs. `full` meta-feature on `lvqr-cli` gains
   `transcode` (next to `whisper`, `c2pa`, `io-uring`). Without
   the feature the crate still builds (scaffold + passthrough);
   `cargo test -p lvqr-transcode` (no features) stays green.

2. **`SoftwareTranscoder` + `SoftwareTranscoderFactory`** in a
   new `src/software.rs` module, feature-gated on `transcode`.
   Pipeline shape (`gst::parse_launch` is fine for the bulk;
   programmatic API for the endpoints):

   ```
   appsrc name=src caps=video/quicktime,variant=iso-fragmented
     ! qtdemux
     ! h264parse
     ! avdec_h264
     ! videoscale ! video/x-raw,width=<W>,height=<H>
     ! videoconvert
     ! x264enc bitrate=<video_bitrate_kbps> tune=zerolatency speed-preset=superfast
     ! h264parse
     ! mp4mux streamable=true fragment-duration=2000
     ! appsink name=sink emit-signals=true
   ```

   `appsrc` caps describe fMP4 fragments. `mp4mux` produces
   fragmented output with 2 s segments (matches LVQR's HLS
   target duration default). `x264enc tune=zerolatency` keeps
   sub-GOP latency reasonable; `speed-preset=superfast` is
   the scaffold default (operators can tune via a future config
   surface).

   Worker thread: `SoftwareTranscoder::on_start` spawns a
   dedicated OS thread that:
   - Owns the gstreamer pipeline (not Send across threads
     reliably on all plugins).
   - Reads from a bounded `std::sync::mpsc::sync_channel(64)`
     holding `Fragment`s pushed from `on_fragment`.
   - Pushes `gst::Buffer`s to `appsrc`.
   - Registers a `new-sample` signal on `appsink` that pulls
     each output sample and forwards it onto the
     `FragmentBroadcasterRegistry` under
     `<broadcast>/<rendition>` with LVQR's `0.mp4` track.

   Full-channel behaviour: drop with `tracing::warn!` and a
   counter bump (`lvqr_transcode_dropped_fragments_total`).
   Same discipline as the whisper worker.

3. **Integration test `crates/lvqr-transcode/tests/software_ladder.rs`**
   (feature-gated on `transcode`; `#[ignore]` if the `transcode`
   feature is on but the required plugins are absent).
   Shape:
   - Boot a `FragmentBroadcasterRegistry`.
   - Install a `TranscodeRunner::with_ladder(
     RenditionSpec::default_ladder(),
     SoftwareTranscoderFactory::new)`.
   - Emit a short video-only fMP4 stream on
     `registry.get_or_create("live/src", "0.mp4", meta)` --
     use a fixture file from `crates/lvqr-conformance/fixtures/`
     or synthesize a few seconds of `testsrc` via a one-shot
     gstreamer pipeline and split into fragments.
   - Wait for output fragments to appear on
     `registry.get_or_create("live/src/720p", "0.mp4", ...)` and
     the matching 480p / 240p broadcasts.
   - Assert:
     - Each output broadcast has at least one GOP.
     - Output fragment counts are non-decreasing and monotonic
       with input.
     - Output bitrate is within +/-30% of the configured
       `video_bitrate_kbps` (coarse check; x264enc
       rate-control jitters at startup).
   - Clean up via `registry.remove` on every broadcast so the
     drain tasks exit and the worker thread joins.

## First decisions (in-commit, refresh the plan)

1. **Audio handling**: for 105 B, VIDEO ONLY. The output
   broadcasts carry only track `"0.mp4"`. Audio passthrough
   (copying `<source>`'s `"1.mp4"` to every rendition) is a
   separate task -- either the tail end of 105 B or the head
   of 106 C. Lean: fold audio passthrough into 105 B so the
   rendition is self-contained for LL-HLS composition in 106
   C. Cost: ~50 LOC for a second Transcoder impl that just
   copies fragments between registry entries.

2. **Output broadcast naming**: `<source>/<rendition>` exactly,
   matching the section 4.6 plan text (`live/cam1` ->
   `live/cam1/720p`). HLS master-playlist composition in 106 C
   will match `<source>` plus any `<source>/*` broadcasts into
   one master with per-rendition variants.

3. **Pipeline build method**: `gst::parse_launch` for the body
   of the pipeline, programmatic `AppSrc::builder()` /
   `AppSink::builder()` for the endpoints. Reasoning: the body
   is static per-rendition and reads well as a string; the
   endpoints need programmatic access to push buffers / set
   callbacks. Common gstreamer-rs idiom; see
   `gstreamer-rs/examples/src/bin/appsrc.rs`.

4. **fMP4 fragment -> qtdemux hand-off**: the first source
   fragment contains the init segment (`ftyp + moov + first
   moof + mdat`); push it as a single `gst::Buffer` with
   `BufferFlags::HEADER` set. Subsequent fragments are just
   `moof + mdat` and push as regular buffers. If upstream doesn't
   attach the init segment (because the drain task started mid-
   broadcast), read it off `bc.meta().init_segment` and push
   first.

5. **gstreamer plugin detection**: at `SoftwareTranscoderFactory`
   construction time, call `gst::ElementFactory::find(name)`
   for each required element; if any is missing, return a
   `TranscoderBuildError::MissingPlugin(Vec<&'static str>)`
   rather than panicking. The factory's `build(ctx)` can then
   return `None` on missing plugins and log a clear warning
   once per missing plugin. Alternative (panic in factory
   construction) is too aggressive for a library crate; the
   `None` path is consistent with the existing factory
   opt-out pattern.

6. **Back-pressure + bounded appsrc**: `appsrc.set_max_bytes(4 *
   1024 * 1024)` (4 MiB) + `block: true` so `push_buffer` blocks
   the worker thread instead of the drain task when the pipeline
   is slow. Drain-side bounded mpsc protects the drain task
   from blocking on the worker thread's back-pressure.

7. **Metric names**: `lvqr_transcode_output_fragments_total{
   transcoder, rendition}` on the output side (sibling of the
   existing `lvqr_transcode_fragments_total`);
   `lvqr_transcode_dropped_fragments_total{transcoder,
   rendition}` for full-channel drops. No new Prometheus knob
   on the admin surface for 105 B; metrics ride through the
   existing `/metrics` endpoint.

## Test shape

1. `cargo test -p lvqr-transcode` (no features): passes
   unchanged; the 16 scaffold tests + 1 doctest are still the
   default workspace gate.
2. `cargo test -p lvqr-transcode --features transcode --lib`: 3-5
   new inline tests on `src/software.rs` covering
   plugin-availability detection + caps negotiation + channel
   backpressure (as much as can be exercised without a real
   pipeline).
3. `cargo test -p lvqr-transcode --features transcode --test
   software_ladder`: the new integration test. On a runner with
   all plugins, ~5-10 s wall clock. On a runner missing plugins,
   skip-with-log branch (look up `gst::ElementFactory::find` for
   each required name; if any is `None`, eprint a message and
   return).
4. `cargo test --workspace`: parity with session 104's 892
   without the `transcode` feature; a second workspace-wide
   test pass under `--features lvqr-transcode/transcode` (via a
   `full` meta-feature) is desirable but OPTIONAL for 105 B --
   if it regresses anything unrelated, pin it as a 106 C task.

## Verification gates (session 105 B close)

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --benches -- -D warnings`
- `cargo clippy -p lvqr-transcode --features transcode
  --all-targets -- -D warnings`
- `cargo test -p lvqr-transcode` (no features; parity with 892
  workspace count)
- `cargo test -p lvqr-transcode --features transcode --lib`
  (new tests passing)
- `cargo test -p lvqr-transcode --features transcode --test
  software_ladder` (new integration test passing)
- `cargo test --workspace` (default features; expect 892 + 3-5
  new inline tests = 895-897)
- `git log -1 --format='%an <%ae>'` MUST read
  `Moheeb Zara <hackbuildvideo@gmail.com>` alone

Prefer targeted `-p <crate> --features transcode` runs during
iteration; only run `--workspace` for the pre-close verification
pass.

## Absolute rules (hard fails if violated)

- NEVER add Claude as author or co-author. No `Co-Authored-By`
  trailers. Verify with `git log -1 --format='%an <%ae>'` after
  every commit.
- No emojis in code, commit messages, or documentation.
- No em-dashes or obvious AI language patterns in prose.
- Max line width 120. fmt + clippy + test must be clean before
  committing.
- Integration tests use real gstreamer pipelines, not mocks. The
  `software_ladder.rs` test must drive actual x264enc output
  through the `FragmentBroadcasterRegistry`; asserting on
  plumbing state without real encoded bytes would be a
  theatrical test.
- Only edit files within
  `/Users/obsidian/Projects/ossuary-projects/lvqr/`.
- Do NOT push or publish without a direct instruction from the
  user.
- If the plan and the code disagree, refresh the plan in the
  same commit as the code change.

## Expected scope + biggest risks

~600-900 LOC across `lvqr-transcode/Cargo.toml` (new feature +
deps), `src/software.rs` (new module, pipeline + worker
thread), `src/lib.rs` (conditional re-export), inline tests, and
the `software_ladder.rs` integration test.

Biggest risks, ranked:

1. **fMP4 fragment -> qtdemux hand-off shape**. The LVQR
   fragments the drain task receives are `moof + mdat` from the
   second fragment onward; qtdemux needs the init segment
   (`ftyp + moov`) first. If the drain task starts mid-broadcast
   (e.g. a transcoder added after ingest begins), the init
   segment must come from `FragmentMeta::init_segment`.
   Mitigation: at `on_start`, if `ctx.meta.init_segment` is
   populated, push it first as a `BufferFlags::HEADER`; only
   then process the mpsc channel. If `init_segment` is empty at
   `on_start`, wait for the first fragment with
   `FragmentFlags::keyframe` and prepend the init then.

2. **gstreamer worker thread lifecycle**. Must cleanly shut
   down on broadcast-end (mpsc sender dropped ->
   `recv().unwrap_err()` -> `appsrc.end_of_stream()` -> wait
   for EOS on bus -> set pipeline to Null -> join). A leaked
   thread or a pipeline stuck in `Playing` at drop blocks the
   tokio runtime on `Drop`. Mitigation: mirror whisper
   worker's shutdown flow exactly; the reference is solid.

3. **Plugin version skew**. `mp4mux`'s `streamable=true` +
   `fragment-duration` properties exist in gst-plugins-good
   >= 1.20; older distros may ship 1.18. Mitigation: at
   factory construction, probe `mp4mux` for the
   `fragment-duration` property via
   `element.find_property("fragment-duration")`; if absent,
   return `None` from `build()` with a clear warn log.

4. **Output fragment packaging**. `mp4mux`'s output via
   `appsink` comes as one buffer per "segment" in its internal
   packager; we need to split into `moof + mdat` fragment
   shape for the registry. Likely the first buffer is init
   (`ftyp + moov`) and subsequent buffers are `moof + mdat`
   each. Verify with a gstreamer test rig before committing
   to an architecture; if `mp4mux` batches multiple fragments
   per output, need `splitmuxsink` instead.

5. **x264enc GOP alignment**. Output ladder should preserve
   source GOP boundaries so LL-HLS segmentation stays
   consistent across renditions. x264enc's `keyint-max` +
   `keyint-min` tuned to the source's keyframe cadence
   (typical 2 s at 30 fps = keyint 60). For 105 B, hard-code
   `keyint-max=60 keyint-min=60` and document as a v1
   limitation; 106 C can add source-keyframe-aware tuning.

6. **CI availability**. Runners without gstreamer plugins
   will fail the `--features transcode` test. Mitigation:
   factory opt-out on missing plugins + test skip-with-log;
   document the gstreamer install recipe in the 105 B close
   block so operators reading the HANDOFF can reproduce.

## After session 105 B

Write a session 105 close block at the top of HANDOFF.md (under
the session 104 push event block). Flip 4.6 row 105 B to DONE in
TIER_4_PLAN.md; keep section 4.6 header at "A + B DONE, C
pending". Refresh `project_lvqr_status.md` memory. Tier 4 item
4.6 is two-thirds done after this session lands; 6 of 8 Tier 4
items + 4.6 A + B remain the status. Session 106 entry point:
Tier 4 item 4.6 session C (`lvqr-cli` wiring +
`--transcode-rendition` flag + LL-HLS master playlist
composition advertising every `<source>/<rendition>` as a
variant; read `tracking/TIER_4_PLAN.md` section 4.6 row 106 C).

After the close doc, commit as a separate "docs:" commit
(mirroring every prior session's pattern). Do NOT push without a
direct user instruction. If the user instructs a push, follow up
with a "docs: session 105 push event" commit that refreshes the
HANDOFF status header to `origin/main synced (head <new_head>)`
as the sessions 99 / 100 / 102 / 103 / 104 push-event commits
did.

Work deliberately. Each commit should tell a future session
exactly what changed and why. Do not mark anything DONE until
verification passes.

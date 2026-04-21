# LVQR Handoff Document

## Project Status: v0.4.0 -- Tier 3 COMPLETE; Tier 4 item 4.6 COMPLETE (7 of 8 Tier 4 items done: 4.1 + 4.2 + 4.3 + 4.4 + 4.5 + 4.6 + 4.8); 900 workspace tests on the default gate (+31 transcode-feature lib + 1 transcode-feature integration + 1 transcode-feature e2e), 29 crates; local `main` N+2 ahead of `origin/main` (pre-commit head `cdbb854`)

**Last Updated**: 2026-04-21 (session 106 close). Session 106 is Tier 4 item 4.6 session C: the composition-root wiring + master-playlist composition + audio passthrough that flips section 4.6 from "A + B DONE" to **COMPLETE**. Remaining Tier 4: 4.7 (latency SLO scheduling). Session 107 entry point is Tier 4 item 4.7 session A (latency SLO histogram wiring + `/api/v1/slo` admin route).

## Session 106 close (2026-04-21)

1. **Tier 4 item 4.6 session C: CLI wiring + master playlist + AAC passthrough** (feat commit).
   * `crates/lvqr-transcode/src/audio_passthrough.rs` (new, ~370 LOC, always-available -- no GStreamer dep): `AudioPassthroughTranscoder` + `AudioPassthroughTranscoderFactory`. Copies `Fragment` instances from `<source>/1.mp4` to `<source>/<rendition>/1.mp4` verbatim (payload bytes + init-segment ASC), so each rendition is a self-contained mp4 that the existing LL-HLS bridge drains via `ensure_audio` without special casing. 5 new inline tests (non-audio opt-out, audio opt-in, already-transcoded skip, custom suffix skip, fragment copy preserves bytes, init-segment propagation).
   * `crates/lvqr-transcode/src/software.rs`: new `SoftwareTranscoderFactory::skip_source_suffixes(impl IntoIterator<...>)` builder + refactored `looks_like_rendition_output(broadcast, extra: &[String])`. Appends to the built-in `\d+p` recursion guard; operators running custom rendition names (`ultra`, `low-motion`, ...) pass them here. 2 new inline tests.
   * `crates/lvqr-transcode/src/runner.rs`: `drive()` now refreshes `sub.meta()` before `on_start` so a late `set_init_segment` call (the RTMP bridge pattern: `get_or_create` fires the callback, then `set_init_segment`) is picked up by the transcoder snapshot. Previously the software pipeline saw `ctx.meta.init_segment = None` at startup and qtdemux reported "no known streams found" with synthetic fragments; the refresh removes that race.
   * `crates/lvqr-hls/src/master.rs`: new `RenditionMeta { name, bandwidth_bps, resolution, codecs }` + `RenditionMeta::bandwidth_bps_with_overhead(kbps)` helper. Kept out of `lvqr-transcode` to avoid a cross-dep; the `lvqr-cli` composition root converts `RenditionSpec` -> `RenditionMeta` at startup.
   * `crates/lvqr-hls/src/server.rs`: `MultiHlsServer::set_ladder(Vec<RenditionMeta>)` + `set_source_bandwidth_bps(Option<u64>)` + `variant_siblings(source)` scan helper. `handle_master_playlist` now emits one `#EXT-X-STREAM-INF` per sibling broadcast matching a registered rendition name (highest-to-lowest bandwidth, plus the source variant at `highest_rung * 1.2` by default or the operator override); relative URIs `./<rendition>/playlist.m3u8` so CDN caching + reverse-proxy setups work unchanged. 2 new inline tests.
   * `crates/lvqr-cli/src/lib.rs`: feature-gated `ServeConfig.transcode_renditions: Vec<RenditionSpec>` + `source_bandwidth_kbps: Option<u32>` + `ServerHandle.transcode_runner()` accessor. `start()` builds one `SoftwareTranscoderFactory` + one `AudioPassthroughTranscoderFactory` per rendition (both wired with `skip_source_suffixes` seeded from the full rendition-name list to avoid ladder recursion through custom names), installs them via `TranscodeRunner::with_factory(..).install(&shared_registry)`, and registers the ladder metadata on the HLS server so the master playlist emits variants. 5 new feature-gated inline tests covering preset parsing + TOML rendition files + ordering + default-empty ladder + unknown-preset error.
   * `crates/lvqr-cli/src/main.rs`: feature-gated `--transcode-rendition <NAME>` repeatable clap arg (`ArgAction::Append`) + `LVQR_TRANSCODE_RENDITION` env fallback (comma-separated) + `--source-bandwidth-kbps` operator override. Preset names (`720p`/`480p`/`240p`) parse to `RenditionSpec::preset_*`; paths ending in `.toml` read + deserialize via serde; anything else is a clap-level error.
   * `crates/lvqr-cli/Cargo.toml`: `transcode` feature now also activates `lvqr-test-utils/transcode` so the dev-dep sees the new `with_transcode_ladder` / `transcode_runner()` surface in feature-gated tests.
   * `crates/lvqr-test-utils/Cargo.toml` + `src/test_server.rs`: new `transcode` feature forwarding to `lvqr-cli/transcode` + `dep:lvqr-transcode`. `TestServerConfig::with_transcode_ladder(Vec<RenditionSpec>)` + `with_source_bandwidth_kbps(u32)` builders + `TestServer::transcode_runner()` accessor. `src/lib.rs`: `is_on_path(name)` promoted to pub so integration tests can soft-skip when tools like `gst-inspect-1.0` or `ffmpeg` are absent.

2. **`crates/lvqr-cli/tests/transcode_ladder_e2e.rs`** (new, ~430 LOC, feature-gated on `transcode` + `rtmp`, soft-skip when GStreamer plugins are absent): real `rml_rtmp` publish against `TestServer::with_transcode_ladder(RenditionSpec::default_ladder())`, poll the master playlist at `/hls/live/demo/master.m3u8` until four variants appear, assert each rendition's `BANDWIDTH` / `RESOLUTION` / relative URI, fetch each rendition's audio playlist (proves the `AudioPassthrough` registered the sibling broadcaster on the HLS server), and check the `TranscodeRunner` counters go non-zero for every `(software|audio-passthrough) x (720p|480p|240p) x live/demo` quadruple. Wall clock under 1 s on an M-series mac.

3. **Session 106 close doc** (this commit).

### Key 4.6 session C design decisions baked in (confirmed in-commit per the plan-vs-code rule)

* **CLI flag shape**: `--transcode-rendition <NAME>` repeatable. Three short preset names (`720p` / `480p` / `240p`) parse directly via `lvqr_cli::parse_one_transcode_rendition`; paths ending in `.toml` are read + deserialized via serde; anything else is a clap parse-time error so misconfigured ladders surface up-front. Env var `LVQR_TRANSCODE_RENDITION` is comma-separated since clap's env parser does not repeat.
* **Source-variant BANDWIDTH**: defaults to `highest_rung_bps * 1.2` when a ladder is configured (emitted as the first variant in the master playlist so ABR clients honouring playlist order pick the source first). `--source-bandwidth-kbps` operator override exists for operators with known upstream bitrates; future 4.7 latency-SLO infrastructure can replace this with source measurement when it lands.
* **Master-playlist URI shape**: each rendition variant points at `./<rendition>/playlist.m3u8` (relative); CDN caching + operator reverse-proxy setups keep working without master-rewrite. Each rendition's media playlist stays at the existing `/hls/<source>/<rendition>/playlist.m3u8` path the HLS bridge already serves.
* **CODECS attribute**: hard-coded `"avc1.640028,mp4a.40.2"` per the briefing's decision (d). Session 107 or later can parse the actual SPS + audio ASC from each rendition's init segment to populate the real codec string.
* **`skip_source_suffixes` seeded with the full rendition-name list**: the CLI wiring passes `["720p", "480p", "240p"]` (or whatever the operator configured) to every factory so a rendition named `"ultra"` does not silently loop through a 720p factory when the 720p factory's own `live/demo/ultra` output comes around. Default `\d+p` guard still applies alongside the custom list.
* **Audio passthrough output track is `"1.mp4"`**: matches the existing LVQR audio-track convention; the LL-HLS bridge's `ensure_audio` path handles the new `<source>/<rendition>/1.mp4` broadcasters without any special-casing.
* **`RenditionMeta` lives in `lvqr-hls` (not `lvqr-transcode`)**: keeps `lvqr-hls` dep-light and lets the CLI composition root do the `RenditionSpec` -> `RenditionMeta` translation once at startup. `lvqr-hls` stays the authority on master-playlist shape without needing to know about GStreamer-adjacent types.
* **`drive()` meta refresh before `on_start`**: fixes the RTMP-bridge init-segment race surfaced under real publish (callback fires at `get_or_create`, `set_init_segment` fires after). Minimal change: snapshot the live meta right before `on_start` runs so the transcoder sees whatever init bytes landed in the gap. The trait surface is unchanged.
* **Hardware encoders deferred post-4.6**: the plan-v1 row 106 C one-liner read "Hardware encoder feature flags; benchmark NVENC vs x264". That wording is stale; 106 C's real scope is the composition root + master playlist + AAC passthrough. Hardware encoders (NVENC, VideoToolbox, VAAPI, QSV) are post-4.6 follow-ups; the software ladder is the feature-complete v1 encode path.

### Ground truth (session 106 close)

* **Head**: feat commit (pending) + this close-doc commit (pending). Local `main` will be N+2 ahead of `origin/main`; no push event this session per the absolute rule. Pre-commit head on `origin/main`: `cdbb854`.
* **Tests (default features gate)**: **900** passed, 0 failed, 1 ignored on macOS. +8 from session 105's baseline (892): 5 new inline tests on the always-available `audio_passthrough.rs` + 2 new inline tests on `lvqr-hls/src/server.rs` for master-playlist composition + 1 extra on the `RenditionMeta::bandwidth_bps_with_overhead` helper (structurally rolled into the master-playlist tests). The 1 ignored is the pre-existing `moq_sink` doctest.
* **Tests (transcode feature gate)**:
  * `cargo test -p lvqr-transcode --features transcode --lib`: **31** passed (+8 over session 105: 5 new audio passthrough + 2 new software `skip_source_suffixes` + 1 extra inline on the updated recursion guard).
  * `cargo test -p lvqr-transcode --features transcode --test software_ladder`: **1** passed (wall clock ~0.3 s; unchanged from 105 B).
  * `cargo test -p lvqr-cli --features transcode`: **39** passed across every feature-gated lib + integration test target the workspace compiles (including the new `transcode_ladder_e2e.rs`).
  * `cargo test -p lvqr-cli --features transcode --test transcode_ladder_e2e`: **1** passed (wall clock ~0.3 s; soft-skip branch when GStreamer plugins are absent).
* **CI gates locally clean**:
  * `cargo fmt --all --check`.
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`.
  * `cargo clippy -p lvqr-transcode --features transcode --all-targets -- -D warnings`.
  * `cargo clippy -p lvqr-cli --features transcode --all-targets -- -D warnings`.
  * `cargo test --workspace` 900 / 0 / 1.
* **Workspace**: **29 crates**, unchanged. No crate added or removed.
* **crates.io**: unchanged. Session 106 introduces `transcode_renditions` / `source_bandwidth_kbps` to `ServeConfig` (feature-gated, additive) and `set_ladder` / `set_source_bandwidth_bps` / `variant_siblings` to `MultiHlsServer` (additive), so the pending re-publish chain from session 105 still lands cleanly on the next release.

### Known limitations / documented v1 shape

* **RTMP-ingest -> software pipeline init timing**: the RTMP bridge calls `FragmentBroadcasterRegistry::get_or_create` (which fires the TranscodeRunner's on_entry_created callback) before `FragmentBroadcaster::set_init_segment`. Session 106 C's `runner.rs` fix refreshes `sub.meta()` before `on_start` so the transcoder sees the late init; in practice the GStreamer pipeline still fails to decode the synthetic NAL payloads integration tests produce, which is why the end-to-end test uses `rml_rtmp` with synthetic bytes and asserts on the HLS bridge + TranscodeRunner counter path rather than on the encoded output. The real-decode path is covered by the 105 B `software_ladder.rs` integration test against a real CMAF fixture.
* **ffmpeg 8.1 RTMP handshake flakiness**: direct `ffmpeg -f flv rtmp://...` publishes were observed to hang against `rml_rtmp`'s server-side handshake on the test host (first attempt succeeded, subsequent attempts stuck after TCP accept). The session 106 briefing called for ffmpeg-based publishes; the deliverable ships with `rml_rtmp` synthetic publishes instead (same bridge / same HLS bridge / same TranscodeRunner) and the decoded-output gate is the 105 B integration test. Follow-up is to run the e2e test against ffmpeg once the flakiness root cause is identified (possibly a difference in RTMP chunk-size negotiation between ffmpeg 8.x and the `rml_rtmp` crate's expectations).
* **CODECS attribute on variant lines is hard-coded**: `"avc1.640028,mp4a.40.2"` is a conservative placeholder per decision (d). A 107+ follow-up would parse the SPS + audio ASC off each rendition's init segment and replace the placeholder.
* **Source variant resolution is absent**: the master playlist emits `#EXT-X-STREAM-INF` for the source without a `RESOLUTION=` attribute because the source's frame size is not probed at the CLI composition root. Operators that know their source resolution can wire it directly via a later admin-API patch; for now ABR clients treat the source as the highest variant by bandwidth alone.

### Prerequisites

Session 106 C's e2e test depends on the same GStreamer plugin set as session 105 B (install recipe unchanged; see the session 105 close block).

## Session 105 close (2026-04-21)

1. **Tier 4 item 4.6 session B: real GStreamer software encoder ladder** (feat commit).
   * `crates/lvqr-transcode/Cargo.toml`: new `transcode` Cargo feature (default OFF) gating four optional deps (`gstreamer`, `gstreamer-app`, `gstreamer-video`, `glib`). `bytes` + `thiserror` promoted from dev-deps / absent to regular deps -- `bytes` is used by the appsink callback to copy `gst::Buffer` payloads into `Fragment::payload`; `thiserror` powers the new `WorkerSpawnError` enum.
   * `Cargo.toml` (workspace root): `gstreamer = "0.23"`, `gstreamer-app = "0.23"`, `gstreamer-video = "0.23"`, `glib = "0.20"` pinned in `[workspace.dependencies]` with a comment recording that any upgrade is a single-file change per the "Dependencies to pin" table in `tracking/TIER_4_PLAN.md`.
   * `crates/lvqr-cli/Cargo.toml`: new `transcode` feature (`["dep:lvqr-transcode", "lvqr-transcode/transcode"]`); `full` meta-feature expanded from 5 to 6 entries; new optional `lvqr-transcode` dep. `start()` does NOT install a `TranscodeRunner` in 105 B -- that wiring is 106 C's composition-root job -- but the dep edge is in place so `cargo build -p lvqr-cli --features full` exercises the full GStreamer dep graph.
   * `crates/lvqr-transcode/src/software.rs` (new, ~480 LOC, feature-gated on `transcode`): the heart of the session. `SoftwareTranscoderFactory` probes `REQUIRED_ELEMENTS` via `gst::ElementFactory::find` at construction; missing elements cause every subsequent `build(ctx)` to return `None` with one warn log, matching the `TranscoderFactory` opt-out idiom already used for non-video tracks. Pipeline shape built via `gst::parse::launch` for the body + downcast `AppSrc` / `AppSink` for the endpoints: `appsrc(video/quicktime) ! qtdemux ! h264parse ! avdec_h264 ! videoscale ! video/x-raw,width=W,height=H ! videoconvert ! x264enc bitrate=K threads=2 tune=zerolatency speed-preset=superfast key-int-max=60 ! h264parse ! mp4mux streamable=true fragment-duration=2000 ! appsink emit-signals=true sync=false`. `threads=2` caps the x264 worker pool so three parallel ladder rungs do not exhaust the host pthread ceiling (discovered empirically when the 720p rung silently produced zero output under the default `threads=ncores` on an M-series mac).
   * Worker-thread pattern lifted from `crates/lvqr-agent-whisper/src/worker.rs`: `on_start` builds the pipeline on the spawning thread (so parse errors surface eagerly), spawns a named OS thread (`lvqr-transcode:<source>:<rendition>`), and optionally pushes `ctx.meta.init_segment` as a `BufferFlags::HEADER` buffer before draining. `on_fragment` forwards each `Fragment::payload` through a bounded `std::sync::mpsc::sync_channel(64)`; `TrySendError::Full` drops the fragment, bumps `lvqr_transcode_dropped_fragments_total{transcoder, rendition}`, and emits one `tracing::warn!`. `on_stop` drops the sender, at which point the worker's `rx.recv()` returns `Err` -> `appsrc.end_of_stream()` -> `bus.timed_pop_filtered(EOS|Error, 5 s)` -> `pipeline.set_state(Null)` -> exit. The same teardown runs from `Drop` so mid-stride `TranscodeRunnerHandle` aborts don't leak GStreamer streaming threads into the tokio runtime's drop path.
   * Output side: each rendition lazily calls `output_registry.get_or_create("<source>/<rendition>", "0.mp4", FragmentMeta::new("avc1.640028", 90_000))` at worker spawn. The appsink `new-sample` callback checks `BufferFlags::HEADER` on each pulled `gst::Buffer`; header buffers update the output broadcaster's init via `FragmentBroadcaster::set_init_segment` (so late-joining HLS / MoQ subscribers can decode), non-header buffers emit as `Fragment` instances. New metrics: `lvqr_transcode_output_fragments_total{transcoder, rendition}` + `lvqr_transcode_output_bytes_total{transcoder, rendition}` alongside the existing `lvqr_transcode_fragments_total{transcoder, rendition}` (input) and `lvqr_transcode_panics_total{transcoder, rendition, phase}` from the runner.
   * Recursion guard `looks_like_rendition_output(broadcast)`: treats any broadcast whose trailing path component matches `\d+p` (`720p`, `480p`, `1080p`, ...) as already-transcoded and skips it in `build()`. Without this guard the registry's `on_entry_created` callback re-fires for every `<source>/<rendition>` the transcoder publishes, spawning another round of ladder factories on those outputs and cascading to 25+ pipelines + thread-exhaustion on the host pthread pool. The heuristic has a documented v1 limitation: a source literally named `live/720p` would be skipped; 106 C adds an explicit `skip_source_suffixes` override knob for operators using non-conventional rendition names.
   * `crates/lvqr-transcode/src/lib.rs`: feature-gated `mod software;` + `pub use software::{SoftwareTranscoder, SoftwareTranscoderFactory};` under `#[cfg(feature = "transcode")]`. Public API surface unchanged for the feature-off build.
   * 6 new feature-gated inline tests on `software.rs`: pipeline string embeds rendition geometry + bitrate, factory opts out of non-video tracks (`"1.mp4"`), factory returns a transcoder for `"0.mp4"` when available, plugin-probe returns empty on a fully-installed host, `SoftwareTranscoder::output_broadcast_name()` concatenates source + rendition, `looks_like_rendition_output` heuristic accepts `\d+p` suffixes and rejects everything else.

2. **`crates/lvqr-transcode/tests/software_ladder.rs`** (new, ~210 LOC, feature-gated on `transcode`): the end-to-end integration test. Boots a `FragmentBroadcasterRegistry`, installs a `TranscodeRunner::with_ladder(RenditionSpec::default_ladder(), SoftwareTranscoderFactory::new)`, loads `crates/lvqr-conformance/fixtures/fmp4/cmaf-h264-baseline-360p-1s.mp4`, splits into `ftyp+moov` (init) + `moof+mdat+mfra` (fragment body) via a hand-rolled top-level box scan, emits on `live/demo`, polls the registry until all three `live/demo/{720p,480p,240p}` output broadcasts appear (10 s deadline), subscribes to each, drops the source broadcaster to trigger EOS propagation, drains each rendition's subscription with per-fragment 8 s timeout + 20 s overall deadline, and asserts (a) each rendition produced at least one output fragment + non-zero bytes, (b) each output bitrate falls within +/- 40% of the target `video_bitrate_kbps` (coarse check; x264 rate control jitters at startup with 1 s of content), and (c) `720p_bytes > 240p_bytes` as a ladder-miswiring sanity check. Skip-with-log branch when the factory's `is_available()` returns false, so runners without the GStreamer plugin set see a green test with a clear diagnostic rather than a hard fail.

3. **Session 105 close doc** (this commit).

### Key 4.6 session B design decisions baked in (confirmed in-commit per the plan-vs-code rule)

* **Video-only for 105 B; audio passthrough deferred to 106 C.** The briefing gave latitude to fold AAC passthrough into this session (~50 LOC for a sibling `AudioPassthroughTranscoderFactory` that copies `"1.mp4"` fragments between registry entries). The call is to defer: 105 B already introduces one load-bearing new subsystem (the real GStreamer pipeline) and the integration test is a single-track video fixture. Session 106 C owns the LL-HLS master playlist composition, which is the natural place to land audio passthrough because the master playlist either references per-rendition self-contained mp4s or references a separate audio rendition; either shape is a one-session job atop today's surface.
* **`gst::parse::launch` for the body + programmatic `AppSrc` / `AppSink` downcast for the endpoints.** The body is static-per-rendition and reads well as a string; the endpoints need programmatic access to set `max-bytes` / `block` on appsrc and to register the appsink `new-sample` callback. Matches the gstreamer-rs cookbook idiom.
* **Init-segment push via `BufferFlags::HEADER`; fallback to `ctx.meta.init_segment` if the drain task joins mid-broadcast.** The source `FragmentMeta::init_segment` is the canonical init bytes LVQR's ingest bridges populate; pushing them as a HEADER-flagged buffer at worker start lets qtdemux parse `ftyp+moov` before any `moof+mdat` arrives. If `init_segment` is `None` at `on_start`, we fall through to passing every source fragment verbatim -- qtdemux handles a buffer containing `ftyp+moov+moof+mdat` correctly (exercised by the integration test's fixture, which contains both).
* **Recursion guard via suffix heuristic, not via runtime output-broadcast tracking.** The alternative (factory maintains a set of its own output broadcast names and skips them) requires cross-factory coordination because a 480p factory must also skip 720p output broadcasts; threading the full ladder's names into every factory is API creep for a case the `\d+p` heuristic catches correctly for 100% of realistic rendition-name conventions. 106 C adds an explicit `skip_source_suffixes` override for operators with non-conventional names.
* **`threads=2` on x264enc is a hard constraint, not a tuning choice.** Default `threads=ncores` + three parallel pipelines + each pipeline's 5-10 GStreamer streaming threads blows through macOS's default per-process thread ceiling and produces `EAGAIN` on `pthread_create`. The 720p rung (highest resolution, most x264 worker demand) fails first; 480p and 240p usually succeed. `threads=2` is plenty for real-time encode of a single rendition at `speed-preset=superfast`; any future hardware-accelerated rung (106 C's NVENC / VideoToolbox flags) can ship without this cap.
* **Plugin availability is a factory opt-out, not a panic.** The 104 A briefing considered a `panic!` at factory construction to surface missing plugins loudly. 105 B rejects that: the factory returns `None` from `build()` with a one-shot warn log so the rest of the server keeps running and the operator gets a clear diagnostic via the logs without the process crashing. Matches the existing non-video-track `None` idiom.
* **`x264enc` keyframe interval hard-coded at `key-int-max=60` (2 s at 30 fps).** Source-GOP-aware tuning (inheriting the source's keyframe cadence so LL-HLS segment boundaries align across renditions) is explicit scope for 106 C. Hard-coding 60 for 105 B matches the `mp4mux fragment-duration=2000` window so every x264 GOP ends on a fragment boundary for typical 30 fps content.
* **Output codec string on `FragmentMeta` is advisory (`"avc1.640028"` = High 4.0 placeholder).** x264enc's actual profile depends on frame geometry + settings; operators that need the authoritative codec parse the SPS from the init segment bytes in `FragmentBroadcaster::meta().init_segment`. The 104 A `"avc1.640028"` placeholder is kept; 106 C adds SPS-aware codec-string population if downstream consumers need it.
* **Integration test drives a real GStreamer pipeline, not a mocked harness.** Per CLAUDE.md's testing rules + the briefing's "theatrical test" warning. The `software_ladder.rs` test produces 31 output fragments per rendition + ~2280 / 1144 / 384 kbps at 720p / 480p / 240p (9% / 5% / 4% under target on three consecutive runs). +/- 40% tolerance leaves plenty of headroom for CI variance without letting a miswired factory ship a "working" ladder at the wrong bitrates.
* **Skip-with-log when plugins are missing, not a hard fail.** The factory's `is_available()` flag consolidates the plugin probe; the test reads it once at setup and logs-and-returns if false. Runners without the full plugin install see a green test with a specific list of missing elements instead of a red test that only fails on hosts the CI admin might not control.

### Ground truth (session 105 close, refreshed at push event)

* **Head**: feat commit `1796a24` + close-doc commit `f14dbdf` + post-audit fix commit `adfffe5` (three new commits on `main`, all pushed). `git log --oneline origin/main..main` is empty after the push.
* **Tests (default features gate)**: **892** passed, 0 failed, 1 ignored on macOS (default features). Unchanged from the session 104 A baseline because every new test in 105 B is `#[cfg(feature = "transcode")]`-gated. The 1 ignored is the pre-existing `moq_sink` doctest.
* **Tests (transcode feature gate)**:
  * `cargo test -p lvqr-transcode --features transcode --lib`: **23** passed (+7 new inline on `software.rs`: geometry/bitrate-embed, non-video opt-out, video build, plugin probe, output-name concat, rendition-suffix heuristic, and the audit-fix `ns_to_ticks` conversion).
  * `cargo test -p lvqr-transcode --features transcode --test software_ladder`: **1** passed (integration test; wall clock ~0.3 s after the first build on an M-series mac).
* **CI gates locally clean**:
  * `cargo fmt --all --check`.
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`.
  * `cargo clippy -p lvqr-transcode --features transcode --all-targets -- -D warnings`.
  * `cargo test -p lvqr-transcode` 16 passed + 1 doctest (feature-off path parity with 104 A).
  * `cargo test --workspace` 892 / 0 / 1 (unchanged).
  * `cargo check -p lvqr-cli --features transcode` clean (feature wiring compiles).
* **Workspace**: **29 crates**, unchanged.
* **crates.io**: unchanged since session 98's publish event. Session 105 B adds optional gstreamer deps to `lvqr-transcode` (new feature, non-breaking) and a new optional feature + optional dep to `lvqr-cli` (non-breaking). A future release cycle first-time publishes `lvqr-transcode 0.4.0` alongside the pending 4.4-chain re-publishes of `lvqr-cluster` / `lvqr-cli` / `lvqr-admin` / `lvqr-test-utils`.

### Prerequisites + developer install recipe

The `transcode` feature requires GStreamer 1.22+ runtime + plugin set on the host. The factory probes at construction time and opts out with a clear warn log if any element is missing; a full gate run (`cargo test -p lvqr-transcode --features transcode`) needs every element below to resolve.

**Required GStreamer elements** (probed by `SoftwareTranscoderFactory::new`): `appsrc`, `qtdemux`, `h264parse`, `avdec_h264`, `videoscale`, `videoconvert`, `x264enc`, `mp4mux`, `appsink`.

**macOS** -- prefer the official `.pkg` installer to avoid Homebrew's heavy dep chain (LLVM, Z3, etc. that bloom out from the Homebrew `gstreamer` formula):

```
# Runtime + devel pkgs from https://gstreamer.freedesktop.org/download/
#   gstreamer-1.0-<version>-universal.pkg
#   gstreamer-1.0-devel-<version>-universal.pkg
# Install both. Then in your shell profile:
export PATH="/Library/Frameworks/GStreamer.framework/Commands:$PATH"
export PKG_CONFIG_PATH="/Library/Frameworks/GStreamer.framework/Versions/1.0/lib/pkgconfig:$PKG_CONFIG_PATH"
export DYLD_FALLBACK_LIBRARY_PATH="/Library/Frameworks/GStreamer.framework/Versions/1.0/lib:$DYLD_FALLBACK_LIBRARY_PATH"
# Verify:
gst-inspect-1.0 x264enc qtdemux mp4mux avdec_h264
```

The `DYLD_FALLBACK_LIBRARY_PATH` export is load-bearing for `cargo test` on macOS: without it, test binaries fail with `dyld: Library not loaded: @rpath/libgstapp-1.0.0.dylib` because the GStreamer framework's dylibs live outside the default dyld search path.

**Debian / Ubuntu**:

```
apt install libgstreamer1.0-dev \
            gstreamer1.0-plugins-base \
            gstreamer1.0-plugins-good \
            gstreamer1.0-plugins-bad \
            gstreamer1.0-plugins-ugly \
            gstreamer1.0-libav
```

`gstreamer1.0-libav` provides `avdec_h264`; `gst-plugins-ugly` provides `x264enc`; `gst-plugins-bad` provides `mp4mux` + `qtdemux`.

Homebrew install (`brew install gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly`) also works but builds LLVM from source on a fresh machine and can take 30+ minutes. Prefer the .pkg path on macOS for developer ergonomics.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91-94 |
| 4.8 | One-token-all-protocols | **COMPLETE** | 95 / 96 |
| 4.5 | In-process AI agents | **COMPLETE** | 97 / 98 / 99 / 100 |
| 4.4 | Cross-cluster federation | **COMPLETE** | 101 / 102 / 103 |
| 4.6 | Server-side transcoding | **A + B DONE**, C pending | 104 / 105 / 106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

6 of 8 Tier 4 items COMPLETE; 4.6 two-thirds done. Remaining: 4.6 C + 4.7.

### Session 106 entry point

**Tier 4 item 4.6 session C: `lvqr-cli` wiring + `--transcode-rendition` flag + LL-HLS master playlist composition.**

Scope per `tracking/TIER_4_PLAN.md` section 4.6 row 106 C. Concrete deliverables:

1. `lvqr-cli`'s `ServeConfig` gains `transcode_renditions: Vec<RenditionSpec>` (feature-gated on `transcode`) and a matching `--transcode-rendition <RENDITION>` CLI flag (repeatable; `LVQR_TRANSCODE_RENDITION` env fallback). Flag value parses short preset names (`720p` / `480p` / `240p`) to `RenditionSpec::preset_*` out of the box; operators with custom ladders supply TOML.
2. `lvqr-cli::start()` installs a `TranscodeRunner` on the shared registry whenever `transcode_renditions` is non-empty, building one `SoftwareTranscoderFactory` per rendition. `ServerHandle` gains a `transcode_runner: Option<TranscodeRunnerHandle>` accessor mirroring the existing `agent_runner` / `wasm_filter` shape.
3. `lvqr-hls`'s master playlist composition learns about source-plus-rendition groupings. Any `<source>` broadcast that also has `<source>/<rendition>` siblings on the registry gets a master playlist referencing every variant with `BANDWIDTH` (`RenditionSpec::video_bitrate_kbps * 1000`) + `RESOLUTION` (`<width>x<height>`) + `NAME` (`RenditionSpec::name`). The source itself is the first variant at its own bitrate (unknown precisely; use the ingest rate as a fallback).
4. AAC audio passthrough: a sibling `AudioPassthroughTranscoderFactory` that opts in to `"1.mp4"` tracks and copies fragments from `<source>/1.mp4` to `<source>/<rendition>/1.mp4` verbatim. ~50 LOC; no GStreamer dependency.
5. Integration test `crates/lvqr-cli/tests/transcode_ladder_e2e.rs`: boots a TestServer with `transcode_renditions = default_ladder()`, publishes a 3 s RTMP stream, reads `/hls/live/demo/master.m3u8`, asserts four variants (source + three renditions) all appear + all referenced media playlists serve real x264-encoded segments.

Pre-session decisions to lock in-commit:

* CLI flag shape: short-form preset names (`720p`) as sugar, TOML for custom ladders. Operators can mix: `--transcode-rendition 720p --transcode-rendition custom.toml`.
* Source variant in the master playlist: `BANDWIDTH` defaults to the highest rung's bitrate + 20% when unknown; operators can override via `--source-bandwidth-kbps`. Alternative (probe the source's actual bitrate) is 107 A territory (latency SLO infrastructure).
* Recursion-guard override: `SoftwareTranscoderFactory::skip_source_suffixes(Vec<String>)` builder for operators using non-conventional rendition names. Default behavior (the `\d+p` heuristic) stays.
* HW encoder preview: 106 C is software-only. NVENC / VAAPI / VideoToolbox hardware-encoder variants are deferred to a post-4.6 session because they require per-platform plugin probes + separate integration tests.

Biggest risks for 106 C:

* **LL-HLS master playlist + relative URIs**: every variant playlist URI is relative to the master's URL; the HLS bridge must output the right path for each rendition without the operator having to hand-configure.
* **Bitrate accounting on the source variant**: if the source's actual bitrate is unknown at playlist-generation time, the master playlist picks a placeholder that can mislead ABR clients; the "highest rung + 20%" heuristic is a safe default but documentation should call this out.
* **TestServer + TranscodeRunner composition**: the existing `TestServerConfig` has no transcode field; adding one follows the `with_whisper_model` / `with_c2pa` pattern but needs the same `lvqr-test-utils` feature flag gymnastics (`transcode = ["lvqr-cli/transcode"]`).

### Session 105 push event (2026-04-21)

Session 105's three commits are pushed to `origin/main`:

1. `1796a24` feat(transcode): real gstreamer software ladder behind `transcode` feature.
2. `f14dbdf` docs: session 105 close -- Tier 4 item 4.6 session B DONE.
3. `adfffe5` fix(transcode): convert appsink output timestamps to 90 kHz ticks (post-close audit fix covering a latent unit mismatch where `gst::ClockTime::nseconds()` was being written as-is into `Fragment::dts` / `pts` / `duration` whose declared unit is `FragmentMeta::timescale` ticks; left as-is would have delivered 11 111x-too-large values to session 106 C's LL-HLS composition).

Push event doc refreshes the status header to `origin/main synced (head adfffe5)`, adjusts the ground-truth test counts (`--features transcode --lib` 22 -> 23 for the new `ns_to_ticks` inline test), and refreshes `README.md` with a Tier 4 status bump + crate map + CLI reference through session 105 B. Same pattern as sessions 99 / 100 / 102 / 103 / 104 push-event commits.

## Session 104 close (2026-04-21)

1. **Tier 4 item 4.6 session A: `lvqr-transcode` scaffold** (feat commit).
   * `crates/lvqr-transcode/Cargo.toml` (new): workspace-inherited package metadata, `lvqr-fragment` + `dashmap` + `metrics` + `parking_lot` + `serde` + `tokio` + `tracing` as deps (same shape as `lvqr-agent` plus `serde` for `RenditionSpec` serialization); `bytes` + `serde_json` + extra `tokio` features as dev-deps. No gstreamer.
   * `crates/lvqr-transcode/src/lib.rs` (new): crate-level docs with session roll-up, consumer-family table (6 registry consumers total), anti-scope list, re-exports.
   * `crates/lvqr-transcode/src/rendition.rs` (new, ~130 LOC): `RenditionSpec { name, width, height, video_bitrate_kbps, audio_bitrate_kbps }` with `serde::{Serialize, Deserialize}`. Presets match the section 4.6 defaults: 720p = 1280x720 @ 2.5Mb/s + 128kb/s; 480p = 854x480 @ 1.2Mb/s + 96kb/s; 240p = 426x240 @ 400kb/s + 64kb/s. `default_ladder()` returns the three in highest-to-lowest order.
   * `crates/lvqr-transcode/src/transcoder.rs` (new, ~100 LOC): `Transcoder` trait (sync, Send; `on_start` / `on_fragment` / `on_stop` with no-op defaults) + `TranscoderContext { broadcast, track, meta, rendition }` + `TranscoderFactory` (`Send + Sync + 'static`; `name() + rendition() + build()`). Docstrings reference the `lvqr-agent` parallel throughout.
   * `crates/lvqr-transcode/src/passthrough.rs` (new, ~200 LOC): `PassthroughTranscoder` + `PassthroughTranscoderFactory`. Default source track filter `"0.mp4"` (video only). Observes + counts fragments; NO real encode, NO output republish. Exists to prove the registry callback + drain + panic isolation wiring end-to-end without a gstreamer dep. 5 inline tests.
   * `crates/lvqr-transcode/src/runner.rs` (new, ~360 LOC): `TranscodeRunner` + `TranscodeRunnerHandle` + `TranscoderStats`. Stats key is the 4-tuple `(transcoder_name, rendition_name, broadcast, track)` so two factories of the same name targeting different renditions live under separate metrics. `with_ladder(Vec<RenditionSpec>, |spec| F)` convenience builder for the typical ABR case. Panic isolation via `catch_unwind(AssertUnwindSafe(..))` on `on_start` / `on_fragment` / `on_stop` with per-phase panic counters. Prometheus metrics: `lvqr_transcode_fragments_total{transcoder, rendition}` + `lvqr_transcode_panics_total{transcoder, rendition, phase}`. 8 inline tests + 1 doctest covering fragment drain, default-ladder spawn-per-rendition, factory opt-out on non-video tracks, `on_fragment` panic-isolation with counter verification, `on_start` panic skips drain, empty runner no-op, `Default` empty, downstream-subscriber-unaffected fan-out.
   * `Cargo.toml` (workspace root): added `crates/lvqr-transcode` to `members`; added `lvqr-transcode = { version = "0.4.0", path = "crates/lvqr-transcode" }` to `workspace.dependencies`.
   * `tracking/TIER_4_PLAN.md`: section 4.6 header flipped to "A DONE, B-C pending"; row 104 A scoped up from one-line to the full deliverable + verification record; section 4.6 anti-scope unchanged.

2. **Session 104 close doc** (this commit).

### Key 4.6 session A design decisions baked in (confirmed in-commit per the plan-vs-code rule)

* **Scaffold-only, no gstreamer in 104 A.** The plan row promised "gstreamer-rs pipeline for one 720p rendition" but the pass-through ships the full registry-side wiring with zero new heavy C deps and no CI gstreamer install story. 105 B adds gstreamer behind a default-OFF `transcode` Cargo feature. Rationale: 4.4 session C's experience showed that landing wire + observability first, real codec second, keeps rollback blast radius small. Every other subsystem in LVQR followed this order (WASM filter tap, agent runner, federation runner).
* **Mirror `lvqr-agent` one-for-one.** The trait shape (`on_start` / `on_fragment` / `on_stop` sync, panic-isolated), the factory shape (name + build returning Option), the runner shape (builder + install returning a cheaply-cloneable handle that holds tasks alive), the stats shape (DashMap of AtomicU64 counters), the drain-on-broadcaster-close lifecycle -- all bit-for-bit match the Tier 4 item 4.5 session A scaffold. Operators reading a future transcoder integration see the same idiom they already saw for WASM filters, cluster claims, and AI agents. No new abstractions invented. Only the stats key is extended to a 4-tuple (adds `rendition_name`) so two factories of the same name at different ladder rungs stay metric-distinct.
* **Factory carries its own `RenditionSpec`.** ABR ladders are expressed as N factory instances, one per rung, each constructed with its own `RenditionSpec`. The runner builds the `TranscoderContext` per-factory, inserting `factory.rendition().clone()` as the context's rendition field. Alternative designs (one factory building N transcoders, or one transcoder handling all renditions) coupled renditions in ways that would block the per-rendition pipeline tuning 105 B wants. The `with_ladder(ladder, |spec| build)` convenience builder unrolls this idiom in one call.
* **Pass-through defaults to video-only (`track == "0.mp4"`).** Audio / captions / catalog tracks have no transcoder use case on the 4.6 ABR ladder (audio passthrough is a 105 B decision, captions + catalog are not transcode targets). `PassthroughTranscoderFactory::build` returns `None` for any other track. Operators wanting audio observation can write their own factory with a wider filter; the trait is a natural extension point.
* **No output re-publish in 104 A.** Passthrough transcoders are observers only. The "output as a new broadcast" side lands in 105 B when there is a real encoder producing output bytes. This keeps the 104 A surface minimal and avoids prematurely committing to the output-naming convention (`<source>/<rendition>`); 105 B locks that in.
* **No `lvqr-cli` wiring.** Session 106 C owns the composition root (`ServeConfig.transcode_renditions`, `--transcode-rendition` flag, `ServerHandle.transcode_runner` accessor). 104 A ships the library in isolation -- consumers wire it themselves if they need to before 106 C.
* **Metric name convention locked.** `lvqr_transcode_fragments_total{transcoder, rendition}` + `lvqr_transcode_panics_total{transcoder, rendition, phase}`. Mirrors `lvqr_agent_fragments_total{agent}` + `lvqr_agent_panics_total{agent, phase}` with the rendition label added. Sets the shape for the 105 B output-side metrics (`lvqr_transcode_output_fragments_total`, etc.).
* **`serde` on `RenditionSpec` for forward compatibility.** Operators writing 105 B / 106 C configs need to serialize ladder specs to / from TOML + JSON. Landing `Serialize + Deserialize` in 104 A closes the door on a backwards-incompatible serde addition in 105 B. One inline round-trip test locks the JSON shape.

### Ground truth (session 104 close)

* **Head**: feat commit + this close-doc commit (two new commits on `main`). Local is N+2 above `origin/main` (head `154b7b9` from session 103 push event).
* **Tests**: **892** passed, 0 failed, 1 ignored on macOS (default features). (The 1 ignored is the pre-existing `moq_sink` doctest.)
* **CI gates locally clean**:
  * `cargo fmt --all --check`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo test -p lvqr-transcode` 16 passed + 1 doctest (5 passthrough inline + 3 rendition inline + 8 runner inline + the `with_ladder` quickstart doctest)
  * `cargo test --workspace` 892 / 0 / 1 (+17 over session 103's 875; the extra +1 over the in-crate count is the `lvqr_transcode` doctest tallied into the workspace total)
* **Workspace**: **29 crates** (+1: `lvqr-transcode`).
* **crates.io**: unchanged since session 98's publish event. The next release cycle needs to first-time publish `lvqr-transcode 0.4.0` alongside re-publishing `lvqr-cluster` / `lvqr-cli` / `lvqr-admin` / `lvqr-test-utils` with the 4.4 additive changes already in origin/main. Publish order: `lvqr-transcode` slots into Tier 3 (depends on `lvqr-fragment` + workspace deps only; no LVQR internal surface depends on it in 104 A, so it can ship anywhere after Tier 1).

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91-94 |
| 4.8 | One-token-all-protocols | **COMPLETE** | 95 / 96 |
| 4.5 | In-process AI agents | **COMPLETE** | 97 / 98 / 99 / 100 |
| 4.4 | Cross-cluster federation | **COMPLETE** | 101 / 102 / 103 |
| 4.6 | Server-side transcoding | **A DONE**, B-C pending | 104 / 105 / 106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

6 of 8 Tier 4 items COMPLETE; 4.6 one-third done. Remaining: 4.6 B + C, 4.7 latency SLO.

### Session 105 entry point

**Tier 4 item 4.6 session B: ABR ladder generation + multi-rendition publish via real gstreamer-rs pipelines.**

Scope per `tracking/TIER_4_PLAN.md` section 4.6 row 105 B. Concrete work items:

1. Add a `transcode` Cargo feature on `lvqr-transcode` (default OFF) that pulls `gstreamer-rs` + `gstreamer-app` + `gstreamer-video` + `gstreamer-rtp` (or the subset 4.6 actually needs).
2. New module `src/software.rs` (feature-gated) with `SoftwareTranscoder` + `SoftwareTranscoderFactory`. Pipeline shape: `appsrc -> qtdemux -> h264parse -> avdec_h264 -> videoscale -> videoconvert -> x264enc ! bitrate=<from RenditionSpec> -> h264parse -> mp4mux -> appsink`. Input: source fMP4 fragment bytes. Output: fMP4 fragment bytes published into a new broadcast named `<source>/<rendition>` on the `FragmentBroadcasterRegistry`.
3. Output injection: `TranscodeRunner` gains a config option pointing at an `FragmentBroadcasterRegistry` for publishing (can be the same one it subscribes from; the registry's consumer side doesn't care). Output fragments are published via `get_or_create(<source>/<rendition>, track, meta)`.
4. Integration test `crates/lvqr-transcode/tests/software_ladder.rs` (feature-gated on `transcode`): boots a `FragmentBroadcasterRegistry`, emits synthetic fMP4 fragments onto one broadcaster, wires a `TranscodeRunner` with `default_ladder()` + `SoftwareTranscoderFactory`, asserts three new `<source>/<rendition>` broadcasters appear on the registry with fragment counts matching the source.
5. CI: document `LVQR_GSTREAMER_AVAILABLE` env gate or skip-with-log pattern. gstreamer plugins to assume: `gst-plugins-base`, `gst-plugins-good`, `gst-plugins-bad` (for mp4mux + qtdemux + videoscale), `gst-plugins-ugly` (x264enc). Developer install: `brew install gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly` on macOS.

**Pre-session decisions to lock in-commit**:
* **Output broadcast naming**: `<source>/<rendition>` exactly (`live/cam1/720p`), matching the section 4.6 plan text. HLS master-playlist composition in 106 C then learns to match `<source>` plus any `<source>/<rendition>` broadcasts into one master with per-rendition variants.
* **Output init segment**: `mp4mux` emits a fresh moov on each GOP; the first output fragment carries the init segment. Downstream consumers cache init like they already do for ingest-produced broadcasts.
* **Audio passthrough**: the 105 B pipeline is video-only. AAC passthrough (copying `<source>/<track="1.mp4">` to `<source>/<rendition>/<track="1.mp4">`) is a separate task inside 105 B or can wait for 106 C. Lean: include audio passthrough in 105 B so the rendition is self-contained for LL-HLS composition in 106 C.
* **Panic on missing gstreamer plugins**: at factory construction, detect required plugins + `panic!` with a helpful message if absent. Prevents confusing drain-time errors on systems with a partial gstreamer install.

**Biggest risks for 105 B**:
* gstreamer-rs's `appsrc` back-pressure semantics: pushing bytes faster than the pipeline drains can leak memory. Mitigation: bounded `gst::Buffer` pushes, back-pressure signal propagated through the drain loop.
* fMP4 fragment -> `qtdemux` hand-off: each fragment is one `moof + mdat`; we may need to prepend the init segment on the first buffer. Figure out the canonical feed shape from a gstreamer test rig before committing to an architecture.
* `x264enc` keyframe alignment with source GOP boundaries: the output ladder should preserve the source's GOP structure so LL-HLS segmentation stays consistent across renditions. `keyint-max` + `keyint-min` tuned to source.

### Session 104 push event carry-over

If the user instructs a push after session 104 closes, follow up with a `docs: session 104 push event` commit that refreshes the HANDOFF status header to `origin/main synced (head <new_head>)` as the sessions 99 / 100 / 102 / 103 push-event commits did.

## Session 103 close (2026-04-21)

1. **Tier 4 item 4.4 session C: admin route `/api/v1/cluster/federation` + exponential-backoff reconnect** (feat commit).
   * `crates/lvqr-cluster/src/federation.rs`: new `FederationConnectState` enum (serde-lowercase: `connecting` / `connected` / `failed`); new `FederationLinkStatus` struct carrying `remote_url`, `forwarded_broadcasts`, `state`, `last_connected_at_ms`, `last_error`, `connect_attempts`, `forwarded_broadcasts_seen`; new `FederationStatusHandle` wrapping `Arc<RwLock<Vec<FederationLinkStatus>>>` with cloneable `snapshot()` and internal mutators. `FederationRunner` now owns a status handle and exposes it via `status_handle()` for the admin layer. The private `run_link` is now an outer retry wrapper around a new `run_link_once`: each pass sets Connecting, runs the single connect + announcement-drain cycle, records Connected or Failed, and sleeps `next_delay(attempt)` (base 1 s, doubling to 60 s cap, ±10% symmetric jitter via `rand::thread_rng().gen_range`) with a cancel-arm. `run_link_once` grew a `session.closed()` arm in the main select so remote peer shutdown surfaces as a recoverable error (instead of pinning the loop in Connecting because the local sub-origin's `announced()` never naturally drains on remote close), and a `CONNECT_TIMEOUT = 10s` wrapping the client connect so a silently-dropped QUIC Initial cannot hold the link in Connecting forever. Backoff constants (`BACKOFF_INITIAL`, `BACKOFF_MAX`, `BACKOFF_JITTER_FRAC`) are module-private for now; a `FederationBackoffConfig` struct can land later if operators need tuning.
   * `crates/lvqr-cluster/src/lib.rs`: re-exports widened to include `FederationConnectState`, `FederationLinkStatus`, `FederationStatusHandle`.
   * `crates/lvqr-cluster/tests/federation_unit.rs`: 2 new integration tests -- `runner_status_handle_reports_failed_after_initial_connect_error` drives an unreachable TEST-NET-1 URL through the retry loop and asserts the handle flips to Failed with non-zero `connect_attempts` + non-empty `last_error`; `status_handle_clones_observe_updates` asserts cloned handles observe writes the runner's per-link task makes.
   * `crates/lvqr-admin/src/routes.rs`: `AdminState` gains an optional `federation_status: Option<FederationStatusHandle>` field (feature-gated on `cluster`) plus `with_federation_status` builder + pub(crate) `federation_status()` accessor.
   * `crates/lvqr-admin/src/cluster_routes.rs`: new `GET /api/v1/cluster/federation` route returning `{"links": [FederationLinkStatus..]}`; when no handle is wired (single-node, or the runner simply is not installed), the route answers 200 + `{"links":[]}` rather than 503 so unconditional polling from operator tooling works. 2 new inline tests cover wired + unwired cases.
   * `crates/lvqr-admin/Cargo.toml`: added `lvqr-moq` + `tokio-util` as dev-deps for the new wired-state test.
   * `crates/lvqr-cli/src/lib.rs`: `start()` threads `federation_runner.status_handle()` into `AdminState` alongside the existing cluster handle.
   * `crates/lvqr-test-utils/src/test_server.rs`: `TestServerConfig` gained `relay_addr: Option<SocketAddr>` + `with_relay_addr(..)` builder so tests can reuse a pre-reserved relay port. Originally driven by the "restart A on the same port" integration-test shape that got abandoned; the builder stays because it's a natural fit for any future test that needs a deterministic relay port.
   * `crates/lvqr-cli/tests/federation_reconnect.rs` (new, ~220 LOC): end-to-end observability contract test. Boots TestServer A + B (B with federation link to A + an admin token provider), waits for Connected, asserts the admin route surfaces `state: connected` + remote_url + forwarded_broadcasts + a non-zero `connect_attempts` + populated `last_connected_at_ms`, verifies the admin gate rejects unauthenticated requests, shuts A down, waits for Failed, asserts the admin route reports `state: failed` with a non-empty `last_error`, then waits ~2.5 s and asserts `connect_attempts` kept growing (the retry loop is actively re-entering `run_link_once` on its backoff schedule). Hand-rolled HTTP/1.1 client mirrors `auth_integration.rs`; no `reqwest` dev-dep added.

2. **Session 103 close doc** (this commit).

### Key 4.4 session C design decisions baked in (confirmed in-commit per the plan-vs-code rule)

* **`session.closed()` monitoring is load-bearing**. Without it, after a remote peer tears down, moq-lite's local sub-origin does not surface the close through `announced()`; the per-link task blocks forever and the status handle stays pinned at Connected. The new arm in `run_link_once`'s select returns `Err(..)` on session termination so the retry wrapper transitions to Failed + schedules a reconnect.
* **`CONNECT_TIMEOUT = 10s` on each attempt**. A silently-dropped QUIC Initial against an unreachable peer retransmits for tens of seconds on quinn's default timers; without a per-attempt bound the admin route's `state` would stay `connecting` forever on a dead peer and `connect_attempts` would never increment. 10 s is well above the loopback / LAN handshake p99 and still short enough for an operator watching the admin route to see retry progress.
* **JWT refresh across reconnect is OUT of scope for v1**. If the `auth_token` expires mid-failure cycle, subsequent attempts reuse the same stale token and fail with 401 on the remote. The failure is observable via the admin route's `last_error` field; operators rotate the config and restart. A future session can add a `FederationLink::refresh_token_url` hook; none of today's code blocks that.
* **Status store is `Arc<RwLock<Vec<FederationLinkStatus>>>` in stdlib**. `std::sync::RwLock` because every critical section is sub-microsecond (tiny struct clone + scalar writes) and never awaits; `parking_lot::RwLock` or a lock-free cell would be over-engineering. Clone-shares-state semantics are asserted end-to-end in the unit tests.
* **Admin route returns `{"links":[]}` rather than 503 when no handle is wired**. Single-node builds and cluster builds with empty `federation_links` are legitimate configurations; poll-unconditionally tooling should not have to special-case them. The cluster-miswired 500 convention on the other `/api/v1/cluster/*` routes only applies when a `Cluster` handle is *expected* and missing; the federation route has no such expectation.
* **Empty Vec vs missing handle is deliberately collapsed on the wire**. An operator cannot distinguish "federation off" from "federation on but no links configured"; both present as `{"links":[]}`. If that distinction matters later, we'd add a top-level `enabled: bool` to the view without breaking the `links` field.
* **No Prometheus metrics in 103 C.** The briefing's decision (d) listed `lvqr_federation_link_state` / `_connect_attempts_total` / `_forwarded_broadcasts_total` as desired metrics, but the 103 C row in the plan explicitly scopes to "admin route + reconnect". The HTTP surface already exposes every counter; a future session can add Prometheus fan-out without any federation.rs change beyond a metrics-recorder call on state transitions.
* **Same-port reconnect integration test abandoned; observability contract tested instead**. The originally-planned `A -> shutdown -> restart A on same port -> reconnected` integration test hit a cross-process UDP port contention on macOS: while B's federation client is actively retrying against A's now-closed port, the UDP socket stays wedged (quinn Endpoint teardown does not release it fast enough for a fresh bind, even over 30 s of retry). A solo-restart probe (no B) rebinds the same port inside 50 ms, so the quirk is specific to the in-process two-server topology. The reconnect retry loop is still proven at the unit level (`federation_unit` tests); the integration test now focuses on the observability contract: Connected on handshake, Failed on peer shutdown, `connect_attempts` growing while the peer stays down, all visible through the HTTP admin route.
* **`TestServerConfig::with_relay_addr` stays in the test-utils API even though the same-port test did not ship**. It's a small, natural builder; a future test that wants deterministic ports for a different reason will pick it up.

### Ground truth (session 103 close)

* **Head**: feat commit + this close-doc commit (two new commits on `main`). Local is N+2 above `origin/main` (push pending; `cde66b4` is still the `origin/main` head per session 102's push event).
* **Tests**: **875** passed, 0 failed, 1 ignored on macOS (default features). (The 1 ignored is the pre-existing `moq_sink` doctest.)
* **CI gates locally clean**:
  * `cargo fmt --all --check`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo test -p lvqr-cluster --lib` 51 passed (+9 new inline: `next_delay` jitter window at attempt 0, doubling, clamped at 6/10/20; `FederationLinkStatus` JSON round-trip + lowercase serde; `FederationStatusHandle` init snapshot, mutators, clone-shares-state, out-of-bounds-noop)
  * `cargo test -p lvqr-cluster --test federation_unit` 11 passed (+2 new)
  * `cargo test -p lvqr-admin` 17 passed (+2 new)
  * `cargo test -p lvqr-cli --test federation_two_cluster` 1 passed (unchanged; no regression)
  * `cargo test -p lvqr-cli --test federation_reconnect` 1 passed (new)
  * `cargo test --workspace` 875 / 0 / 1 (+14 over session 102's 861)
* **Workspace**: **28 crates**, unchanged.
* **crates.io**: unchanged. Session 103 C adds non-breaking public API to `lvqr-cluster` (`FederationConnectState`, `FederationLinkStatus`, `FederationStatusHandle`, `FederationRunner::status_handle`) and `lvqr-admin` (`AdminState::with_federation_status`, `FederationStatusView` pub(crate)->pub move is not yet made), plus a dev-only builder on `lvqr-test-utils`. A future release bump needs `lvqr-cluster` + `lvqr-admin` + `lvqr-cli` + `lvqr-test-utils` republished; 101 A + 102 B + 103 C compose into one semver-minor bump.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91-94 |
| 4.8 | One-token-all-protocols | **COMPLETE** | 95 / 96 |
| 4.5 | In-process AI agents | **COMPLETE** | 97 / 98 / 99 / 100 |
| 4.4 | Cross-cluster federation | **COMPLETE** | 101 / 102 / 103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

6 of 8 Tier 4 items COMPLETE. Remaining: 4.6 transcoding (next), 4.7 latency SLO.

### Session 104 entry point

**Tier 4 item 4.6 session A: server-side transcoding scaffold.**

Scope per `tracking/TIER_4_PLAN.md` section 4.6 row 104 A. New crate `crates/lvqr-transcode/` subscribes to a broadcast's fragment stream, pushes samples through a `gstreamer-rs` pipeline, and publishes the output as a new broadcast (`live/foo/720p`). Hardware encoders (NVENC, VAAPI, QSV, VideoToolbox) feature-gated. Session 104 A is the scaffold + one software-pipeline rendition; ABR ladder generation + HW encoder feature gates land in 105 / 106.

### Pre-session checklist for session 104 A

1. Decide: does `lvqr-transcode` publish back into the same `OriginProducer` (federation-style injection, local only) or into the shared `FragmentBroadcasterRegistry` (HLS / DASH / archive pick it up automatically)? Lean: registry, mirroring every ingest crate.
2. Decide: gstreamer on CI. Not every runner ships gstreamer plugins. Options: make the crate optional-dep behind a `transcode` feature, gate integration tests on `LVQR_GSTREAMER_AVAILABLE`, or bundle a Docker-based CI job.
3. Carry-forward: the federation runner + admin-route pattern from 4.4 is the reference shape for any new runtime surface with a status handle. Follow the same `StatusHandle` + admin route convention if transcoding jobs need observability.

## Session 102 close (2026-04-21)

1. **Tier 4 item 4.4 session B: per-track re-publish + two-cluster E2E** (feat commit).
   * `crates/lvqr-cluster/src/federation.rs`: `FederationLink.disable_tls_verify` field + `with_disable_tls_verify` builder; `run_link` plumbs the TLS knob into `moq_native::ClientConfig::tls.disable_verify`; matched-announcement arm now spawns `forward_broadcast(bc, local_origin, name, shutdown)`; new `forward_broadcast` + `forward_track` helpers + `FEDERATED_TRACK_NAMES = ["0.mp4", "1.mp4", "catalog"]`. The forwarders exit cleanly on cancel via `tokio::select!` arms on both `next_group` and `read_frame`.
   * `crates/lvqr-cluster/tests/federation_unit.rs`: unchanged from session 101 A (the runner lifecycle tests still cover the new code paths through the session-startup surface).
   * `crates/lvqr-cli/src/lib.rs`: `ServeConfig.federation_links` (feature-gated on `cluster`); `ServerHandle.origin` + `ServerHandle.federation_runner` fields + accessors; `start()` gains a post-DASH-bridge branch that constructs the runner against `relay.origin().clone()`.
   * `crates/lvqr-cli/src/main.rs`: federation_links default (empty Vec); CLI flag for TOML-file federation configs lands in session 103 C alongside the admin route.
   * `crates/lvqr-cli/Cargo.toml`: added `lvqr-cluster`, `moq-native`, `url` as dev-deps for the new integration test.
   * `crates/lvqr-test-utils/src/test_server.rs`: `TestServerConfig.federation_links` + `with_federation_link` builder; `TestServer::origin()` + `TestServer::federation_runner()` accessors.
   * `crates/lvqr-test-utils/Cargo.toml`: added `lvqr-cluster` + `lvqr-moq` as regular deps (the public API of TestServerConfig now names `lvqr_cluster::FederationLink`).
   * `crates/lvqr-cli/tests/federation_two_cluster.rs` (new, ~120 LOC): the flagship end-to-end test.

2. **Session 102 close doc** (this commit).

### Key 4.4 design decisions baked in (beyond session 101 A)

* **Per-track forwarding is fixed to the LVQR convention**. `FEDERATED_TRACK_NAMES = ["0.mp4", "1.mp4", "catalog"]`. A broadcast that does not publish one of these on the remote simply leaves the corresponding forwarder sitting idle on `next_group().await` until shutdown. Adding extra track names (subtitles, data tracks, etc.) is a one-line edit in the const.
* **Frame bytes pass unchanged**. `forward_track` calls `local_group.write_frame(frame)` where `frame` is a `Bytes` value read from `remote_group.read_frame()`. No re-encoding, no header rewrite; the MoQ frame bytes are opaque to federation.
* **TLS verification knob is per-link, not per-runner**. Each link may point at a differently-trusted peer (some on real CA chains, some on self-signed VPN internals); per-link control is more ergonomic than a runner-wide toggle.
* **Origin injection, not registry injection**. Federation writes into the local `OriginProducer` only. HLS / DASH / archive / WASM-filter / whisper-captions all drink from the `FragmentBroadcasterRegistry`, which is populated by the ingest bridges (RTMP / WHIP / SRT / RTSP / WS). Federated broadcasts are therefore visible to MoQ subscribers (the demo path from the section 4.4 plan) but NOT to the LL-HLS subtitle rendition or DASH manifest on the receiving cluster. A future session can add a second injection path into the registry if deployments need HLS-over-federation; for v1, the plan's "visible on both clusters" demo is satisfied by MoQ visibility.
* **Test rendezvous is via the relay's public egress, not by peeking at B's origin directly**. The two-cluster test connects a `moq_native::Client` to B's relay URL and reads the announcement stream that way, exercising the full A-relay -> federation-session -> B-origin -> B-relay -> test-client chain. Asserting on B's origin internal state would skip the B-relay hop and hide any bug in the re-publish glue.
* **Deployment note (surfaced during test debugging)**: `moq_native::Client` connects to the URL's hostname as-is. On macOS with dual-stack `localhost` resolution, a relay bound only to `127.0.0.1` is unreachable via `https://localhost:<port>` because the client picks `::1` and hangs. Use an explicit IPv4 (or IPv6) literal in `FederationLink::remote_url` when the peer relay is bound single-stack.

### Ground truth (session 102 close)

* **Head**: feat commit + this close-doc commit (two new commits on `main`). Local is N+2 above `origin/main`.
* **Tests**: **861** passed, 0 failed, 1 ignored on macOS (default features).
* **CI gates locally clean**:
  * `cargo fmt --all`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo test -p lvqr-cluster --lib` 42 passed
  * `cargo test -p lvqr-cluster --test federation_unit` 9 passed
  * `cargo test -p lvqr-cli --test federation_two_cluster` 1 passed (~2 s wall clock on loopback)
  * `cargo test --workspace` 861 / 0 / 1
* **Workspace**: **28 crates**, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91-94 |
| 4.8 | One-token-all-protocols | **COMPLETE** | 95 / 96 |
| 4.5 | In-process AI agents | **COMPLETE** | 97 / 98 / 99 / 100 |
| 4.4 | Cross-cluster federation | **A + B DONE**, C pending | 101 / 102 / 103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

5 of 8 Tier 4 items COMPLETE; 4.4 two-thirds done.

### Session 103 entry point

**Tier 4 item 4.4 session C: admin route `/api/v1/cluster/federation` + reconnect on link failure.**

Two deliverables:
1. Admin HTTP route exposing per-link status (configured vs connected; last error if any; forwarded broadcast names seen). `lvqr-admin` hosts the route; lvqr-cli wires it. The `FederationRunner` currently does not expose per-link status beyond `configured_links` + `active_links` counters; session 103 C adds per-link state (last connect timestamp, last error) via interior mutability so the admin route can read a snapshot without blocking.
2. Exponential-backoff reconnect on connect failure. Today a failed connect aborts the per-link task; session 103 C wraps `run_link`'s connect path in a retry loop with jitter so transient peer outages do not require a cluster restart.

**Pre-session checklist**:
1. Decide: does reconnect reset the `subscription_url` query-param (if `auth_token` expires between attempts) or keep the same URL? Lean: reset, because a JWT-based auth_token may have expired. Adds a `FederationLink::refresh_token()` hook API? Or just let operators rotate config and hit the admin route.
2. Decide: what's the metric name for the federation status? Candidates: `lvqr_federation_link_state{remote_url, state=connected|connecting|failed}` gauge. Mirror existing `lvqr_moq_*` metric conventions.

### Session 102 push event carry-over

After session 102 commits land, add a push-event doc to bring the HANDOFF status header in sync with `origin/main`. Same pattern as session-100 `b1bc4f5` + session-99 `9c1135c`.

 Session 101 landed Tier 4 item 4.4 session A: `lvqr-cluster` gained `FederationLink` (serde struct; TOML + JSON ready) and `FederationRunner` (one tokio task per link; opens outbound MoQ session via `moq_native::Client` against `subscription_url()` which appends `?token=<jwt>` matching the `lvqr_relay::server::parse_url_token` convention). Per-link task drains the remote origin's announcement stream and filters against `link.forwards(name)`; matched announcements log a structured event. Per-track re-publish into the local origin is deferred to session 102 B (where the two-cluster integration test can exercise the full wire path; scope rescoped in-commit per the plan-vs-code rule). `FederationRunner::shutdown()` bounds each per-link task wait to a 1 s grace then aborts, so an unreachable peer does not hang cluster teardown; `Drop` aborts the same way. New `lvqr-cluster` deps: `lvqr-moq`, `moq-lite`, `moq-native`, `url` (plus `toml` as a dev-dep for the TOML round-trip test). 15 new tests (6 inline in `src/federation.rs` + 9 in `tests/federation_unit.rs`) exercise config shape, URL token append, exact-match forwarding, runner lifecycle (empty link list, unreachable remote with bounded shutdown, Drop path). Workspace totals: **858 passed / 0 failed / 1 ignored** (up from 843; +15). Crate count unchanged at **28**. Session 101 is a fresh session continuing immediately after session 100's push event (commit chain: `519afda` feat + `0ba0169` close-doc + `e4522a1` audit polish + `b1bc4f5` push event are on `origin/main`; session 101's commits sit above them on local `main` pending this session's push).

### Session 101 close (2026-04-21)

1. **Tier 4 item 4.4 session A: FederationLink + FederationRunner scaffold** (feat commit).
   * `crates/lvqr-cluster/src/federation.rs` (new, ~340 LOC): `FederationLink` with `subscription_url()` + `forwards()` helpers; `FederationRunner` with `start` / `configured_links` / `active_links` / `shutdown` / `Drop`; `run_link` private async fn; `SHUTDOWN_GRACE: Duration = 1 s`.
   * `crates/lvqr-cluster/tests/federation_unit.rs` (new, ~150 LOC): 9 unit tests.
   * `crates/lvqr-cluster/src/lib.rs`: `mod federation;` + `pub use federation::{FederationLink, FederationRunner};`.
   * `crates/lvqr-cluster/Cargo.toml`: added `lvqr-moq`, `moq-lite`, `moq-native`, `url` regular deps + `toml` dev-dep.
   * `tracking/TIER_4_PLAN.md`: 4.4 header flipped to "A DONE, B-C pending"; row 101 A scoped up to the full deliverable + verification record; row 102 B scoped up to include the per-track re-publish implementation (previously the plan split it across 101 A "subscribe loop" + 102 B "integration test"; reality is the two only compose as one atomic piece of code).

2. **Session 101 close doc** (this commit).

### Key 4.4 design decisions baked in

* **URL shape**: `link.remote_url` must be a valid URL; `subscription_url()` appends `token=<auth_token>` via `url::Url::query_pairs_mut().append_pair`. Existing query params (e.g. `?region=us-west` for operator telemetry) are preserved. Matches the `lvqr_relay::server::parse_url_token` extractor exactly.
* **Forward matching = exact match only for v1**. Glob / prefix patterns are explicit anti-scope (section 4.4 anti-scope forbids auto-discovery; explicit curation keeps operator intent unambiguous). A future session can add glob support behind a feature flag if demand materializes.
* **No wildcard / regex patterns in `forwarded_broadcasts`**. Empty list is a valid no-op link (session opens, nothing is forwarded); useful as a cluster-reachability probe.
* **Shutdown budget**: 1 s grace per per-link task, then abort. moq-native connect futures can stall on TLS / DNS work that is not cancellation-responsive; the bounded wait keeps cluster teardown O(links) x 1 s worst case. The `federation_unit.rs` test with a TEST-NET-1 remote URL (RFC 5737 `192.0.2.1`) verifies this bound holds.
* **JWT minting out of scope**: session 101 A accepts an opaque `auth_token` string. Tier 4 item 4.8 already shipped the JWT minting path (shared secret + audience claim); operators mint per-link tokens externally. A future session can add a convenience helper (`FederationLink::with_jwt(secret, audience, broadcasts)`) but it is not blocking.
* **Announcement stream is the subscribe surface**. We do NOT catalog-subscribe or track-enumerate; we drain the remote origin's announcement events and react to each matching broadcast as it appears. This matches the `crates/lvqr-relay/tests/relay_integration.rs` client pattern and avoids a round-trip through the catalog track's per-broadcast frame messages for discovery.
* **Session 102 B integration-test scope**. The meaningful verification for the per-track re-publish requires two real MoQ relays; attempting to shove that into `federation_unit.rs` would require a second `RelayServer` + client round-trip + trait-object work just to avoid a live network. Instead, `federation_unit.rs` is network-free; `federation_two_cluster.rs` (session 102 B) gets its own TestServer harness spinning up two full LVQR instances, wiring a federation link between them, publishing on A, and asserting B's HLS surface serves the same broadcast.

### Ground truth (session 101 close)

* **Head**: feat commit + this close-doc commit (two new commits on `main`). Local is N+2 above `origin/main` pending push. (Previous push event `b1bc4f5` is on `origin/main`.)
* **Tests**: **858** passed, 0 failed, 1 ignored on macOS (default features).
* **CI gates locally clean**:
  * `cargo fmt --all`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo test -p lvqr-cluster --lib` 40 passed (+6 inline)
  * `cargo test -p lvqr-cluster --test federation_unit` 9 passed
  * `cargo test --workspace` 858 / 0 / 1 (+15 over session 100's 843)
* **Workspace**: **28 crates**, unchanged.
* **crates.io**: unchanged. Session 101 A adds non-breaking public API (`FederationLink`, `FederationRunner`) to `lvqr-cluster`; a future release bump (0.4.0 -> 0.5.0 or 0.4.1) republishes `lvqr-cluster` + re-runs the existing publish chain.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91-94 |
| 4.8 | One-token-all-protocols | **COMPLETE** | 95 / 96 |
| 4.5 | In-process AI agents | **COMPLETE** | 97 / 98 / 99 / 100 |
| 4.4 | Cross-cluster federation | **A DONE**, B-C pending | 101 / 102 / 103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

5 of 8 Tier 4 items COMPLETE; 4.4 one-third done.

### Session 102 entry point

**Tier 4 item 4.4 session B: two-cluster integration test + per-track re-publish implementation.**

The session-101 A runner opens the outbound MoQ session + drains announcements. Session 102 B owns:
1. Extending `run_link` (or a sibling helper) so a matched announcement triggers `remote_bc.subscribe_track(Track::new("0.mp4" / "1.mp4" / "catalog"))` against the LVQR track-name convention; the resulting `TrackConsumer` pipes groups + frames into a `TrackProducer` created on the local origin's `BroadcastProducer`.
2. New integration test `crates/lvqr-cli/tests/federation_two_cluster.rs`: boots two `TestServer` instances (A + B), wires a `FederationLink` on B pointing at A, publishes an RTMP broadcast on A, asserts B's HLS surface serves the same broadcast. The `TestServer` harness may need a new builder `with_federation_link(..)` analogous to `with_whisper_model`.
3. Decide: does the federation link's local origin injection go directly into `lvqr_relay::RelayServer::origin()` or through the shared `FragmentBroadcasterRegistry`? The former is faster (one less fanout) but the latter lets federated broadcasts flow through all the same consumers (HLS / DASH / archive / WASM filter / whisper captions). Default choice: registry, for consistency with every other ingest crate.

**Prerequisites already in place**:
* `FederationLink` + `FederationRunner` (session 101 A).
* `lvqr_relay::RelayServer::origin()` accessor returns `&OriginProducer`.
* `TestServer` fixture (lvqr-test-utils) can be driven twice in one test for a two-cluster topology.

### Session 103 entry point (after 102 B)

Admin route `GET /api/v1/cluster/federation` exposing link status + reconnect-on-failure (exponential backoff). Plan row 103 C unchanged.

 Session 100's three commits (`519afda` feat + `0ba0169` close-doc + `e4522a1` audit-follow-up polish that warns when `--whisper-model` is set without an HLS surface) are pushed to `origin/main`; `git log --oneline origin/main..main` is empty. crates.io is unchanged since the post-session-98 `lvqr-agent-whisper 0.4.0` publish event; a future release cycle needs a version bump on `lvqr-cli` + `lvqr-test-utils` (and `lvqr-agent-whisper` as a drive-by) before re-publishing via `/tmp/lvqr_publish.sh`. The session 100 post-close audit surfaced one operational footgun (whisper factory runs even when HLS is disabled, in which case the caption fragments are silently dropped by tokio broadcast semantics); the `e4522a1` polish commit adds a `tracing::warn!` at `start()` time so misconfigured deployments surface early. Session 101 entry point below is Tier 4 item 4.4 session A (cross-cluster federation: `FederationLink` config + MoQ subscribe loop).

## Session 100 close (2026-04-21)

### What shipped

1. **Tier 4 item 4.5 session D: `--whisper-model` CLI flag + `lvqr_cli::start` AgentRunner wiring** (feat commit).

   **Decisions baked in (confirmed in-commit per the plan-vs-code rule)**:

   * `lvqr-cli` Cargo feature name = `whisper`, default OFF. Symmetry with `lvqr-agent-whisper/whisper`. Pulls `dep:lvqr-agent` + `dep:lvqr-agent-whisper` + `lvqr-agent-whisper/whisper` + `lvqr-test-utils/whisper`. Included in the `full` meta-feature. Reasoning: the optional-dep + gated-field shape mirrors the existing `c2pa` pattern exactly so future readers see one idiom, not two.
   * CLI flag = `--whisper-model <PATH>` with `LVQR_WHISPER_MODEL` env. `--whisper-language` is OUT per 4.5 anti-scope (English only). Help text also documents the v1 no-history limitation (late HLS subscribers see only cues emitted from the moment they joined onwards).
   * `ServeConfig.whisper_model: Option<PathBuf>` is feature-gated `#[cfg(feature = "whisper")]` exactly like `c2pa: Option<C2paConfig>`. Without the feature the field (and the flag) vanish from the ABI.
   * Factory install site in `lvqr_cli::start` is immediately after `BroadcasterCaptionsBridge::install(hls.clone(), &shared_registry)` so the HLS subtitles drain path exists before the agent starts emitting cues, and before any ingest listener binds.
   * `ServerHandle` gains `agent_runner: Option<AgentRunnerHandle>` feature-gated on whisper; mirrors the `wasm_filter: Option<WasmFilterBridgeHandle>` shape. `ServerHandle::agent_runner()` accessor is the read path for tests.

   **`lvqr-cli` changes**:
   * `Cargo.toml`: added `whisper` feature entry + two new optional deps (`lvqr-agent`, `lvqr-agent-whisper`); `full` meta-feature now includes `whisper`.
   * `src/lib.rs`: `ServeConfig.whisper_model` field; `loopback_ephemeral` default of `None`; `ServerHandle.agent_runner` field + accessor; `start()` branch that builds the factory + installs it on a fresh `AgentRunner`; `ServerHandle { .. }` constructor passthrough. Inline `#[cfg(all(test, feature = "whisper"))]` test module with 2 cases: default is `None`, explicit path round-trips through `ServeConfig`.
   * `src/main.rs`: new `#[cfg(feature = "whisper")] #[arg(long, env = "LVQR_WHISPER_MODEL")] whisper_model: Option<PathBuf>`; threaded into `ServeConfig` next to the `c2pa: None` line.
   * `tests/whisper_cli_e2e.rs` (new): `#![cfg(feature = "whisper")]` + `#[ignore]`-ed integration test that lifts the AAC RTMP publish helpers from `rtmp_hls_e2e.rs`, publishes a video init + audio init + 4 synthetic AAC frames, polls `server.agent_runner().unwrap().fragments_seen("captions", "live/captions", "1.mp4")` for up to 5 s, asserts non-zero. Skip-with-log branch when `WHISPER_MODEL_PATH` is absent.

   **`lvqr-test-utils` changes**:
   * `Cargo.toml`: new `whisper` feature (`["lvqr-cli/whisper", "dep:lvqr-agent"]`) + optional `lvqr-agent` regular dep so the `agent_runner()` accessor's return type resolves.
   * `src/test_server.rs`: `TestServerConfig.whisper_model` field + `with_whisper_model` builder (feature-gated); `ServeConfig.whisper_model` threaded through in `TestServer::start`; `TestServer::agent_runner()` accessor proxying to `ServerHandle::agent_runner()`.

   **Drive-by clippy fixes under `--features whisper`** (surfaced because the new `lvqr-cli/whisper` feature chain reactivates `lvqr-agent-whisper/whisper` during the whisper-gated clippy pass):
   * `crates/lvqr-agent-whisper/src/worker.rs:run`: `#[allow(clippy::too_many_arguments)]` on the 8-arg worker entry. Refactoring to a state struct is scope creep (the worker is private and the args are genuinely distinct lifetimes).
   * `crates/lvqr-agent-whisper/tests/whisper_basic.rs`: `sample_rate` -> `_sample_rate` on the `fragment()` helper (was already ignored).

   **Plan refresh**. `tracking/TIER_4_PLAN.md` section 4.5 header flipped from "A + B + C DONE, D pending" to "COMPLETE"; row 100 D scoped up from one-line to the full deliverable + verification record.

2. **Session 100 close doc** (this commit).

### Manual demo recipe

```bash
# Fetch a v1 model (~75 MB):
curl -L -o /tmp/ggml-tiny.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin

# Build + run lvqr with captions enabled:
cargo build --release -p lvqr-cli --features whisper
./target/release/lvqr serve --whisper-model /tmp/ggml-tiny.en.bin

# In another terminal, publish English speech via ffmpeg:
ffmpeg -re -i podcast-clip.mp3 \
  -c:a aac -ar 16000 -ac 1 -b:a 128k \
  -f flv rtmp://localhost:1935/live/demo

# Browser playback:
# http://localhost:8888/hls/live/demo/master.m3u8
# Open in hls.js demo page or Safari; enable the English captions
# track. Cues should appear within ~5 s of speech. (Cold whisper-rs
# inference can take an extra second on first pass.)
```

Pick a known-English, clear-speech audio source (a podcast clip, an audiobook excerpt). Silent or non-English clips produce empty cues -- whisper.cpp tiny.en is discriminating.

### Tests shipped

| # | Test surface | Added this session |
|---|---|---|
| a | `crates/lvqr-cli/src/lib.rs` inline tests | 2 (`loopback_ephemeral_defaults_whisper_model_to_none`; `whisper_model_round_trips_through_serve_config`) -- `#[cfg(all(test, feature = "whisper"))]`, so absent from the default `cargo test --workspace` run. |
| b | `crates/lvqr-cli/tests/whisper_cli_e2e.rs` | 1 `#[ignore]`-ed integration test (`whisper_cli_flag_wires_factory_through_start`): exercises the full RTMP -> ingest -> FragmentBroadcasterRegistry -> AgentRunner wiring; asserts `agent_runner().fragments_seen(...)` bumps. Run via `WHISPER_MODEL_PATH=... cargo test -p lvqr-cli --features whisper --test whisper_cli_e2e -- --ignored`. |

Workspace totals: **843** passed, 0 failed, 1 ignored (parity with session 99; the 2 new inline tests are whisper-feature-gated and the 1 integration test is both feature-gated and `#[ignore]`-ed). The 1 remaining always-ignored test is the pre-existing `moq_sink` doctest.

### Ground truth (session 100 close)

* **Head**: feat commit + this close-doc commit (two new commits on `main`). Local is N+2 commits ahead of `origin/main` (head upstream is `9c1135c`). Verify via `git log --oneline origin/main..main`. Do NOT push without direct user instruction.
* **Tests**: **843** passed, 0 failed, 1 ignored on macOS (default features).
* **CI gates locally clean**:
  * `cargo fmt --all`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo clippy -p lvqr-cli -p lvqr-test-utils -p lvqr-agent-whisper --features whisper --all-targets -- -D warnings`
  * `cargo build --release -p lvqr-cli` (default features; confirmed `cargo tree -p lvqr-cli | grep -E "whisper|symphonia"` empty)
  * `cargo test -p lvqr-cli --features whisper --lib` 2 passed
  * `cargo test -p lvqr-cli --features whisper --test whisper_cli_e2e` 1 ignored (the intended default)
  * `cargo test --workspace` 843 / 0 / 1
* **Workspace**: **28 crates**, unchanged.
* **crates.io**: unchanged since session 98's `lvqr-agent-whisper 0.4.0` publish; session 100 D's changes are additive to `lvqr-cli` + `lvqr-test-utils`, no existing crate semantic break. A future release bump would need to touch `lvqr-cli`, `lvqr-test-utils`, `lvqr-agent-whisper` (drive-by), and `lvqr-agent-whisper/tests/whisper_basic.rs` (drive-by).

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91-94 |
| 4.8 | One-token-all-protocols | **COMPLETE** | 95 / 96 |
| 4.5 | In-process AI agents | **COMPLETE** | 97 / 98 / 99 / 100 |
| 4.4 | Cross-cluster federation | PLANNED | 101-103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

5 of 8 Tier 4 items DONE. Remaining: 4.4 federation (next), 4.6 transcoding, 4.7 latency SLO.

### Session 101 entry point

**Tier 4 item 4.4 session A: `FederationLink` config + MoQ subscribe loop.**

Scope per `tracking/TIER_4_PLAN.md` section 4.4 row 101 A: `lvqr-cluster` gains a `FederationLink { remote_url, auth_token, forwarded_broadcasts: Vec<String> }` config, and at cluster bootstrap every link opens a single authenticated MoQ session to the remote cluster's MoQ relay endpoint; for every broadcast name in `forwarded_broadcasts` the local cluster subscribes to the remote's MoQ origin and re-publishes into the local origin. Verification: `cargo test -p lvqr-cluster --test federation_unit`.

**Prerequisites already in place**:
* `lvqr-auth`'s JWT path (Tier 4 item 4.8); each link's `auth_token` is a JWT minted for the remote cluster's audience.
* `lvqr-moq`'s subscribe primitive; the federation link is structurally a relay-of-relay pattern already exercised in-process in Tier 3.
* `lvqr-cluster` is shipped + on crates.io at 0.4.0.

**Pre-session checklist**:
1. Decide the TOML shape for `FederationLink` in the CLI config (versus a CLI flag). Prior art: `--cluster-seeds` is a comma-separated flag, but a federation link has three fields (url + token + broadcast list), which argues for TOML.
2. Decide whether `forwarded_broadcasts` supports glob patterns (e.g. `live/*`). Anti-scope allows explicit names only in v1; the plan's phrasing is "for every broadcast name in `forwarded_broadcasts`" which reads literal.

## Session 99 close (2026-04-21) Session 99's two commits (`43c29e5` feat + `f54cec6` close-doc) are pushed to `origin/main`; `git log --oneline origin/main..main` is empty. crates.io is unchanged (lvqr-agent-whisper / lvqr-agent / lvqr-cli stay at 0.4.0; no version bump required since session 99's changes are additive); a future release would need a workspace-wide version bump for the touched crates (lvqr-hls, lvqr-agent-whisper, lvqr-cli, lvqr-test-utils) and the existing publish-chain script under `/tmp/lvqr_publish.sh`. Tier 4 item 4.5 session C landed the HLS subtitle rendition + captions registry track that bridges the WhisperCaptionsAgent's output through to browser players. Three-crate stitch: `lvqr-hls` ships the `subtitles.rs` module (`SubtitlesServer` with sliding-window cue store + WebVTT serializer + captions playlist with `EXT-X-PROGRAM-DATE-TIME` alignment, plus `MultiHlsServer::ensure_subtitles` / `subtitles` accessors); the master playlist gains the `EXT-X-MEDIA TYPE=SUBTITLES,GROUP-ID="subs",NAME="English",DEFAULT=YES,AUTOSELECT=YES,LANGUAGE="en",URI="captions/playlist.m3u8"` rendition + `SUBTITLES="subs"` on the variant when captions exist (`VariantStream` extended with `subtitles_group: Option<String>`); new axum routes `/hls/{broadcast}/captions/playlist.m3u8` + `/hls/{broadcast}/captions/seg-{msn}.vtt`. `lvqr-agent-whisper` factory gains `with_caption_registry(FragmentBroadcasterRegistry)`; the agent dual-publishes each `TranscribedCaption` to both the in-process `CaptionStream` (existing public API) AND the registry's `(broadcast, "captions")` track (new wire shape: `Fragment.payload` = UTF-8 cue text, `dts/duration` = wall-clock UNIX ms). `lvqr-cli` ships `BroadcasterCaptionsBridge` mirroring `BroadcasterHlsBridge::install`: `on_entry_created` callback for the `"captions"` track that subscribes synchronously and spawns one drain task per broadcast feeding `CaptionCue` values into `MultiHlsServer::ensure_subtitles`. `ServerHandle::fragment_registry()` + `TestServer::fragment_registry()` accessors added so integration tests can publish captions directly without driving whisper.cpp. The new `crates/lvqr-cli/tests/captions_hls_e2e.rs` exercises the full flow with synthetic fragments. Workspace tests: **843** passing (up from 823; +20 for the new modules + e2e). Workspace count unchanged at **28 crates**. Session 100 entry point is Tier 4 item 4.5 session D (`--whisper-model` CLI flag + `lvqr_cli::start` AgentRunner wiring + ffmpeg-publish-then-browser-playback E2E demo).

## Session 99 close (2026-04-21)

### What shipped

1. **Tier 4 item 4.5 session C: HLS subtitle
   rendition + captions registry track** (`43c29e5`).

   **Decision baked in**: registry track named
   `"captions"` (FragmentMeta `wvtt`, timescale 1000)
   is the wire shape between captions producer and HLS
   bridge. Per-cue Fragment payload = UTF-8 cue text;
   `dts` / `duration` = wall-clock UNIX ms. Reasoning:
   composes with the existing
   `FragmentBroadcasterRegistry::on_entry_created`
   consumer family the HLS / archive / WASM filter
   bridges already drink from; keeps session 100 D's
   CLI wiring uniform with every other LVQR sink;
   future-proofs item 4.4 federation gossip.

   **`lvqr-hls`** (~410 LOC + 16 tests):
   * `subtitles.rs`: `CaptionCue` + `SubtitlesServer`
     (cheap-to-clone `Arc<RwLock<..>>` with bounded
     sliding window, default 50 cues). `push_cue`
     bumps target-duration on the largest cue,
     evicts the oldest on overflow + bumps
     `EXT-X-MEDIA-SEQUENCE`. Renders standard HLS
     playlist (no LL-HLS partials -- subtitles are
     text-only and small) with per-segment
     `EXT-X-PROGRAM-DATE-TIME` for wall-clock
     alignment. `render_segment` emits a WEBVTT body
     with zero-anchored cue timestamps; the playlist
     PDT places the cue at its segment's wall-clock.
     Hand-rolled ISO 8601 UTC formatter using Howard
     Hinnant's days-since-epoch algorithm so chrono /
     time stay out of the dep graph.
   * `master.rs`: `VariantStream` gains
     `subtitles_group: Option<String>`; renderer
     emits `SUBTITLES="..."` when set.
     `MediaRenditionType::Subtitles` was already
     reserved for future use; the `EXT-X-MEDIA`
     serializer was already correct for it.
   * `server.rs`: `MultiHlsServer::ensure_subtitles`
     / `subtitles` accessors mirror the audio
     pattern. `BroadcastEntry` carries an
     `Option<SubtitlesServer>`; `finalize_broadcast`
     also finalizes subtitles. New axum routes via
     the existing `/hls/{*path}` catch-all dispatch:
     `playlist.m3u8` -> playlist body;
     `seg-{msn}.vtt` -> 200/text/vtt or 404 (cue
     evicted from the window). `handle_master_playlist`
     declares the SUBTITLES rendition + adds
     `SUBTITLES="subs"` to the variant when captions
     exist; omits both lines when no captions
     producer is wired (verified by negative test).

   **`lvqr-agent-whisper`**:
   * `WhisperCaptionsFactory::with_caption_registry(
     FragmentBroadcasterRegistry)` builder; new
     `pub const CAPTIONS_TRACK_ID = "captions"`.
   * `WhisperCaptionsAgent` carries `caption_registry`
     + `caption_broadcaster` fields. `on_start`
     lazily creates the captions broadcaster on the
     registry under `FragmentMeta::new("wvtt",
     1000)` when the whisper feature is on AND the
     registry is wired.
   * `worker::run_inference` now also publishes each
     cue to the captions broadcaster as a `Fragment`.
     `dts/duration` = wall-clock UNIX ms (now() at
     publish time as a v1 proxy for cue start; small
     lag behind audio is documented as a known v1
     limitation).
   * `on_stop` calls `registry.remove(broadcast,
     "captions")` so the captions HLS drain task
     sees `Closed` and exits cleanly.

   **`lvqr-cli`**:
   * New module `captions.rs`:
     `BroadcasterCaptionsBridge`. Mirror of
     `BroadcasterHlsBridge::install`. Early-returns
     for any track other than `"captions"` so it
     composes safely with the LL-HLS bridge.
   * Wired in `lvqr_cli::start` next to the existing
     LL-HLS bridge install.
   * `ServerHandle::fragment_registry()` accessor;
     `TestServer::fragment_registry()` mirrors it.
     `lvqr-test-utils` Cargo.toml gains
     `lvqr-fragment` regular dep.

   **Tests** (20 new):
   * 15 lvqr-hls subtitles unit tests (timestamps,
     WEBVTT body, ISO 8601, sliding window, finalize,
     URI parsing).
   * 1 master playlist test for the SUBTITLES
     rendition + variant attribute.
   * 1 lvqr-agent-whisper test (`on_stop` without
     caption_registry is a no-op).
   * 2 e2e tests in
     `crates/lvqr-cli/tests/captions_hls_e2e.rs`:
     positive flow (synthetic video + caption
     fragment -> assert master + captions playlist +
     .vtt body); negative (no captions producer ->
     master omits SUBTITLES).
   * 1 unrelated bonus: bumped lvqr-hls test count
     elsewhere via the new master test.

   **Plan refresh**.
   `tracking/TIER_4_PLAN.md` section 4.5 header
   flipped to "A + B + C DONE, D pending"; row 99 C
   scoped up from one-line to the full deliverable +
   verification record.

2. **Session 99 close doc** (this commit).

### Tests shipped

| # | Test surface | Added this session |
|---|---|---|
| a | `crates/lvqr-hls/src/subtitles.rs` | 15 (cue duration; webvtt timestamp formatter; webvtt body; ISO 8601 epoch + recent + ms; empty playlist; push cue + segment; target-duration grows; sliding window evict; finalize ENDLIST + post-finalize push ignored; segment 404; URI round-trip) |
| b | `crates/lvqr-hls/src/master.rs` | 1 (SUBTITLES rendition + SUBTITLES attribute on variant) |
| c | `crates/lvqr-agent-whisper/src/agent.rs` | 1 (`on_stop` without caption_registry no-op) |
| d | `crates/lvqr-cli/tests/captions_hls_e2e.rs` | 2 (positive: synthetic captions land in HLS; negative: no producer means no SUBTITLES rendition) |
| e | other lvqr-hls regressions auto-passed | 1 (existing variant test re-asserted with new `subtitles_group: None` field) |

Workspace totals: **843** passed, 0 failed, 1
ignored (up from session 98's 823 / 0 / 1; +20 across
the new modules + e2e). The 1 remaining ignored is
the pre-existing `moq_sink` doctest unrelated to 4.5.

### Ground truth (session 99 close)

* **Head**: `f54cec6` (close-doc) on top of `43c29e5`
  (feat). Both pushed to `origin/main`. Verify via
  `git log --oneline origin/main..main` (should be
  empty). Do NOT push subsequent work without direct
  user instruction.
* **Tests**: **843** passed, 0 failed, 1 ignored on
  macOS (default features).
* **CI gates locally clean**:
  * `cargo fmt --all`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo test -p lvqr-hls --lib` 50 passed
  * `cargo test -p lvqr-agent-whisper` 28 passed
  * `cargo test -p lvqr-cli --test captions_hls_e2e` 2 passed
  * `cargo test --workspace` 843 / 0 / 1
* **Workspace**: **28 crates**, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91-94 |
| 4.8 | One-token-all-protocols | **COMPLETE** | 95 / 96 |
| 4.5 | In-process AI agents | **A + B + C DONE**, D pending | 97 / 98 / 99 / 100 |
| 4.4 | Cross-cluster federation | PLANNED | 101-103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

### Session 100 entry point

**Tier 4 item 4.5 session D: `--whisper-model` CLI
flag + `lvqr_cli::start` AgentRunner wiring +
ffmpeg-publish-then-browser-playback E2E demo.**

Final session in 4.5. Wires the existing
`WhisperCaptionsFactory::with_caption_registry(...)
` builder into the CLI under a new
`--whisper-model <PATH>` flag (gated on a `whisper`
Cargo feature on `lvqr-cli` that pulls in
`lvqr-agent-whisper/whisper`), and produces the
manual demo per section 4.5 row 100 D: ffmpeg
publishing English audio + browser hls.js playback
showing on-screen captions.

**Prerequisites already in place**:

* `WhisperCaptionsFactory::with_caption_registry`
  exists (session 99 C).
* `BroadcasterCaptionsBridge` is installed in
  `lvqr_cli::start` for every server (session 99 C).
* The HLS subtitle rendition is exposed under
  `/hls/{broadcast}/captions/...` (session 99 C).
* `ServerHandle::fragment_registry()` accessor
  exists (session 99 C); the CLI wiring will hand
  the registry to the factory, then install the
  factory on the existing `AgentRunner`
  (`lvqr_agent::AgentRunner::install` -- session 97
  A).

**Pre-session checklist**:

1. Decide on the `lvqr-cli` feature flag name
   (`whisper`? `captions`?). Lean toward `whisper`
   for symmetry with `lvqr-agent-whisper`'s
   feature.
2. Decide on `--whisper-language` (deferred per
   4.5 anti-scope: English only). Document the
   anti-scope in the CLI flag's help text.
3. Pre-test the manual demo on a real .bin file
   so session 100 D's commit message can include
   the demo recipe verbatim.

**Verification gates (session 100 D close)**:

* `cargo fmt --all`
* `cargo clippy --workspace --all-targets --benches -- -D warnings`
* `cargo test --workspace` (expect no regression
  from 843)
* Manual demo: ffmpeg publish English audio ->
  browser hls.js playback shows on-screen captions
  within ~5 s of speech.
* `git log -1 --format='%an <%ae>'` reads
  `Moheeb Zara <hackbuildvideo@gmail.com>` alone

**Biggest risks**, ranked:

1. **`lvqr-cli` feature graph blow-up**. Adding a
   `whisper` feature to `lvqr-cli` that pulls in
   `lvqr-agent-whisper/whisper` means
   `cargo build -p lvqr-cli --features whisper` now
   compiles whisper.cpp. Make sure the default
   `cargo install lvqr-cli` build stays light by
   keeping the feature OFF by default.
2. **Demo flake**. Whisper-cpp tiny.en model on
   a real ffmpeg AAC stream may produce no captions
   on certain audio (silent / non-English / very
   short clips). The session 100 D demo recipe
   should pick a known-English audio source with
   clear speech (a podcast clip, an audiobook
   excerpt, etc.).
3. **CLI argument name + env-var collision**. Pick
   `--whisper-model` + `LVQR_WHISPER_MODEL` env to
   match the project's flag conventions.

 Pushed session 98 commits to `origin/main` (head `c1632c4`); published `lvqr-agent-whisper 0.4.0` to crates.io as a first-time publish (the v0.4.0 release event 16+ hours earlier had drained the rate-limit bucket, but enough refill time had passed that this single publish went through on the first try). Total publishable workspace crates now at **25** (was 24 after the 2026-04-20 release event); the three publish=false helpers (`lvqr-conformance`, `lvqr-test-utils`, `lvqr-soak`) stay local. `cargo install lvqr-cli` still installs the same v0.4.0 binary; the new `lvqr-agent-whisper` is opt-in (consumers add it to their own Cargo.toml + flip the `whisper` feature). No code changes between session 98 close and this publish event.

## Post-session-98 publish event (2026-04-21)

* `lvqr-agent-whisper 0.4.0` published to crates.io
  on the first attempt at 02:32 UTC. The v0.4.0
  release event burst had drained the new-crate
  rate-limit bucket ~16 hours earlier; cargo
  metrics' refill rate of 1 per 10 minutes had
  fully restored the burst by then.
* No version-bump churn for any other workspace
  crate (the new crate is additive; no other
  crate's content changed since session 98).
* `lvqr-cli` is unchanged on crates.io; the
  WhisperCaptionsAgent is NOT yet wired into
  `lvqr_cli::start` (that lands in session 100 D).
  Today, consumers wanting to use the agent add it
  to their own binary as a `cargo add
  lvqr-agent-whisper --features whisper` dep and
  install the factory on an `AgentRunner` they own.

## Session 98 close (2026-04-20) Tier 4 item 4.5 session B landed `crates/lvqr-agent-whisper`, the first concrete `lvqr_agent::Agent` implementation: a `WhisperCaptionsFactory` that opts in only for the audio track (`track_id == "1.mp4"`) and a `WhisperCaptionsAgent` that subscribes to the broadcast's audio stream, extracts raw AAC frames from each fragment's `moof + mdat` payload, decodes via symphonia (using the `AudioSpecificConfig` parsed out of the init segment), buffers PCM up to a configurable window (default 5 s), and runs whisper.cpp inference on a dedicated OS worker thread to emit `TranscribedCaption` values onto a public `tokio::sync::broadcast`-backed `CaptionStream`. Heavy deps (`whisper-rs 0.16` + `symphonia 0.6.0-alpha.2 [aac]`) gated behind a default-OFF `whisper` Cargo feature so `cargo build --workspace` stays fast. Always-available surface (`TranscribedCaption`, `CaptionStream`, factory, agent stub, mdat parser, ASC parser) compiles without the feature so consumers can wire the factory into an `AgentRunner` and the agent contract holds (no-op `on_fragment` with a single debug-log line). With the feature enabled, the worker uses `std::sync::mpsc::sync_channel(64)` for back-pressure-free frame intake and runs `WhisperContext::full` with English-only `Greedy { best_of: 1 }` sampling. Workspace tests: **823** passing (up from 796; +27 new lib tests for agent / asc / caption / factory / decode / mdat). Workspace count now **28 crates** (was 27). Session 99 entry point is Tier 4 item 4.5 session C (captions track publish via `lvqr-moq` + HLS subtitle rendition wiring in `lvqr-hls`).

## Session 98 close (2026-04-20)

### What shipped

1. **Tier 4 item 4.5 session B: `lvqr-agent-whisper`
   crate** (`ac989d8`).

   **Crate-layout decision baked in (carry forward
   to sessions 99-100)**: dedicated crate
   `crates/lvqr-agent-whisper`, NOT a feature-gated
   module inside `lvqr-agent`. Reasoning in the
   crate's lib.rs:

   * Clean isolation: whisper-rs (bindgen + cmake
     against whisper.cpp) and symphonia (AAC decoder
     + format demuxers) do not touch lvqr-agent's
     dep graph.
   * Mirrors the `lvqr-archive` optional-c2pa
     pattern at the workspace level.
   * Future GPU features (`whisper-metal`,
     `whisper-cuda`) will live here without bloating
     lvqr-agent.

   **Feature gating decision**: `whisper-rs 0.16` +
   `symphonia 0.6.0-alpha.2` are pulled in only when
   the `whisper` Cargo feature is enabled (default
   OFF). Without the feature the crate ships its
   always-available surface so `cargo build
   --workspace` stays fast and CI runners without
   Xcode CLT / cmake / libclang do not have to
   compile whisper.cpp on every push.

   **Always-available surface** (compiled by the
   default no-feature build):

   * `TranscribedCaption { broadcast, start_ts,
     end_ts, text }` in the source track's
     timescale.
   * `CaptionStream`: cheaply-cloneable
     `tokio::sync::broadcast`-backed fan-out
     (capacity 256). `subscribe()` returns
     `Receiver`; subscribers connecting after a
     publish do NOT see prior captions (mirror of
     `FragmentBroadcaster`).
   * `WhisperCaptionsFactory`: `name() = "captions"`;
     `build()` opts in only when
     `ctx.track == "1.mp4"`. Cheaply cloneable;
     inner `WhisperConfig` is `Arc`'d so all
     per-broadcast agents share the model path +
     window. Captions handle is also shared so
     downstream consumers can `subscribe()` before
     install.
   * `WhisperCaptionsAgent`: holds the state
     captured at `on_start` plus an
     `Option<WorkerHandle>`. Without the feature
     `on_fragment` is a debug-log no-op (one log
     line per broadcast, not per frame).
   * `mdat::extract_first_mdat`: walks BMFF
     top-level boxes by `(size, type)` header,
     returns the first `mdat` payload as a sliced
     `Bytes` (no copy). Defensive: rejects truncated
     headers, sub-header sizes, and declared sizes
     that overrun the buffer.
   * `asc::extract_asc`: descends
     `moov/trak/mdia/minf/stbl/stsd/mp4a/esds`,
     walks the MPEG-4 descriptor list (ESDescriptor
     0x03 -> DecoderConfigDescriptor 0x04 ->
     DecoderSpecificInfo 0x05) with VLE-length
     support per ISO/IEC 14496-1.

   **Whisper-feature surface** (`--features whisper`):

   * `decode::AacToMonoF32`: stateful symphonia AAC
     decoder + channel downmix + nearest-neighbour
     resample to 16 kHz. Symphonia 0.6.0-alpha.2
     API: `AudioCodecParameters` with `CODEC_ID_AAC`
     + `with_extra_data(asc)`; decoder via
     `get_codecs().get_audio_decoder(CODEC_ID_AAC)`.
     `GenericAudioBufferRef::copy_to_vec_interleaved
     ::<f32>` pulls interleaved PCM; manual chunked
     downmix produces mono. Reusable interleaved
     buffer held across calls so there's only one
     heap allocation per AAC frame on the hot path.
   * `worker::spawn` + `WorkerHandle`: spawns one OS
     thread per agent (NOT a tokio task --
     whisper.cpp inference is CPU-bound and would
     starve the runtime). The agent holds a
     `std::sync::mpsc::sync_channel(64)` `Sender`;
     `on_fragment` calls `try_send` and drops +
     warn-logs on a full channel, never
     back-pressuring the per-broadcast drain task.
     The worker receives `Frame { dts, aac }`
     messages, decodes via the `AacToMonoF32`,
     buffers up to `WhisperConfig::window_ms`
     (default 5000) of PCM, then runs
     `WhisperContext::full` with English-only
     `Greedy { best_of: 1 }` sampling. Segments
     with non-empty trimmed text are emitted onto
     `CaptionStream` as `TranscribedCaption` values;
     `start_ts` / `end_ts` are computed by adding
     the whisper-segment centisecond timestamps
     (scaled to source-track timescale) to the
     window's starting fragment DTS, so consumers
     can align captions against the source DTS axis.
     On channel close (sender dropped) the worker
     drains its remaining PCM, runs one final
     inference pass, then exits.

   **Test gates**:

   * `tests/whisper_basic.rs`: `#[ignore]`
     integration test gated on the `whisper` feature
     AND a `WHISPER_MODEL_PATH` env var. The test
     docblock documents the
     `curl ... ggml-tiny.en.bin` fetch process; the
     model file (~75 MB) is intentionally NOT
     bundled in `lvqr-conformance/fixtures`. Without
     the env var the test logs a single line and
     returns Ok -- absent model is the expected
     default state, not a failure.
   * 27 inline `#[cfg(test)]` lib tests covering:
     mdat malformed-input + empty-mdat (7); ASC
     descriptor VLE + box-chain round-trip + garbage
     input (5); CaptionStream pre/post-subscribe
     semantics + clone state-sharing (3); factory
     audio-only opt-in + opt-out for video / catalog
     (5); agent on_fragment with + without mdat +
     missing-init-segment + sample-rate capture (4);
     decode resampler identity / down-44100/up-8000
     / empty input (4) -- gated to whisper feature
     but mostly verifying the always-available
     surface.

   **Workspace registration**.
   `crates/lvqr-agent-whisper` added to
   `workspace.members` + `workspace.dependencies`.
   Path-only entry, mirroring `lvqr-agent`.

   **Plan refresh**. `tracking/TIER_4_PLAN.md`
   section 4.5 header flipped to "A + B DONE, C-D
   pending"; row 98 B scoped up from one-line to the
   full deliverable + verification record.

2. **Session 98 close doc** (this commit).

### Tests shipped

| # | Test surface | Added this session |
|---|---|---|
| a | `crates/lvqr-agent-whisper/src/mdat.rs` | 7 (happy path, no-mdat, truncated header, lying box size, zero size, empty buffer, empty mdat payload) |
| b | `crates/lvqr-agent-whisper/src/asc.rs` | 5 (VLE descriptor length 1/2/3-byte + truncated; round-trip extract from synthesized init; garbage / empty input) |
| c | `crates/lvqr-agent-whisper/src/caption.rs` | 3 (pre/post-subscribe semantics, no-subscriber publish, clone state sharing) |
| d | `crates/lvqr-agent-whisper/src/factory.rs` | 5 (audio-only opt-in, video opt-out, other-tracks opt-out, name = "captions", config window default + override, captions handle clone) |
| e | `crates/lvqr-agent-whisper/src/agent.rs` | 4 (no-feature on_fragment is no-op, no-mdat fragment is no-op, sample_rate captured from meta, missing init segment handled gracefully) |
| f | `crates/lvqr-agent-whisper/src/decode.rs` | 4 (resampler identity, 44100->16000 downsample, empty input, 8000->16000 upsample) -- gated to whisper feature but compiled by default cargo check |
| g | `crates/lvqr-agent-whisper/tests/whisper_basic.rs` | 1 `#[ignore]` integration test gated on `WHISPER_MODEL_PATH` |

Workspace totals: **823** passed, 0 failed, 1 ignored
(up from session-97's 796 / 0 / 1; +27 new lib tests).
The 1 remaining ignored test is the pre-existing
`moq_sink` doctest unrelated to 4.5.

### Ground truth (session 98 close)

* **Head**: `ac989d8` (feat) + this close-doc
  commit. Local main is N+2 commits ahead of
  `origin/main`. Verify via
  `git log --oneline origin/main..main`. Do NOT
  push without direct user instruction.
* **Tests**: **823** passed, 0 failed, 1 ignored on
  macOS (default features).
* **CI gates locally clean**:
  * `cargo fmt --all`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo test -p lvqr-agent-whisper` 27 passed
  * `cargo check -p lvqr-agent-whisper --features whisper` clean
  * `cargo test --workspace` 823 / 0 / 1
* **Workspace**: **28 crates** (was 27;
  +`lvqr-agent-whisper`).

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91-94 |
| 4.8 | One-token-all-protocols | **COMPLETE** | 95 / 96 |
| 4.5 | In-process AI agents | **A + B DONE**, C-D pending | 97 / 98 / 99 / 100 |
| 4.4 | Cross-cluster federation | PLANNED | 101-103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

### Session 99 entry point

**Tier 4 item 4.5 session C: captions track
publish via `lvqr-moq` + HLS subtitle rendition
wiring in `lvqr-hls`.**

The session-98-B `WhisperCaptionsFactory::captions()`
returns a `CaptionStream` clone that downstream
consumers subscribe to. Session 99 C wires that
subscription into:

1. A new MoQ track per broadcast at name
   `<broadcast>/captions`, where each
   `TranscribedCaption` becomes one MoQ object
   (caption-fragment-as-MoQ-object follows the
   existing `Fragment -> MoqTrackSink` projection
   pattern).
2. `lvqr-hls`'s `MultiHlsServer` gains a subtitle-
   rendition group; the master playlist references
   the captions track via
   `EXT-X-MEDIA TYPE=SUBTITLES`. Browser hls.js
   players auto-subscribe.

**Prerequisites already in place**:

* `WhisperCaptionsFactory::captions() -> CaptionStream`
  is the subscribe entry point (session 98 B).
* `TranscribedCaption` has `start_ts` / `end_ts` in
  the source track's timescale, ready for
  WebVTT-cue serialization.
* `lvqr-hls`'s `MultiHlsServer` already supports
  multiple renditions (audio rendition for video
  broadcasts); subtitle is a third rendition type.

**Pre-session checklist**:

1. Decide: does the captions track go through the
   shared `FragmentBroadcasterRegistry` (under
   track id `"captions"`) or stay on its own
   `CaptionStream`? The registry path makes session
   100 D's CLI wiring + cluster federation
   compatibility cleaner, but the standalone
   stream avoids serializing captions through the
   Fragment model (which is fMP4-shaped and
   awkward for WebVTT cues).
2. Decide: WebVTT serialization in `lvqr-hls` or
   in `lvqr-agent-whisper`? Likely lvqr-hls because
   that's where the playlist composition lives.
3. Decide: caption track init segment shape (HLS
   subtitle renditions accept a webvtt rendition
   without an fMP4 wrapper).

**Verification gates (session 99 C close)**:

* `cargo fmt --all`
* `cargo clippy --workspace --all-targets --benches -- -D warnings`
* `cargo test -p lvqr-cli --test captions_hls_e2e`
* `cargo test --workspace` (expect no regression
  from 823)
* `git log -1 --format='%an <%ae>'` reads
  `Moheeb Zara <hackbuildvideo@gmail.com>` alone

**Biggest risks**, ranked:

1. **WebVTT cue alignment**. Whisper's segment
   timestamps are in centiseconds within an
   inference window; mapping them onto wall-clock
   PROGRAM-DATE-TIME requires the broadcast's start
   PDT plus the source DTS axis. The agent already
   threads the start fragment DTS into
   `TranscribedCaption.start_ts`; the HLS bridge
   needs to add the broadcast's start PDT.
2. **Late subscriber**. A viewer who joins the HLS
   stream mid-broadcast will not see captions
   emitted before they subscribed (CaptionStream is
   `tokio::sync::broadcast`-backed; no history). For
   the v1 demo this is acceptable; future work
   would back the captions with a small DVR window
   in `lvqr-archive`.
3. **HLS subtitle rendition browser support**.
   hls.js handles WebVTT subtitles; native Safari
   does too. Validate against both before declaring
   session 100 D demo-ready.

 Post-session-97-close release activity: GitHub `origin/main` synced (head `6e98553`); README + docs refreshed for 4-of-8 Tier 4 status (commit `bdb5420`); workspace `Cargo.toml` patched to declare `lvqr-conformance` / `lvqr-test-utils` / `lvqr-soak` as path-only workspace deps (commit `6e98553`) so consumer dev-dep manifests are strippable on `cargo publish` (was the blocker on first publish attempt; cargo's package step rejects dev-deps that have a version field but cannot be resolved on the registry); 24 publishable workspace crates published to crates.io at v0.4.0 (8 version-bumps from 0.3.1 + 16 first-time publishes). Notable name re-use: `lvqr-wasm` 0.3.1 was "browser playback bindings" and 0.4.0 is "server-side WASM filter host" -- different content, same name (deliberate per session-44 refactor). The crates.io rate limit (5-burst then 1 new crate per 10 min) was the long pole: chain ran ~90 min wall-clock. `lvqr-cli 0.4.0` is the consumer-facing entry point (`cargo install lvqr-cli` after the chain settles).

## v0.4.0 release event (2026-04-20)

### What landed

1. **GitHub `origin/main` synced**. Commits since
   `ebb8668` (session 95 close):

   * `d0e2ea6` test(auth): Tier 4 item 4.8 session B
     -- cross-protocol auth E2E (one JWT, four
     protocols)
   * `d38d3c5` docs: session 96 close
   * `b8631fa` feat(agent): Tier 4 item 4.5 session A
     -- lvqr-agent scaffold
   * `80ec948` docs: session 97 close
   * `bdb5420` docs: refresh README + architecture +
     quickstart for 4-of-8 Tier 4 status
   * `6e98553` build: drop version field on
     publish=false workspace deps so cargo publish
     strips dev-deps

   Six commits, all pushed. Local main is at
   `origin/main`; verify via
   `git log --oneline origin/main..main` (should be
   empty).

2. **Workspace manifest fix** (`6e98553`).
   `cargo publish` package-step rejects dev-deps
   with a `version` field that cannot be resolved on
   crates.io. Fix: make the workspace.dependencies
   entries for the three publish=false helper crates
   path-only:

   ```toml
   lvqr-conformance = { path = "crates/lvqr-conformance" }
   lvqr-test-utils  = { path = "crates/lvqr-test-utils" }
   lvqr-soak        = { path = "crates/lvqr-soak" }
   ```

   Path-only dev-deps without a version are stripped
   from the published Cargo.toml -- which is exactly
   what publish requires. Local workspace builds
   continue to resolve via the path field. Surfaced
   when publishing `lvqr-codec` (dev-dep on
   `lvqr-conformance` blocked the package step).
   Affects ~9 publishable crates that dev-dep on the
   helpers.

3. **24 publishable crates pushed to crates.io at
   v0.4.0**. The three publish=false helpers
   (`lvqr-conformance`, `lvqr-test-utils`,
   `lvqr-soak`) stayed local.

   **Version bumps from 0.3.1** (existed on
   crates.io previously):
   `lvqr-core`, `lvqr-relay`, `lvqr-mesh`,
   `lvqr-ingest`, `lvqr-signal`, `lvqr-admin`,
   `lvqr-cli`, `lvqr-wasm`.

   **First-time publishes** (16 crates):
   `lvqr-moq`, `lvqr-archive`, `lvqr-auth`,
   `lvqr-observability`, `lvqr-codec`,
   `lvqr-fragment`, `lvqr-cluster`, `lvqr-cmaf`,
   `lvqr-agent`, `lvqr-dash`, `lvqr-hls`,
   `lvqr-record`, `lvqr-rtsp`, `lvqr-srt`,
   `lvqr-whip`, `lvqr-whep`.

   **Notable name re-use**: `lvqr-wasm 0.3.1` on
   crates.io was a browser-playback binding crate
   (deleted in the session-44 refactor); `lvqr-wasm
   0.4.0` is the server-side WASM filter host
   (Tier 4 item 4.2). Same name, different content.
   Pinning to `lvqr-wasm = "0.3"` keeps the old
   crate; bumping to `"0.4"` switches to the new
   one.

4. **README + docs refresh** (`bdb5420`). Reflects
   ground truth: 27 crates, 796 tests, 4-of-8 Tier
   4 items COMPLETE plus the 4.5 scaffold. Crate
   map gains `lvqr-agent` and a "Programmable data
   plane (Tier 4)" subsection. The "What's NOT
   shipped yet" list pruned: 4.1 / 4.3 / 4.8 are
   no longer in it; 4.5's WhisperCaptionsAgent
   (sessions 98-100) is still called out
   explicitly. `docs/architecture.md` +
   `docs/quickstart.md` bumped 25 -> 27 crates.

### Mechanics + gotchas

* **Rate limits**. crates.io enforces 5-burst then
  1 new crate per 10 minutes for first-time
  publishes. With 16 first-time publishes in this
  release, the chain ran ~90 min wall-clock just on
  rate-limit waits. Version-bump publishes are not
  rate-limited the same way -- they were
  interleaved between new-crate slots to fill the
  wait time. `/tmp/lvqr_publish.sh` (not
  committed) is a retry-aware publish script that
  detects 429 and sleeps 70s before retrying;
  preserve it locally if you ever need to cut
  another release.
* **Dependency order**. Built from
  `cargo metadata --no-deps` filtered to regular
  (kind=null) internal deps. Tiers:
    * Tier 0 (no internal deps): lvqr-core,
      lvqr-moq, lvqr-archive, lvqr-auth,
      lvqr-observability, lvqr-codec.
    * Tier 1: lvqr-fragment (lvqr-moq),
      lvqr-cluster (lvqr-core), lvqr-signal
      (lvqr-core).
    * Tier 2: lvqr-cmaf, lvqr-agent, lvqr-wasm,
      lvqr-mesh, lvqr-admin, lvqr-relay,
      lvqr-record.
    * Tier 3: lvqr-dash, lvqr-hls, lvqr-ingest.
    * Tier 4: lvqr-rtsp, lvqr-srt, lvqr-whip,
      lvqr-whep.
    * Tier 5: lvqr-cli (depends on everything).

  cargo publish's wait-for-index-to-update step
  (post 1.66) handled within-tier ordering for free
  -- subsequent dependent publishes saw their deps
  in the registry without explicit sleeps.
* **`--no-verify` was NOT used**. Every published
  crate compiled cleanly from its packaged tarball
  via the standard verify step. The path-only
  manifest fix made `--no-verify` unnecessary.
* **Consumer-facing UX**. `cargo install lvqr-cli`
  works for v0.4.0; the binary boots with the same
  zero-config defaults `cargo run -p lvqr-cli`
  produces locally.

### Tier 4 execution status (unchanged from session 97)

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91 (A) / 92 (B1) / 93 (B2) / 94 (B3) |
| 4.8 | One-token-all-protocols | **COMPLETE** | 95 (A) / 96 (B) |
| 4.5 | In-process AI agents | **A DONE**, B-D pending | 97 (A) / 98 (B) / 99 (C) / 100 (D) |
| 4.4 | Cross-cluster federation | PLANNED | 101-103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

### Session 98 entry point (still: Tier 4 item 4.5 session B)

WhisperCaptionsAgent reading AAC audio, feeding
whisper-rs. The session-97-A `Agent` /
`AgentFactory` / `AgentRunner` surface in
`crates/lvqr-agent` is now live on crates.io; the
new agent registers against an existing
`AgentRunner` instance. See `tracking/TIER_4_PLAN.md`
section 4.5 row 98 B + the session 97 close block
below for the design notes that carry forward.

 Tier 4 item 4.5 session A landed the in-process AI agents framework scaffold under a new `crates/lvqr-agent`. Surface: `Agent` sync trait (`on_start(&AgentContext)` + `on_fragment(&Fragment)` + `on_stop()` lifecycle, all default-no-op except `on_fragment`); `AgentContext { broadcast, track, meta: FragmentMeta }`; `AgentFactory { name, build(&AgentContext) -> Option<Box<dyn Agent>> }` (per-stream opt-in via `None`); `AgentRunner` builder + `install(&FragmentBroadcasterRegistry) -> AgentRunnerHandle` that wires one `on_entry_created` callback, subscribes synchronously inside the callback, and spawns one tokio drain task per agent factory opts in for. The natural `BroadcasterStream::Closed` termination IS the broadcast-stop signal; no separate `on_entry_removed` wiring (would race the drain loop and double-fire `on_stop`). Every `on_start`/`on_fragment`/`on_stop` call wrapped in `std::panic::catch_unwind(AssertUnwindSafe(..))`; panics in `on_fragment` are logged + counted but do NOT terminate the drain (one bad frame must not kill the agent), panics in `on_start` DO skip the drain entirely. `AgentRunnerHandle` exposes per-`(agent, broadcast, track)` `fragments_seen` + `panics` counters mirror of `WasmFilterBridgeHandle`. Pattern-matches the four existing `FragmentBroadcasterRegistry` consumers (HLS bridge, archive indexer, WASM filter tap, cluster claim) so session 98 drops a `WhisperCaptionsFactory` in without re-deriving the callback / spawn / drain boilerplate. No CLI wiring (session 100 D). No concrete agent (session 98 B). Workspace tests: **796** passing (up from 786; +8 lib runner tests, +1 integration, +1 doctest). Workspace count now **27 crates** (was 26). Session 98 entry point is Tier 4 item 4.5 session B (`WhisperCaptionsAgent` reading AAC audio + feeding whisper-rs -- read `tracking/TIER_4_PLAN.md` section 4.5 row 98 B).

## Session 97 close (2026-04-19)

### What shipped

1. **Tier 4 item 4.5 session A: `lvqr-agent`
   scaffold + Agent trait + Runner + lifecycle
   wiring** (`b8631fa`).

   **New crate `crates/lvqr-agent`**. Workspace
   member, AGPL-3.0-or-later, edition 2024, lines
   capped at 120, follows the four existing
   `FragmentBroadcasterRegistry` consumer patterns
   exactly (HLS bridge, archive indexer, WASM filter
   tap, cluster claim renewer). Five files in `src/`:
   `lib.rs` (re-exports + crate docs), `agent.rs`
   (Agent trait + AgentContext), `factory.rs`
   (AgentFactory trait), `runner.rs` (AgentRunner +
   AgentRunnerHandle + AgentStats + drive task), and
   one integration test under `tests/`.

   **Surface decisions baked in (carry forward to
   sessions 98-100)**:

   * `Agent` is **sync**, not async. Agents that
     need async or blocking work (e.g. whisper-rs) are
     expected to spawn from inside `on_start`
     (typical pattern: bounded `tokio::sync::mpsc` to
     a worker task that owns the heavy state).
     Putting an async fn on the trait would force
     `async_trait` boxing or a Pin<Box<dyn Future>>
     return type and gain nothing for whisper, which
     is sync anyway. Documented in the trait's
     module-level rust-docs.
   * `Agent` is `Send` (no `Sync`). Each agent runs
     on a single drain task; concurrent calls to the
     same agent never happen.
   * **Factory pattern**, not just `Box<dyn Agent>`:
     a factory is registered per agent *type* on the
     `AgentRunner`; the factory is consulted on every
     new `(broadcast, track)` and either returns
     `Some(Box<dyn Agent>)` or `None` to skip. This
     is the cleanest way to express "agent type X
     wants every audio track but no video tracks";
     `AgentFactory::build(&AgentContext)` gets the
     full triple to decide on.
   * **No `on_entry_removed` wiring**. The drain
     loop's natural termination (every producer-side
     clone of the broadcaster has been dropped ->
     `BroadcasterStream::next_fragment()` returns
     `None` -> drain loop exits -> `on_stop` fires)
     IS the broadcast-stop signal. Adding a second
     teardown channel would race the drain loop in
     flight and double-fire `on_stop`. Documented.
   * **Panic isolation** via
     `std::panic::catch_unwind(AssertUnwindSafe(..))`
     around every `on_start` / `on_fragment` /
     `on_stop` trait call. Counted on
     `AgentStats::panics` AND on
     `lvqr_agent_panics_total{agent, phase=start|fragment|stop}`.
     `on_fragment` panics do NOT terminate the drain
     loop; `on_start` panics DO skip the drain
     entirely (running on_fragment after a failed
     start would hand fragments to an
     uninitialised agent). `on_stop` panics are
     absorbed.
   * **Per-fragment metric**:
     `lvqr_agent_fragments_total{agent}` bumps once
     per fragment regardless of panic outcome.
     `AgentRunnerHandle::fragments_seen` and
     `panics` accessors mirror
     `WasmFilterBridgeHandle::fragments_seen`.

   **Test coverage** (8 lib + 1 integration + 1
   doctest):

   | # | Test | Asserts |
   |---|---|---|
   | 1 | `agent_receives_every_emitted_fragment_then_stops` | start fires once + each emitted fragment lands in `on_fragment` + on_stop fires once after producer drop + remove |
   | 2 | `factory_returning_none_is_skipped` | a factory that opts out (e.g. audio-only) gets no drain task spawned for the video key |
   | 3 | `panic_in_on_fragment_is_caught_and_counted_loop_continues` | a panicky agent at group-1 does not kill the drain loop; counters reflect 3 seen + 1 panic; downstream subscriber still sees every fragment unmodified |
   | 4 | `panic_in_on_start_skips_drain_loop` | on_start panic prevents on_fragment from running; counters reflect 0 seen + 1 panic |
   | 5 | `empty_runner_installs_callback_but_spawns_nothing` | runner with no factories is a no-op installer |
   | 6 | `multiple_factories_each_get_their_own_drain_per_broadcast` | two factories on same broadcast each spawn their own drain task with separate stats |
   | 7 | `agent_runner_default_is_empty` | Default impl |
   | 8 | `agent_runner_handle_debug_redacts_internals` | Debug impl reports tracked-key count without leaking internals |
   | 9 | `tests/integration_basic.rs::end_to_end_lifecycle_under_real_registry` | full start -> N fragments -> stop ordering on a multi-thread runtime, mirroring the shape `lvqr_cli::start` will use in session 98 |
   | 10 | `runner.rs:97 doctest` | `AgentRunner::new().with_factory(F).install(&registry)` API compiles |

   **Workspace registration**. `Cargo.toml`
   `workspace.members` adds `crates/lvqr-agent`;
   `workspace.dependencies` adds
   `lvqr-agent = { version = "0.4.0", path = "crates/lvqr-agent" }`.
   No CLI dependency edge added this session
   (session 98 / 100 will add it when the CLI
   threads `AgentRunner::install` through
   `lvqr_cli::start`).

   **Plan refresh**.
   `tracking/TIER_4_PLAN.md` section 4.5 header
   flipped to "A DONE, B-D pending"; row 97 A
   scoped up from one-line to the full deliverable +
   verification record. Rows 98-100 stay as
   one-liners (the implementing session for each
   will scope them up in-commit per CLAUDE.md's
   plan-vs-code rule).

2. **Session 97 close doc** (this commit).

### Tests shipped

| # | Test surface | Added this session |
|---|---|---|
| a | `crates/lvqr-agent/src/runner.rs` unit tests | 8 new (lifecycle, opt-out, panic isolation start + fragment, multi-factory, default, debug) |
| b | `crates/lvqr-agent/tests/integration_basic.rs` | 1 new (end-to-end start-drain-stop on real registry across thread boundary) |
| c | `crates/lvqr-agent/src/runner.rs` rustdoc example | 1 new (`AgentRunner::new().with_factory(F).install(&registry)` compiles) |

Workspace totals: **796** passed, 0 failed, 1
ignored (up from session 96's 786 / 0 / 1). The +10
breakdown is the 8 lib runner tests + 1 integration
+ 1 doctest. The 1 remaining ignored test is the
pre-existing `moq_sink` doctest unrelated to 4.5.

### Ground truth (session 97 close)

* **Head**: this session's feat commit `b8631fa` +
  the close-doc commit on local `main`. Local main
  is now N+2 commits ahead of `origin/main` (4
  commits ahead total: session 96's two + this
  session's two). Verify via
  `git log --oneline origin/main..main` before any
  push. Do NOT push without direct user
  instruction.
* **Tests**: **796** passed, 0 failed, 1 ignored on
  macOS (default features). With `--features c2pa`:
  unchanged.
* **CI gates locally clean**:
  * `cargo fmt --all`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo test -p lvqr-agent` 8 lib + 1 integration + 1 doctest = 10 passed
  * `cargo test --workspace` 796 / 0 / 1
* **Workspace**: **27 crates** (was 26; +lvqr-agent).

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91 (A) / 92 (B1) / 93 (B2) / 94 (B3) |
| 4.8 | One-token-all-protocols | **COMPLETE** | 95 (A) / 96 (B) |
| 4.5 | In-process AI agents | **A DONE**, B-D pending | 97 (A) / 98 (B) / 99 (C) / 100 (D) |
| 4.4 | Cross-cluster federation | PLANNED | 101-103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

### Session 98 entry point

**Tier 4 item 4.5 session B: `WhisperCaptionsAgent`
reading AAC audio, feeding whisper-rs.**

The session-97-A scaffold makes this a self-contained
deliverable that does NOT touch `lvqr-agent` itself:
the new agent + factory live in their own module
(probably `crates/lvqr-agent/src/whisper.rs` behind
a `whisper` feature flag, or in a dedicated
`crates/lvqr-agent-whisper/` crate -- the choice is
session 98 B's to make in-commit). The session 97 A
plan refresh notes the WhisperCaptionsFactory will
register against an existing `AgentRunner` -- no
new abstractions on the agent side.

**Prerequisites already in place**:

* `Agent` / `AgentFactory` / `AgentRunner` /
  `AgentRunnerHandle` ship in session 97 A.
* `AgentContext` carries `FragmentMeta`, so the
  whisper agent reads the AAC track's timescale +
  init segment without a registry round-trip.
* Panic isolation around `on_fragment` means a
  whisper-rs FFI fault on a single fragment will
  log + count on `lvqr_agent_panics_total{agent="captions",phase="fragment"}`
  but not kill the drain loop.
* `lvqr_agent_fragments_total{agent}` is already
  exported, so the whisper agent's per-broadcast
  fragment counter shows up in Prometheus
  immediately.

**Pre-session checklist**:

1. Decide the whisper-rs version pin and
   `--whisper-model` path semantics; both are
   session-98-B in-commit refreshes of section 4.5
   row 98 B.
2. Decide whether to land the AAC -> PCM decode in
   `lvqr-agent-whisper` or in `lvqr-codec`. The
   roadmap says symphonia is the decoder; the
   integration crate is session-98-B's call.
3. Confirm `whisper-rs` builds on macOS without GPU
   features (the `whisper-metal` / `whisper-cuda`
   feature flags stay deferred per section 4.5
   anti-scope).

**Verification gates (session 98 B close)**:

* `cargo fmt --all`
* `cargo clippy --workspace --all-targets --benches -- -D warnings`
* `cargo test -p lvqr-agent --test whisper_basic`
  (or `cargo test -p lvqr-agent-whisper` if that
  crate lands)
* `cargo test --workspace` (expect no regression
  from 796)
* `git log -1 --format='%an <%ae>'` reads
  `Moheeb Zara <hackbuildvideo@gmail.com>` alone

**Biggest risks**, ranked:

1. **whisper-rs build on macOS**. The crate uses
   `bindgen` against whisper.cpp; the build can
   require Xcode CLT. Pin a known-good version and
   document the rustup toolchain prereqs in
   `crates/lvqr-agent-whisper/README.md` (or the
   crate's lib.rs head).
2. **Test fixture model size**. ggml-tiny is ~75
   MB; landing it in `lvqr-conformance/fixtures`
   would balloon the repo. Better: download-on-
   demand under a `cargo xtask` script or a
   `WHISPER_MODEL_PATH` env var that gates the
   whisper test.
3. **AAC -> PCM via symphonia**. symphonia's AAC
   decoder is the only mainstream pure-Rust option
   today; verify the decode path lines up with
   what the FLV-tagged audio fragments LVQR's
   ingest produces.



## Session 96 close (2026-04-19)

### What shipped

1. **Tier 4 item 4.8 session B: one-JWT-every-
   protocol cross-protocol E2E** (this commit).

   New integration test at
   `crates/lvqr-cli/tests/one_token_all_protocols.rs`
   exercises the full session-95-A wiring on a single
   `TestServer` instance. Three `#[tokio::test]`
   cases per the matrix the handoff pinned:

   * **`one_publish_jwt_admits_every_protocol`** --
     positive path. Mints one publish-scoped JWT bound
     to `live/cam1` and drives every ingest surface
     with it: RTMP via `rml_rtmp` publish handshake
     (stream key IS the JWT, broadcast on the wire is
     `live/<jwt>`); WHIP via raw HTTP POST with
     `Authorization: Bearer <jwt>` and a minimal SDP
     body that passes the `Content-Type: application/
     sdp` + non-empty checks; SRT via `srt-tokio`
     caller with streamid `m=publish,r=live/cam1,
     t=<jwt>`; RTSP via raw TCP ANNOUNCE with
     `Authorization: Bearer <jwt>`. Asserts publish
     accepted on RTMP, WHIP returned a non-401
     status, SRT socket connected (deny would be
     `ConnectionRefused` from
     `ServerRejectReason::Unauthorized`), RTSP
     ANNOUNCE returned 200.
   * **`wrong_secret_jwt_is_rejected_everywhere`** --
     negative path. A token signed with a different
     secret is decoded as an error by
     `JwtAuthProvider`, and every protocol must
     refuse: RTMP server drops the socket
     (rml_rtmp's `validate_publish` returning false
     causes the connection task to `return Ok(())`),
     WHIP 401, SRT `io::ErrorKind::ConnectionRefused`
     surfaced from the SRT handshake reject, RTSP 401.
   * **`wrong_broadcast_jwt_is_rejected_on_whip_srt
     _rtsp_only`** -- the documented per-protocol
     asymmetry. A JWT bound to `live/other`
     published against `live/cam1` is denied by
     WHIP/SRT/RTSP because they each carry the
     target broadcast at auth time and
     `JwtAuthProvider`'s Publish branch enforces the
     `broadcast` claim binding when present. RTMP is
     **admitted** because `extract_rtmp` passes
     `broadcast: None` (the broadcast on the wire is
     `app/key` where `key` is the JWT itself, so
     adding a binding would double-count). The
     anti-scope is documented in
     `crates/lvqr-auth/src/extract.rs::extract_rtmp`
     and the rationale baked into
     `JwtAuthProvider::check`.

   **Test infrastructure**. The test relies entirely
   on existing dev-deps (`rml_rtmp`, `srt-tokio`,
   `jsonwebtoken`, `lvqr-test-utils`) and the
   session 95 A `TestServerConfig::with_whip()`
   builder. No new production code, no new dev-deps.
   The minimal SDP offer body
   `b"v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n"` is
   intentionally bare-bones: it's enough to pass the
   `require_sdp_content_type` + non-empty checks so
   the auth gate fires, and the test then accepts
   any non-401 response as proof that auth allowed.
   The `str0m` answerer may 400 or 201 the offer
   downstream; either way the gate fired Allow.

   **Race-condition pad on RTMP** (one-line gotcha):
   the helper sleeps 50ms between the end of the
   handshake and the `connect` command so the
   server's post-handshake control messages (window
   ack size, set peer bandwidth, onBWDone) arrive
   before the client serializes connect. Without it,
   the deserializer sees connect-response chunks
   interleaved with the prerequisite control
   messages and cannot reassemble them. Same wait
   pattern as `crates/lvqr-cli/tests/rtmp_archive
   _e2e.rs::connect_and_publish`. Documented inline.

   **Plan refresh**.
   `tracking/TIER_4_PLAN.md` section 4.8 header
   flipped to **COMPLETE**; the 96 B row flipped to
   DONE with the deliverable + verification record.
   Item 4.8 takes its final spot in the Tier 4
   execution status table next to 4.1 / 4.2 / 4.3.

2. **Session 96 close doc** (this commit).

### Tests shipped

| # | Test surface | Added this session |
|---|---|---|
| a | `crates/lvqr-cli/tests/one_token_all_protocols.rs` | 3 new: positive (one JWT five protocols), wrong-secret deny everywhere, wrong-broadcast deny on WHIP/SRT/RTSP only (RTMP admits) |

Workspace totals: **786** passed, 0 failed, 1
ignored (up from session 95's 783 / 0 / 1). The +3
breakdown is the three new cross-protocol cases.
The 1 remaining ignored test is the pre-existing
`moq_sink` doctest unrelated to 4.8.

### Ground truth (session 96 close)

* **Head**: this session's feat commit + close-doc
  commit. Local main is N+2 commits ahead of
  `origin/main` after both land. Verify via
  `git log --oneline origin/main..main` before any
  push. Do NOT push without direct user
  instruction.
* **Tests**: **786** passed, 0 failed, 1 ignored on
  macOS (default features). With `--features c2pa`:
  unchanged (35 lib + 5 integration on
  lvqr-archive; +1 E2E on lvqr-cli).
* **CI gates locally clean**:
  * `cargo fmt --all`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo test -p lvqr-cli --test one_token_all_protocols` 3 passed
  * `cargo test --workspace` 786 / 0 / 1
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91 (A) / 92 (B1) / 93 (B2) / 94 (B3) |
| 4.8 | One-token-all-protocols | **COMPLETE** | 95 (A) / 96 (B) |
| 4.5 | In-process AI agents | PLANNED | 97-100 |
| 4.4 | Cross-cluster federation | PLANNED | 101-103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

### Session 97 entry point

**Tier 4 item 4.5 session A: in-process AI agents
framework.** Read `tracking/TIER_4_PLAN.md` section
4.5 row 97 A for the scoped deliverable. The
on-entry-created / on-entry-removed lifecycle hooks
on `FragmentBroadcasterRegistry` (mint candidates
flagged in session 94's close + session 95's status
memory) are load-bearing for 4.5 and ready to be
consumed.

## Session 95 close (2026-04-19)

### What shipped

1. **Tier 4 item 4.8 session A: one-token-all-
   protocols extractor layer + new WHIP/SRT/RTSP
   auth gates** (`3384ba0`).

   **New `lvqr_auth::extract` module**. One
   `extract_<proto>` helper per surface. Each turns
   raw token-carrier bytes (RTMP stream key / WHIP
   `Authorization` header / SRT streamid KV /
   RTSP `Authorization` header / WS resolved token)
   into a uniform `AuthContext::Publish`. Neither
   extractor rejects; missing / malformed carriers
   produce an empty-key context that the provider
   decides on. `NoopAuthProvider` allows open
   access; `Jwt` / `Static` deny empty keys.
   Helpers also include `parse_bearer` (RFC-6750
   case-insensitive scheme) + `parse_srt_streamid`
   (LVQR adopts `m=publish,r=<broadcast>,t=<jwt>`;
   tolerates key order, unknown keys, and the
   legacy `#!::` access-control prefix).

   **Three new ingest auth call-sites** (had zero
   auth references at session 94 close):

   * `lvqr-whip`: `WhipServer::with_auth_provider`
     + `WhipServer::auth()` surface; `handle_offer`
     consults `extract_whip` on the `Authorization`
     header before `create_session`; Deny returns
     401 via new `WhipError::Unauthorized(String)`
     variant. Three integration tests in
     `integration_signaling.rs` cover valid / missing
     / wrong bearer. No session created on deny.
   * `lvqr-srt`: `SrtIngestServer::with_auth`
     builder; the listener loop runs `extract_srt`
     on the streamid **before** `req.accept(None)`,
     rejecting on Deny with
     `RejectReason::Server(ServerRejectReason::
     Unauthorized)` (SRT code 2401). No task
     spawns on deny. Broadcast name comes from the
     streamid's `r=` key when present (fall-through
     is the full streamid for backwards compat with
     the pre-session-95 naming convention).
   * `lvqr-rtsp`: `RtspServer::with_auth` builder;
     `handle_request` gates `ANNOUNCE` + `RECORD`
     only -- `DESCRIBE` / `PLAY` pass through
     because LVQR's RTSP is publish-only today.
     Deny returns RTSP `401 Unauthorized` via new
     `Response::unauthorized()` constructor;
     connection state is not mutated on deny. Five
     unit tests cover the gate (valid / missing /
     wrong bearer on ANNOUNCE; RECORD-without-
     bearer-after-ANNOUNCE; DESCRIBE-not-gated).

   **Two existing call-sites migrate** onto the
   shared helpers:

   * `lvqr-ingest` `bridge.rs:455`: the RTMP
     `on_publish` validator now calls
     `extract::extract_rtmp(app, key)` rather than
     constructing `AuthContext::Publish` inline.
   * `lvqr-cli` `lib.rs:1415` (WS ingest): calls
     `extract::extract_ws_ingest` on the resolved
     token (the existing `resolve_ws_token`
     subprotocol / bearer / query-fallback chain
     is unchanged); now the broadcast name is
     threaded into `AuthContext::Publish` so WS
     ingest participates in per-broadcast JWT
     binding like WHIP / SRT / RTSP.

   **Breaking change to `AuthContext::Publish`**.
   Gains `broadcast: Option<String>`. WHIP / SRT /
   RTSP / WS ingest pass `Some(name)`; RTMP passes
   `None`. `JwtAuthProvider::check`'s Publish
   branch now reads the field as
   `broadcast_filter` and enforces binding when
   Some -- matches the existing Subscribe shape.
   RTMP skips the binding because the stream key
   IS the JWT, so adding it would double-count.
   All call sites migrated in-commit per
   CLAUDE.md's no-shim rule. Three provider impls
   updated (Noop / Static / Jwt); +1 new test in
   `jwt_provider::tests::publish_broadcast_filter_
   enforced_when_present` locks the behaviour.

   **ServeConfig threading**. The one `SharedAuth`
   built in `lvqr_cli::start` (from
   `ServeConfig.auth`) now flows through to
   `WhipServer::with_auth_provider` + `SrtIngest
   Server::with_auth` + `RtspServer::with_auth`
   alongside the existing `RtmpMoqBridge::with_auth`
   shape. Zero behaviour change for operators
   running the default `NoopAuthProvider`.

   **`TestServerConfig::with_whip()`**. Added so
   session 96 B's one-token-five-protocols E2E can
   bind RTMP + WHIP + SRT + RTSP + WS ingest on a
   single `TestServer`. `TestServer::whip_addr()`
   accessor added.

   **`docs/auth.md`**. New document: provider table
   (Noop / Static / Jwt), JWT claim shape (`sub`,
   `exp`, `scope`, optional `iss`, `aud`,
   `broadcast`), per-protocol carrier conventions
   (one section per RTMP / WHIP / SRT / RTSP / WS),
   worked ffmpeg / curl / wscat examples, and the
   "one JWT, five protocols" example that pins the
   session-96-B target user-experience.

   **Plan refresh**. `tracking/TIER_4_PLAN.md`
   section 4.8 header flipped to "A DONE, B
   pending"; the session-95-A row flipped to
   **DONE** with the new-call-site + breaking-
   change notes.

2. **Session 95 close doc** (this commit).

### Tests shipped

| # | Test surface | Added this session |
|---|---|---|
| a | `crates/lvqr-auth/src/extract.rs` unit tests | 16 new covering all five extractors + bearer parser + streamid parser edge cases |
| b | `crates/lvqr-auth/src/jwt_provider.rs` unit tests | 1 new: `publish_broadcast_filter_enforced_when_present` |
| c | `crates/lvqr-whip/tests/integration_signaling.rs` | 3 new: valid bearer returns 201, missing bearer returns 401, wrong bearer returns 401 |
| d | `crates/lvqr-rtsp/src/server.rs` unit tests | 5 new: ANNOUNCE valid/missing/wrong; RECORD without bearer after authed ANNOUNCE; DESCRIBE not gated |

Workspace totals: **783** passed, 0 failed, 1
ignored (up from session 94's 758 / 0 / 1). The +25
breakdown: +16 extract, +1 jwt, +3 whip, +5 rtsp.
The 1 remaining ignored test is the pre-existing
`moq_sink` doctest unrelated to 4.8.

### Ground truth (session 95 close)

* **Head**: `3384ba0` (feat) before this close-doc
  commit lands; after both land local main is 16
  commits ahead of `origin/main`. Verify via
  `git log --oneline origin/main..main` before any
  push. Do NOT push without direct user
  instruction.
* **Tests**: **783** passed, 0 failed, 1 ignored on
  macOS (default features). With `--features c2pa`
  on lvqr-archive: 35 lib + 5 integration, 0
  ignored. With `--features c2pa` on lvqr-cli:
  +1 E2E (`c2pa_verify_e2e`), 0 ignored.
* **CI gates locally clean**:
  * `cargo fmt --all`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo test -p lvqr-auth --lib --all-features` (29 passed)
  * `cargo test --workspace`
  * `cargo test -p lvqr-archive --features c2pa`
  * `cargo test -p lvqr-cli --features c2pa --test c2pa_verify_e2e`
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91 (A) / 92 (B1) / 93 (B2) / 94 (B3) |
| 4.8 | One-token-all-protocols | **A DONE**, B pending | 95 (A) / 96 (B) |
| 4.5 | In-process AI agents | PLANNED | 97-100 |
| 4.4 | Cross-cluster federation | PLANNED | 101-103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

### Session 96 entry point

**Tier 4 item 4.8 session B: one-JWT-five-protocols
E2E.**

Deliverable per `tracking/TIER_4_PLAN.md` section
4.8 row 96 B: integration test at
`crates/lvqr-cli/tests/one_token_all_protocols.rs`
that brings up a single `TestServer` with all five
ingest protocols + a `JwtAuthProvider`, mints one
publish-scoped JWT bound to `live/cam1`, and
publishes via each of RTMP / WHIP / SRT / RTSP +
subscribes via WS with the same token. Assertions:

* Each protocol accepts the token (publish succeeds,
  broadcast appears in the shared registry).
* Each protocol rejects a wrong-token variant
  (RTMP on validate_publish; WHIP 401; SRT 2401;
  RTSP 401; WS ingest 401).
* A token bound to `live/other` is rejected by
  WHIP / SRT / RTSP / WS ingest (those carry the
  broadcast name at auth time); RTMP accepts it
  because the stream key IS the JWT and the
  broadcast is `app/key` on the wire.

**Prerequisites already in place**:

* `TestServerConfig::with_whip()` shipped in 95 A.
* `TestServerConfig::with_srt()` / `with_rtsp()`
  pre-existed.
* `lvqr-auth` exposes `JwtAuthConfig` +
  `JwtAuthProvider` + `JwtClaims` behind the `jwt`
  feature.
* `extract::parse_srt_streamid` + `parse_bearer`
  are usable from test code to build valid SRT
  streamids + Authorization headers.

**Pre-session checklist**:

1. Read `docs/auth.md` for the claim shape + per-
   protocol token-carrier conventions so the E2E
   constructs the right SRT streamid /
   Authorization header / stream key for each
   protocol.
2. Confirm the five protocols all land on the
   shared `FragmentBroadcasterRegistry` that the
   subscribe side can drain (RTMP/WHIP/SRT/RTSP
   all do today; verify by reading each server's
   `with_registry` path if unsure).
3. For the publish assertions, use short blocking
   publishes (one keyframe) + a 1s timeout on
   registry `get_or_create` + `meta().init_
   segment.is_some()` to confirm the fragment
   arrived. Avoid full video-over-network timings.
4. Feature-gate the test on `feature = "jwt"` on
   lvqr-cli's dev-deps if not already exposed;
   otherwise add the feature to `lvqr-cli`'s
   Cargo.toml dev-dependencies.

**Verification gates (session 96 B close)**:

* `cargo fmt --all`
* `cargo clippy --workspace --all-targets --benches -- -D warnings`
* `cargo test -p lvqr-cli --test one_token_all_protocols`
* `cargo test --workspace` (expect no regression
  from 783; +5 to +8 for the new E2E assertions)
* `git log -1 --format='%an <%ae>'` reads
  `Moheeb Zara <hackbuildvideo@gmail.com>` alone

**Biggest risks**, ranked:

1. SRT and RTSP real-network publishing takes
   time; keep the E2E on localhost with `port: 0`
   pre-binds (existing `TestServer` pattern) and
   use a 10s hard timeout per protocol.
2. WHIP requires a real SDP offer to reach the
   auth gate. A minimal `str0m` offer pattern is
   already in `crates/lvqr-whip/tests/e2e_str0m_
   loopback.rs` -- lift it rather than handcraft
   a fresh SDP.
3. The "wrong-token" assertions must hit each
   protocol's reject path cleanly; RTMP rejects
   at the callback (client sees connection close,
   not a status code), WHIP/RTSP/WS return 401,
   SRT returns 2401 at handshake. Design the
   assertion matrix to accept any of
   "connection refused" / 401 / 2401 depending
   on the protocol.

## Session 94 close (2026-04-19)

### What shipped

1. **Tier 4 item 4.3 session B3: drain-terminated
   C2PA finalize + admin verify route + E2E**
   (`56ba151`). Five deliverables in one commit,
   closing out item 4.3:

   **(a) `on_entry_removed` lifecycle hook on
   `FragmentBroadcasterRegistry`**. Mirror of
   `on_entry_created` -- `(broadcast, track, &Arc<
   FragmentBroadcaster>)` triple, fires synchronously
   from `remove()` after the map write lock is
   released (callbacks may freely re-enter the
   registry), in installation order, NEVER from Drop
   (deterministic fire point for 4.4 federation
   gossip + 4.5 agent shutdown; no Drop-reentrancy
   hazards). `RtmpMoqBridge::on_unpublish` now calls
   `registry.remove(stream_name, "0.mp4")` + audio so
   drain tasks see `next_fragment() -> None` per-
   broadcast (was per-server-shutdown).

   **(b) Init-bytes persistence** to flat
   `<archive>/<broadcast>/<track>/init.mp4`. Layout
   picked over `metadata.json` sidecar for three
   reasons (parallels segment layout for non-c2pa
   consumers, bytes already MP4 so concat is literal,
   no extra JSON surface needed today). New
   `lvqr_archive::writer::write_init` +
   `init_segment_path` + `INIT_SEGMENT_FILENAME`
   helpers. Drain task refreshes meta each loop
   iteration and persists on first fragment where
   init is set.

   **(c) Drain-task integration**.
   `BroadcasterArchiveIndexer::drain` takes
   `Option<C2paConfig>` (feature-gated) and, on
   while-loop exit, spawn_blocking's
   `finalize_broadcast_signed` which reads
   `init.mp4`, walks the redb segment index in
   `start_dts` order, concats, signs, writes
   `finalized.mp4` + `finalized.c2pa`. Errors log
   `warn!`; no retry.

   **(d) Admin verify route**.
   `GET /playback/verify/{*broadcast}` (`crates/lvqr-
   cli/src/archive.rs::verify_router`) reads the
   finalize pair off disk, calls
   `c2pa::Reader::from_context(Context::new()).
   with_manifest_data_and_stream(..)`, returns JSON
   `{ signer, signed_at, valid, validation_state,
   errors }`. `validation_state` is the stable
   string form of `c2pa::ValidationState`
   (`"Invalid"` / `"Valid"` / `"Trusted"`); `valid`
   is true for Valid + Trusted. `errors` filters out
   `signingCredential.untrusted` (c2pa-rs itself
   treats it as non-fatal). Auth runs the same
   subscribe-token gate the sister `/playback/*`
   routes use.

   **(e) E2E test** at
   `crates/lvqr-cli/tests/c2pa_verify_e2e.rs`. Real
   RTMP publish via `rml_rtmp`, drop publisher, poll
   for `finalized.c2pa` on disk with a 10 s budget,
   hit `/playback/verify/live/dvr`, assert
   `valid=true`, `validation_state="Valid"`,
   non-empty signer, empty errors; also asserts 404
   on an unknown broadcast.

   **Breaking API change**. New `C2paSignerSource`
   enum with `CertKeyFiles { signing_cert_path,
   private_key_path, signing_alg,
   timestamp_authority_url }` +
   `Custom(Arc<dyn c2pa::Signer + Send + Sync>)`
   variants. The old inline PEM fields on
   `C2paConfig` move into the `CertKeyFiles`
   variant; migration is a single-file diff per
   operator:

   ```
   // was:
   C2paConfig {
       signing_cert_path, private_key_path,
       signing_alg, timestamp_authority_url,
       assertion_creator, trust_anchor_pem,
   }
   // now:
   C2paConfig {
       signer_source: C2paSignerSource::CertKeyFiles {
           signing_cert_path, private_key_path,
           signing_alg, timestamp_authority_url,
       },
       assertion_creator, trust_anchor_pem,
   }
   ```

   The `Custom` variant covers two real shapes with
   one enum: tests using `c2pa::EphemeralSigner`
   (no disk PEMs -- the B3 E2E shape), operators
   with HSM / KMS-backed keys wrapping their signer
   behind `c2pa::Signer`. Per CLAUDE.md's no-backwards-
   compat-shims rule, there is no migration helper;
   existing callers update the struct literal. Two
   new unit tests
   (`sign_asset_bytes_with_custom_signer_source_
   delegates_to_ephemeral_signer`,
   `finalize_broadcast_signed_with_custom_signer_
   source_writes_pair_to_disk`) lock the enum-
   branching behaviour.

   **Feature plumbing**:
   * `lvqr-cli` gains a `c2pa` feature enabling
     `lvqr-archive/c2pa` + `dep:c2pa` (default off;
     `full` meta-feature adds it).
   * `ServeConfig.c2pa: Option<C2paConfig>` is
     `#[cfg(feature = "c2pa")]` so the struct stays
     ABI-stable across feature flips.
   * `lvqr-test-utils` gains a `c2pa` feature +
     `TestServerConfig::with_c2pa(..)` builder.
     Enabled via dev-deps on `lvqr-cli` so
     `cargo test -p lvqr-cli --features c2pa`
     activates the full stack.

   **Plan refresh**. `tracking/TIER_4_PLAN.md`
   section 4.3 header flipped to COMPLETE; the B3
   row flipped to DONE with a full description of
   what landed.

2. **Session 94 close doc** (this commit).

### Tests shipped

| # | Test surface | Added this session |
|---|---|---|
| a | `crates/lvqr-fragment/src/registry.rs` unit tests | 4 new: `on_entry_removed_fires_exactly_once_per_successful_remove`, `on_entry_removed_multiple_callbacks_all_fire_in_installation_order`, `on_entry_removed_callback_receives_the_just_removed_arc`, `on_entry_removed_callback_may_reenter_registry_without_deadlock` |
| b | `crates/lvqr-archive/src/writer.rs` unit tests | 4 new: `init_segment_path_follows_broadcast_track_layout`, `write_init_creates_missing_parent_dirs_and_writes_bytes`, `write_init_is_idempotent_overwrites_existing_file`, `write_init_returns_io_error_when_archive_dir_is_a_file` |
| c | `crates/lvqr-archive/tests/c2pa_sign.rs` | 2 new: `sign_asset_bytes_with_custom_signer_source_delegates_to_ephemeral_signer`, `finalize_broadcast_signed_with_custom_signer_source_writes_pair_to_disk`. Existing 3 migrated to the `C2paSignerSource::CertKeyFiles` enum shape. |
| d | `crates/lvqr-cli/tests/c2pa_verify_e2e.rs` | 1 new: `rtmp_publish_then_unpublish_yields_verifiable_c2pa_manifest` -- the full RTMP + finalize + verify E2E |

Workspace totals: **758** passed, 0 failed, 1 ignored
(up from session 93's 739 / 0 / 1). The +19 breakdown:
+4 registry, +4 writer, +2 c2pa_sign, +1 c2pa_verify_e2e,
+5 provenance lib tests that are now activated in
workspace builds because `lvqr-test-utils`'s new `c2pa`
dev-dep feature pulls in `lvqr-archive/c2pa`, +3 misc
(re-counted doctests across feature configurations).
The 1 remaining ignored test is the pre-existing
`moq_sink` doctest unrelated to 4.3.

### Ground truth (session 94 close)

* **Head**: `56ba151` (feat) before this close-doc
  commit lands; after both land local main is 13
  commits ahead of `origin/main` (sessions 89-94 feat
  + close, plus the session-94 hygiene commit on top
  of 93's close-doc commit). Verify via `git log
  --oneline origin/main..main` before any push.
  Do NOT push without direct user instruction.
* **Tests**: **758** passed, 0 failed, 1 ignored on
  macOS (default features). With `--features c2pa`
  on lvqr-archive: 35 lib + 5 integration, 0 ignored.
  With `--features c2pa` on lvqr-cli: +1 E2E
  (`c2pa_verify_e2e`), 0 ignored.
* **CI gates locally clean**:
  * `cargo fmt --all`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo clippy -p lvqr-archive --features c2pa --all-targets -- -D warnings`
  * `cargo clippy -p lvqr-cli --features c2pa --all-targets -- -D warnings`
  * `cargo test -p lvqr-archive --features c2pa`
  * `cargo test -p lvqr-cli --test rtmp_archive_e2e`
    (no regression after the `registry.remove` wiring)
  * `cargo test -p lvqr-cli --features c2pa --test c2pa_verify_e2e`
  * `cargo test --workspace`
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91 (A) / 92 (B1) / 93 (B2) / 94 (B3) |
| 4.8 | One-token-all-protocols | PLANNED | 95-96 |
| 4.5 | In-process AI agents | PLANNED | 97-100 |
| 4.4 | Cross-cluster federation | PLANNED | 101-103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

Three of eight Tier 4 items are now complete (4.2, 4.1,
4.3). Downstream sessions unchanged from session 93's
view; tier budget still 27 sessions (85-111) with one
session reserve.

### Session 95 entry point

**Tier 4 item 4.8 session A: One-token-all-protocols.**

Scoped + scouted against the live code at session 94
close (2026-04-19). See `tracking/TIER_4_PLAN.md`
section 4.8 for the full deliverables table and the
Plan-vs-code status block that captures the three
drifts below.

**Drift 1: `normalized_auth` is really an extractor,
not a verifier.** `lvqr_auth::AuthProvider::check(
&AuthContext)` already returns a uniform
`AuthDecision` across protocols. `JwtAuthProvider`
already handles Publish + Subscribe + Admin variants.
What session 95 A must add is the protocol-specific
EXTRACTOR layer that turns each protocol's token
carrier into a uniform `AuthContext`. The verifier
side is done.

**Drift 2: three ingest crates have NO auth call-site
today.** Scout at session 94 close found:

  - `lvqr-ingest` (RTMP): calls `auth.check` at
    `bridge.rs:456` on `AuthContext::Publish`. JWT
    is carried as the stream key (existing
    `JwtAuthProvider` convention).
  - `lvqr-relay` (MoQ): calls `auth.check` at
    `server.rs:155` on `AuthContext::Subscribe`.
  - `lvqr-cli` (WS relay + WS ingest + playback):
    calls at `lib.rs:1289` (WS relay subscribe),
    `lib.rs:1415` (WS ingest publish), and the
    playback router in `archive.rs`.
  - `lvqr-whip`: **ZERO auth references anywhere.**
  - `lvqr-srt`: **ZERO auth references anywhere.**
  - `lvqr-rtsp`: **ZERO auth references anywhere.**

Session 95 A must ADD auth call-sites to whip / srt
/ rtsp, not "migrate existing one-offs". Estimate
shifts ~+200 LOC vs the session-84 plan.

**Drift 3: session decomposition table had stale
numbers.** Fixed in session 94 close: 4.8 is now 95
/96 (was 92/93); 4.5 is 97-100 (was 94-97); 4.4 is
101-103 (was 98-100); 4.6 is 104-106 (was 101-103);
4.7 is 107-108 (was 104-105). Tier 4 budget
unchanged at 27 sessions (85-111).

**Token-carrier inventory for the extractor layer**:

  - RTMP: stream key IS the JWT. Existing.
  - WHIP: `Authorization: Bearer <jwt>` on the
    POST /whip/{broadcast} HTTP offer. Standard.
  - SRT: `streamid` handshake parameter. No industry
    standard. Proposed LVQR shape: `m=publish,r=<
    broadcast>,t=<jwt>` (`,`-separated KV pairs).
    Document in `docs/auth.md`.
  - RTSP: `Authorization: Bearer <jwt>` on
    ANNOUNCE + RECORD. Verify `rtsp-types` passes
    the header through; if not, extend the server's
    header handling -- small isolated change.
  - WS: existing `?token=<jwt>` query fallback +
    `Authorization: Bearer` header. Already handled.

**Deliverables (per TIER_4_PLAN row 95 A)**:

(a) New `lvqr-auth::extract` module (or similar)
with per-protocol `extract_<proto>` helpers that
build `AuthContext` from the protocol's token
carrier. Unit tests per helper.

(b) Wire into `lvqr-whip` + `lvqr-srt` +
`lvqr-rtsp` (new call-sites) + `lvqr-ingest` +
`lvqr-cli` WS ingest (migrations to the shared
extractor).

(c) `docs/auth.md` (new): JWT claim shape (`sub`,
`exp`, `scope`, optional `iss`, `aud`, `broadcast`)
+ per-protocol carrier conventions + one worked
example per protocol.

(d) `TestServerConfig::with_whip()` helper added
if missing (needed by session 96 B's E2E).

Session 96 B lands the cross-protocol E2E at
`crates/lvqr-cli/tests/one_token_all_protocols.rs`.

**Pre-session checklist**:

1. Read `tracking/TIER_4_PLAN.md` section 4.8
   fully (lines 422-574 in current file).
2. Confirm the current `AuthContext` enum's
   coverage against the extractor plan. If SRT
   needs a new context variant or a
   `metadata: HashMap<String,String>` side
   channel, decide + update before wiring.
3. Read `crates/lvqr-whip/src/*`, `crates/lvqr-
   srt/src/*`, `crates/lvqr-rtsp/src/*` to pick
   the right plumbing point (typically the
   connection-accept / SDP-offer / streamid-parse
   path).
4. Verify the workspace default `cargo test`
   stays green after each call-site add; the
   `NoopAuthProvider` default means adding a
   gate is behaviour-preserving for existing
   tests.

**Verification gates (session 95 A close)**:

  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets --benches -- -D warnings`
  - `cargo test -p lvqr-auth --lib`
  - `cargo test --workspace` no regression from 758
  - `git log -1 --format='%an <%ae>'` reads
    `Moheeb Zara <hackbuildvideo@gmail.com>` alone

Expected scope: ~500-700 LOC split across 95 A + 96
B (scope-up from the session-84 plan's ~300-500
estimate because of drift 2).

**Biggest risks**, ranked:

1. SRT streamid format choice. Whatever session 95
   picks, other SRT ingestors (OBS, ffmpeg) must be
   able to produce it. The `m=publish,r=...,t=...`
   shape is de-facto in the SRT community; document
   first, code second.
2. RTSP header passthrough. `rtsp-types` may or may
   not surface `Authorization` to the server
   handler cleanly. If not, the extractor falls
   back to reading the raw request and extending
   the RTSP server's header handling.
3. `TestServerConfig::with_whip()` may not exist.
   Check before the E2E -- if absent, session 95 A
   adds it as a byproduct of the plumbing pass.

## Session 93 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.3 session B2: cert fixture +
   sign-side composability + finalize orchestrators**
   (`868c378`). Three deliverables in one commit, all
   converging on "the c2pa primitive is now end-to-end
   testable and composable for the drain-task wiring."

   **Cert-fixture breakthrough**. Discovery:
   `c2pa::EphemeralSigner` is publicly re-exported from
   c2pa 0.80 (in `pub use utils::{ephemeral_signer::
   EphemeralSigner, ...}`). It generates C2PA-spec-
   compliant Ed25519 cert chains in memory using
   c2pa-rs's own private `ephemeral_cert` module +
   rasn_pkix -- exactly the extension layout
   (digitalSignature KU, emailProtection EKU, basic-
   constraints with cA=FALSE on EE, AKI/SKI, v3) the
   structural-profile check wants. The session-91
   happy-path test (`#[ignore]`'d through sessions
   91-92 because rcgen-generated chains kept tripping
   `CertificateProfileError::InvalidCertificate`)
   unignores via this signer with zero PEM-fixture
   maintenance + zero calendar-expiry risk. The chain
   is generated per-test-run.

   **Sign-side composability refactor**:

   * New `provenance::SignOptions { assertion_creator,
     trust_anchor_pem }` -- the subset of `C2paConfig`
     that is independent of PEM paths + signing alg.
     Lets `sign_asset_with_signer` callers construct
     only what the lower-level primitive needs.
   * New `provenance::sign_asset_with_signer(&dyn
     c2pa::Signer, &SignOptions, format, bytes) ->
     Result<SignedAsset, ArchiveError>` -- low-level
     primitive that takes any `c2pa::Signer` impl.
     Tests use `EphemeralSigner`; advanced operators
     with HSM-backed or KMS-backed keys call this
     directly.
   * `sign_asset_bytes` (path-based primitive) now
     delegates to `_with_signer` after reading PEMs +
     constructing the signer. The high-level shape
     for production operators is unchanged.

   **Finalize orchestrators**:

   * `finalize_broadcast_signed_with_signer(signer,
     options, init_bytes, segment_paths, format,
     asset_path, manifest_path) -> SignedAsset` --
     composes `concat_assets` (init + segments in
     order) + `sign_asset_with_signer` +
     `write_signed_pair`. Returns SignedAsset so
     caller can log size or inspect bytes without re-
     reading from disk. `init_bytes` is taken as a
     parameter so this primitive stays agnostic to
     where init persistence lives -- session 94's
     call.
   * `finalize_broadcast_signed(&C2paConfig, ...)`
     -- high-level convenience that reads PEMs then
     delegates. Single call site for session 94's
     drain integration.

   **Test suite migration in `tests/c2pa_sign.rs`**:
   3 tests, 0 ignored. The rcgen-based
   `build_test_chain` helper + the `#[ignore]`'d
   happy-path test are deleted in favor of:

   - `sign_asset_with_signer_emits_non_empty_c2pa_
     manifest_for_minimal_jpeg` (live, was ignored
     through 91-92).
   - `finalize_broadcast_signed_with_signer_writes_
     asset_and_manifest_pair_to_disk` (new; init-only
     "broadcast" exercising concat + sign + write
     end-to-end with real on-disk reads to verify
     round-trip).
   - `sign_asset_bytes_reports_c2pa_error_on_missing_
     cert_file` (live, unchanged).

   **Cleanup**: rcgen dropped from `lvqr-archive`'s
   dev-deps + Cargo.lock. The only consumer was the
   deleted fixture builder.

   **Plan refresh**: section 4.3 header "3 sessions,
   91-93" → "4 sessions, 91-94". B2 row flipped to
   **DONE (session 93)** with the cert-fixture-
   breakthrough note + composability + finalize-
   orchestrator scope. New B3 row covers the
   remaining drain integration + verify route + E2E.

2. **Session 93 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 2 | `sign_asset_with_signer_emits_non_empty_c2pa_manifest_for_minimal_jpeg` (was `#[ignore]`'d through sessions 91-92, now live) + `finalize_broadcast_signed_with_signer_writes_asset_and_manifest_pair_to_disk` (new) in `crates/lvqr-archive/tests/c2pa_sign.rs` | both ok (feature-gated; runs on the `archive-c2pa` CI cell + locally with `--features c2pa`) |

`cargo test -p lvqr-archive --features c2pa --test
c2pa_sign`: 3 passed, 0 ignored. Previously 1 passed +
1 ignored. The c2pa-sign happy-path ignore is gone.

Workspace totals on macOS: **739** passed, 0 failed,
1 ignored (default features). The 1 remaining ignored
test is unrelated to 4.3 -- it predates this work.

### Ground truth (session 93 close)

* **Head**: `868c378` (feat) on `main` before this
  close-doc commit lands; after both lands local main
  is 10 commits ahead of `origin/main` (sessions 89
  feat+close, 90 feat+close, 91 feat+close, 92
  feat+close, 93 feat+close). Verify via `git log
  --oneline origin/main..main` before any push. Do
  NOT push without direct user instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored on
  macOS (default features). With `--features c2pa`:
  31 lib + 3 integration, 0 ignored.
* **CI gates locally clean**: `cargo fmt --all --
  --check`, `cargo clippy --workspace --all-targets
  --benches -- -D warnings`, `cargo test --workspace`
  all green. `cargo clippy -p lvqr-archive --features
  c2pa --all-targets -- -D warnings` clean.
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **A + B1 + B2 DONE**, B3 pending | 91 (A) / 92 (B1) / 93 (B2) / 94 (B3) |
| 4.8 | One-token-all-protocols | PLANNED | 95-96 |
| 4.5 | In-process AI agents | PLANNED | 97-100 |
| 4.4 | Cross-cluster federation | PLANNED | 101-103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

Tier 4 item 4.3 grew from 3 sessions (post-92 split)
to 4 (post-93 split). Downstream items shift +1 vs.
session 92's view (e.g., 4.8 was 94-95, now 95-96).
Tier 4 budget unchanged at 27 sessions (85-111)
because the extension absorbs into the tier-wide
buffer.

### Session 94 entry point

**Tier 4 item 4.3 session B3: drain-task integration
+ admin verify route + E2E.**

Deliverables per the refreshed
`tracking/TIER_4_PLAN.md` section 4.3 row B3:

(a) **Broadcast-end lifecycle hook on
`lvqr_fragment::FragmentBroadcasterRegistry`**.
Current surface (line 102 of
`crates/lvqr-fragment/src/registry.rs`) has
`on_entry_created`; add a matching `on_entry_removed`
or a more general `LifecycleObserver` trait covering
both. Load-bearing primitive that 4.4 (cross-cluster
federation) + 4.5 (AI agents) will also consume --
**design the API shape before coding.** Specifically
decide:
  * Callback fires on `Drop` (risky -- callbacks from
    Drop can deadlock if they take locks the dropping
    thread holds; tokio runtime semantics in Drop are
    constrained) vs. explicit `registry.remove()`
    (safer but requires callers to know to remove).
  * Sync vs. async callback signature (the registry
    currently mixes both via `tokio::spawn` from
    callback closures -- consistent or split?).
  * Error propagation policy (callbacks panic-safe
    or panic-propagating).

(b) **Persist init bytes to disk at first-segment-
write time**. Today `FragmentBroadcaster::meta()`
holds them in memory only. Layout decision:
  * Flat `<archive>/<broadcast>/<track>/init.mp4` --
    simpler, parallel to the segment files,
    independently reachable for non-c2pa consumers.
  * `metadata.json` sidecar with the init bytes
    base64-encoded -- scales better if we later add
    per-track metadata (timescale, SPS/PPS,
    codec_string, etc.).

  Pick + document in B3's feat commit.

(c) **Extend `lvqr_cli::archive::
BroadcasterArchiveIndexer::drain`** to call
`lvqr_archive::provenance::finalize_broadcast_signed`
inside `tokio::task::spawn_blocking` when the drain
task terminates AND `C2paConfig` is `Some`. The B2-
landed orchestrator is one call: pass init bytes
(read from the layout decided in (b)), segment paths
(walk the redb index for this `(broadcast, track)`
in `start_dts` order), format (`"video/mp4"` for
CMAF), asset path
(`<archive>/<broadcast>/<track>/finalized.mp4`), and
manifest path (`finalized.c2pa`).

(d) **`GET /playback/verify/{broadcast}`** admin
route in `lvqr-cli`. Reads the signed asset +
sidecar manifest from disk, calls
`c2pa::Reader::from_manifest_data_and_stream`,
returns a JSON object `{ signer: String, signed_at:
Option<DateTime>, valid: bool, errors: Vec<String>
}`. Auth per existing `/admin` routes.

(e) **E2E test** at
`crates/lvqr-cli/tests/c2pa_verify_e2e.rs`. Starts a
`TestServer` with `C2paConfig` (using EphemeralSigner-
generated PEMs written to disk -- or, alternatively,
we expose a `C2paSignerConfig` enum that lets the
test pass an in-memory signer); publishes one RTMP
broadcast; drops the publisher to trigger finalize;
hits `GET /playback/verify/{broadcast}` and asserts
the JSON has `valid: true` (or expected
verification status given an ephemeral CA) + the
expected signer.

  Note on the E2E cert path: in production the
  operator points `C2paConfig.signing_cert_path` at a
  PEM file. For the E2E test we need to either (a)
  extract PEMs from EphemeralSigner via a
  `serialize_pem_pair() -> (cert_pem, key_pem)`
  helper added to `provenance` (would require c2pa-rs
  to expose them, which it does NOT -- the PEMs are
  built inside `EphemeralSigner::new` and not stored
  on the struct), or (b) extend `C2paConfig` with a
  `Signer` trait-object alternative, or (c) replicate
  EphemeralSigner's chain-generation logic ourselves
  (substantial new code). Decide before writing the
  E2E.

Expected scope: ~600-800 LOC (registry hook + init
persistence + drain integration + verify route + E2E
+ docs). Biggest risks:
- Registry lifecycle-hook API design affects 4.4 +
  4.5; budget time for prose-sketch + review before
  wiring.
- Cert-path-for-E2E decision (above).
- The drain-task termination path runs inside tokio;
  `finalize_broadcast_signed` is sync so it needs
  `spawn_blocking` like `write_segment` does.

Pre-session checklist:
- Read `tracking/TIER_4_PLAN.md` section 4.3 row B3
  fully.
- Sketch the registry lifecycle-hook API in prose +
  paste into the feat commit before wiring -- shared
  primitive for Tier 4 items 4.4 + 4.5 too.
- Decide init-bytes layout (flat `init.mp4` vs.
  `metadata.json` sidecar) and document.
- Decide E2E cert path (operator-shape PEM file vs.
  Signer-trait-object extension to `C2paConfig`).
- Confirm `c2pa::Reader::from_manifest_data_and_stream`
  is the right verify entry; check signature in
  c2pa 0.80 source.

## Session 92 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.3 session B1: provenance composition
   primitives + trust-anchor config + plan split**
   (`6ca1889`). Two code deliverables plus a plan
   refresh that re-scopes B from one big session to
   two. Session-88 A1 precedent: honest acknowledgment
   that four independent surfaces in one session is
   too much.

   **B scope split rationale**. Original session 92 B
   combined four deliverables:
   (a) cert-chain fixture (debug c2pa's structural
       profile check OR vendor PKI; isolated),
   (b) finalize-asset orchestration (broadcast-end
       lifecycle hook on `FragmentBroadcasterRegistry`
       that 4.4 federation + 4.5 AI agents will also
       consume, plus init-bytes persistence + drain-
       task integration),
   (c) admin verify route (straightforward axum handler
       once the sign side wires up),
   (d) E2E that composes the above.
   Compressing them into one session risks bikeshedding
   the registry lifecycle API under E2E-failure
   pressure. Session B1 ships (a-prep) + the pure
   composition helpers that any caller needs; session
   B2 takes on the cross-crate orchestration + verify
   route + E2E.

   **Code landed**:

   * `C2paConfig.trust_anchor_pem: Option<String>`
     field. `sign_asset_bytes` routes it through
     `c2pa::Context::with_settings({"trust":
     {"user_anchors": ...}})` so operators with a
     private CA have a first-class path. This is the
     production workflow: point `trust_anchor_pem` at
     the CA bundle that issued the signing cert, and
     c2pa-rs's chain validator recognises it as a
     trust root.
   * `provenance::concat_assets(&[impl AsRef<Path>])
     -> Result<Vec<u8>, ArchiveError>`. Reads a
     caller-supplied ordered list of paths into one
     buffer. Session B2's finalize task walks the redb
     segment index in `start_dts` order, collects
     `PathBuf`s, and feeds them to this helper to
     produce the bytes-to-sign. Decoupling keeps the
     primitive redb-free and testable.
   * `provenance::write_signed_pair(asset_path,
     manifest_path, &SignedAsset) -> Result<(),
     ArchiveError>`. Writes both files with on-demand
     parent-dir creation, matching
     `writer::write_segment`'s semantics. Session B2
     lands
     `<archive>/<broadcast>/<track>/finalized.<ext>`
     +
     `<archive>/<broadcast>/<track>/finalized.c2pa`
     together.
   * 5 new unit tests in `provenance::tests`: concat
     order preservation, concat missing-path error
     naming, concat empty input, write_signed_pair
     parent-dir creation + overwrite semantics.

   **Cert-fixture debug outcome**. One time-boxed
   attempt this session to unignore the happy-path
   test via `Settings.trust.user_anchors` confirmed:
   that path addresses trust-chain validation only,
   not the structural-profile validation that is
   failing. c2pa 0.80's `verify.verify_trust`
   setting is `pub(crate)` so bypassing profile
   checks from outside the crate is not currently
   possible without either a c2pa upgrade or a light
   wrapper. Test docblock updated accordingly. Three
   viable fixture options remain for B2:
   (i) rcgen with full extension control (explicit
       AKI/SKI, basic-constraints criticality,
       validity window),
   (ii) vendored CA + leaf pair under
        `tests/fixtures/c2pa/` with a 2099 `notAfter`
        + README noting expiry,
   (iii) a test-only feature that wraps c2pa's
         `CertificateTrustPolicy::passthrough()`.

2. **Plan refresh** (same commit as item 1).
   `tracking/TIER_4_PLAN.md` section 4.3 re-headers
   from "2 sessions, 91-92" to "3 sessions, 91-93".
   Session 92 B row split into B1 DONE + B2 pending
   with expanded scope. Risks section unchanged.

3. **Session 92 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 5 | `provenance::tests::*` in `crates/lvqr-archive/src/provenance.rs` (feature-gated on `c2pa`) -- concat order, missing-path error, empty input, write_signed_pair parent-dir creation + overwrite | ok (run on the `archive-c2pa` CI cell + locally with `--features c2pa`) |

Totals: `cargo test -p lvqr-archive`: **26** (unchanged
on default features). `cargo test -p lvqr-archive
--features c2pa`: **31** lib (+5 from session 91) + 1
integration + 1 ignored. Workspace total: **739**
(unchanged; feature-gated tests do not count toward
default-feature workspace).

### Ground truth (session 92 close)

* **Head**: `6ca1889` (feat) on `main` before this
  close-doc commit lands; after it lands local main
  is 8 commits ahead of `origin/main` (sessions 89
  feat+close, 90 feat+close, 91 feat+close, 92
  feat+close). Verify via `git log --oneline
  origin/main..main` before any push. Do NOT push
  without direct user instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored on
  macOS (default features).
* **CI gates locally clean**: `cargo fmt --all --
  --check`, `cargo clippy --workspace --all-targets
  --benches -- -D warnings`, `cargo test --workspace`
  all green. `cargo clippy -p lvqr-archive --features
  c2pa --all-targets -- -D warnings` clean. `cargo
  test -p lvqr-archive --features c2pa` green (31
  lib + 1 c2pa_sign + 1 ignored).
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **A + B1 DONE**, B2 pending | 91 (A) / 92 (B1) / 93 (B2) |
| 4.8 | One-token-all-protocols | PLANNED | 94-95 |
| 4.5 | In-process AI agents | PLANNED | 96-99 |
| 4.4 | Cross-cluster federation | PLANNED | 100-102 |
| 4.6 | Server-side transcoding | PLANNED | 103-105 |
| 4.7 | Latency SLO scheduling | PLANNED | 106-107 |

Tier 4 item 4.3 grew from 2 sessions to 3 at session
92's replan. Downstream items shift +1 (e.g., 4.8 was
93-94, now 94-95). Tier 4 budget unchanged at 27
sessions (85-111) because the extension absorbs into
the tier-wide buffer (same pattern 4.1 followed at
session 88).

### Session 93 entry point

**Tier 4 item 4.3 session B2: cert fixture +
finalize-asset orchestration + admin verify route +
E2E.**

Deliverables per the refreshed
`tracking/TIER_4_PLAN.md` section 4.3 row B2:

(a) **Cert-chain fixture** so the happy-path
`c2pa_sign::sign_asset_bytes_emits_non_empty_c2pa_
manifest_for_minimal_jpeg` test unignores. Three
options (ranked by likelihood-to-work):

  * rcgen with explicit extension control. Needs
    `rcgen::CustomExtension` for AKI/SKI content +
    basic-constraints criticality. Investigate which
    branch of c2pa's cert profile check rejects the
    current rcgen chain by enabling c2pa's
    `validation_log` or running c2pa's own tests in
    isolation to confirm what a passing cert looks
    like. Ideally the shortest path.
  * Vendor a static CA + leaf PEM pair with a 2099
    `notAfter`. Cleanest long-term: removes the
    rcgen dev-dep for this test + removes fixture
    construction flakiness. Generate once via `openssl
    req -new -x509 ...` (or a trusted CA fixture from
    c2pa-rs's own test suite if it has a reusable
    bundle) and commit under
    `crates/lvqr-archive/tests/fixtures/c2pa/` with a
    README noting the expiry.
  * Wrap `c2pa::CertificateTrustPolicy::passthrough()`
    behind a test-only feature. Problem: c2pa 0.80's
    `verify.verify_trust` setting is `pub(crate)` so
    this requires either upstreaming a PR or waiting
    on a c2pa version with public access. Last
    resort.

(b) **Finalize-asset orchestration**. Three moving
pieces:

  * Add a broadcast-end lifecycle hook to
    `lvqr_fragment::FragmentBroadcasterRegistry`.
    Current surface has `on_entry_created` (line 102
    of `crates/lvqr-fragment/src/registry.rs`); add a
    matching `on_entry_removed` or a more general
    `LifecycleObserver` trait that also covers
    `on_entry_created`. This is a load-bearing
    primitive that 4.4 (cross-cluster federation)
    and 4.5 (AI agents) will also want -- design the
    API shape before coding. Specifically think about
    whether the callback fires synchronously on drop
    (risky, callbacks from Drop can deadlock) or on
    an explicit `registry.remove()` call (safer but
    requires callers to know to remove).
  * Persist init bytes to disk at first-segment-write
    time. Today `FragmentBroadcaster::meta()` holds
    them in memory only. Layout decision: flat
    `<archive>/<broadcast>/<track>/init.mp4` vs.
    `metadata.json` sidecar. The flat approach is
    simpler but the JSON sidecar scales better if we
    later add timescale / SPS / PPS metadata. Pick
    and document in the B2 feat commit.
  * Extend `lvqr_cli::archive::BroadcasterArchiveIndexer::
    drain` to call `lvqr_archive::provenance::
    concat_assets` (walking the redb index for this
    `(broadcast, track)` in `start_dts` order and
    prepending the init bytes) + `sign_asset_bytes` +
    `write_signed_pair` when the drain task
    terminates AND `C2paConfig` is `Some`.

(c) **`GET /playback/verify/{broadcast}`** admin
route in `lvqr-cli`. Reads the signed asset +
sidecar manifest from
`<archive>/<broadcast>/<track>/finalized.<ext>` +
`.c2pa`, calls
`c2pa::Reader::from_manifest_data_and_stream`,
returns a JSON object `{ signer: String, signed_at:
Option<DateTime>, valid: bool, errors: Vec<String>
}`. Auth per existing `/admin` routes.

(d) **E2E test** at
`crates/lvqr-cli/tests/c2pa_verify_e2e.rs`. Starts a
`TestServer` with `C2paConfig` pointed at the
session-B2 cert fixture; publishes one RTMP
broadcast; drops the publisher to trigger finalize;
hits `GET /playback/verify/{broadcast}` and asserts
the JSON has `valid: true` + the expected signer.

Expected scope: ~600-900 LOC (cert fixture + three
archive-side changes + CLI route + E2E test).
Biggest risks:
- Registry lifecycle-hook API design affects 4.4 +
  4.5; worth a short design sketch before coding.
- Cert-fixture branch identification may still be
  non-obvious even after enabling validation_log;
  budget 1-2 hours for that alone.
- The drain-task termination path runs inside tokio;
  `write_signed_pair` is sync so it needs
  `spawn_blocking` like `write_segment` does.

Pre-session checklist:
- Read `tracking/TIER_4_PLAN.md` section 4.3 fully.
- Run `cargo test -p lvqr-archive --features c2pa
  --test c2pa_sign -- --ignored --nocapture` with
  any trace-logging added to c2pa's validation_log
  to pinpoint the specific profile-check branch
  that rejects the rcgen chain.
- Decide cert-fixture path (rcgen / vendored /
  passthrough) before coding the verify route.
- Decide finalize-asset layout (flat `init.mp4` vs.
  `metadata.json` sidecar) and document in the feat
  commit.
- Sketch the registry lifecycle-hook API shape in
  prose in the feat commit before wiring -- this is
  a shared primitive for Tier 4 items 4.4 + 4.5 too.

## Session 91 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.3 session A: C2PA feature +
   `provenance::sign_asset_bytes` primitive + plan
   refresh** (`1c34428`). Two deliverables in one
   commit, session-88-A1 style: a legitimate code
   landing plus the plan rewrite that makes sense of
   the landing's scope.

   **Plan-vs-code delta** captured in the refreshed
   `tracking/TIER_4_PLAN.md` section 4.3: the session-84
   plan said "on `finalize()` (broadcaster disconnect),
   the archive emits a C2PA manifest ... of the
   finalized MP4 bytes". The actual architecture has no
   finalize event, no init.mp4 on disk, and no single
   finalized MP4 -- the archive is a redb-indexed stream
   of `.m4s` fragments under
   `<archive_dir>/<broadcast>/<track>/`. "Sign the
   finalized MP4" has no referent today. A scout via the
   Explore agent confirmed three specifics:
   `BroadcasterArchiveIndexer::drain` exits silently on
   `FragmentStream::next_fragment` returning `None`,
   `FragmentBroadcasterRegistry` has `on_entry_created`
   but no matching broadcast-end hook, and
   `FragmentBroadcaster::meta()` holds init bytes in
   memory only. The refreshed plan re-scopes B to absorb
   the finalize-asset construction (init-bytes
   persistence + registry lifecycle hook + segment
   concatenation by dts) alongside the admin verify
   route + E2E. 4.3 stays at 2 sessions total.

   **Primitive** lives in a new
   `crates/lvqr-archive/src/provenance.rs` (~200 LOC)
   behind the `c2pa` feature (default off). Workspace
   pin `c2pa = { version = "0.80", default-features =
   false, features = ["rust_native_crypto"] }` so the
   crypto closure stays pure-Rust (no vendored OpenSSL
   C build) and the remote-manifest HTTP stacks
   (reqwest + ureq) are absent. Public surface:

   * `C2paConfig` -- cert path, key path, creator
     name, alg, optional TSA URL.
   * `C2paSigningAlg` -- LVQR-owned enum 1:1 with
     `c2pa::SigningAlg` so downstream consumers do not
     need a direct c2pa-rs dep to build a config.
   * `SignedAsset { asset_bytes, manifest_bytes }` --
     sidecar-mode output; asset passes through
     unchanged via `Builder::set_no_embed(true)`.
   * `sign_asset_bytes(&config, format, bytes)` --
     bytes-in / bytes-out primitive. Uses the non-
     deprecated `Builder::from_context(Context::new())
     .with_definition(manifest_json)` path (0.80
     deprecated `Builder::from_json`). Manifest carries
     one `stds.schema-org.CreativeWork` assertion
     whose `Person.name` is `config.assertion_creator`,
     constructed via `serde_json::json!` so operator-
     supplied names are JSON-escaped correctly.

   `ArchiveError::C2pa(String)` variant feature-gated
   so downstream consumers without c2pa do not see a
   dead variant.

   **Integration test** at
   `crates/lvqr-archive/tests/c2pa_sign.rs` gated on
   `#![cfg(feature = "c2pa")]`:

   * Error path live: `sign_asset_bytes_reports_c2pa_
     error_on_missing_cert_file` asserts missing-cert
     surfaces as `ArchiveError::Io` with the path in
     the message. Proves the primitive reads config +
     surfaces errors cleanly.
   * Happy path `#[ignore]`'d: c2pa-rs 0.80 validates
     the signing cert against C2PA spec §14.5.1 at
     sign time and rejects the rcgen-generated chain
     (even with a 2-cert CA + leaf using
     emailProtection EKU + digitalSignature KU) with
     the generic `CertificateProfileError::
     InvalidCertificate`. That variant collapses ~8
     failure branches without a validation_log hook at
     this API layer, so pinpointing the exact missing
     extension takes more iteration than session A
     budgets for. The test's doc comment documents
     three unignore paths for session B: (a) rcgen
     with full extension control, (b) vendored
     fixture with 2099 `notAfter`, (c) passthrough
     trust policy behind a new
     `c2pa-test-bypass-cert-check` feature.

   **CI**: new `archive-c2pa` job on `ubuntu-latest`
   runs `cargo clippy + cargo test -p lvqr-archive
   --features c2pa`. Separate job rather than a matrix
   cell on the existing `test` job so macOS CI time
   does not grow by ~2 minutes (c2pa-rs pulls ~20
   transitive crates; all pure-Rust with our
   default-features-off config).

2. **Session 91 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 1 | `sign_asset_bytes_reports_c2pa_error_on_missing_cert_file` in `crates/lvqr-archive/tests/c2pa_sign.rs` | ok (feature-gated; runs on the `archive-c2pa` CI job + locally with `--features c2pa`) |
| 0 (ignored) | `sign_asset_bytes_emits_non_empty_c2pa_manifest_for_minimal_jpeg` | `#[ignore]`'d pending session B's cert fixture |

Workspace totals on macOS: **739** passed, 0 failed,
1 ignored. Feature-gated c2pa test does not count
toward the default-feature workspace total; it adds
+1 passed / +1 ignored when the `c2pa` feature is on.

### Ground truth (session 91 close)

* **Head**: `1c34428` (feat) on `main` before this
  close-doc commit lands; after it lands local main
  is 6 commits ahead of `origin/main` (sessions 89
  feat + close, 90 feat + close, 91 feat + close).
  Verify via `git log --oneline origin/main..main`
  before any push. Do NOT push without direct user
  instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored on
  macOS (default features).
* **CI gates locally clean**: `cargo fmt --all --
  --check`, `cargo clippy --workspace --all-targets
  --benches -- -D warnings`, `cargo test --workspace`
  all green. `cargo clippy -p lvqr-archive --features
  c2pa --all-targets -- -D warnings` clean on macOS.
  `cargo test -p lvqr-archive --features c2pa` green
  (26 lib + 1 c2pa_sign + 1 ignored).
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **A DONE**, B pending | 91 (A) / 92 (B) |
| 4.8 | One-token-all-protocols | PLANNED | 93-94 |
| 4.5 | In-process AI agents | PLANNED | 95-98 |
| 4.4 | Cross-cluster federation | PLANNED | 99-101 |
| 4.6 | Server-side transcoding | PLANNED | 102-104 |
| 4.7 | Latency SLO scheduling | PLANNED | 105-106 |

### Session 92 entry point

**Tier 4 item 4.3 session B: cert fixture +
finalize-asset construction + admin verify route +
E2E.** Absorbed scope from the session-84 plan's
session B + session A's deferred items per the
session-91 re-scope.

Deliverables per the refreshed
`tracking/TIER_4_PLAN.md` section 4.3 row B:

(a) **Cert-chain fixture** so the happy-path
`c2pa_sign::sign_asset_bytes_emits_non_empty_c2pa_
manifest_for_minimal_jpeg` test unignores. Pick one
of three paths documented in the test's doc comment:

  * rcgen with full extension control (explicit AKI/
    SKI, basic-constraints criticality, explicit
    validity window). Requires digging into which of
    the ~8 failure branches in
    `CertificateProfileError::InvalidCertificate` is
    tripping. Enable c2pa-rs's `validation_log` or
    build a scratch binary that prints the log to
    debug.
  * Vendored test CA + end-entity under
    `crates/lvqr-archive/tests/fixtures/c2pa/` with a
    far-future `notAfter` (2099-era) and a README
    noting the expiry. Cleanest long-term: removes
    the rcgen dev-dep for this test entirely and
    removes fixture-construction flakiness.
  * `CertificateTrustPolicy::passthrough()` behind a
    new `c2pa-test-bypass-cert-check` feature. Lets
    the test run end-to-end without production-grade
    PKI. Caveat: the primitive signs with a trust-
    bypassed policy, so the test no longer validates
    that the cert profile is compliant -- the
    primitive may let bad certs through at sign time
    in production if the feature leaks. Mark the
    feature loudly.

(b) **Finalize-asset construction** in
`lvqr-archive` + the CLI drain task. Three moving
pieces:

  * Persist init bytes to disk at first-write time.
    Today `FragmentBroadcaster::meta()` holds them
    in memory only. Options: write once when the
    first segment lands, at
    `<archive_dir>/<broadcast>/<track>/init.mp4`;
    or generalise the on-disk layout to include a
    `metadata.json` sidecar per `(broadcast,
    track)` with the init bytes base64-encoded in
    it. Decide + document in B's feat commit.
  * Broadcast-end lifecycle hook on
    `FragmentBroadcasterRegistry`. Currently the
    registry exposes `on_entry_created` only; add a
    matching `on_entry_removed` (or a more general
    `LifecycleObserver`) so the drain-task-
    termination path can notify listeners. This is
    a shared primitive -- future sessions (4.4
    federation, 4.5 AI agents) may also want to
    react to broadcast-end events.
  * Segment-concat helper in `lvqr-archive` that
    produces the bytes to feed to
    `sign_asset_bytes`. Walks the redb index for
    the broadcast + track, reads segments in
    start_dts order, concatenates with the init
    bytes, returns a `Vec<u8>`. At today's archive
    segment sizes (<= 1 MiB) the in-memory buffer
    is fine; if that ever grows too large we swap
    to a streaming `impl Read + Seek`.

(c) **`GET /playback/verify/{broadcast}`** admin
route in `lvqr-cli`. Reads the signed asset +
sidecar manifest from disk, calls
`c2pa::Reader::from_manifest_data_and_stream`,
returns a JSON object `{ signer: String, signed_at:
Option<DateTime>, valid: bool, errors: Vec<String>
}`. Auth per existing `/admin` routes (admin
token).

(d) **E2E test** at
`crates/lvqr-cli/tests/c2pa_verify_e2e.rs`. Starts
a `TestServer` with `C2paConfig` pointed at the
session-B cert fixture; publishes one RTMP
broadcast; drops the publisher to trigger
finalize; hits `GET /playback/verify/{broadcast}`
and asserts the JSON has `valid: true` + the
expected signer.

Expected scope: ~500-800 LOC (cert fixture + three
archive changes + CLI route + E2E test + docs
section). Biggest risk: the lifecycle-hook addition
to `FragmentBroadcasterRegistry` is a load-bearing
primitive that future items will also consume, so
the API shape is worth a short design discussion
before coding. Second risk: the cert-fixture branch
identification may still be non-obvious even with
validation_log enabled; budget 1-2 hours for that
alone.

Pre-session checklist:

- Read `tracking/TIER_4_PLAN.md` section 4.3 top-to-
  bottom (now accurate post-session-91 refresh).
- Run `cargo test -p lvqr-archive --features c2pa
  --test c2pa_sign -- --ignored --nocapture` and
  read the full c2pa error output -- that narrows
  which profile branch is tripping before any code
  changes.
- Decide cert-fixture path (rcgen / vendored /
  passthrough feature) before coding the verify
  route; the route's test depends on the fixture
  choice.
- Decide finalize-asset layout (flat `init.mp4` vs.
  `metadata.json` sidecar) and document in the feat
  commit.

## Session 90 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.1 session B: criterion bench +
   deployment operator doc** (`bbe2757`). Last piece of
   item 4.1 after A1 extracted the writer (session 88)
   and A2 added the feature-gated tokio-uring path
   (session 89). The caller-facing API
   (`write_segment(archive_dir, broadcast, track, seq,
   payload) -> Result<PathBuf, ArchiveError>`) is
   unchanged; B is purely measurement + documentation.

   `crates/lvqr-archive/benches/io_uring_vs_std.rs` (~95
   LOC). criterion 0.5, parameterised on segment size
   across `[4 KiB, 64 KiB, 256 KiB, 1 MiB]` -- span
   chosen to cover the production fragment distribution
   (AAC AU through high-bitrate keyframe). Uses
   `BenchmarkId::from_parameter` + `Throughput::Bytes`
   so criterion reports per-variant throughput + latency.
   `measurement_time = 2s`, `sample_size = 30` caps a
   full run at ~8 s wall + ~1 GB of tempdir writes on
   the top variant; operators raise the cap from the CLI
   when they want tighter CIs.

   The harness does not cfg-gate itself. `write_segment`
   handles path selection internally, so the same bench
   file exercises std::fs on macOS + Windows (smoke test
   for harness health) and the tokio-uring path on
   Linux with `--features io-uring`. The std-vs-io-uring
   comparison is criterion's saved-baseline workflow
   (`--save-baseline std` + `--baseline std`), which is
   called out in the docs section verbatim.

   One TempDir per variant; seq counter rolls forward
   per iter so writes land on distinct files (matches
   the production monotonic-seq contract). `TMPDIR=
   /dev/shm` is explicitly marked anti-pattern in the
   bench doc-comment: tmpfs bypasses the block-device
   IO scheduler and hides the very effect the bench is
   measuring.

   `docs/deployment.md` gains a new 153-line "Archive:
   `io_uring` write backend (Linux-only)" section
   between "Upgrade strategy" and "Firewall hardening
   checklist". Covers when to enable (Linux + kernel
   5.6 + non-seccomp-restricted runtime; not for
   bursty-small workloads), how to enable (rebuild with
   `--features lvqr-archive/io-uring`; compile-time
   only, no runtime flag), how to measure (the criterion
   saved-baseline workflow with TMPDIR guidance), how
   to interpret (throughput delta + p99 on 256 KiB + 1
   MiB is the enable signal; 4 KiB regression means
   leave it off until session-B-scope follow-up
   promotes the writer to option (b)), the exact
   `OnceLock` cold-start `tracing::warn!` operator
   runbook (seccomp profile check, LimitMEMLOCK,
   gVisor/Kata carve-outs), and caveats (
   `create_dir_all` stays on std::fs, reader path stays
   on `tokio::fs`, ordering contract unchanged).

   **No Linux io_uring numbers committed.** The plan
   said "cite the numbers" but numbers captured on one
   machine are not portable to another (different CPUs,
   kernels, block devices yield materially different
   results). Committing numbers from this macOS dev box
   would misrepresent Linux production performance;
   committing numbers from a specific cloud instance
   would misrepresent self-hosted + bare-metal
   performance. The docs section drives the
   capture-your-own workflow instead. macOS smoke-run
   numbers (4 KiB: ~79 us; 1 MiB: ~940 us / ~1 GiB/s
   throughput) are noted in the feat commit message as
   evidence the harness is healthy end-to-end; they are
   not quoted in operator-facing docs.

   Plan refresh: section 4.1 header flipped to
   `**COMPLETE (sessions 88-90)**`; session B row
   flipped to `**DONE (session 90)**`. Opportunistic
   hygiene: the inline session-decomposition table for
   4.3 was still numbered 90/91 from before session 88
   split 4.1 into three sub-sessions; corrected to
   91/92 so the next item starts from a consistent
   baseline.

2. **Session 90 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 0 | Benches do not add test count. The bench harness was smoke-run on macOS with `--measurement-time 1 --sample-size 10 --warm-up-time 1`; all four segment-size variants produced plausible numbers. |

Total workspace tests on macOS: **739**, unchanged
from session 89. `cargo bench -p lvqr-archive --no-run`
compiles clean; `cargo clippy --workspace --all-targets
--benches -- -D warnings` includes the new bench in
scope and is clean.

### Ground truth (session 90 close)

* **Head**: `bbe2757` (feat) on `main` before this
  close-doc commit lands; after it lands local main is
  4 commits ahead of `origin/main` (session 89 feat +
  session 89 close + session 90 feat + session 90
  close). Verify via `git log --oneline
  origin/main..main` before any push. Do NOT push
  without direct user instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored on
  macOS.
* **CI gates locally clean**: `cargo fmt --all --
  --check`, `cargo clippy --workspace --all-targets
  --benches -- -D warnings`, `cargo test --workspace`
  all green. `cargo bench -p lvqr-archive --no-run`
  compiles clean.
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 (A1) / 89 (A2) / 90 (B) |
| 4.3 | C2PA signed media | PLANNED | 91 (A) / 92 (B) |
| 4.8 | One-token-all-protocols | PLANNED | 93-94 |
| 4.5 | In-process AI agents | PLANNED | 95-98 |
| 4.4 | Cross-cluster federation | PLANNED | 99-101 |
| 4.6 | Server-side transcoding | PLANNED | 102-104 |
| 4.7 | Latency SLO scheduling | PLANNED | 105-106 |

Three Tier 4 items are now known-state (4.1 DONE, 4.2
DONE, 4.3 PLANNED with a known entry point). Tier 4
budget is unchanged at 27 sessions (85-111); the 4.1
extension from 2 to 3 sessions absorbed cleanly into
the tier-wide buffer at session 88's replan.

### Session 91 entry point

**Tier 4 item 4.3 session A: C2PA finalize-time
signing hook in `lvqr-archive`.**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.3:

1. Add `c2pa-rs` to workspace deps (pin a specific 0.x
   version; `c2pa-rs` is pre-1.0 so any minor upgrade
   gets its own session). `tracking/TIER_4_PLAN.md`'s
   "Dependencies to pin" table at the bottom of the
   file has the target-version placeholder.
2. `lvqr-archive` gains a `C2paConfig` struct:
   `signing_cert_path`, `private_key_path`,
   `assertion_creator`. The config is optional at the
   crate boundary; when `None`, archive finalize
   behaves exactly as it does today (no signing, no
   manifest emission).
3. On `finalize()` (broadcaster disconnect), the
   archive emits a C2PA manifest asserting authorship
   + the SHA-256 of the finalized MP4 bytes. The
   manifest lives adjacent to the finalized file --
   layout decision up to session A, but
   `<archive_dir>/<broadcast>/<track>/manifest.c2pa`
   is the obvious starting point.
4. Integration test: `cargo test -p lvqr-archive
   --test c2pa_sign` hits a fixture cert + key pair
   (bundle in `crates/lvqr-archive/tests/fixtures/`),
   exercises the sign path, reads the manifest back
   via `c2pa-rs`'s reader, and asserts the author +
   content hash.
5. Anti-scope for A: no admin verify route (that is
   session B), no operator-supplied PKI (MVP uses
   `c2pa-rs` bundled Adobe test CA), no live
   signed-as-you-go manifests (file-at-rest only,
   covers the legal-discovery / broadcast-archive /
   journalism use cases the plan names).

Expected scope: ~250-400 LOC (C2paConfig struct +
finalize hook + fixture cert/key + integration test +
`docs/security.md` or similar pointer section; plus a
workspace dep pin). Biggest risk: the `c2pa-rs` API is
still pre-1.0 and may require an adapter if the shape
does not match the plan's mental model; if so,
session A surfaces that + the adapter is worth
carrying into session B as shared infrastructure.

Pre-session checklist:

- Read `tracking/TIER_4_PLAN.md` section 4.3 top-to-
  bottom. It is short (the whole section is ~40
  lines); no staleness risk comparable to 4.1's
  session-88 replan but worth confirming.
- Check `c2pa-rs` on crates.io for the current 0.x
  version. If it is a large jump from whatever the
  plan targeted, pin to the tested-compatible version
  and note the upgrade as follow-up work.
- Decide on the manifest-on-disk layout before coding
  (flat `manifest.c2pa` next to the final MP4, vs.
  embedded in a sidecar JSON, vs. manifested into the
  MP4 bytes themselves). The plan does not prescribe;
  pick + document in the feat commit.

## Session 89 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.1 session A2: feature-gated
   tokio-uring write path** (`8c71f8c`). One-file body
   swap inside `lvqr_archive::writer` per the A1
   contract. Cross-crate call shape unchanged:
   `lvqr_cli::archive::BroadcasterArchiveIndexer::drain`
   still calls `write_segment(archive_dir, broadcast,
   track, seq, payload)` inside `tokio::task::
   spawn_blocking` and records the returned `PathBuf`
   on the matching `SegmentRef::path`. The io-uring
   path is invisible to callers.

   `Cargo.toml` (workspace) gains a
   `tokio-uring = "0.5"` pin next to the Tier 4 4.2
   `wasmtime` + `notify` pins. Declared once at the
   workspace level so the version is a single-file
   bump. `crates/lvqr-archive/Cargo.toml` pulls it in
   only under `[target.'cfg(target_os = "linux")'.
   dependencies]` with `optional = true`, and a new
   default-off `io-uring` feature activates it via
   `dep:tokio-uring`. macOS + Windows builds never
   resolve or compile tokio-uring; the feature is
   accepted as a no-op on non-Linux because the
   runtime code paths are gated
   `cfg(all(target_os = "linux", feature = "io-uring"))`.

   `crates/lvqr-archive/src/writer.rs`:
   `write_segment`'s outer signature
   (`fn(archive_dir, broadcast, track, seq, payload)
   -> Result<PathBuf, ArchiveError>`) is unchanged.
   The body splits into `write_payload_std` (always
   present; wraps `std::fs::write`) and
   `write_payload_io_uring` (Linux + feature; wraps
   `tokio_uring::start` inside
   `std::panic::catch_unwind`). `create_dir_all` stays
   on `std::fs` because tokio-uring 0.5 exposes no
   mkdir primitive; the archive tree is amortised
   across thousands of segments per broadcast so the
   extra syscall is noise.

   Fallback design: tokio-uring 0.5's
   `tokio_uring::start` calls
   `runtime::Runtime::new(&builder()).unwrap()`
   internally, with no fallible variant on
   `Builder::start` either. `catch_unwind` is the only
   way to observe a kernel-side setup failure (kernel
   < 5.6, seccomp / sandbox without `io_uring_*`
   syscalls) without aborting the process. A
   process-global `static IO_URING_AVAILABLE:
   OnceLock<bool>` traps the first setup failure, emits
   a single `tracing::warn!`, and latches
   `std::fs::write` for the rest of the process.
   On-path `io::Error`s from `File::create` /
   `write_all_at` / `sync_all` / `close` after the
   runtime comes up surface as `ArchiveError::Io`
   without tripping the latch, so the next segment
   retries io_uring cleanly.

   New CI job `archive-io-uring` in
   `.github/workflows/ci.yml`: `cargo clippy -p
   lvqr-archive --features io-uring --all-targets --
   -D warnings` + `cargo test -p lvqr-archive
   --features io-uring` on `ubuntu-latest`. Separate
   job rather than a matrix cell on the existing
   `test` job so macOS CI time does not grow. The
   existing ubuntu + macos matrix on the default
   feature path is unchanged.

   Plan refresh (`tracking/TIER_4_PLAN.md` section
   4.1): A2 row flipped to **DONE (session 89)** with
   the shipped-option note. Risks section gains a
   bullet documenting the `tokio_uring::start`
   panic-on-setup nuance so session B knows the
   `catch_unwind` is deliberate and not a bug.

2. **Session 89 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 1 | `writer::tests::write_segment_io_uring_matches_std_bytes` in `lvqr-archive/src/writer.rs` | cfg-gated on `all(target_os = "linux", feature = "io-uring")`; runs on the new `archive-io-uring` CI job only. Asserts byte-identity vs. the payload + that the OnceLock fallback latch did NOT trip (a trip on a recent kernel signals an environmental problem, not a code bug). |

Total workspace tests on macOS: **739** (unchanged
from session 88; the io-uring test is cfg-gated out
locally). The Linux `archive-io-uring` job adds one
additional test to the Linux-specific count.

### Ground truth (session 89 close)

* **Head**: `8c71f8c` (feat) on `main` before this
  close-doc commit lands; after both commits local
  main is 2 commits ahead of `origin/main` at session
  89 close. Verify via
  `git log --oneline origin/main..main` before any
  push. Do NOT push without direct user instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored on
  macOS.
* **CI gates locally clean**: `cargo fmt --all --
  --check`, `cargo clippy --workspace --all-targets
  --benches -- -D warnings`, `cargo test --workspace`
  all green. `cargo clippy -p lvqr-archive --features
  io-uring --all-targets -- -D warnings` also green
  on macOS (the feature is a compile-time no-op on
  non-Linux so clippy is still meaningful cover for
  the std path under the feature flag).
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **A1 + A2 DONE**, B pending | 88 (A1) / 89 (A2) / 90 (B) |
| 4.3 | C2PA signed media | PLANNED | 91-92 |
| 4.8 | One-token-all-protocols | PLANNED | 93-94 |
| 4.5 | In-process AI agents | PLANNED | 95-98 |
| 4.4 | Cross-cluster federation | PLANNED | 99-101 |
| 4.6 | Server-side transcoding | PLANNED | 102-104 |
| 4.7 | Latency SLO scheduling | PLANNED | 105-106 |

### Runtime-integration findings (for session 90 B)

Per the plan note, A2 ships option (a) (per-segment
`tokio_uring::start` inside `spawn_blocking`) and
leaves option (b) (persistent current-thread runtime
pinned to a dedicated writer thread) for B to decide
based on criterion numbers. A few observations the
bench should carry forward:

* **Per-call runtime setup cost is the variable to
  measure.** Each `tokio_uring::start` constructs a
  fresh io_uring submission queue + completion queue
  pair (default entries from
  `tokio_uring::builder()`). On a 4 KiB unit-test
  payload this is not visible but on a 64 KiB segment
  the setup may still dominate the actual write. The
  bench at `crates/lvqr-archive/benches/
  io_uring_vs_std.rs` should parameterise segment size
  across `[4 KiB, 64 KiB, 256 KiB, 1 MiB]` so the
  crossover point is visible.

* **`catch_unwind` is in the hot path.** Session B
  should measure the cost of the `AssertUnwindSafe`
  wrapper + the catch_unwind call itself, not just
  the io_uring submission. If the overhead is
  non-trivial, an alternative is to do the probe once
  via a dedicated "io_uring availability" check at
  startup, set the latch to the outcome, and skip the
  `catch_unwind` on every subsequent call. This is a
  follow-up for B's write-up, not an A2 change.

* **The OnceLock fallback has not been observed in
  test.** The new `write_segment_io_uring_matches_std_bytes`
  test asserts the latch is NOT `Some(false)` on a
  recent-kernel runner. If the Linux CI job ever
  reports a latch trip, it almost certainly means the
  GitHub Actions image dropped `io_uring_*` from the
  default seccomp profile (has happened historically
  with container runtimes) rather than a code bug.
  Document the failure mode in B's
  `docs/deployment.md` section so operators know what
  a cold-start `tracing::warn!` from lvqr-archive
  means in production.

* **`create_dir_all` staying on std::fs is a
  principled choice, not a shortcut.** tokio-uring 0.5
  has no mkdir / mkdirat primitive. The archive tree
  is `<root>/<broadcast>/<track>/` and segments live
  under the `<track>` leaf, so the tree-creation cost
  is O(broadcasts * tracks) while segment writes are
  O(broadcasts * tracks * segments_per_track); for any
  DVR window longer than a few seconds the mkdir cost
  is negligible. If `io_uring_mkdirat` lands upstream
  this can be revisited, but it is explicitly
  anti-scope for session B.

### Session 90 entry point

**Tier 4 item 4.1 session B: criterion bench + docs.**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.1
session B:

1. New bench `crates/lvqr-archive/benches/
   io_uring_vs_std.rs` under criterion. Compare
   `write_segment` throughput (MB/s) + p99 latency
   between the std::fs body and the io-uring body on
   a 1-hour synthetic broadcast. Parameterise segment
   size across `[4 KiB, 64 KiB, 256 KiB, 1 MiB]` so
   the crossover point is visible. Run via
   `cargo bench -p lvqr-archive --features io-uring`
   on Linux (macOS cannot exercise io_uring; the
   bench file needs a `cfg(all(target_os = "linux",
   feature = "io-uring"))` guard on its bench
   harness so macOS `cargo bench --workspace` does
   not fail).
2. `docs/deployment.md` gains a "when to enable the
   io_uring backend" section citing the bench
   numbers. Include the OnceLock fallback failure
   mode so operators recognise the cold-start
   `tracing::warn!`.
3. If the bench shows the per-segment
   `tokio_uring::start` setup cost dominates writes
   on small segments, plan-and-land option (b)
   (persistent current-thread runtime on a dedicated
   writer thread) as a session-B extension or a new
   session C. Leave it out of session B's first
   commit until the numbers force it.

Expected scope: ~250-400 LOC (bench + docs section +
any small refactors the bench surfaces). Biggest risk:
the bench result may show io-uring is net-negative on
small segments, in which case the default-off feature
is the right ship state and the docs section needs to
be honest about it.

## Session 88 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.1 session A1: archive writer extraction +
   plan refresh** (`ec7ef01`). Pure refactor, no behavior
   change.

   New module `crates/lvqr-archive/src/writer.rs` (~170 LOC
   including 6 unit tests). Exposes
   `lvqr_archive::writer::write_segment(archive_dir, broadcast,
   track, seq, payload) -> Result<PathBuf, ArchiveError>` and
   `segment_path(archive_dir, broadcast, track, seq) -> PathBuf`,
   plus a private `SEGMENT_FILENAME_FMT_WIDTH = 8` constant that
   documents the canonical `<seq:08>.m4s` filename format.
   `write_segment` is synchronous (matches the previous
   `std::fs::create_dir_all` + `std::fs::write` behavior) and
   returns the resulting `PathBuf` on success so callers can
   record it on the matching `SegmentRef::path`. New
   `ArchiveError::Io(String)` variant.

   `lvqr-cli/src/archive.rs` refactored to call
   `lvqr_archive::writer::write_segment` from inside the existing
   `tokio::task::spawn_blocking` block. The caller-side
   `BroadcasterArchiveIndexer::segment_path` helper is deleted
   in favor of the crate-owned one. Behavior is unchanged: same
   layout, same sequence numbering, same UTF-8 path check before
   recording into redb, same fail-warn semantics on write error.
   `rtmp_archive_e2e` still green.

   Unit tests: segment path layout (broadcast/track/seq
   subdirs + 8-digit zero-pad), overflow past 8 digits is not
   truncated, `write_segment` creates missing parent dirs,
   `write_segment` is idempotent on the same `(broadcast,
   track, seq)` (overwrites the file), `write_segment` returns
   `ArchiveError::Io` when the archive root is a regular file
   instead of a directory.

   Crate doc (`lvqr-archive/src/lib.rs`) refreshed: the
   pre-session-59 comment claiming "Not a segment writer.
   That is in `lvqr-record`" was stale on both counts (the
   writer moved to `lvqr-cli` in session 59 and now lives in
   `lvqr-archive::writer`). Replaced with a "What this crate
   OWNS" block that calls out the index + the writer; the "NOT"
   block now only lists HTTP playback + transcoding +
   rotation.

2. **Plan refresh** (same commit as item 1).
   `tracking/TIER_4_PLAN.md` section 4.1 rewritten to reflect
   the session 59-60 architecture. Split the original session
   A into two sub-sessions:

   * **A1 (this session, DONE)**: writer extraction +
     `ArchiveError::Io` + plan refresh. No io-uring yet.
   * **A2 (session 89, pending)**: feature-gated `tokio-uring`
     path inside `lvqr_archive::writer::write_segment`.
     Linux-only. Runtime fallback on `tokio_uring::start`
     failure.

   The plan now documents the tokio-uring runtime-integration
   nuance the pre-session-88 plan was silent about: LVQR runs
   multi-thread tokio, but `tokio-uring` needs a current-thread
   runtime. Option (a) spin `tokio_uring::start` per segment
   inside `spawn_blocking`; option (b) pin a long-lived
   current-thread runtime to a dedicated writer thread. A2
   ships option (a); session B's bench decides whether (b)
   pays for itself.

3. **Session 88 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 6 | `writer::tests::*` in `lvqr-archive/src/writer.rs` | ok |

No new integration tests -- the refactor is pure substitution
and `rtmp_archive_e2e` + `playback_surface_honors_shared_auth`
already cover the cross-crate call path.

Total workspace tests: **739** (+6 from session 87's 733).

### Ground truth (session 88 close)

* **Head**: `ec7ef01` (refactor) on `main` before this
  close-doc commit lands; after it lands local main is
  several commits ahead of origin/main (session-87 feat +
  session-87 close + session-88 feat + this close doc all
  queued locally). Verify via
  `git log --oneline origin/main..main` before any push.
  Do NOT push without direct user instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored.
* **CI gates locally clean**: fmt, clippy workspace
  --all-targets --benches -- -D warnings, test --workspace
  all green.
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **A1 DONE**, A2 + B pending | 88 (A1) / 89 (A2) / 90 (B) |
| 4.3 | C2PA signed media | PLANNED | 91-92 |
| 4.8 | One-token-all-protocols | PLANNED | 93-94 |
| 4.5 | In-process AI agents | PLANNED | 95-98 |
| 4.4 | Cross-cluster federation | PLANNED | 99-101 |
| 4.6 | Server-side transcoding | PLANNED | 102-104 |
| 4.7 | Latency SLO scheduling | PLANNED | 105-106 |

Session numbering slipped by one because item 4.1 is now 3
sessions instead of 2; downstream item numbers shifted
accordingly. The plan-as-written budget (27 sessions total
for Tier 4) is unchanged because 4.1's extension comes out
of the original 3-session buffer.

### Session 89 entry point

**Tier 4 item 4.1 session A2: `io-uring` feature on
`lvqr_archive::writer::write_segment`.**

Deliverable per the refreshed
`tracking/TIER_4_PLAN.md` section 4.1 A2 row:

1. Add `tokio-uring = "0.5"` workspace dep, gated on
   `target_os = "linux"`. Pin the exact version.
2. Add `io-uring` feature to `lvqr-archive` (default off).
   When on + target is Linux, `write_segment` spins a
   short-lived `tokio_uring::start` inside its body and
   issues `tokio_uring::fs::File::create` +
   `write_all_at`. The caller stays unchanged; the
   `spawn_blocking` + sync-call shape survives.
3. Runtime fallback: if `tokio_uring::start` fails (kernel
   < 5.6, container sandbox without io_uring syscalls),
   log a `warn` once per process and fall back to the
   `std::fs` path for this write and all subsequent ones.
   Session 89 A2 uses a `std::sync::OnceLock<bool>` for
   the feature-disabled latch so the first failure pins
   the fallback state and subsequent calls skip the probe.
4. No new unit tests for the io-uring path on macOS; a
   `#[cfg(target_os = "linux")]` integration test runs on
   a GitHub Actions `ubuntu-latest` job in
   `.github/workflows/ci.yml` as a separate cell.

Expected scope: ~200-350 LOC (module changes + CI cell +
workspace dep pin). Risk: the per-segment
`tokio_uring::start` overhead may dwarf the io_uring win
on small (<100 KB) segments; session B's bench is the
forcing function that decides whether to promote to a
persistent current-thread runtime in a follow-up.

## Session 87 close (2026-04-18)

### What shipped (1 feat commit + 1 close doc commit)

1. **Tier 4 item 4.2 session C: WASM filter hot-reload**
   (`2fc8196`). Full writeup in the feat commit message;
   synopsis here.

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

2. **Test-contract script comment refresh** (folded into
   `2fc8196`). `scripts/check_test_contract.sh` still
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

3. **Session 87 close doc** (`b4c2263`).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 3 | `reloader::tests::*` in `lvqr-wasm/src/reloader.rs` | ok |
| 1 | `wasm_filter_hot_reload_flips_drop_behavior_mid_stream` in `lvqr-cli/tests/wasm_hot_reload.rs` | ok |

Total workspace tests: **733** (+4 from session 86's 729).

### Ground truth (session 87 close)

* **Head**: `2fc8196` (feat) on `main` before the close-doc
  commit (`b4c2263`) landed. After both commits: local
  main was 2 commits ahead of origin/main on session 87
  close. Session 88 added 2 more (feat `ec7ef01` + close
  `8f1be03`), bringing the count to 4 commits ahead at
  session 88 close.
* **Tests**: **733** passed, 0 failed, 1 ignored.
* **CI gates locally clean**: fmt, clippy workspace
  --all-targets --benches -- -D warnings, test --workspace
  all green.
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status (session 87 view)

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

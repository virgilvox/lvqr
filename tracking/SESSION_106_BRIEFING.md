# Session 106 briefing -- Tier 4 item 4.6 session C

**Kick-off prompt (copy-paste into a fresh session):**

---

You are continuing work on LVQR, a Rust live video streaming server.
Tier 3 is closed. Tier 4 items 4.1 (io_uring archive writes), 4.2
(WASM filters), 4.3 (C2PA signed media), 4.4 (cross-cluster
federation), 4.5 (AI agents + whisper captions), and 4.8
(one-token-all-protocols) are COMPLETE. Tier 4 item 4.6 sessions A
(`lvqr-transcode` scaffold) and B (real GStreamer software ladder
behind a default-OFF `transcode` feature) are DONE. Local `main` is at
`origin/main` (head `79e978c`). Session 106 is Tier 4 item 4.6 session
C: `lvqr-cli` composition-root wiring + the `--transcode-rendition`
CLI flag + an `AudioPassthroughTranscoderFactory` sibling + LL-HLS
master-playlist composition advertising every `<source>/<rendition>`
as a variant.

## Prerequisite: GStreamer install (unchanged from 105 B)

The `transcode` Cargo feature on `lvqr-transcode` (and by extension
`lvqr-cli`'s `transcode` feature) pulls `gstreamer` 0.23 +
`gstreamer-app` + `gstreamer-video` + `glib`. The session 106 C
integration test will need a working GStreamer 1.22+ runtime + the
plugin set base / good / bad / ugly + `gst-libav` on the host.

* macOS: prefer the official `/Library/Frameworks/GStreamer.framework`
  pkgs (runtime + devel) over Homebrew; the Homebrew `gstreamer`
  formula has an LLVM transitive that builds from source. Set PATH +
  PKG_CONFIG_PATH + DYLD_FALLBACK_LIBRARY_PATH in the shell that runs
  `cargo test` (see the session 105 close block in `HANDOFF.md` for
  the exact exports).
* Debian / Ubuntu: `apt install libgstreamer1.0-dev
  gstreamer1.0-plugins-{base,good,bad,ugly} gstreamer1.0-libav`.

Verify with `gst-inspect-1.0 x264enc qtdemux mp4mux avdec_h264`. If
any plugin is missing, surface the absent plugins to the user before
touching code -- a partial install produces confusing drain-time
errors rather than the clean factory-construction opt-out that 105 B
wired.

## Read first, in this order

1. `CLAUDE.md`. Project rules. AGPL-3.0-or-later + commercial
   dual-license. No Claude attribution in commits. No emojis. No
   em-dashes. Max line width 120.
2. `tracking/HANDOFF.md`. Read from the top through the session 105
   close block. The "Session 106 entry point" callout is ground
   truth. Note the three newest commits on `main`:
   * `1796a24` feat(transcode): real gstreamer software ladder
     behind `transcode` feature (session 105 B).
   * `f14dbdf` docs: session 105 close.
   * `adfffe5` fix(transcode): convert appsink output timestamps
     to 90 kHz ticks (audit fix).
   * `79e978c` docs: session 105 push event -- HANDOFF + README
     refresh.
3. `tracking/TIER_4_PLAN.md` section 4.6 (the header is "A + B DONE,
   C pending"). Row 106 C is a one-liner today: "Hardware encoder
   feature flags; benchmark NVENC vs x264." That wording is stale --
   the plan-vs-code rule applies, so scope row 106 C in-commit to
   match the actual session deliverable (CLI wiring + LL-HLS master
   playlist + AAC passthrough). Hardware encoders are explicitly
   deferred post-4.6.
4. `crates/lvqr-transcode/src/{lib,software,passthrough,runner,rendition,transcoder}.rs`.
   The 105 B surface is what 106 C consumes. Key types your new
   code names: `RenditionSpec`, `SoftwareTranscoderFactory::new`,
   `TranscodeRunner::with_ladder`, `TranscodeRunnerHandle`.
5. `crates/lvqr-cli/src/lib.rs` + `src/main.rs`. The composition
   root. Read through `ServeConfig` + `start()` + `ServerHandle` +
   the three existing optional-field patterns (`c2pa`, `whisper`,
   `wasm_filter`). Your new `transcode_renditions` field follows
   the `whisper` shape exactly (feature-gated `Option<Vec<..>>`
   defaulting to empty; flag parses into it; `start()` conditionally
   builds a runner; `ServerHandle` exposes the handle).
6. `crates/lvqr-cli/Cargo.toml`. The `transcode` feature + optional
   `lvqr-transcode` dep + `full` meta-feature were wired in 105 B;
   you will use them but not re-write them.
7. `crates/lvqr-hls/src/`. The existing master-playlist code lives
   here. Read `lib.rs` + the master-playlist renderer + the
   `BroadcasterHlsBridge::install` path in `lvqr-cli/src/lib.rs` so
   you know which file actually emits the master playlist today.
   Session 106 C extends that path to scan the registry for
   `<source>/<rendition>` siblings.
8. `crates/lvqr-agent-whisper/src/factory.rs` + the `--whisper-model`
   path through `lvqr-cli` as the reference precedent for
   "CLI flag -> ServeConfig -> factory install in start() -> handle
   on ServerHandle".
9. `crates/lvqr-cli/tests/whisper_cli_e2e.rs` as the integration-test
   shape precedent (feature-gated, `#[ignore]`-able when the heavy
   dep set is absent, publishes through an ingress and asserts on
   the egress).
10. Auto-memory at
    `/Users/obsidian/.claude/projects/-Users-obsidian-Projects-ossuary-projects-lvqr/memory/`.
    `project_lvqr_status.md` is refreshed through session 105 close.

## Ground truth (session 105 push event, 2026-04-21)

* Head: `79e978c` on `main`, synced with `origin/main`. Verify with
  `git log --oneline origin/main..main` (empty).
* Tests (default features): **892** passed, 0 failed, 1 ignored on
  macOS. The 1 ignored is the pre-existing `moq_sink` doctest.
* Tests (transcode feature):
  `cargo test -p lvqr-transcode --features transcode --lib` 23
  passed (+7 inline on `software.rs`);
  `cargo test -p lvqr-transcode --features transcode --test
  software_ladder` 1 passed (~0.3 s wall clock after first build,
  31 output fragments per rendition, 720p ~2280 kbps / 480p
  ~1144 kbps / 240p ~384 kbps against target 2500 / 1200 / 400 --
  within +/-10% of target across three consecutive runs).
* Workspace: 29 crates. 25 published to crates.io at v0.4.0; 3 are
  `publish = false`; `lvqr-transcode` is a pending first-time
  publish.
* Gates green: `cargo fmt --all --check`;
  `cargo clippy --workspace --all-targets --benches -- -D warnings`;
  `cargo clippy -p lvqr-transcode --features transcode --all-targets
  -- -D warnings`; `cargo test --workspace` 892 / 0 / 1.
* Carry-forward:
  * `SoftwareTranscoderFactory::new(rendition, output_registry)`
    already exists and probes GStreamer plugins at construction.
    Your wiring calls it three times (once per rendition in
    `ServeConfig.transcode_renditions`) inside `start()`.
  * `TranscodeRunner::with_ladder(ladder, |spec| factory_from(spec))`
    is the install shape. Lock the runner handle onto `ServerHandle`
    alongside `agent_runner` / `wasm_filter`.
  * `lvqr_transcode_output_fragments_total{transcoder, rendition}`
    is the metric downstream surfaces read to find live renditions;
    the LL-HLS master-playlist composer can read the registry
    directly instead, but a metric-driven path is a future option.
  * `SoftwareTranscoder` writes output `dts` / `pts` / `duration`
    in 90 kHz ticks (post-audit fix `adfffe5`). The HLS bridge's
    existing conversion math applies unchanged.
  * The recursion guard `looks_like_rendition_output(broadcast)`
    treats any trailing `\d+p` component as already-transcoded. A
    custom rendition name like "ultra" would bypass the guard;
    session 106 C adds an explicit
    `SoftwareTranscoderFactory::skip_source_suffixes(Vec<String>)`
    builder for operators using non-conventional names -- the
    default heuristic stays.

## Session 106 scope -- four deliverables

1. **CLI flag + ServeConfig wiring on `lvqr-cli`** (feature-gated on
   `transcode`).
   * `ServeConfig.transcode_renditions: Vec<RenditionSpec>`,
     `#[cfg(feature = "transcode")]`, default empty.
   * `--transcode-rendition <RENDITION>` repeatable clap arg
     (`action = ArgAction::Append`) + `LVQR_TRANSCODE_RENDITION`
     env fallback (comma-separated when read from env). Parse short
     preset names (`720p`, `480p`, `240p`) to `RenditionSpec::preset_*`
     directly; a path ending in `.toml` is read + deserialized as a
     custom `RenditionSpec` via serde. Anything else is an error at
     clap parse time so operators see the failure up front.
   * `start()`: when `transcode_renditions` is non-empty, build one
     `SoftwareTranscoderFactory` per rendition (share the
     `fragment_registry` clone for both input and output), optionally
     layer the 106 C audio-passthrough factory (see deliverable 3
     below), install via
     `TranscodeRunner::with_factory(..).install(&fragment_registry)`,
     and stash the returned `TranscodeRunnerHandle` on
     `ServerHandle.transcode_runner: Option<TranscodeRunnerHandle>`.
   * `ServerHandle.transcode_runner()` accessor mirrors the
     `agent_runner()` / `wasm_filter()` shape.
   * `lvqr-test-utils` gains `TestServerConfig.transcode_renditions`
     + `with_transcode_ladder(Vec<RenditionSpec>)` builder
     (feature-gated on a new `transcode` feature that forwards to
     `lvqr-cli/transcode`).
   * 2 new inline tests in `lvqr-cli` (feature-gated): default
     `ServeConfig::loopback_ephemeral()` has no renditions;
     `--transcode-rendition 720p` parses to
     `RenditionSpec::preset_720p()`.

2. **LL-HLS master playlist composition** on `lvqr-hls`.
   * The existing `BroadcasterHlsBridge::install` path currently
     emits one media playlist per source broadcast. Session 106 C
     extends the master-playlist composer to scan the registry for
     `<source>/<rendition>` siblings when composing
     `<source>/master.m3u8` and emit one `#EXT-X-STREAM-INF` per
     sibling.
   * Each variant line needs `BANDWIDTH` (the rendition's
     `video_bitrate_kbps * 1000` + a 10% overhead margin),
     `RESOLUTION=<W>x<H>`, `CODECS="avc1.640028,mp4a.40.2"` (or the
     real init-segment-derived strings if easy; otherwise a
     placeholder), `NAME="<rendition.name>"`, and the relative URI
     to the variant's media playlist (`./<rendition>/playlist.m3u8`
     resolves to `/hls/<source>/<rendition>/playlist.m3u8` under the
     existing HLS routing).
   * The source variant itself is the first entry at an operator-
     supplied or heuristic bandwidth (default: the highest rung's
     bandwidth + 20%). `--source-bandwidth-kbps <N>` CLI flag
     overrides per-broadcast; 107 A's latency SLO infrastructure can
     replace this with source-measurement later.
   * The variant ordering in the master playlist is highest-to-
     lowest bandwidth, matching the HLS ABR-client expectation.
   * Unit test in `lvqr-hls`: synthetic registry with a source +
     three renditions produces a master playlist with four variants
     and the expected `BANDWIDTH` + `RESOLUTION` + `NAME`.

3. **AAC audio passthrough sibling transcoder** (tail end of 106 C).
   * New `AudioPassthroughTranscoderFactory` in `lvqr-transcode`.
     Always-available (NOT feature-gated on `transcode`) because
     it carries no GStreamer dep; it copies `Fragment` instances
     from `<source>/1.mp4` to `<source>/<rendition>/1.mp4`
     verbatim. Pattern-matches the existing `PassthroughTranscoder`
     shape (observe + republish instead of observe-only).
   * The factory takes a `rendition: RenditionSpec` +
     `output_registry: FragmentBroadcasterRegistry` + an optional
     `skip_source_suffixes: Vec<String>` (defaults to the `\d+p`
     heuristic). `build(ctx)` returns `Some` only for
     `ctx.track == "1.mp4"` and broadcasts that do not look like
     already-transcoded outputs.
   * `start()` installs one `AudioPassthroughTranscoderFactory` per
     rendition alongside the `SoftwareTranscoderFactory`. This
     means 6 drain tasks per source (3 video + 3 audio) instead of
     3.
   * 3-5 inline tests on `audio_passthrough.rs`: non-audio-track
     opt-out, audio-track opt-in, rendition-output skip, fragment
     copies preserve payload bytes.

4. **End-to-end integration test**
   `crates/lvqr-cli/tests/transcode_ladder_e2e.rs` (feature-gated on
   `transcode` + `rtmp`; skip-with-log if GStreamer plugins are
   absent). Boots a `TestServer` with
   `.with_transcode_ladder(RenditionSpec::default_ladder())`,
   publishes 3 s of test-pattern video + silent AAC via ffmpeg RTMP,
   waits for the HLS master playlist at
   `/hls/live/demo/master.m3u8`, asserts the master has four
   variants (source + three renditions) with correct `BANDWIDTH`
   / `RESOLUTION` / `NAME` attributes, fetches each variant's media
   playlist + a keyframe segment to confirm real bytes flow end to
   end. Reuse the ffmpeg RTMP helpers from
   `crates/lvqr-cli/tests/rtmp_hls_e2e.rs` + the hand-rolled HTTP
   client from `auth_integration.rs`. Do NOT add `reqwest`.

## First decisions to lock in-commit (plan-vs-code rule applies)

(a) **CLI flag shape**: `--transcode-rendition <NAME>` repeatable.
    Short presets (`720p` / `480p` / `240p`) as clap parse sugar;
    `.toml` suffix triggers a `toml::from_str::<RenditionSpec>`
    read; anything else is a clap validation error. Env var
    `LVQR_TRANSCODE_RENDITION` is comma-separated since clap's env
    parser does not repeat.

(b) **Source-variant BANDWIDTH**: defaults to `highest_rung_kbps *
    1.2 * 1000`. `--source-bandwidth-kbps <N>` operator override.
    107 A can replace this with source-measurement when the
    latency-SLO infrastructure lands.

(c) **Master-playlist URI base**: the master playlist currently
    served at `/hls/<source>/master.m3u8` uses relative URIs
    (`./<rendition>/playlist.m3u8`) so CDN caching and operator
    reverse-proxy setups keep working without master-playlist
    rewrites. Each rendition's media playlist stays at the same
    absolute path the existing HLS bridge already publishes.

(d) **CODECS attribute**: hard-coded
    `"avc1.640028,mp4a.40.2"` for 106 C. 107 or later can parse the
    actual SPS + audio ASC from the rendition's init segment and
    replace the placeholder.

(e) **`SoftwareTranscoderFactory::skip_source_suffixes`**: new
    builder method on the factory (not a breaking change: the
    existing `new()` constructor stays). Operators using
    non-conventional rendition names pass their full list of
    sibling names; the factory appends them to the built-in
    `\d+p` heuristic rather than replacing it. One new inline test.

(f) **Audio passthrough track name**: output on
    `<source>/<rendition>/1.mp4` to match the standard LVQR
    `"1.mp4"` audio-track convention. The master playlist's
    `#EXT-X-MEDIA` entry for audio groups references the audio
    track per HLS spec; confirm the existing HLS bridge already
    emits audio renditions the way we need.

(g) **Hardware encoders deferred**: the plan-v1 row 106 C reads
    "Hardware encoder feature flags; benchmark NVENC vs x264" which
    is stale. 106 C's real scope is CLI wiring + HLS composition +
    AAC passthrough. Refresh row 106 C in the same commit that
    flips its status to DONE, and note HW encoders as a
    post-4.6 item.

## Test shape

1. `cargo test -p lvqr-transcode` (no features): 16 passed + 1
   doctest, unchanged from 104 A / 105 B. 106 C adds the
   always-available `AudioPassthroughTranscoderFactory` inline
   tests (~3-5 new tests) so this count goes up to ~19-21.
2. `cargo test -p lvqr-transcode --features transcode --lib`: 23
   passed + any new inline on `software.rs` for the
   `skip_source_suffixes` builder (~1-2 new tests).
3. `cargo test -p lvqr-transcode --features transcode --test
   software_ladder`: unchanged, 1 passed.
4. `cargo test -p lvqr-cli --features transcode`: 2-3 new inline
   tests on `lvqr-cli/src/lib.rs` covering the
   `ServeConfig.transcode_renditions` round-trip.
5. `cargo test -p lvqr-hls`: 1-2 new inline tests on the master-
   playlist composer.
6. `cargo test -p lvqr-cli --test transcode_ladder_e2e`: the new
   end-to-end test. Wall clock 5-15 s depending on the ffmpeg RTMP
   publish + HLS generation path.
7. `cargo test --workspace`: default features; expect 892 + the new
   always-available audio-passthrough tests = ~895-897.

## Verification gates (session 106 C close)

* `cargo fmt --all --check`.
* `cargo clippy --workspace --all-targets --benches -- -D warnings`.
* `cargo clippy -p lvqr-transcode --features transcode --all-targets
  -- -D warnings`.
* `cargo clippy -p lvqr-cli --features transcode --all-targets
  -- -D warnings`.
* `cargo test -p lvqr-transcode` green (no features parity).
* `cargo test -p lvqr-transcode --features transcode --lib` green.
* `cargo test -p lvqr-transcode --features transcode --test
  software_ladder` green.
* `cargo test -p lvqr-cli --features transcode` green.
* `cargo test -p lvqr-cli --features transcode --test
  transcode_ladder_e2e` green.
* `cargo test -p lvqr-hls` green.
* `cargo test --workspace` (default features) green.
* `git log -1 --format='%an <%ae>'` reads
  `Moheeb Zara <hackbuildvideo@gmail.com>` alone for every commit.

Prefer targeted `-p <crate> --features transcode` runs during
iteration; only run `--workspace` for the pre-close verification
pass.

## Absolute rules (hard fails if violated)

* NEVER add Claude as author or co-author. No `Co-Authored-By`
  trailers. Verify with `git log -1 --format='%an <%ae>'` after
  every commit.
* No emojis in code, commit messages, or documentation.
* No em-dashes or obvious AI language patterns in prose.
* Max line width 120. fmt + clippy + test must be clean before
  committing.
* Integration tests use real ingress + egress (real RTMP publish,
  real HLS fetch), not mocks. Asserting on plumbing state without
  real encoded bytes would be a theatrical test.
* Only edit files within
  `/Users/obsidian/Projects/ossuary-projects/lvqr/`.
* Do NOT push or publish without a direct instruction from the
  user.
* If the plan and the code disagree, refresh the plan in the same
  commit as the code change.

## Expected scope + biggest risks

~800-1200 LOC across `lvqr-cli/src/lib.rs` (+ `src/main.rs` flag
wiring), a new `crates/lvqr-transcode/src/audio_passthrough.rs`,
`crates/lvqr-hls/src/` master-playlist extension, inline tests, and
the new integration test. Plus smaller edits on
`lvqr-test-utils/src/test_server.rs`,
`crates/lvqr-cli/Cargo.toml`, and plan / HANDOFF / README.

Risks, ranked:

1. **LL-HLS master-playlist relative-URI scheme**. The existing
   master playlist is emitted for single-variant broadcasts; your
   new multi-variant path has to keep the per-rendition media
   playlist URLs stable across a CDN + operator reverse-proxy
   chain. Read the existing HLS bridge before deciding on the URL
   scheme. Mitigation: start with relative URIs that resolve at
   the master's path depth (`./<rendition>/playlist.m3u8`).
2. **Source-variant BANDWIDTH**. If the source has a real bitrate
   the master playlist should advertise it accurately; without a
   bitrate measurement the "+20% of highest rung" heuristic is a
   decent placeholder but operators with a non-conventional setup
   may see ABR clients pick the wrong variant. Document the
   limitation in the session 106 close block.
3. **Master-playlist emission trigger**. The existing HLS bridge
   emits a master playlist when the source broadcaster appears.
   The renditions appear later (after the source publishes its
   first fragment and the transcoder worker spins up). The master
   playlist needs to either (a) be re-rendered when a rendition
   registers, or (b) be rendered on HTTP GET (dynamic) rather than
   statically. Dynamic rendering is the safer call.
4. **Audio passthrough fragmentation**. LVQR audio fragments are
   already one raw AAC frame per `Fragment`; the passthrough just
   copies them into the rendition-track broadcaster. Watch out for
   `FragmentMeta::init_segment`: the source's ASC (AudioSpecific
   Config) needs to propagate to every rendition's audio broadcast
   so late-joining HLS subscribers can decode.
5. **Feature flag chaos in `lvqr-test-utils`**. Adding a
   `transcode` feature on test-utils means `lvqr-cli`'s dev-deps
   need to activate it in the feature-gated test paths. Follow
   the existing `whisper` / `c2pa` pattern exactly.
6. **CI availability**. Runners without GStreamer will fail the
   `--features transcode` integration test. Mitigation: factory
   opt-out on missing plugins + test skip-with-log already exists
   in the 105 B surface; new 106 C tests should mirror it. Document
   the install recipe in the 106 C close block.

## After session 106 C

* Write a "Session 106 close" block at the top of `HANDOFF.md`
  under the session 105 push event block.
* Flip section 4.6 header in `TIER_4_PLAN.md` from "A + B DONE, C
  pending" to **COMPLETE**; scope row 106 C's one-liner into the
  full deliverable + verification record in the same commit.
* Refresh `project_lvqr_status.md` memory.
* Tier 4 item 4.6 is COMPLETE after this session lands; 7 of 8
  Tier 4 items COMPLETE. Remaining: 4.7 (latency SLO scheduling).
* Session 107 entry point: Tier 4 item 4.7 session A (latency SLO
  histogram wiring + `/api/v1/slo` admin route). Read
  `tracking/TIER_4_PLAN.md` section 4.7 row 107 A.
* Commit feat + docs as two commits. Do NOT push without a direct
  user instruction; if pushed, follow up with a `docs: session 106
  push event` commit that refreshes the HANDOFF status header to
  `origin/main synced (head <new>)` and refreshes `README.md` with
  the full 4.6 completion (Tier 4 progress hook, 4.6 bullet, crate
  map entry, CLI reference) as the session 105 push event did.

Work deliberately. Each commit should tell a future session exactly
what changed and why. Do not mark anything DONE until verification
passes.

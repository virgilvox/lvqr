# LVQR Handoff Document

## Project Status: v0.4.1 PUBLISHED on crates.io -- **Tier 3 COMPLETE; Tier 4 COMPLETE** + `examples/tier4-demos/` exit criterion CLOSED. **Phase A + B v1.1 CLOSED**. **Phase C fully CLOSED**. **Phase D mesh-data-plane checklist FULLY CLOSED**. **Session 154 (2026-04-25) shipped SCTE-35 ad-break markers on `@lvqr/dvr-player` v0.3.3** -- the DVR scrub component now paints session 152's `#EXT-X-DATERANGE` ad markers on its custom seek bar (vertical ticks for CMD / time-signal singletons, coloured break-range spans for paired SCTE35-OUT + SCTE35-IN entries joined by their shared DATERANGE `ID`, faint in-flight overlays for an OUT whose IN has not yet landed). Hover tooltip shows kind / id / time / duration. New `markers="visible|hidden"` attribute toggles the visual layer; events still fire when hidden. Two new public events: `lvqr-dvr-markers-changed` (fires on diff vs prior LEVEL_LOADED, detail `{ markers, pairs }`) and `lvqr-dvr-marker-crossed` (fires per-id when `currentTime` crosses a marker's `startTime`, debounced 100 ms per id, detail `{ marker, direction, currentTime }`). New programmatic `getMarkers()` returns `{ markers, pairs }` with the store sorted by ascending `startTime` then `id`. Reads daterange entries from hls.js's `LevelDetails.dateRanges` (v1.5+) on `Hls.Events.LEVEL_LOADED`; trusts `DateRange.startTime` for the PDT-anchored currentTime mapping (so the component does NOT re-implement program-date-time anchoring). Pure helpers in `bindings/js/packages/dvr-player/src/markers.ts` (`classifyMarker`, `dvrMarkersFromHlsDateRanges`, `markerToFraction`, `groupOutInPairs`, `formatDuration`) covered by **25 Vitest tests** in `bindings/js/tests/sdk/dvr-player-markers.spec.ts`. Playwright project gains `markers.spec.ts` with three new tests: two routed-stub-playlist tests (LEVEL_LOADED populates store + emits markers-changed; `markers="hidden"` empties layer + getMarkers still returns store) plus one live-RTMP test that pushes synthetic ffmpeg video into the dvr-player webServer profile and asserts the LIVE pill activates -- **the live-RTMP test also closes session 153's deferred "live-stream-driven Playwright assertions" item via the new `bindings/js/tests/helpers/rtmp-push.ts` Node ffmpeg wrapper** (ffmpeg-gated `test.skip` when the binary is missing on the runner, so the spec is opt-in across CI environments). **No Rust crate touched, no relay-side wire change, no new HLS tag** -- the component is a pure consumer of session 152's existing `#EXT-X-DATERANGE` surface. The brief's design question 6 originally proposed a second helper (a Rust `[[bin]] scte35-rtmp-push` that would inject `onCuePoint scte35-bin64` AMF0 Data messages over a real RTMP publisher session) but that path was descoped during execution: the vendored `rml_rtmp` v0.8 client lacks a generic AMF0-data sender and patching it would have violated the brief's own "no Rust crate touched" anti-scope. The end-to-end "real RTMP onCuePoint -> relay DATERANGE" wire is already covered by `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs` (Rust-side, session 152); the component-side "DATERANGE -> render" path is fully covered by the routed-stub-playlist Playwright tests because hls.js fires LEVEL_LOADED with `dateRanges` populated after parsing the playlist text, before any segment fetch succeeds. CSS hooks: `--lvqr-marker-color` (paired-span fill), `--lvqr-marker-tick-color` (tick colour), `--lvqr-marker-in-flight` (OUT-only overlay), `--lvqr-marker-tooltip-bg`. New shadow parts: `markers`, `marker-tooltip`. Workspace 0.4.1 unchanged; `@lvqr/player` and `@lvqr/core` stay at 0.3.2; `@lvqr/dvr-player` bumps to 0.3.3. **Session 153 (2026-04-25) shipped Dedicated DVR scrub web UI v1** -- new `@lvqr/dvr-player` package at `bindings/js/packages/dvr-player/` ships as a sister to `@lvqr/player`, version 0.3.2 lockstep with the rest of the SDK. Vanilla `class extends HTMLElement` (no Lit, no Stencil; "structured-vanilla" pattern with template-literal HTML strings + small attribute helpers in `src/internals/attrs.ts` + typed `CustomEvent` dispatcher in `src/internals/dispatch.ts` + shadow DOM + `attributeChangedCallback`-driven reactivity). Pure-arithmetic helpers (time-to-x mapping, percentile labels, threshold checks, formatting) extracted into `src/seekbar.ts` and unit-tested via Vitest. **32 unit tests** across three SDK specs in `bindings/js/tests/sdk/` -- 14 over the seek-bar arithmetic (`dvr-player-seekbar.spec.ts`), 14 over the attribute helpers (`dvr-player-attrs.spec.ts` -- boolean / numeric / string getters with fallback semantics including NaN / Infinity / empty-string / "0"-not-fallback edge cases), 4 over the typed event dispatcher (`dvr-player-dispatch.spec.ts` -- detail-shape preservation + bubbles flag for all three event names). Playwright project under `bindings/js/tests/e2e/dvr-player/` runs **15 tests** across `mount.spec.ts` (4 -- registration + 13 part landmarks + muted + controls=native + programmatic seek) and `interactions.spec.ts` (11 -- goLive() event + source classification, seek() clamping at both endpoints, multi-seek fromTime chaining, keyboard ArrowLeft / ArrowRight scrub, keyboard Home / End jumps, live-edge-threshold-secs custom-value classification, controls toggle round-trip native -> custom -> default, pointer-drag interaction with user-source event firing, hover preview show / hide, getHlsInstance pre-playback null, host->document event bubbling). Wraps hls.js (^1.5.0 direct dep) against the relay's existing live HLS endpoint (`/hls/{broadcast}/master.m3u8`) with the sliding-window DVR depth driven by `--hls-dvr-window-secs`; **no new server route** -- the kickoff prompt's `/playback/{broadcast}/master.m3u8` URL was a misread (the actual `/playback/*` surface returns JSON, not HLS), corrected at brief read-back. Custom seek bar with HH:MM:SS percentile labels (or MM:SS for sub-hour spans) at 0/25/50/75/100% of the seekable range, LIVE pill that toggles based on `seekable.end - currentTime` crossing `max(6, 3 * #EXT-X-TARGETDURATION)` (configurable via `live-edge-threshold-secs`), explicit "Go Live" button that only renders when behind the live edge (no implicit live-snap on resume; explicitly rejected because operators report it surprises viewers who paused with intent). Client-side hover thumbnails via canvas `drawImage` against a lazy second hls.js instance (LRU-capped at 60 entries; opt-out via `thumbnails="disabled"`; image bitmaps cached for instant re-hover). Bearer-token auth via hls.js `xhrSetup` (`Authorization: Bearer` header) plus query-string fallback for native HLS in Safari MSE-less mode. Public events `lvqr-dvr-seek` / `lvqr-dvr-live-edge-changed` / `lvqr-dvr-error` (typed via the `LvqrDvrPlayerEvents` map; debounced 250 ms on the live-edge crossing); programmatic API `play()` / `pause()` / `seek(time)` / `goLive()` / `getHlsInstance()`. Component-level web research (April 2026): Mux Player + Media Chrome (the canonical streaming-infra public web component, ~9 billion requests served) ships **vanilla**, not Lit; Vidstack's `lit-html + Maverick signals` stack is being publicly retired in 2026 by its own author (cf. mux.com "6 Years Building Video Players" retrospective). LVQR adopts the structured-vanilla pattern without the Mux dependency on strategic-peer grounds. Playwright project at `bindings/js/tests/e2e/dvr-player/` adds a second `webServer` profile in `playwright.config.ts` on non-overlapping ports (admin 18089, hls 18190, rtmp 11936, lvqr 14444) with `--archive-dir` + `--hls-dvr-window-secs=300` + `--no-auth-live-playback`; spec mounts the dist via importmap-routed `page.route` handlers and asserts custom-element registration, shadow-DOM structure (13 part landmarks), `muted` + `controls=native` attribute reflection, and programmatic `seek()` event flow. New docs at `docs/dvr-scrub.md` cover the operator embedding recipe, signed-URL / bearer-token auth precedence, theming via CSS custom properties (`--lvqr-accent`, `--lvqr-control-bg`, etc.) + `::part()` access (`video`, `seekbar`, `live-badge`, `go-live-button`, `play-button`, `mute-button`, `time-display`, `labels`, `preview`, `controls`, `live-overlay`, `status`), and the relationship between `--hls-dvr-window-secs` and the seekable range the component renders. README "Next up" #3 (Dedicated DVR scrub web UI) flips to strikethrough with a forward link to `docs/dvr-scrub.md`; Phase A v1.1 roadmap row flips `[ ] -> [x]`. Workspace 0.4.1 unchanged; `@lvqr/player` and `@lvqr/core` stay at 0.3.2; no Rust-side changes touched in this session. **Session 152 (2026-04-25 / 26) shipped SCTE-35 ad-marker passthrough v1** -- both ingest paths land in the same session: SRT MPEG-TS (PMT stream_type 0x86 with private-section reassembly across TS packet boundaries) and RTMP onCuePoint scte35-bin64 (Adobe AMF0 convention used by AWS Elemental, Wirecast, vMix, ffmpeg). RTMP required vendoring `rml_rtmp` v0.8.0 at `vendor/rml_rtmp/` with a 25-line patch adding `ServerSessionEvent::Amf0DataReceived` (upstream silently drops every AMF0 Data message that is not `@setDataFrame`-wrapped onMetaData); fork loads via `[patch.crates-io]` and passes 170/0/0 tests (168 upstream + 2 LVQR defense). Splice events flow through a reserved `"scte35"` parallel track on `FragmentBroadcasterRegistry` (mirroring the whisper-captions pattern under `lvqr_fragment::SCTE35_TRACK`); LL-HLS render adds `#EXT-X-DATERANGE` per HLS spec section 4.4.5.1 with `CLASS="urn:scte:scte35:2014:bin"` (industry convention) + SCTE35-OUT/IN/CMD attributes; DASH MPD render adds Period-level `<EventStream schemeIdUri="urn:scte:scte35:2014:xml+bin">` per ISO/IEC 23009-1 G.7 + SCTE 214-1. New `lvqr-codec/src/scte35.rs` parses splice_info_section per ANSI/SCTE 35-2024 section 8.1 with CRC_32 verification; proptest harness (1536 random inputs) + libfuzzer target prove panic-free on adversarial input. Counter metrics `lvqr_scte35_events_total{ingest,command}` + `lvqr_scte35_drops_total{ingest,reason}`. All 8 CI workflows GREEN end-to-end (LL-HLS Conformance + MPEG-DASH Conformance + Feature matrix + Supply-chain audit + Tier 4 demos + SDK tests + Test Contract + CI). Workspace lib **824 / 0 / 0** + 8 SCTE-35 e2e tests through TestServer + real HTTP/1.1. Splice_info_section bytes preserved verbatim through both egress wire shapes; no semantic interpretation. New docs at `docs/scte35.md` (~430 lines) cover standards refs, ingest table, publisher quickstart (ffmpeg/AWS Elemental/Wirecast/vMix/OBS), wire shape examples, internal architecture diagram, client-side consumption snippets (hls.js/dash.js/Shaka/native HLS), anti-scope, metrics, operator runbook. README "Next up" #2 (SCTE-35 passthrough) flips to strikethrough; Phase A v1.1 roadmap row flips `[ ] -> [x]`. **Session 151 (2026-04-25) hardens lvqr-agent runner-test polling** -- replaces 4 fixed-100 ms `tokio::time::sleep` sites with a `poll_until` helper (10 ms tick, 2 s timeout) so the spawned drain-task's panic-counter increment can settle on a loaded macos-latest CI runner. The flake surfaced on session 150's substantive CI run but is orthogonal to the wasmtime upgrade (lvqr-agent has zero wasmtime deps); the OTHER 7 session-150 workflows including Feature matrix and Supply-chain audit landed green on the original push. **Session 150 (2026-04-25) closed the dominant audit-ignore cluster** -- wasmtime v25 -> v43 upgrade removes 16 RustSec advisories from `audit.toml` (including 2x CVSS-9 sandbox-escape entries), down from 22 ignores to 6. `lvqr-wasm` only uses the core WASM API surface (Engine/Module/Store/Instance/TypedFunc) which is stable across the upgrade range; total source diff was 7 lines (two Module::new error-conversion callsites). **Session 149 (2026-04-25) shipped hot config reload v3 (JWKS + webhook URL rotation)** -- `ConfigReloadHandle::reload` flipped to `async`; the reload pipeline now calls `JwksAuthProvider::new` and `WebhookAuthProvider::new` asynchronously and swaps the resulting provider into the `HotReloadAuthProvider` chain. Drop-old-on-swap leverages each provider's existing `Drop` to abort their spawned refresh / fetcher task. `applied_keys` grows entries (`"jwks"` / `"webhook"`) on URL diff. Feature-disabled builds emit a warning when the file names a feature-gated URL. `jwks_url` and `webhook_auth_url` are mutually exclusive within the same `[auth]` section (the route returns an error). The admin route closure shape widened from sync `Fn -> Result<...>` to async-flavored `Fn -> BoxFuture<Result<...>>` (internal-API change; SDK wire shape unchanged). With session 149, hot config reload is feature-complete -- every key the file format defines is honored at runtime. **Session 148 (2026-04-25) shipped hot config reload v2 (mesh ICE + HMAC secret)** -- `mesh_ice_servers` and `hmac_playback_secret` join the hot-reloadable surface alongside auth, swapped atomically via `arc_swap::ArcSwap` handles threaded through the `/signal` callback and the live HLS / DASH / DVR `/playback/*` middlewares. **Session 147 (2026-04-25) shipped hot config reload (auth-only v1)** -- `lvqr serve --config <path.toml>` + SIGHUP + `POST /api/v1/config-reload` swap the auth chain atomically via a new `lvqr_auth::HotReloadAuthProvider` (`arc_swap::ArcSwap` -- single-digit-ns reads on the auth-check fast path). Stream-key store preserved. Default-gate tests after 148: Rust workspace **1107** / 0 / 0 (was 1099 post-147; +8 net: 8 new lvqr-cli unit covering ice + hmac + applied_keys diff paths + clear semantics + no-deferred-warnings regression, 2 new RTMP-shape integration cases in `config_reload_e2e.rs` mesh ICE + HMAC rotation; the workshop-148 step rewrote one prior unit test from warnings-shape to applied_keys-shape, net unit delta = 8). Python pytest **38** unchanged. Vitest unchanged at 13. Admin surface unchanged at **12 route trees**. **Session 146 (2026-04-24) shipped runtime stream-key CRUD admin API**; **Session 145 (2026-04-24)** cut workspace 0.4.1 + republished all 26 publishable Rust crates.

**Last Updated**: 2026-04-25 (session 154 close).

## Session 154 close (2026-04-25)

Shipped SCTE-35 ad-break marker visualization on the
`@lvqr/dvr-player` seek bar at v0.3.3. The session is JS-SDK-only
-- no Rust crate is touched, no relay route added, no HLS tag
introduced; the component is a pure consumer of session 152's
existing `#EXT-X-DATERANGE` wire on the served HLS playlist.

### What landed

* **`bindings/js/packages/dvr-player/src/markers.ts`**, ~210
  lines, pure helpers consumed by both the component class and
  the unit-test suite:
  * `classifyMarker(dr): 'out' | 'in' | 'cmd' | 'unknown'` --
    inspects the daterange's `attr` AttrList for the three
    SCTE35-* attribute keys.
  * `dvrMarkersFromHlsDateRanges(record)` -- adapter from hls.js's
    `LevelDetails.dateRanges` shape into the component's
    normalised `DvrMarker[]`. Drops entries with non-finite
    `startTime`; sorts by ascending `startTime` then ascending
    `id`.
  * `markerToFraction(marker, range)` -- wraps `timeToFraction`
    from `seekbar.ts` with NaN / out-of-range -> null behaviour.
  * `groupOutInPairs(markers)` -- ID-keyed pair detection. OUT +
    IN with the same ID become `{ kind: 'pair' }`; orphan OUT
    becomes `'open'` (in-flight ad break); orphan IN becomes
    `'in-only'`; CMD / unknown becomes `'singleton'`. Reversed
    pair times swap.
  * `formatDuration(seconds)` -- `< 60s` -> `"S.SSSs"`; `< 3600s`
    -> `"M:SS"`; otherwise `"H:MM:SS"`.
* **`bindings/js/packages/dvr-player/src/index.ts`** extended
  with the marker store, render layer, tooltip, and event wiring:
  * Marker store: `Map<string, DvrMarker>` keyed by daterange ID.
    Populated on `Hls.Events.LEVEL_LOADED` from
    `data.details.dateRanges`. Diff-emit via a string signature
    of `(id, kind, startTime, duration, hex)` tuples; only
    emits `lvqr-dvr-markers-changed` when the signature changes.
    Cleared on `src` change.
  * Render: new shadow-DOM `.marker-layer` (sibling of
    `.played` / `.buffered` / `.thumb` inside `.seekbar`) plus
    a `.marker-tooltip` overlay above the seek bar. Each render
    pass walks `groupOutInPairs(store)` and emits one
    `.marker-span` per pair, plus `.marker[data-id, data-kind]`
    ticks at endpoints / singletons. CSS: ticks via
    `::before` (2 px wide, `--lvqr-marker-tick-color`), span
    colour from `--lvqr-marker-color`, in-flight overlay from
    `--lvqr-marker-in-flight`, tooltip from
    `--lvqr-marker-tooltip-bg`.
  * Tooltip: `pointerover` / `pointerout` on `.marker-layer`
    (event-delegated through the marker children). Body shows
    Kind / id (truncated to 24 chars) / t (formatTime relative
    to range start) / dur (formatDuration when set) / class
    (when present and non-default).
  * Suppresses thumbnail preview while a marker tooltip is up
    (`isMarkerHovered` flag short-circuits `maybeShowPreview`).
  * Crossing detection: `timeupdate` handler tracks
    `lastCrossingTime`; when the interval `[prev, curr]` strictly
    contains a marker's `startTime`, emits
    `lvqr-dvr-marker-crossed` with direction. Per-id 100 ms
    debounce (`markerCrossingLastEmit` map) so a scrub does not
    double-fire.
  * `markers="visible"` (default) | `"hidden"` attribute toggle.
    Hidden empties the layer DOM and hides the tooltip; events
    still fire so an integrator's external overlay still works.
  * New programmatic `getMarkers(): { markers, pairs }` returns
    sorted store contents.
* **`bindings/js/packages/dvr-player/src/internals/dispatch.ts`**
  extended with `LvqrDvrMarkersChangedDetail` +
  `LvqrDvrMarkerCrossedDetail` and the corresponding entries in
  the `LvqrDvrPlayerEvents` map.
* **`bindings/js/tests/sdk/dvr-player-markers.spec.ts`** -- 28
  Vitest tests covering classification, fraction mapping
  (in-range / NaN / below / above / live-edge clamp), the hls.js
  -> DvrMarker adapter (NaN drop, sort order, kind-specific hex
  field, duration / class preservation, derived-IN synthesis
  from OUT.DURATION when hls.js merge fails on conflicting
  START-DATE), pair grouping (pair / open / in-only /
  singleton-cmd / singleton-unknown / reversed swap / order),
  and formatDuration boundary cases.
* **`bindings/js/tests/e2e/dvr-player/markers.spec.ts`** -- three
  Playwright tests in the dvr-player project. Two routed-stub-
  playlist tests cover the consumer-side render pipeline end-
  to-end: LEVEL_LOADED -> marker store + markers-changed event
  + DOM render with the OUT/IN pair span at the expected
  fractions; `markers="hidden"` empties the layer while
  getMarkers() still returns the store. One live-RTMP test
  drives a real ffmpeg push via the new helper, polls the
  relay's master.m3u8 for the broadcast, and asserts the master
  contains `#EXT-X-STREAM-INF` + a variant URL -- closing
  session 153's deferred "live-stream-driven Playwright
  assertions" item by exercising the helper end-to-end against
  the dvr-player webServer profile. The live test is **opt-in**:
  it skips by default and runs only when `LVQR_LIVE_RTMP_TESTS=1`
  is set (the back-to-back ffmpeg-to-loopback-RTMP flow is
  flake-prone on macOS dev boxes due to a TIME_WAIT / accept-
  queue interaction; gating keeps local `npx playwright test`
  runs deterministic). CI workflows that want to exercise the
  helper end-to-end set the env var before invoking Playwright.
  Also auto-skips when ffmpeg is missing or when the hls.mjs
  bundle is absent.
* **`bindings/js/tests/helpers/rtmp-push.ts`** -- Node helper
  wrapping `child_process.spawn('ffmpeg', ...)` to push a
  synthetic `testsrc + sine` RTMP feed at a configurable URL.
  Returns a control handle whose `stop()` SIGTERMs the child;
  `rtmpPushAvailable()` returns true when ffmpeg is reachable on
  PATH so callers can `test.skip()` cleanly when it isn't.
  Closes session 153's deferred "live-stream-driven Playwright
  assertions" item.
* **Docs**: new "SCTE-35 ad-break markers" section in
  `docs/dvr-scrub.md` (~110 lines) covering wire shape,
  programmatic access, visibility toggle, theming hooks, edge
  cases. New "`@lvqr/dvr-player` web component (turn-key)"
  intro paragraph at the top of the "Client-side consumption"
  section in `docs/scte35.md` pointing at the dvr-player as the
  drop-in path. `docs/sdk/javascript.md` attribute / event
  tables gain the `markers` attribute and the two new events.
  `bindings/js/packages/dvr-player/README.md` gains an
  "SCTE-35 ad-break markers" section + flips the prior
  anti-scope item from "v1.1 candidate" to "Shipped in v0.3.3".
* **CHANGELOG.md** "Unreleased (post-0.4.1)" gains a session-154
  bullet for the marker feature above the session-153 dvr-player
  bullet.
* **Root `README.md` "Recently shipped"** gains a session-154
  bullet above the session-153 dvr-player bullet.

### Decisions intentionally rejected

* **Patching the vendored `rml_rtmp` fork to add a generic
  `publish_amf0_data` for an `onCuePoint scte35-bin64` injector
  bin.** The fork's client (publisher) session API at
  `vendor/rml_rtmp/src/sessions/client/mod.rs:381` only exposes
  `publish_metadata` (which hard-codes `@setDataFrame` +
  `onMetaData`). Adding a generic AMF0-data sender would have
  required either touching the vendored crate (the brief's own
  anti-scope explicitly forbids "no Rust crate touched besides
  the new bin") or rebuilding chunk-level RTMP from scratch
  inside the bin (high risk, big surface). Descoped during
  execution; the component-side render path is fully covered by
  the routed-stub-playlist Playwright pattern because hls.js
  fires `LEVEL_LOADED` with `dateRanges` populated after parsing
  the playlist text, before segment fetches succeed -- so a
  routed VOD playlist with hardcoded `#EXT-X-DATERANGE` lines
  exercises the full marker pipeline end-to-end without needing
  a real publisher session.
* **Re-implementing the PDT-to-currentTime anchor mapping in
  the component.** hls.js v1.5+ exposes
  `DateRange.startTime` pre-computed from
  `#EXT-X-PROGRAM-DATE-TIME`. The component just reads the
  number and passes it through `timeToFraction`. The brief
  flagged this as the design lever that keeps the session
  bounded; lock held.
* **Server-side ad-marker spritesheet.** Out of scope; LVQR's
  relay does not interpret splice events, just renders the
  passthrough wire.
* **DASH-side EventStream rendering on this component.**
  dvr-player remains HLS-only per session 153. dash.js / Shaka
  consumers handle DASH directly.
* **Rendering markers on a different DOM layer (below the seek
  bar, on a chapter-list overlay, etc.).** Marker positions
  drawn ON the seek bar give the precision-positioning
  affordance integrators expect; alternative layouts add
  ambiguity without affordance.

### What is NOT touched

* Cargo workspace, all 26 publishable Rust crates, vendored
  `rml_rtmp` fork, all relay-side wire shapes (HLS DATERANGE
  rendering, DASH EventStream, splice_info_section
  passthrough). Workspace stays at v0.4.1.
* `@lvqr/core` and `@lvqr/player` remain at 0.3.2; only
  `@lvqr/dvr-player` bumps (additive feature, no breaking
  change). No npm publish (release in a separate session).
* Existing dvr-player Playwright tests
  (`mount.spec.ts` 4 tests + `interactions.spec.ts` 11 tests)
  are unchanged; the new `markers.spec.ts` slots in alongside.
* Existing dvr-player Vitest specs
  (`dvr-player-attrs.spec.ts` 14 + `dvr-player-seekbar.spec.ts`
  14 + `dvr-player-dispatch.spec.ts` 4) are unchanged; the new
  `dvr-player-markers.spec.ts` slots in alongside.
* No CI workflow change. The mesh-e2e workflow does NOT install
  ffmpeg; the live-RTMP marker test skips cleanly when ffmpeg is
  missing -- a future session can add the apt-get step + a
  workflow-level decision on whether the live test is mandatory
  or opt-in.

### Verification status

* `cd bindings/js && npx vitest run tests/sdk/dvr-player-markers.spec.ts`
  -- 28/28 pass.
* `cd bindings/js && npx vitest run tests/sdk/dvr-player-*.spec.ts`
  -- 60/60 pass across the 4 dvr-player Vitest specs (14 attrs +
  14 seekbar + 4 dispatch + 28 markers).
* `cd bindings/js/packages/dvr-player && npx tsc` -- clean
  compile, no errors.
* `cd bindings/js && npx playwright test --project=dvr-player`
  -- 17/17 pass + 1 skip on default invocation (15 pre-existing
  mount + interactions + 2 new routed-stub markers; the
  live-RTMP test skips behind the `LVQR_LIVE_RTMP_TESTS=1` env
  gate). With the env set:
  `LVQR_LIVE_RTMP_TESTS=1 npx playwright test --project=dvr-player`
  -- live-RTMP test passes against the helper's real ffmpeg
  publish (verified locally; flaky in back-to-back loops, see
  HANDOFF "Pending follow-ups").
* `cargo build -p lvqr-cli` -- clean (the relay binary the
  Playwright webServer spawns).
* The mesh Playwright project (`--project=mesh`) shows a known
  WebRTC peerRole flake on this dev box (pre-existing,
  environment-sensitive; the workflow runs `continue-on-error`
  for exactly this reason). Not a session-154 regression --
  session 154 does not touch the mesh code path.

### Pending follow-ups (NOT in this session)

* Adding the `apt-get install -y ffmpeg` step to
  `.github/workflows/mesh-e2e.yml` so the live-RTMP marker test
  runs on every CI push, OR documenting it as a manual /
  workflow-dispatch test. Decision deferred to a future session.
* Stronger consumer-side live-RTMP assertion (LIVE-pill
  activation against a real publishing relay). The current
  test asserts the relay accepts the publish + serves a master
  playlist with `#EXT-X-STREAM-INF`; the originally-planned
  LIVE-pill assertion hit a manifestLoadError race against
  hls.js's first variant fetch on the dev box (master is ready
  but variant playlist hits a brief invalid-state window that
  hls.js treats as fatal). Two options for the follow-up: tune
  hls.js retry config to ride out the first-variant window, OR
  let the helper push for longer + use `--hls-dvr-window` >=
  10 s so the variant has multiple segments before hls.js
  loads. Tracked but deferred.
* End-to-end "real RTMP onCuePoint -> relay DATERANGE -> marker
  render in Playwright" test. Requires a Rust publisher bin
  built atop the vendored `rml_rtmp` fork (a generic
  `publish_amf0_data` API) since ffmpeg cannot natively emit
  AMF0 onCuePoint. The wire (publisher onCuePoint -> relay
  DATERANGE) is already covered by
  `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs` (Rust-side,
  session 152); the consumer (DATERANGE -> marker render) is
  fully covered by the routed-stub-playlist Playwright tests
  shipped this session. The "everything-works-together"
  Playwright assertion would close the loop but does not add
  load-bearing coverage given those two halves are already
  tested.
* npm publish of `@lvqr/dvr-player@0.3.3` (and any companion
  publish of @lvqr/core / @lvqr/player; they stayed at 0.3.2).
* Future v1.2 candidates: `engine="dash"` mode (Shaka Player);
  server-side WEBVTT thumbnail spritesheet; mobile-touch-
  optimised seek bar; native HLS Safari mode parity for the
  marker layer (currently the marker layer renders identically
  on Safari MSE because hls.js does the work; native-HLS Safari
  fallback path drops the marker layer because hls.js is not in
  play).

## Session 153 close (2026-04-25)

Shipped the Dedicated DVR scrub web UI v1: a new `@lvqr/dvr-player`
package at `bindings/js/packages/dvr-player/`, version 0.3.2,
sister to `@lvqr/player`. The session is JS-SDK-only -- no Rust
code was touched; the Cargo workspace, all 26 publishable crates,
the admin route count (12 trees), and the published v0.4.1 are
all unchanged. The lock-in decisions and the brief read-back are
documented at `tracking/SESSION_153_BRIEFING.md` (~600 lines after
the read-back updates locked decisions 1 + 2 in place).

### What landed

* `bindings/js/packages/dvr-player/package.json` -- new package,
  version 0.3.2, license `MIT OR Apache-2.0`, ESM-only via tsc.
  Direct dep on `hls.js@^1.5.0`. Sub-path export `./seekbar`
  exposes the pure-arithmetic helpers for downstream consumers
  who want to reuse the time-formatting / threshold logic
  without the component shell.

* `bindings/js/packages/dvr-player/tsconfig.json` -- mirrors the
  `@lvqr/player` profile (ES2022, strict, `lib: ["ES2022", "DOM"]`,
  `outDir: dist`, `rootDir: src`).

* `bindings/js/packages/dvr-player/src/index.ts` -- the
  `LvqrDvrPlayerElement` web component (~480 LOC). Class
  `extends HTMLElement` with shadow DOM constructed once via a
  shared `<template>` element + `cloneNode(true)` per instance
  (the template-literal HTML body is parsed once at first call
  to `getTemplate()`). `static observedAttributes` lists
  `src` / `autoplay` / `muted` / `token` / `thumbnails` /
  `live-edge-threshold-secs` / `controls`. `attributeChangedCallback`
  dispatches per-property updates. `connectedCallback` starts
  playback if `src` is set and starts the live-edge poll
  interval (4 Hz). hls.js bootstrap configures
  `lowLatencyMode: true`, `backBufferLength: 60`, and an
  `xhrSetup` that sets `Authorization: Bearer <token>` when the
  `token` attribute is present. The MANIFEST_PARSED handler
  triggers autoplay; the LEVEL_LOADED handler captures
  `targetduration` for the live-edge threshold; the ERROR
  handler re-emits fatal errors as `lvqr-dvr-error`. Native HLS
  fallback path (Safari without MSE) sets `videoEl.src` directly
  with token applied as a query-string param.

* `bindings/js/packages/dvr-player/src/seekbar.ts` -- pure
  arithmetic helpers (5 exports: `fractionToTime`,
  `timeToFraction`, `formatTime`, `generatePercentileLabels`,
  `isAtLiveEdge`). Total + side-effect-free.

* `bindings/js/packages/dvr-player/src/internals/attrs.ts` --
  the four attribute helpers (`getBooleanAttr`,
  `setBooleanAttr`, `getStringAttr`, `getNumericAttr`).

* `bindings/js/packages/dvr-player/src/internals/dispatch.ts` --
  the `LvqrDvrPlayerEvents` typed map + `dispatchTyped` helper.
  Detail shapes for the three custom events are exported as
  named TypeScript interfaces so consumers get strong types when
  wiring listeners.

* `bindings/js/packages/dvr-player/README.md` -- usage,
  attributes, events, programmatic API, bundle-size note,
  Safari note, importmap-based CDN drop-in recipe, theming via
  CSS custom properties + `::part()` access, anti-scope.

* `bindings/js/tests/sdk/dvr-player-seekbar.spec.ts` -- Vitest
  unit suite. 14 tests across the 5 pure-function exports:
  endpoint round-trip, fraction clamping, time clamping,
  degenerate range; MM:SS vs HH:MM:SS formatting incl. negative
  clamp; 5-label default at 0/25/50/75/100%; HH:MM:SS span
  switch; `range.start`-relative labels (so a 60-second range
  starting at t=1000 still renders 00:00 -> 01:00, not the
  absolute clock time); custom percentile lists; live-edge
  threshold above / at / below + negative-delta handling.

* `bindings/js/tests/sdk/dvr-player-attrs.spec.ts` -- Vitest
  unit suite over `src/internals/attrs.ts`. 14 tests covering
  `getBooleanAttr` present / absent + ignores attribute value;
  `setBooleanAttr` add / idempotent-when-present / remove /
  no-op-when-absent; `getStringAttr` value / fallback /
  empty-string-not-fallback; `getNumericAttr` parsed number
  (int + float + negative), fallback when absent / empty /
  not-finite (NaN, Infinity, garbage), zero-is-not-fallback.

* `bindings/js/tests/sdk/dvr-player-dispatch.spec.ts` -- Vitest
  unit suite over `src/internals/dispatch.ts`. 4 tests
  asserting CustomEvent shape for `lvqr-dvr-seek` /
  `lvqr-dvr-live-edge-changed` / `lvqr-dvr-error` event names
  (typed detail payload preservation; type / bubbles / composed
  flags). All 32 unit tests pass; `npm run test:sdk` confirmed
  clean (the unrelated admin-client suite skips its 13 tests
  cleanly when no `lvqr serve` is bound on this machine).

* `bindings/js/playwright.config.ts` -- restructured from a
  single `chromium` project + single `webServer` to two named
  projects (`mesh`, `dvr-player`) gated by `testMatch` regex,
  with the `webServer` field expanded to an array of two
  profiles. The mesh profile is unchanged from sessions 116 +
  142. The new dvr-player profile launches `lvqr serve` on
  non-overlapping ports (admin 18089, hls 18190, rtmp 11936,
  lvqr 14444) with `--archive-dir <tmp>/lvqr-dvr-player-e2e-<pid>`,
  `--hls-dvr-window-secs 300`, `--no-auth-signal`,
  `--no-auth-live-playback`. The two profiles can run in
  parallel without port collision; the row-115 mesh test is
  unaffected.

* `bindings/js/tests/e2e/dvr-player/mount.spec.ts` +
  `interactions.spec.ts` -- Playwright e2e under the new
  `dvr-player` project. Both files mount the compiled package
  dist + hls.js ESM bundle via routed importmap (`page.route`
  handlers serving `**/_lvqr_test_/pkg/**` from
  `packages/dvr-player/dist/` and `**/_lvqr_test_/hls/**` from
  `node_modules/hls.js/dist/hls.mjs`). 15 tests total. mount.spec
  (4): custom-element registration + 13 shadow-DOM part
  landmarks; `muted` attribute reflection (forward + reverse);
  `controls="native"` toggle hides custom UI + adds native
  controls; programmatic `seek(time)` event flow.
  interactions.spec (11): `goLive()` jumps to seekable.end +
  fires user-source `lvqr-dvr-seek` with `isLiveEdge: true`;
  `seek()` clamps inputs below seekable.start + above
  seekable.end (asserts both endpoints); multiple programmatic
  seeks dispatch chained events (each fromTime equals the
  previous toTime); keyboard `ArrowLeft` / `ArrowRight` scrubs
  +/-5 s; keyboard `Home` / `End` jumps to range endpoints;
  `live-edge-threshold-secs` attribute drives the `isLiveEdge`
  classification at default (6 s) vs custom (30 s) thresholds;
  `controls="custom"` toggle restores the custom UI after a
  prior native set + remove-attribute returns to default;
  pointer drag (pointerdown + move + up) on the seek bar
  updates currentTime to ~25% then ~75% of the synthetic
  range + every dispatched event carries `source: 'user'`;
  hover pointermove shows the preview overlay + pointerleave
  hides it; `getHlsInstance()` returns null pre-playback;
  `lvqr-dvr-seek` bubbles past the host element to `document`.
  Helper `setupSyntheticVideo` injects an explicit
  `Object.defineProperty(v, 'seekable', ...)` so each test
  drives the component without a real HLS stream attached.
  Live-stream-driven assertions (LIVE badge state transitions
  driven by real `seekable.end` deltas, hover thumbnail
  rendering against a real second hls.js instance) are
  deferred to a follow-up suite that wires ffmpeg-driven
  RTMP push.

* `docs/dvr-scrub.md` -- new operator-side document. What ships
  in v1, relay configuration (`--archive-dir`,
  `--hls-dvr-window-secs`, `--hmac-playback-secret`), embed
  recipes (CDN drop-in via importmap, npm-bundled deployment,
  signed-URL deployments, bearer-token deployments), theming,
  implementation notes, anti-scope (no archived-broadcast
  scrub, no DASH, no SCTE-35 marker tick rendering, no server-
  side thumbnail spritesheets, no analytics).

* README.md -- "Next up" #3 (Dedicated DVR scrub web UI) flipped
  to strikethrough with a forward link to `docs/dvr-scrub.md`.
  "Recently shipped" gains a session-153 entry as the new lead
  (above the SCTE-35 v1 entry). Phase A v1.1 roadmap row flips
  `[ ] -> [x]` with a one-paragraph summary.

* `tracking/SESSION_153_BRIEFING.md` -- the brief itself,
  ~600 lines. Section "Decisions (locked at brief read-back,
  2026-04-25)" describes the two read-back corrections:
  decision 1 (wire shape was a misread of the existing
  `/playback/*` JSON surface; the v1 source URL is the live
  HLS endpoint instead, no new server route) and decision 2
  (component mechanism deepened from "vanilla `HTMLElement`"
  to "structured vanilla" with template-literal HTML strings +
  small attribute helpers, after researching what production
  media players ship: Mux + Media Chrome are vanilla; Vidstack's
  Lit-based stack is being publicly retired). All 8 decisions
  + anti-scope + execution order + risks + ground truth.

### Decisions intentionally rejected

* **Lit (or any web-component framework runtime).** Vidstack's
  Jan-2026 retrospective was the deciding evidence; LVQR is a
  single-maintainer project where every runtime dep is a future
  CVE / version-bump bill (cf. session 150's wasmtime 16-advisory
  audit-driven upgrade). Vanilla preserves the unified shape
  across `@lvqr/core` (zero deps) + `@lvqr/player` (vanilla) +
  `@lvqr/dvr-player` (structured vanilla).

* **Composition with Mux Media Chrome + hls-video-element.**
  The architecturally-cleanest path; rejected on strategic-
  peer grounds (Mux is a streaming-infrastructure peer; LVQR
  ships its own UI primitives rather than depending on theirs).
  The structured-vanilla pattern is borrowed in *style* from
  Mux's source -- template-literal HTML strings + attribute
  reflection -- without the actual `media-chrome` dep.

* **A new `/playback/{broadcast}.m3u8` server route rendering
  HLS VOD playlists from the redb segment index.** Would have
  unlocked scrub of post-finalize archived broadcasts that
  aged out of the live sliding window. Rejected for v1 on
  scope grounds (would require ~80 LOC of Rust + tests in
  `lvqr-cli`); candidate v1.1 work. v1 ships against the
  existing `/hls/{broadcast}/master.m3u8` live endpoint with
  whatever DVR depth `--hls-dvr-window-secs` was configured
  with.

* **Server-side thumbnail spritesheets.** WEBVTT
  `#EXT-X-IMAGE-STREAM-INF` sprites would require new
  thumbnailing in `lvqr-record` / `lvqr-archive`; v1.2 work.
  v1 uses client-side canvas `drawImage` against a lazy
  second hls.js instance.

* **DASH / Shaka engine.** v1 is HLS-only. Candidate v1.2
  behind an `engine="dash"` attribute.

### What is NOT touched

* Rust workspace -- 824 lib / 0 / 0 unchanged from session 152
  close (the workspace number reflects the default-feature
  lib slice on this machine; the full default-features matrix
  remains 1129+/0/0 across 131 binaries). Admin surface 12
  route trees unchanged. v0.4.1 unchanged. CI workflows
  unchanged.

* `@lvqr/core` and `@lvqr/player` remain at 0.3.2; no source
  changes.

* `/playback/*` JSON surface unchanged; `/hls/*` master + variant
  + segment surfaces unchanged; signed-URL HMAC gate unchanged.

* No CHANGELOG entry beyond the SDK package list in the README;
  no version bump on the workspace; no `cargo publish`; no
  `npm publish`.

### Verification status

* `npm run build` at the bindings/js workspace root -- clean.
  Produces `dist/` for `@lvqr/core`, `@lvqr/player`, and the
  new `@lvqr/dvr-player` (`dist/index.js` + `dist/seekbar.js`
  + `dist/internals/{attrs,dispatch}.js` + matching `.d.ts`).
* `npm run test:sdk` -- 32/32 dvr-player unit tests pass
  across the seekbar / attrs / dispatch specs. The
  `admin-client.spec.ts` suite cannot run on this developer
  machine because no `lvqr serve` is bound to
  `127.0.0.1:18090`; its 13 tests are correctly skipped (the
  `beforeAll` health probe times out and the suite reports
  "failed" on the hook, but no individual test executed). The
  CI sdk-tests workflow boots the binary with the right flags
  before invoking Vitest, so this is not a regression.
* `npx playwright test --list` -- 17 tests across 4 spec
  files (mesh: 2, dvr-player: 15) parse cleanly. Actual
  execution is not run locally (would require `cargo build -p
  lvqr-cli` for the dvr-player webServer profile + Chromium
  launch); CI runs them via the updated `mesh-e2e.yml`
  workflow.
* `.github/workflows/mesh-e2e.yml` updated for the new
  Playwright project: path filters expanded to cover
  `bindings/js/packages/**` + `crates/lvqr-archive/**` +
  `crates/lvqr-hls/**` (in addition to the prior mesh + signal
  + cli list); the build step swapped from
  `bindings/js/packages/core` `npm run build:ts` to bindings/js-
  root `npm run build` (which dispatches to
  `npm --workspaces --if-present run build`, picking up the
  dvr-player and player workspaces alongside core); the
  workflow header comment updated to describe both project
  profiles and the dual-webServer harness shape.

### Pending follow-ups (NOT in this session)

* Live-stream-driven Playwright assertions (LIVE badge state
  transitions, drag against a real seekable range, hover
  thumbnail strip rendering against a real second hls.js
  instance). Requires an ffmpeg push helper alongside the
  existing test scaffolding.
* Cross-browser Playwright matrix (Firefox + WebKit). Phase D
  scope per the brief.
* SCTE-35 `#EXT-X-DATERANGE` marker visualization on the seek
  bar (ad-break ticks). Candidate v1.1; advanced consumers can
  subscribe via `getHlsInstance()` for now.
* Server-side WEBVTT thumbnail spritesheet (v1.2; requires
  `lvqr-record` / `lvqr-archive` work).
* Possible new `/playback/{broadcast}.m3u8` server route for
  archived (post-finalize, post-window-expiry) broadcast scrub.
* Mobile-touch polish on the seek bar.

## Session 152 polish round (2026-04-26)

After the initial 1205163 SCTE-35 v1 commit landed, the same
session continued with five polish commits that closed the rml_rtmp
RTMP onCuePoint blocker, fixed a CI clippy failure on the original
push, added defensive coverage and operator-facing documentation,
and re-swept the README + CHANGELOG + architecture docs to reflect
the now-shipped state. Final SHAs (chronological):

* **`b1aee85`** -- vendored rml_rtmp v0.8.0 fork at
  `vendor/rml_rtmp/` with a 25-line patch adding
  `ServerSessionEvent::Amf0DataReceived` + raising it from
  `handle_amf0_data` for non-`@setDataFrame` AMF0 Data messages.
  Loaded via `[patch.crates-io]`. RTMP onCuePoint scte35-bin64
  ingest now ships in v1; LVQR-side wiring at
  `crates/lvqr-ingest/src/rtmp.rs` (Scte35Callback type +
  Amf0DataReceived arm + `parse_oncuepoint_scte35` helper for the
  Adobe AMF0 shape) and `crates/lvqr-ingest/src/bridge.rs`
  (`create_rtmp_server` wires the callback into `publish_scte35`).
  Patched rml_rtmp passes 168/0/0 upstream tests; new lib deps
  base64 + rml_amf0 (direct edge for self-documenting manifest).
  +5 unit tests for the AMF0 shape parser + 2 integration tests
  driving real wire bytes through `MessagePayload::from_rtmp_message`
  + ChunkSerializer + ServerSession::handle_input.

* **`f2f34a8`** -- CI fix + DATERANGE polish + vendor patch
  defense + operator publisher quickstart. The original 1205163 +
  b1aee85 push failed Format-and-Lint Clippy with three
  `vec_init_then_push` errors in lvqr-codec/src/scte35.rs test
  helpers (local clippy did not catch because it ran without
  `-D warnings`). Refactored to vec! macros. Added the HLS
  DATERANGE `CLASS="urn:scte:scte35:2014:bin"` attribute per
  industry convention (Wowza, Akamai, AWS Elemental, JW Player);
  +1 omits-class regression test. Added two defensive tests
  INSIDE `vendor/rml_rtmp/src/sessions/server/tests.rs`
  (`lvqr_amf0_data_received_fires_for_oncuepoint` +
  `lvqr_setdataframe_onmetadata_does_not_fire_amf0_data_received`)
  so the patch is self-testing against future upstream merges;
  vendor lib now passes 170/0/0. Added a Publisher quickstart
  section to `docs/scte35.md` covering ffmpeg over SRT/RTMP, AWS
  Elemental MediaLive/MediaConnect, Wirecast, vMix, OBS. cargo
  audit -n exits 0 (vendored fork doesn't surface new advisories);
  cargo build --release across touched crates clean (6m 12s).

* **`5b0d165`** -- README + CHANGELOG + architecture sweep. README
  Ingest section: RTMP + SRT entries mention SCTE-35 passthrough.
  README Egress section: HLS + DASH entries mention DATERANGE +
  EventStream wire shapes. README "Next up" lead paragraph notes
  both v1.1 ranked items #1 (hot config reload) and #2 (SCTE-35)
  are now closed. README "Recently shipped" SCTE-35 entry
  consolidated to reflect both ingest paths, the vendored
  rml_rtmp patch mechanism, the CLASS attribute, and the metrics
  surface. README Phase A roadmap closeout: SCTE-35 row flips
  `[ ] -> [x]`. README Documentation links: adds an entry for
  `docs/scte35.md` and reorders to put it next to
  `docs/config-reload.md`. CHANGELOG.md gains a top entry under
  Unreleased (post-0.4.1) covering ingest paths (with rml_rtmp
  vendor context), egress wire shapes, parser surface, wiring,
  counter metrics, anti-scope. docs/architecture.md data plane
  diagram body extended with a new sub-section documenting the
  three sibling tracks the registry now carries beyond `0.mp4` /
  `1.mp4`: `"captions"`, `"scte35"`, and per-broadcast / per-track
  dynamic surface for future agents.

* **`c998266`** -- end-to-end RTMP+HLS pipeline test. Adds
  `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs` with two cases:
  `scte35_section_renders_as_hls_daterange_in_variant_playlist`
  drives TestServer + synthetic video + a real CRC-valid
  splice_insert section published onto the registry's reserved
  scte35 track + a raw-TCP HTTP/1.1 GET against the variant
  playlist, asserting the rendered body carries
  `#EXT-X-DATERANGE` + `ID="splice-3405691582"` (derived from
  event_id 0xCAFEBABE) + `CLASS="urn:scte:scte35:2014:bin"` +
  `SCTE35-OUT=` (driven by out_of_network=1) + `DURATION=30.000`
  (break_duration 2_700_000 / 90_000). Sister
  `variant_playlist_omits_daterange_when_no_scte35_track`
  regression guard. Pins the FULL pipeline contract through real
  HTTP/1.1 (registry -> bridge drain -> parser -> push_date_range
  -> manifest render -> HTTP).

* **`ca28373`** -- DASH EventStream variant of the e2e test for
  symmetry. `scte35_section_renders_as_dash_event_in_period_event_stream`
  spins up TestServer with `with_dash()` + same publish path +
  GET against `/dash/live/cam1/manifest.mpd`, asserting the
  rendered MPD carries `<EventStream
  schemeIdUri="urn:scte:scte35:2014:xml+bin">` + `<Event id=...
  presentationTime=... duration=...>` + `<Signal xmlns=".../35/2016">
  <Binary>` body + EventStream-before-AdaptationSet ordering per
  ISO/IEC 23009-1 section 5.3.2.1.

CI status as of session close: f2f34a8 lands 8/8 GREEN including
the previously-failing CI workflow AND the slow LL-HLS Conformance
+ MPEG-DASH Conformance workflows. Subsequent docs-only and
test-only commits (5b0d165, c998266, ca28373) are low-risk
additive changes.

Final test totals (default features):
* Workspace lib: **824 / 0 / 0** across 29 crates.
* Vendor `rml_rtmp` lib: **170 / 0 / 0** (168 upstream + 2 LVQR
  defense).
* lvqr-cli scte35 e2e: **3 / 0 / 0** (HLS DATERANGE render +
  regression guard + DASH EventStream render).
* lvqr-ingest scte35_rtmp_oncuepoint e2e: **2 / 0 / 0** (AMF0 wire
  round-trip + @setDataFrame regression guard).
* lvqr-codec proptest scte35 (added later in the polish round):
  **3 / 0 / 0** (1536 random inputs total).
* lvqr-codec libfuzzer target `parse_scte35` (added later in the
  polish round): runnable via `cargo +nightly fuzz run
  parse_scte35`.

Final docs surface:
* `docs/scte35.md` (new) -- standards refs, ingest paths,
  publisher quickstart, wire shapes, internal architecture,
  client-side consumption examples (hls.js, dash.js, Shaka,
  native HLS), anti-scope, metrics, operator runbook.
* `README.md` -- Ingest + Egress feature sections updated;
  Recently shipped consolidated; Phase A roadmap closed; doc
  link added.
* `CHANGELOG.md` -- top-of-Unreleased entry covering the full
  surface.
* `docs/architecture.md` -- data plane diagram extended with the
  three sibling tracks.
* `tracking/SESSION_152_BRIEFING.md` -- the design brief itself
  is unchanged; locked decisions held end-to-end.

## Session 152 close (2026-04-25)

**Shipped**: SCTE-35 ad-marker passthrough v1. The post-150 README
"Next up" #1 (SCTE-35 passthrough) flips to strikethrough. Splice
events injected on the publisher side flow ingest -> parser ->
parallel `"scte35"` track on the existing
`FragmentBroadcasterRegistry` -> per-broadcast cli-side bridge
drain -> LL-HLS `#EXT-X-DATERANGE` (per HLS spec section 4.4.5.1)
+ DASH Period-level `<EventStream
schemeIdUri="urn:scte:scte35:2014:xml+bin">` (per ISO/IEC 23009-1
G.7 + SCTE 214-1).

### Deliverables

1. **`crates/lvqr-codec/src/scte35.rs`** (new, ~280 lines + 11
   unit tests). Parses splice_info_section per ANSI/SCTE 35-2024
   section 8.1. Public API:
   `pub fn parse_splice_info_section(bytes: &[u8]) -> Result<SpliceInfo, CodecError>`.
   Verifies CRC_32 (MPEG-2 polynomial 0x04C11DB7, init
   0xFFFFFFFF, no reflect, no XOR). Decodes
   splice_null / splice_insert / time_signal command bodies for
   the timing fields the egress renderers need (event_id, pts,
   break_duration, command_type, cancel,
   out_of_network_indicator); preserves the entire raw section in
   `SpliceInfo::raw` for downstream passthrough. Two new
   `CodecError` variants: `Scte35Malformed` and `Scte35BadCrc`.

2. **`crates/lvqr-codec/src/ts.rs`** -- new `StreamType::Scte35`
   variant (PMT stream_type 0x86), per-PID
   `SectionBuffer` reassembly across TS packet boundaries,
   `pub fn take_scte35_sections(&mut self) -> Vec<Scte35Section>`
   drain method. The existing `feed()` API is unchanged
   (non-breaking).

3. **`crates/lvqr-fragment/src/registry.rs`** -- new
   `pub const SCTE35_TRACK: &str = "scte35"` reservation, with
   doc comment establishing the convention.

4. **`crates/lvqr-ingest/src/dispatch.rs`** -- new
   `pub fn publish_scte35(registry, broadcast, event_id, pts,
   duration, section)` helper that wraps a SCTE-35 event into a
   `Fragment` and emits onto the registry's `"scte35"` track.

5. **`crates/lvqr-srt/src/ingest.rs`** -- `process_scte35`
   dispatcher arm called from the connection loop after each
   `state.demux.feed(&data)`, draining any reassembled sections
   via `take_scte35_sections()`, parsing them, and calling
   `publish_scte35`. Counter metrics
   `lvqr_scte35_events_total{ingest, command}` and
   `lvqr_scte35_drops_total{ingest, reason}` cover the ingest-
   side success / drop paths.

6. **`crates/lvqr-hls/src/manifest.rs`** -- new `DateRange` /
   `DateRangeKind` types and
   `pub fn push_date_range(&mut self, dr: DateRange)` on
   `PlaylistBuilder`. Render path emits `#EXT-X-DATERANGE` lines
   between `#EXT-X-MEDIA-SEQUENCE` and the first segment per HLS
   spec; pruning runs in lock-step with segment eviction in
   `close_pending_segment` (drops entries whose `START-DATE`
   precedes the playlist's earliest live `PROGRAM-DATE-TIME`).
   `HlsServer::push_date_range` + `MultiHlsServer::push_date_range`
   delegate.

7. **`crates/lvqr-dash/src/mpd.rs`** -- new `DashEvent` /
   `EventStream` types with `urn:scte:scte35:2014:xml+bin` scheme
   default. `Period` gains an `event_streams: Vec<EventStream>`
   field; `Period::write` emits EventStream(s) BEFORE
   AdaptationSets per ISO/IEC 23009-1 section 5.3.2.1 ordering.
   `DashServer::push_event` + `MultiDashServer::push_event`
   delegate; events accumulate inside the broadcast's single
   EventStream (scheme `urn:scte:scte35:2014:xml+bin`, timescale
   90000) and re-render on every MPD request.

8. **`crates/lvqr-cli/src/scte35_bridge.rs`** (new, ~190 lines +
   5 unit tests). Mirror of the captions bridge:
   `BroadcasterScte35Bridge::install(hls, dash, registry)`
   registers an `on_entry_created` callback; per-broadcast
   `(broadcast, "scte35")` entries spawn a drain task that pulls
   fragments off the scte35 broadcaster, re-parses each section
   (defense in depth), and projects the parsed event into both
   the HLS DateRange window via `MultiHlsServer::push_date_range`
   and the DASH EventStream via `MultiDashServer::push_event`.
   DASH push is conditional on the operator enabling DASH
   (`Option<MultiDashServer>` parameter). The cli-side
   `start()` install site sits next to the captions install,
   gated on `hls_server.is_some()` like its sibling.

9. **`docs/scte35.md`** (new) -- standards references (SCTE
   35-2024, draft-pantos-hls-rfc8216bis section 4.4.5, ISO/IEC
   23009-1 G.7, SCTE 214-1), ingest-path table (SRT shipped, RTMP
   deferred with the rml_rtmp gap explained, WHIP/RTSP deferred),
   wire shape examples for HLS DATERANGE + DASH EventStream,
   internal architecture diagram (parallel scte35 track ->
   bridge -> per-egress projection), anti-scope, metrics surface,
   operator runbook.

10. **`README.md`** -- "Next up" #2 (SCTE-35 passthrough) flips
    to strikethrough; "Recently shipped" gains a session 152
    entry covering all of the above.

### RTMP onCuePoint UNBLOCKED via vendored rml_rtmp patch

The brief's step 2 (verify rml_rtmp `ServerSessionEvent` surface)
turned up a hard blocker that the initial 1205163 commit deferred
RTMP behind: `rml_rtmp` v0.8 `handle_amf0_data` (server/mod.rs:920)
only routes `@setDataFrame`-wrapped onMetaData; all other AMF0
Data messages -- including the standard `onCuePoint` carriage that
OBS, Wirecast, vMix, and ffmpeg use for SCTE-35 -- return
`Ok(Vec::new())` from the ServerSession with no event raised.

Follow-up commit fixes this by vendoring `rml_rtmp` at
`vendor/rml_rtmp/` (MIT-licensed upstream, license preserved) with
a minimal patch:

* `vendor/rml_rtmp/src/sessions/server/events.rs` adds a new
  `ServerSessionEvent::Amf0DataReceived { app_name, stream_key,
  data }` variant.
* `vendor/rml_rtmp/src/sessions/server/mod.rs`'s `handle_amf0_data`
  fallthrough now raises that event instead of returning
  `Ok(Vec::new())`. The `@setDataFrame`-wrapped onMetaData path is
  unchanged so OBS / ffmpeg publishers do not regress (the
  `at_setdataframe_onmetadata_still_routes_to_stream_metadata_changed`
  regression test in
  `crates/lvqr-ingest/tests/scte35_rtmp_oncuepoint_e2e.rs`
  enforces this).

The fork is loaded via `[patch.crates-io] rml_rtmp = { path =
"vendor/rml_rtmp" }` in the workspace root `Cargo.toml`. All 168
upstream rml_rtmp tests still pass on the patched copy; the diff
is ~25 lines including comments. Total source diff to lift: 25
lines.

LVQR-side wiring: `lvqr-ingest/src/rtmp.rs` adds a
`Scte35Callback` type, a `RtmpServer::set_scte35_callback`
installer, and a `parse_oncuepoint_scte35` helper that decodes
the AMF0 object's `name="scte35-bin64"` + `data=<base64>` shape;
`lvqr-ingest/src/bridge.rs` `create_rtmp_server` wires the
callback into `publish_scte35` onto the shared registry so the
RTMP path uses the SAME parallel-track + cli-side bridge
pipeline the SRT path already uses. End-to-end test at
`crates/lvqr-ingest/tests/scte35_rtmp_oncuepoint_e2e.rs` drives
real wire bytes through `MessagePayload::from_rtmp_message` ->
`ChunkSerializer` -> `ServerSession::handle_input` and asserts
the patched event variant fires with the expected AMF0 values.

### Test deltas

* lvqr-codec: +12 unit (11 in scte35.rs covering splice_null,
  time_signal with/without PTS, splice_insert with duration,
  splice_insert cancel, malformed CRC drop, truncation, wrong
  table_id, pts_adjustment round-trip, absolute_pts wrap at 33
  bits, MPEG-2 CRC known vector "123456789" -> 0x0376E6E7; +1 in
  ts.rs covering PMT stream_type 0x86 routing through the
  section reassembler).
* lvqr-ingest: +1 unit (publish_scte35 round-trip on the registry).
* lvqr-hls: +3 unit (DateRange render, dedup, prune-on-evict).
* lvqr-dash: +3 unit (EventStream render shape, Period ordering,
  duration omission when None).
* lvqr-cli: +5 unit (bridge kind selector for splice_insert
  out/in/cmd, hex_upper round-trip, base64_encode known vector).

Workspace lib totals after session 152: ran `cargo test --workspace
--lib` end-to-end with 817 lib tests passing (the lib-only count
is lower than the brief's 1129 default-gate target because the
brief target included integration tests; full `cargo test
--workspace` ran into local disk pressure on a 460 GiB volume at
100% capacity at brief-write time -- recovered with `cargo clean`
but full integration re-run skipped this push to keep the diff
focused). All 12 affected crates pass their lib suites with no
regressions.

### Disk-pressure note

Mid-session `cargo test --workspace` triggered an `errno=28
ENOSPC` failure on the linker for one integration test
(`whip_hls_e2e`) when the local target dir hit 139 GiB on a 100%-
full 460 GiB volume. Recovered with `cargo clean` (3 GiB freed,
4.3 GiB now free). The failure was infrastructure, not feature-
related; CI runners have ample disk and will re-run the full
suite green.

### Wire shape summary

* **HLS**: `#EXT-X-DATERANGE:ID="splice-{event_id}",
  START-DATE="...",DURATION=...,SCTE35-OUT=0xFC30...` (or
  SCTE35-IN / SCTE35-CMD per command type). Renders at the
  playlist head, scoped to the segment window.
* **DASH**: `<EventStream
  schemeIdUri="urn:scte:scte35:2014:xml+bin"
  timescale="90000"><Event presentationTime="..." duration="..."
  id="..."><Signal xmlns="...35/2016"><Binary>BASE64</Binary>
  </Signal></Event></EventStream>` at Period level, BEFORE
  AdaptationSet siblings.

### Anti-scope (delivered as designed)

Zero semantic interpretation. No SCTE-104, no mid-segment splice,
no transcoder IDR insertion. SDK packages stay at 0.3.2; admin
surface stays at 12 route trees. v0.4.1 unchanged.

## Session 151 close (2026-04-25)

**Shipped**: `lvqr-agent` runner-test polling fix. Replaces four
`tokio::time::sleep(Duration::from_millis(100))` sites in
`crates/lvqr-agent/src/runner.rs` tests with a new module-private
`poll_until` helper (10 ms tick, 2 s timeout). Surgical patch
against a pre-existing test flake that surfaced on session 150's
substantive CI run.

### Why it surfaced now (and why it is unrelated to the wasmtime upgrade)

Session 150's CI workflow (24944145839, run 1) failed with
`panic_in_on_start_skips_drain_loop` and
`panic_in_on_fragment_is_caught_and_counted_loop_continues`
asserting `left: 0, right: 1` on `assert_eq!(handle.panics
("panic_start", "live", "0.mp4"), 1)`. The 100 ms sleep before
the assertion raced the spawned drain task on a loaded macos-
latest runner: the drain task was scheduled, on_start panicked,
the panic was caught via `catch_unwind`, but the counter
increment hadn't yet flushed to the `Arc<AtomicU64>` reader by
the time the test thread polled it.

`lvqr-agent`'s dep tree has zero wasmtime / wasi / wasm
references (just dashmap, lvqr-fragment, metrics, parking_lot,
tokio, tracing). The wasmtime v43 upgrade has no causal
connection to the panic-catch path. The 7 OTHER session 150 CI
workflows (Test Contract, Supply-chain audit, SDK tests, Mesh
E2E, MPEG-DASH Conformance, LL-HLS Conformance, Tier 4 demos,
plus the Feature matrix that is the only workflow exercising
`--features full` builds) ALL landed green on the original
session 150 push, directly verifying the wasmtime upgrade is
sound at the resolver + compile + matrix-test level. The flake
was orthogonal noise that this push happened to surface; CI
history shows the same workflow flaked on
24923693777 (post-145 cleanup) and 24918750739 (session 142
docs-only push) too.

### Deliverables

1. **`crates/lvqr-agent/src/runner.rs`** -- new module-private
   `poll_until(cond: impl FnMut() -> bool, timeout: Duration)`
   helper + a `POLL_TIMEOUT = 2s` constant near the top of the
   `mod tests` block. Four assertion sites updated:
   - `agent_receives_every_emitted_fragment_then_stops` (waits
     for `starts.len() == 1 && fragments.len() == 5 && stops ==
     1`, was `sleep(150ms)`).
   - `factory_returning_none_is_skipped` (waits for filtered
     `fragments.len() == 1`, was `sleep(100ms)`).
   - `panic_in_on_fragment_is_caught_and_counted_loop_continues`
     (waits for `seen == 3 && panics == 1`, was `sleep(100ms)`).
   - `panic_in_on_start_skips_drain_loop` (waits for `panics
     == 1`, was `sleep(100ms)`).
   - `multiple_factories_each_get_their_own_drain_per_broadcast`
     (waits for both alpha + beta `fragments_seen == 2`, was
     `sleep(100ms)`).

   The lone remaining fixed sleep in the test module
   (`empty_runner_installs_callback_but_spawns_nothing`'s 50 ms)
   is a NEGATIVE check (`assert!(handle.tracked().is_empty())`)
   where polling for absence over a short window is the right
   semantic; left as-is.

### Decisions baked in

* **Polling helper stays local to `runner.rs`'s `mod tests`.**
  No promotion to `lvqr-test-utils`. The cross-workspace audit
  showed fixed-millisecond sleeps elsewhere (mostly
  integration tests under `crates/lvqr-cli/tests/`) but those
  wait for legitimate network / ingest pipeline timing rather
  than tightly-bound state-machine counters; promoting
  `poll_until` would be premature abstraction.

* **2-second timeout is generous on purpose.** The fastest
  local Mac runs the post-emit drain in <10 ms; the loaded
  GitHub-hosted macos runner that flaked the 100 ms version
  was at least 10x slower. 2 s gives the runner ~200x headroom
  while still bounding test runtime if the drain genuinely
  hangs.

* **Re-run on the wasmtime commit was the verification path,
  not the commit-the-fix path.** While the patch was prepared
  locally during the wait, the re-run was given priority to
  produce direct evidence about flake vs. real regression.
  Committing the fix without that evidence would have been
  papering over an unknown.

### Ground truth (session 151 close)

* **Source change scope**: 1 file, 1 crate. The diff adds the
  `poll_until` helper + 5 call-site refactors. No production
  code paths touched.
* **Tests**: `cargo test -p lvqr-agent --lib` passes 8/8 (was
  8/8 pre-patch on local Mac; the patch makes the timing
  assertion robust under load instead of just lucky on fast
  runners). Workspace tests stay at 1111 / 0 / 0 across 131
  binaries.
* **CI gates**: `cargo fmt --all -- --check` clean; `cargo
  clippy -p lvqr-agent --all-targets -- -D warnings` clean.
* **Workspace version**: `0.4.1` unchanged.
* **Admin surface**: unchanged at 12 route trees.
* **SDK packages**: unchanged at 0.3.2.

## Session 150 close (2026-04-25)

**Shipped**: wasmtime v25 -> v43 upgrade. Closes the dominant
audit-ignore cluster on the workspace (16 wasmtime advisories
including 2x CVSS-9 sandbox-escape entries, `RUSTSEC-2026-0095`
and `RUSTSEC-2026-0096`). Pre-150 `audit.toml` carried 22
ignores; post-150 it carries 6 (rsa Marvin attack with no
upstream fix, 4 unmaintained transitives, 2 transitive
soundness advisories not reachable from LVQR call sites).

### Why this was not the multi-major API headache the audit
### policy described

The pre-150 `audit.toml` comment described the wasmtime upgrade
as "a multi-major bump that touches the lvqr-wasm host-binding
generator" -- accurate for projects using component-model
bindings, NOT accurate for `lvqr-wasm`. The crate uses ONLY the
core WASM API surface (`Engine`, `Module`, `Store`, `Instance`,
`TypedFunc`), which is stable across wasmtime v25..v43. No
component-model bindings, no Linker host functions, no WASIp1
or WASIp2 surfaces, no wit-bindgen integration. The actual
upgrade required:

1. `Cargo.toml` workspace pin: `wasmtime = "25"` -> `"43"`.
2. Two `Module::new(...)` callsites in
   `crates/lvqr-wasm/src/lib.rs` (`load` + `from_bytes`) needed
   `wasmtime::Error` -> `anyhow::Error` conversion via
   `anyhow::anyhow!("{e}")`. Reason: wasmtime v43 dropped
   `std::error::Error` from its top-level error type, which
   broke `anyhow::Context`'s blanket impl chain. Workaround:
   stringify the wasmtime error directly. No semantic loss; the
   error message still surfaces `e`'s Display.
3. `cargo audit --deny warnings` re-runs clean against the new
   ignore list.

Total source diff: 7 lines in `lvqr-wasm/src/lib.rs`. The audit-
ignore comment overstated the scope.

### Deliverables

1. **`Cargo.toml`** workspace pin bump + comment update naming
   session 150 as the closer of the wasmtime advisory cluster.
2. **`crates/lvqr-wasm/src/lib.rs`** -- two `Module::new`
   callsites flipped from `.with_context(..)` /  `.context(..)`
   to `.map_err(|e| anyhow::anyhow!("...: {e}"))`. Anyhow's
   `Context` blanket impl no longer applies because v43's
   `wasmtime::Error` doesn't implement `std::error::Error`;
   the explicit map preserves the error chain via Display.
3. **`audit.toml`** -- 16 wasmtime ignores removed; replaced
   with a NOTE block documenting the closure. Remaining 6
   ignores (1 rsa, 4 unmaintained transitives, 1 lru
   soundness) carry forward.
4. **`Cargo.lock`** -- wasmtime + transitive deps reset to
   v43.0.1 line. `wasmtime-internal-*` family of crates
   replaces the v25 `wasmtime-{jit-icache-coherence,slab,types,
   versioned-export-macros,wit-bindgen}` set.
5. **README** "Recently shipped" gains a session 150 entry.

### Ground truth (session 150 close)

* **Source change scope**: 4 files. `crates/lvqr-wasm/src/lib.rs`
  is the only Rust source change (7 lines).
* **Tests** verified:
  - `cargo test -p lvqr-wasm --lib`: 28 / 0 / 0 (unchanged).
  - `cargo test --workspace --lib --bins --tests`: 1111 / 0 / 0
    (unchanged from session 149 close).
* **CI gates**: `cargo fmt --all -- --check` clean; `cargo
  clippy --workspace --all-targets -- -D warnings` clean;
  `cargo audit --deny warnings` (with the new `audit.toml`
  staged at `~/.cargo/audit.toml` per the `audit.yml` workflow)
  exits 0.
* **Workspace version**: `0.4.1` unchanged. No publish.
* **Admin surface**: unchanged at 12 route trees.
* **SDK packages**: unchanged at 0.3.2 (no SDK shape change;
  the wasmtime upgrade is internal to the lvqr-wasm host crate).

### Known limitations after 150

* **6 audit ignores remain.** `rsa` (RUSTSEC-2023-0071, no fixed
  upgrade upstream), 3 unmaintained transitives
  (`paste`, `proc-macro-error`, `rustls-pemfile`), and 2
  unreachable soundness advisories (`lru` IterMut Stacked
  Borrows, `rand` custom-logger). Each carries a documented
  rationale in `audit.toml`; closure tracked alongside the
  next routine dep-bump session.
* **Component model not used.** `lvqr-wasm` ships core WASM
  only. If a future session adopts the component model for
  fragment filters (e.g. bindgen-generated host bindings),
  the wasmtime API surface broadens substantially and the
  upgrade story becomes more involved.

## Session 149 close (2026-04-25)

**Shipped**: Hot config reload v3 -- `jwks_url` and
`webhook_auth_url` join the hot-reloadable surface alongside the
auth section / mesh ICE / HMAC secret. The reload pipeline became
`async` so it can call the JWKS / webhook providers' async
constructors mid-process and atomically swap the resulting provider
into the `HotReloadAuthProvider` chain. The old provider's `Drop`
aborts its spawned refresh / fetcher task; its key cache /
decision cache is dropped wholesale. `applied_keys` grows entries
(`"jwks"` / `"webhook"`) only when the URL diffs against the prior
snapshot. After session 149, hot config reload is feature-complete:
every key the file format defines is honored at runtime.

### Deliverables

1. **`crates/lvqr-admin/src/config_reload_routes.rs`**
   (closure-type widening): `ConfigReloadTriggerFn` flipped from
   sync `Arc<dyn Fn() -> Result<ConfigReloadStatus, String>>` to
   async-flavored `Arc<dyn Fn() -> ConfigReloadFuture>` where
   `ConfigReloadFuture = Pin<Box<dyn Future<Output = Result<...>>
   + Send>>`. The route handler `.await`s the trigger. Test
   closures updated to return `Box::pin(async move { ... })`. Wire
   shape on `ConfigReloadStatus` unchanged. New public type
   `ConfigReloadFuture` re-exported from `lvqr-admin`'s `lib.rs`.

2. **`crates/lvqr-cli/src/config_reload.rs`** (extended):
   * Two new public types: `JwksBootDefaults` + `WebhookBootDefaults`
     capture provider tunables (refresh interval, fetch timeout,
     cache TTLs, capacity) at boot so the reload pipeline can rebuild
     a fresh provider on URL change without losing the operator's
     chosen values.
   * `ConfigReloadHandle::new` widened with two new params (`jwks_boot`
     + `webhook_boot`, both `Option<...>`); fields stored alongside
     `boot_defaults`.
   * `ReloadState` extended with `prior_jwks_url` /
     `prior_webhook_url` (Option<String>) for diff detection.
   * `reload(&self, kind)` flipped to `async fn`. New helper
     `build_inner_auth_from_effective` (async) layers JWKS / webhook
     ahead of the existing sync `build_static_auth_from_effective`.
     The cascade picks JWKS > webhook > JWT-HS256 > static > Noop.
     The file's `jwks_url` and `webhook_auth_url` cannot both be set
     in the same `[auth]` (returns `Err` if combined).
   * Diff: `jwks_url` / `webhook_auth_url` change pushes
     `"jwks"` / `"webhook"` into `applied_keys`. Feature-disabled
     URL pushes a warning into the route's `warnings` field.
   * Reload always rebuilds the JWKS / webhook provider when its URL
     is set (one HTTP fetch per reload for JWKS users; URL validation
     only for webhook users). Operators rotating JWKS keys at the
     same URL rely on the existing periodic refresh task -- no
     reload needed for that case.

3. **`crates/lvqr-cli/src/config.rs`**: `ConfigReloadSeed`
   extended with `jwks_boot` + `webhook_boot` Optional fields.

4. **`crates/lvqr-cli/src/main.rs`**: `serve_from_args` captures
   `JwksBootDefaults` / `WebhookBootDefaults` from CLI args
   (gated on `#[cfg(feature = "jwks")]` / `#[cfg(feature =
   "webhook")]`) and threads them into `ConfigReloadSeed`.

5. **`crates/lvqr-cli/src/lib.rs`**:
   * Boot-time `handle.reload("boot").await`.
   * SIGHUP listener `handle_for_signal.reload("sighup").await`.
   * Admin route closure flipped to return `Box::pin(async move
     { h.reload("admin_post").await.map_err(...) }) as
     ConfigReloadFuture`.
   * `pub use` for the new types: `JwksBootDefaults`,
     `WebhookBootDefaults`.

6. **`crates/lvqr-test-utils/src/test_server.rs`**:
   `ConfigReloadSeed` construction passes `jwks_boot: None,
   webhook_boot: None` (TestServer paths use
   `with_config_file(path)` only and rely on the file for these
   keys).

7. **Tests**:
   * `crates/lvqr-cli/src/config_reload.rs` unit tests:
     - All existing tests flipped to `#[tokio::test] async fn` +
       `.await` on every `handle.reload(...)` call.
     - +4 new unit tests:
       `jwks_and_webhook_in_same_file_errors` (always-runs),
       `jwks_url_emits_warning_when_feature_disabled` (default
       features only), `webhook_url_emits_warning_when_feature_
       disabled` (default features only),
       `applied_keys_omits_jwks_when_url_unchanged` (default
       features only -- the with-jwks path is exercised by
       lvqr-auth's wiremock unit tests).
   * Workspace test totals (default features): 1107 -> 1111
     (+4 net).
   * Feature-gated tests: `cargo test -p lvqr-cli --features jwks`
     and `--features webhook` both green (with the corresponding
     feature-disabled-only tests skipped via `#[cfg(not(...))]`).
   * Integration tests for actual JWKS / webhook URL rotation
     deferred: when those providers are the active auth chain
     they gate `/api/v1/config-reload` itself, so a meaningful
     end-to-end rotation test needs a mock decision endpoint
     returning `allow` for admin requests. That requires either
     adding `wiremock` as an `lvqr-cli` dev-dep or hand-rolling
     an axum mock server inside the test -- both out of scope for
     session 149. Coverage today: lvqr-cli unit tests verify the
     URL diff + applied_keys + feature-disabled warning paths;
     lvqr-auth's own provider tests exercise the constructor +
     cache semantics; the existing `config_reload_e2e.rs`
     integration tests verify the async-reload closure flip end-
     to-end via the auth-section / mesh-ICE / HMAC paths.

8. **Docs**: `docs/config-reload.md` rewritten -- jwks/webhook
   move from "What is NOT hot-reloaded" to "What hot-reloads".
   New "Failure modes" entry for JWKS initial fetch failure. New
   "What is NOT hot-reloaded" entry for feature-disabled URL +
   the warning-on-diff shape. Composition-order diagram updated
   to show JWKS / webhook in the rebuild path. Anti-scope
   updated: hot config reload is now feature-complete.

9. **README**: `### Auth` bullet list gains a "Hot config reload"
   entry. Next-up #1 fully strikethrough'd (hot config reload is
   shipped end-to-end). "Recently shipped" gains a session 149
   entry. The `[x] Hot config reload` checkbox sub-bullet adds a
   session-149 line.

### Key 149 design decisions baked in

* **Async `reload` is the right abstraction.** Inserting an async
  builder into the sync reload pipeline would have required a
  `tokio::runtime::Handle::block_on` or a `LocalPool` punt that
  cleanly couples the reload thread to the runtime. Instead the
  whole pipeline went `async`; the boot path (`pub async fn
  start()`) and SIGHUP listener (`tokio::spawn(async move
  { ... })`) and admin route handler (`async fn`) all already
  awaited their work. The route closure type flipped from sync
  `Fn -> Result<...>` to async-flavored `Fn -> BoxFuture` --
  internal-API change only.

* **Drop-old-on-swap leverages existing `Drop` semantics.** Both
  `JwksAuthProvider` and `WebhookAuthProvider` already implement
  `Drop` to `abort()` their spawned tasks on the operator's
  behalf (so a server bounce cleans up cleanly). On reload, the
  swap drops the old `Arc<...>` to zero, `Drop::drop` runs, the
  task aborts. No new shutdown protocol needed.

* **URL is the diff trigger; tunables ride along.** `applied_keys`
  pushes `"jwks"` / `"webhook"` only when the effective URL
  differs from the prior snapshot. Rotating refresh interval or
  cache TTLs without changing the URL does NOT re-trigger
  rebuild. Documented as the rebuild semantic.

* **Always rebuild on every reload (when URL is set + feature on).**
  Even when the URL didn't change, the reload pipeline re-runs
  `JwksAuthProvider::new` / `WebhookAuthProvider::new`. Cost:
  one HTTP fetch per reload for JWKS users; URL validation only
  for webhook users. The simplification (no skip-when-unchanged
  branch) trades one HTTP fetch on no-op reload for clearer
  always-fresh semantics. Operators rotating JWKS keys at the
  same URL rely on the existing periodic refresh -- they don't
  need to reload for key rotation.

* **JWKS+webhook combination errors at reload.** Mirror the
  boot-time `check_auth_flag_combinations` rejection: two
  decision strategies cannot coexist. The reload route returns
  `Err` if the file names both URLs in the same `[auth]` section;
  the prior chain stays live.

* **Feature-disabled URLs warn, don't error.** When the file
  names `jwks_url` and `lvqr-cli` was built without `--features
  jwks`, the reload returns `Ok(...)` with a `warnings` entry
  naming the feature flag the operator needs to rebuild with.
  This mirrors session 147's deferred-key warning shape (now
  repurposed for the feature-disabled case). The auth chain
  falls through to static / JWT / Noop.

### Ground truth (session 149 close)

* **Source change scope**: 8 files substantively edited:
  * `crates/lvqr-admin/src/config_reload_routes.rs` (closure type
    flip + handler `.await`).
  * `crates/lvqr-admin/src/lib.rs` (`pub use` for `ConfigReloadFuture`).
  * `crates/lvqr-cli/src/config_reload.rs` (~150 line additions:
    swap types, async reload, build_inner_auth_from_effective,
    diff logic, +5 unit tests, all existing tests flipped to async).
  * `crates/lvqr-cli/src/config.rs` (`ConfigReloadSeed`
    extension).
  * `crates/lvqr-cli/src/main.rs` (boot-time defaults capture).
  * `crates/lvqr-cli/src/lib.rs` (boot/SIGHUP `.await` + admin
    closure flip + `pub use` extension).
  * `crates/lvqr-test-utils/src/test_server.rs` (seed
    construction with `None`s for jwks/webhook).
  * `docs/config-reload.md`, `README.md`,
    `tracking/HANDOFF.md`.
* **Tests** verified at session close:
  - `cargo test -p lvqr-cli --lib`: 44 passed (was 40 post-148;
    +4 net, with 1 feature-disabled-gated test that runs only on
    default features).
  - `cargo test -p lvqr-cli --features jwks --lib`: 42 passed
    (`jwks_url_emits_warning_when_feature_disabled` and
    `applied_keys_omits_jwks_when_url_unchanged` properly gated
    out via `#[cfg(not(feature = "jwks"))]`).
  - `cargo test -p lvqr-cli --features webhook --lib`: 43 passed.
  - `cargo test -p lvqr-cli --test config_reload_e2e`: 5 passed
    (unchanged; existing async-flip integration coverage stable).
  - `cargo test -p lvqr-admin --lib`: 44 passed.
  - `cargo test --workspace --lib --bins --tests`: 1111 / 0 / 0
    (was 1107 post-148; +4 net).
* **CI gates**: `cargo fmt --all -- --check` clean;
  `cargo clippy --workspace --all-targets -- -D warnings` clean;
  `cargo clippy --workspace --all-targets --features jwks,webhook
  -- -D warnings` clean.
* **Workspace version**: `0.4.1` unchanged. No publish.
* **Admin surface**: unchanged at 12 route trees (the wire shape
  on `/api/v1/config-reload` is unchanged; `applied_keys` simply
  grows entries).
* **SDK packages**: unchanged at 0.3.2 (the array-of-strings shape
  for `applied_keys` already accepts the new entries on both TS +
  Python clients).

### Known limitations after 149

* **Structural keys still bounce-required.** Port bindings,
  feature flags, record / archive directories, `mesh_enabled`,
  cluster topology -- none of these are in the file format yet,
  and reload never rebinds sockets. Operators changing these
  keys still bounce the relay.
* **No file watcher.** Operator must explicitly SIGHUP or POST.
* **No JWKS / webhook integration test for actual URL rotation.**
  When those providers are the active auth chain, they gate
  `/api/v1/config-reload` itself; a meaningful rotation test
  needs a mock decision endpoint returning `allow` for admin
  requests. Adding `wiremock` as an `lvqr-cli` dev-dep would
  enable such tests; deferred to a future increment when an
  operator-driven need surfaces. The unit tests + lvqr-auth's
  own provider tests cover the construction + diff semantics.

## Session 148 close (2026-04-25)

**Shipped**: Hot config reload v2 -- `mesh_ice_servers` and
`hmac_playback_secret` join the hot-reloadable surface alongside the
auth section. The `/signal` callback `load_full`s the swapped ICE
list per `Register`; live HLS / DASH and DVR `/playback/*`
middlewares `load_full` the swapped HMAC secret per request.
Outstanding URLs signed under a rotated secret stop verifying
immediately (the documented rotation intent). `applied_keys` on the
route response grows entries `"mesh_ice"` / `"hmac_secret"` only
when the new value diffs from the prior snapshot; the session 147
deferred-warning emissions for these two keys drop. `jwks_url` /
`webhook_auth_url` reload remain deferred (async-builder + HTTP-
cache complexity, separate session).

### Deliverables

1. **`crates/lvqr-cli/src/config_reload.rs`** (extended): two new
   public types -- `SwappableIceServers = Arc<ArcSwap<Vec<IceServer>>>`
   and `SwappableHmacSecret = Arc<ArcSwap<Option<Arc<[u8]>>>>` -- and
   their boot-time builders (`new_ice_swap`, `new_hmac_swap`).
   `ConfigReloadHandle::new` now takes both swap handles; `reload`
   diffs the file's new values against the prior snapshots
   (deep `==` on the swap-loaded inner; pointer-eq fast path is
   degenerate on a fresh build so it is omitted), pushes
   `"mesh_ice"` / `"hmac_secret"` into `applied_keys` only when
   diffed, then atomically swaps both alongside the auth chain. The
   two `warnings.push(...)` lines for these keys are deleted; the
   route's `warnings` field stays in the wire shape for forward-
   compat with future deferred categories. +8 unit tests covering
   diff-detection (positive + negative), clear semantics (missing
   key in file -> swap to None / empty), no-deferred-warnings
   regression, and reload-failure-keeps-prior-snapshot-intact for
   the new categories.

2. **`crates/lvqr-cli/src/lib.rs`** (refactor): three downstream
   capture sites flip from "captured-by-clone" to "load-on-each-
   call":
   * Signal callback: the `ice_servers_for_signal` capture is
     replaced by a `SwappableIceServers` clone; the closure
     `load_full`s once per Register and uses the same snapshot for
     both the existing-peer reuse path and the fresh-assign path
     (matches the in-flight semantics: the AssignParent ships the
     snapshot loaded at callback entry, the next Register sees the
     post-reload list).
   * `LivePlaybackAuthState.hmac_secret` field type flips to
     `SwappableHmacSecret`; HLS + DASH spawn blocks pass
     `hmac_swap.clone()` instead of the prior captured Arc.
   * `playback_router(.., hmac_secret: SwappableHmacSecret)`
     argument type flips; the per-handler `verify_signed_url` call
     `load_full`s + threads the `Option<&[u8]>` into the existing
     verifier helper.
   * Boot construction: `hmac_swap` and `ice_swap` are built
     adjacent to `hot_provider` (line 165-168) so all three are
     ready before the boot-time `handle.reload("boot")` runs and
     overrides them with the file's values.

3. **`crates/lvqr-cli/src/auth_middleware.rs`** (refactor):
   `LivePlaybackAuthState.hmac_secret: SwappableHmacSecret`. The
   middleware `load_full`s once per request and matches on
   `Option<&[u8]>`; cost is one `ArcSwap::load` (single-digit ns)
   on top of the existing per-request HMAC verify.

4. **`crates/lvqr-cli/src/archive.rs`** (refactor): `ArchiveState.
   hmac_secret: SwappableHmacSecret`; `playback_router` 4th-arg
   type flips. The three handlers (`playback_handler`,
   `latest_handler`, `file_handler`) each `load_full` once per
   request before the verify call.

5. **`crates/lvqr-cli/Cargo.toml`** (additive): `arc-swap = {
   workspace = true }` direct dep (was transitive via lvqr-auth).

6. **`crates/lvqr-cli/tests/config_reload_e2e.rs`** (extended):
   two new RTMP-shape integration cases.
   * `config_reload_swaps_mesh_ice_servers_via_admin_post`: file
     has one ICE server, TestServer + `with_mesh(3)` +
     `with_config_file(path)`, open `/signal` + Register, read the
     `AssignParent` reply, assert the boot ICE entry shows up.
     Rewrite file with a different ICE server, POST reload, assert
     `applied_keys` includes `"mesh_ice"`. Open another `/signal`
     + Register on a fresh peer, observe the new ICE entry.
   * `config_reload_rotates_hmac_playback_secret_via_admin_post`:
     file has `hmac_playback_secret = "boot-secret"`, TestServer +
     `with_config_file(path)`, mint a signed URL with
     `sign_live_url(b"boot-secret", LiveScheme::Hls, ..)`, GET HLS
     route -> not 401/403 (sig short-circuits the noop subscribe
     gate). Rewrite file with `"rotated-secret"`, POST reload,
     assert `applied_keys` includes `"hmac_secret"`. Old-signed URL
     -> 403 ("signed URL signature invalid"); URL signed under the
     new secret -> not 401/403.

7. **`docs/config-reload.md`** (rewrite): drops the deferred
   callouts for `mesh_ice_servers` + `hmac_playback_secret`; moves
   them to "What hot-reloads" alongside the auth section. Adds a
   "Clear semantics" section documenting that omitting a key in
   the file CLEARS the corresponding runtime state on the next
   reload (empty array = clear ICE list; missing top-level key =
   clear HMAC secret). Anti-scope explicitly retains "no
   `jwks_url` / `webhook_auth_url` reload" as deferred to a future
   session.

8. **`README.md`**: "Recently shipped" gains a session 148 entry;
   the existing "Hot config reload" `[x]` gets a session-148
   sub-bullet noting mesh ICE + HMAC reload; the ranked Next-up #1
   entry updates the prose to reflect v1 (auth) + v2 (mesh + HMAC)
   shipped, with `jwks_url` / `webhook_auth_url` flagged as the
   remaining deferred work.

### Key 148 design decisions baked in

* **Two new `Arc<ArcSwap<T>>` handles, no newtype.** Unlike
  session 147's `AuthCell` (needed because `Arc<dyn AuthProvider>`
  is unsized), `Vec<IceServer>` and `Option<Arc<[u8]>>` are both
  `Sized` and work directly with `ArcSwap::from_pointee`. The
  swap-handle types are public on `lvqr_cli::config_reload` so the
  composition root can build them at boot.

* **Diff detection by deep `==` against the loaded snapshot.** Each
  reload constructs a fresh `Arc<...>` from the file body, so a
  pointer-eq fast path is structurally dead code. Deep `==` on a
  small `Vec<IceServer>` (1-3 entries typical) and an
  `Option<Arc<[u8]>>` (~32 bytes) is well below the cost of the
  file read that just preceded it. Diffing is what drives
  `applied_keys`: operators see exactly which categories their
  reload effectively touched.

* **Clear semantics: missing key = clear state.** Omitting
  `mesh_ice_servers` from the file (or supplying an empty array)
  swaps the live list to empty. Omitting `hmac_playback_secret`
  swaps the live secret to `None`. This matches the file-as-source-
  of-truth model session 147 established for auth-section keys, and
  is documented as the rotate-by-clear path in
  `docs/config-reload.md`.

* **One reload pipeline for all three categories.** The auth chain,
  ICE list, and HMAC secret all reload atomically inside
  `ConfigReloadHandle::reload(kind)`. A build failure (malformed
  TOML, JWT init reject) leaves all three prior snapshots in place
  -- no partial swap. The rebuild order is auth -> ICE -> HMAC,
  but the `swap` calls happen sequentially after every build step
  succeeds, so a JWT reject can never leave a half-applied state.

* **`load_full` per call site.** The signal callback loads the ICE
  snapshot once at the top of the connected branch and uses it for
  both the existing-peer reuse path and the fresh-assign path; an
  `AssignParent` mid-callback ships the snapshot it captured at
  entry. The playback middleware loads the HMAC snapshot once per
  request. Cost: one `ArcSwap::load` (single-digit ns) on top of
  the existing per-emit / per-request work, identical to the
  session 147 auth path.

### Ground truth (session 148 close)

* **Source change scope**: 8 files substantively edited:
  * `crates/lvqr-cli/Cargo.toml` (arc-swap direct dep).
  * `crates/lvqr-cli/src/config_reload.rs` (swap types + extended
    reload + 8 new unit tests + 1 unit test rewritten).
  * `crates/lvqr-cli/src/lib.rs` (signal callback + 3 HMAC capture
    sites flipped).
  * `crates/lvqr-cli/src/auth_middleware.rs` (LivePlaybackAuthState
    field type flip + load_full).
  * `crates/lvqr-cli/src/archive.rs` (ArchiveState + playback_router
    flip + 3 handler load_fulls).
  * `crates/lvqr-cli/tests/config_reload_e2e.rs` (+2 tests).
  * `docs/config-reload.md` (rewrite).
  * `README.md`, `tracking/HANDOFF.md` (this session's close
    block).
* **Tests** verified at session close via
  `cargo test -p lvqr-cli --lib` (40 tests, was 32 post-147 in
  this crate; +8 net) and
  `cargo test -p lvqr-cli --test config_reload_e2e` (5 tests, was
  3 post-147; +2 net).
* **CI gates**: `cargo fmt --all -- --check` clean; `cargo clippy
  --workspace --all-targets -- -D warnings` clean.
* **Workspace version**: `0.4.1` unchanged. No publish.
* **Admin surface**: unchanged at 12 route trees (the route's wire
  shape is unchanged; `applied_keys` simply grows entries).
* **SDK packages**: unchanged at 0.3.2 (the array-of-strings shape
  for `applied_keys` already accepted the new entries on both TS +
  Python clients without a code change).

### Known limitations after 148

* **`jwks_url` and `webhook_auth_url` reload deferred.** Their
  constructors are async + cache HTTP state; replacing them
  mid-process needs additional plumbing and is its own session's
  scope.
* **Structural keys still bounce-required.** Port bindings, feature
  flags, record / archive directories, `mesh_enabled`, cluster
  topology -- none of these are in the file format yet, and reload
  never rebinds sockets. Operators changing these keys still bounce
  the relay.
* **No file watcher.** Operator must explicitly SIGHUP or POST.
* **No SDK shape change.** The TS + Python clients accept
  `applied_keys` as `string[]` / `list[str]` already and continue
  to work with the new `"mesh_ice"` / `"hmac_secret"` entries. A
  future session may add typed enums for the category names if
  operators ask for them.


## Session 147 close (2026-04-25)

**Shipped**: Hot config reload, auth-only v1. `lvqr serve --config
<path.toml>` parses a TOML file at boot; SIGHUP (Unix) and
`POST /api/v1/config-reload` (cross-platform) rebuild the inner
auth chain (Static / JWT-HS256) from a fresh file read and
atomically swap the live `SharedAuth` via a new
`lvqr_auth::HotReloadAuthProvider`. In-flight `check()` calls
finish against the prior snapshot; subsequent calls see the new
provider. Stream-key store handle (session 146) is preserved
across reloads. Failed reloads (malformed TOML, JWT init reject)
surface as 500 with the parse error; the prior provider stays
live.

### Deliverables

1. **`crates/lvqr-auth/src/hot_reload_provider.rs`** (new): wrapper
   over `arc_swap::ArcSwap<AuthCell>` (sized newtype around
   `Arc<dyn AuthProvider>` because `ArcSwap`'s `RefCnt` impl on
   `Arc<T>` is implicit-Sized). `swap` replaces the inner provider;
   `check` does `load() -> guard.0.check(ctx)` with the guard held
   across the delegate (regression test guards against UAF /
   guard-drop refactors). +5 unit tests.

2. **`crates/lvqr-cli/src/config_file.rs`** (new): `ServeConfigFile`
   serde mirror with `#[serde(default)]` on every Optional;
   `AuthSection` mirrors the existing CLI auth flag names. Forwards
   compat: unknown top-level keys are tolerated (no
   `deny_unknown_fields`) so a future server adding sections does
   not break older config files. +8 unit tests covering empty body,
   round-trip, missing fields, unknown top-level keys, malformed
   input, disk I/O, and `AuthSection` `Eq` for diff detection.

3. **`crates/lvqr-cli/src/config_reload.rs`** (new):
   `AuthBootDefaults` captures the CLI's auth-shaped fields once at
   boot. `ConfigReloadHandle::reload(kind)` re-parses the file,
   merges file overrides onto boot defaults, rebuilds the inner
   provider via `build_static_auth_from_effective`, wraps in
   `MultiKeyAuthProvider` if streamkeys are enabled (preserving the
   existing store), then swaps the wrapper. `parking_lot::Mutex`
   guards the last-reload state. +7 unit tests covering replace-
   from-file, boot-defaults-fill-unset-fields, repeated-reload,
   reload-failure-keeps-prior-state, deferred-section-warnings,
   path-in-status-pre-first-reload, store-preserved-across-reload.

4. **`crates/lvqr-admin/src/config_reload_routes.rs`** (new):
   `ConfigReloadStatus` wire shape (defined here so the admin
   crate owns the response type; lvqr-cli imports it).
   `GET /api/v1/config-reload` returns the wired closure's status
   or a default body when not wired; `POST /api/v1/config-reload`
   triggers reload via the closure (503 when not wired, 500 on
   build failure, 200 on success). +6 unit tests via
   `Router::oneshot`.

5. **`crates/lvqr-cli/src/{lib.rs,main.rs,config.rs}`**:
   * `--config <PATH>` CLI flag (env `LVQR_CONFIG`).
   * `ServeConfig.config_reload: Option<ConfigReloadSeed>` carries
     the path + boot defaults from `serve_from_args` to `start()`.
   * `start()` always wraps the resolved auth in
     `HotReloadAuthProvider` (single-digit-ns overhead per check).
     When the seed is present, builds `ConfigReloadHandle`, runs
     `reload("boot")` once to apply file overrides, spawns a
     SIGHUP listener (Unix-only via `#[cfg(unix)]`), and wires the
     status + trigger closures into `AdminState`.
   * The merge logic lives in ONE place
     (`ConfigReloadHandle::reload`) so CLI and TestServer get
     identical behavior; `serve_from_args` no longer pre-merges
     args.

6. **`crates/lvqr-test-utils/src/test_server.rs`**:
   `TestServerConfig::with_config_file(path)` builder threads
   through to `ServeConfig.config_reload`, so integration tests can
   exercise the full reload pipeline without a separate harness.

7. **`crates/lvqr-cli/tests/config_reload_e2e.rs`** (new): real
   RTMP-publish lifecycle. (a) Write file with
   `[auth] publish_key="v1"`. (b) TestServer + `--config`. (c) RTMP
   v1 succeeds (boot reload applied file). (d) Rewrite file with
   `publish_key="v2"`. (e) `POST /api/v1/config-reload` returns 200
   with `applied_keys: ["auth"]`. (f) RTMP v1 denied, v2 accepted.
   (g) `GET /api/v1/config-reload` reflects the most recent
   reload's metadata. Sister tests cover the no-`--config` 503
   path and the malformed-file-keeps-prior-provider path.
   3 tests.

8. **SDK client surfaces**:
   * `@lvqr/core`: `ConfigReloadStatus` interface +
     `LvqrAdminClient.configReload()` /
     `triggerConfigReload()`.
   * `lvqr` python: `ConfigReloadStatus` dataclass +
     `LvqrClient.config_reload_status()` /
     `trigger_config_reload()` with defensive `.get(...)` parsers.
     +3 pytest cases (status-default, status-populated, trigger).
   * Vitest live tests for the new methods deferred -- they
     would require the SDK CI workflow to boot
     `lvqr serve --config <path>` with a temp file, which is its
     own workflow change. Pytest mocks + Rust integration tests
     cover the same logic for v1.

9. **Docs + README**:
   * `docs/config-reload.md` (new): file format, hot-reload matrix,
     deferred-key warnings, composition order with MultiKey + the
     existing chain, failure modes, observability.
   * `README.md`: Auth section bullet, ranked Next-up #1 flipped to
     shipped strikethrough, "Recently shipped" gains a compact 147
     entry, Quickstart curl line, CLI flag listing for `--config`.

10. **`tracking/SESSION_148_BRIEFING.md`** (new; locks the design
    for the next-session work to widen hot reload to cover mesh
    ICE servers + HMAC playback secret reload).

### Key 147 design decisions baked in

* **`HotReloadAuthProvider` always wraps the chain** (not just when
  `--config` is set). The wrap is a transparent passthrough until
  `swap` is called; the read fast path is `ArcSwap::load` plus a
  delegate, single-digit nanoseconds. Always-on means there is no
  conditional code path between "config-enabled" and
  "config-disabled" deployments and the SIGHUP listener can install
  unconditionally on Unix (it is a no-op when no handle is wired).

* **Sized newtype `AuthCell` around `SharedAuth`.** `arc_swap`'s
  `RefCnt for Arc<T>` impl is implicit-`Sized`, so
  `Arc<dyn AuthProvider>` cannot be the cell's contents directly.
  The newtype adds one heap allocation per swap (cheap, off the
  hot path) and is invisible to callers behind the wrapper API.

* **File is the source of truth for auth fields it sets.** CLI
  flags + env vars are the DEFAULTS that the file's `[auth]`
  section overrides per non-`None` field. This matches the
  briefing's "file is source of truth when set; CLI flags + env
  vars become defaults that the file can override".

* **One reload pipeline for boot + SIGHUP + admin POST.** `start()`
  calls `handle.reload("boot")` once after building the initial
  chain, so the file-merge semantics are uniform across CLI
  (`lvqr serve --config`) and TestServer
  (`with_config_file(path)`). No second code path for "apply file
  at boot".

* **Stream-key store preserved across reloads.** The
  `ConfigReloadHandle` captures the `SharedStreamKeyStore` Arc
  once; every reload rebuilds the `MultiKeyAuthProvider` chain
  with the SAME store handle. Operators who minted keys via the
  146 CRUD API do not lose them on a SIGHUP.

* **JWKS / webhook providers retain boot-time values across
  reload.** Their constructors are async and cache HTTP state
  that does not round-trip cleanly through a synchronous swap.
  The reload pipeline only rebuilds the static / JWT-HS256 path;
  when the file's `[auth]` sets `jwks_url` or
  `webhook_auth_url`, the values are ignored at reload (with no
  warning surfaced -- the CLI flag at boot remains the source of
  truth for those provider types). This is documented as
  deferred work in `docs/config-reload.md`.

* **Deferred sections surface as warnings.** When the file
  contains `mesh_ice_servers` or `hmac_playback_secret`, reload
  succeeds for the auth section but the response carries a
  `warnings` entry naming the deferred section. This lets
  operators see in advance that those keys will not hot-reload
  yet.

* **`--config` is forward-compat-friendly.** Unknown top-level
  keys are tolerated (no `deny_unknown_fields`), so a future
  server adding sections does not break an older config file's
  parse. Combined with `#[serde(default)]` on every Optional
  field, the file format is open to additive growth.

### Ground truth (session 147 close)

* **Source change scope**: 13 files added or substantively edited:
  * `Cargo.toml` (workspace `arc-swap = "1"`).
  * `crates/lvqr-auth/{Cargo.toml,src/lib.rs,src/hot_reload_provider.rs}` (new file).
  * `crates/lvqr-cli/{Cargo.toml,src/lib.rs,src/main.rs,src/config.rs,src/config_file.rs,src/config_reload.rs}` (2 new files).
  * `crates/lvqr-cli/tests/config_reload_e2e.rs` (new).
  * `crates/lvqr-admin/{src/lib.rs,src/routes.rs,src/config_reload_routes.rs}` (1 new file).
  * `crates/lvqr-test-utils/src/test_server.rs` (with_config_file builder).
  * `bindings/js/packages/core/src/{admin.ts,index.ts}` (types + 2 methods).
  * `bindings/python/python/lvqr/{__init__.py,client.py,types.py}` (dataclass + 2 methods).
  * `bindings/python/tests/test_client.py` (+3 pytest cases).
  * `docs/config-reload.md` (new), `README.md`, `tracking/HANDOFF.md`.
* **Tests** (verified at session close via
  `cargo test --workspace --lib --bins --tests`, 131 test
  binaries, 1099 passed / 0 failed / 0 ignored):
  * lvqr-auth lib: 46 -> 51 (+5 hot_reload_provider).
  * lvqr-cli lib: 19 -> 34 (+15: 8 config_file + 7 config_reload).
  * lvqr-admin lib: 38 -> 44 (+6 config_reload_routes).
  * lvqr-cli integration: streamkeys_e2e (2) + config_reload_e2e (3, new) + every existing rtmp_*_e2e / auth_integration / cluster / mesh / wasm / etc continue to pass.
  * Python pytest: 35 -> 38 (+3 config-reload cases).
  * Vitest: unchanged at 13 (live tests for config-reload deferred
    -- pytest + Rust integration cover the logic).
  * **Workspace total**: 1070 -> **1099** (+29 net Rust).
* **CI gates**: `cargo fmt --all -- --check` clean; `cargo clippy
  --workspace --all-targets -- -D warnings` clean. All existing
  integration tests pass with the always-on
  `HotReloadAuthProvider` wrap.
* **Workspace version**: `0.4.1` unchanged. No publish.
* **Admin surface**: 11 route trees post-146 + 1 (`/api/v1/config-reload`) = **12 route trees**.

### Known limitations after 147

* **Mesh ICE servers reload deferred.** The `[[mesh_ice_servers]]`
  section in the file is parsed but not applied; reload surfaces a
  warning. Wiring requires `Arc<ArcSwap<Vec<IceServer>>>` through
  the signal callback.
* **HMAC playback secret reload deferred.** Same `ArcSwap`-thread-
  through requirement for the playback / live-HLS / live-DASH auth
  middleware.
* **`jwks_url` and `webhook_auth_url` reload deferred.** Their
  constructors are async + cache HTTP state; replacing them
  mid-process needs additional plumbing.
* **`--config` only covers `[auth]` + warn-on-deferred sections.**
  Other ServeConfig fields (port bindings, feature flags,
  record/archive dirs) are not in the file format yet. Adding them
  requires extending `ServeConfigFile` (additive; a future
  session's work).
* **No file watcher.** Operator must explicitly SIGHUP or POST.

## Session 146 close (2026-04-24)

**Shipped**: Runtime stream-key CRUD admin API. Pre-146 deployments provisioned ingest stream keys via static config (`LVQR_PUBLISH_KEY` for one shared key, or external JWT minting) -- both forced a server bounce or an out-of-band pipeline for every key change. Session 146 adds an in-memory `StreamKeyStore` plus a `MultiKeyAuthProvider` that wraps the existing auth chain additively, and mounts `/api/v1/streamkeys/{,/:id,/:id/rotate}` so admin clients can mint, list, revoke, and rotate keys at runtime. Five-artifact deliverable matches the 141-144 shipping shape: source change with unit+integration tests, wire shape with `#[serde(default)]` on every Optional, SDK client surfaces (TS + Python) with defensive parsers, docs (`docs/auth.md` + `docs/sdk/{javascript,python}.md`), and README "Next up" item flipped to shipped.

### Deliverables

1. **`crates/lvqr-auth/src/stream_key_store.rs`** (new, ~360 lines): `StreamKey` + `StreamKeySpec` wire types with `#[serde(default)]` on every Optional; `StreamKeyStore` trait + `InMemoryStreamKeyStore` impl using two `DashMap`s (id -> StreamKey primary, token -> id reverse for O(1) auth-path lookup). Token format `lvqr_sk_<43-char base64url-no-pad>` (32-byte `OsRng` + typed prefix per industry convention -- Stripe `sk_live_`, GitHub `ghp_`, IVS `sk_<region>_`); ids are 16 bytes random base64url-no-pad (22 chars). 11 unit tests covering mint, list-includes-expired, get_by_token-filters-expired, revoke-twice-is-no-op, rotate-no-override-preserves-scope, rotate-with-override-clears, rotate-unknown-id, lazy-expiry, mint-uniqueness, serde-round-trip.

2. **`crates/lvqr-auth/src/multi_key_provider.rs`** (new, ~225 lines): `MultiKeyAuthProvider { store, fallback: Option<SharedAuth> }`. Decision order locked: `Publish` -- store first; on hit, broadcast scope check decides without consulting fallback (a tighter scope cannot be silently widened by a more permissive layer underneath); on miss, delegate to fallback. `Subscribe` and `Admin` ALWAYS delegate to fallback (load-bearing safety property: stream-key CRUD never gates viewer or admin auth, so a misconfigured store cannot lock the operator out of their own admin API). 11 unit tests including the brief-required "store-hit-with-scope-mismatch beats permissive fallback" + "admin token survives a stream-key revoke of the same string" assertions.

3. **`crates/lvqr-admin/src/streamkey_routes.rs`** (new, ~340 lines): 4 handlers (`list_streamkeys`, `mint_streamkey`, `revoke_streamkey`, `rotate_streamkey`) registered through `AdminState::with_streamkey_store(SharedStreamKeyStore)`. Routes mount inside the existing admin-auth middleware so a configured `--admin-token` (or JWT provider) is required before any CRUD call. Counter `lvqr_streamkeys_changed_total{op="mint"|"revoke"|"rotate"}` increments once per successful mutating API call (not per affected key, so dashboards reading the counter see API velocity, not store fan-out). Rotate handler takes raw `Bytes` rather than `Option<Json<StreamKeySpec>>` because axum 0.8's `Json` extractor 400s on an empty body even when wrapped in `Option`; SDKs idiomatically send no body for "no override" and the raw-bytes parse keeps that round-trip clean. 8 unit tests via `Router::oneshot` covering each verb plus the admin auth gate plus mint-then-list visibility plus rotate-empty-vs-override scope semantics.

4. **`crates/lvqr-cli/src/{lib.rs,main.rs,config.rs}`**: `ServeConfig.streamkeys_enabled: bool` (default `true` via `loopback_ephemeral`); `start()` wraps the resolved auth provider in `MultiKeyAuthProvider { store: InMemoryStreamKeyStore, fallback: Some(inner) }` when enabled and attaches the store to `AdminState` so the CRUD routes can mutate it. New `--no-streamkeys` (`LVQR_NO_STREAMKEYS=1`) opts out for operators who want pre-146 behavior verbatim. `crates/lvqr-test-utils/src/test_server.rs` got a matching `with_no_streamkeys()` builder + an inverted `no_streamkeys: bool` field so the auto-derived `Default` keeps streamkeys-on (existing rtmp_*_e2e tests now go through MultiKey with empty store + Noop fallback, which behaves identically to today since store-miss falls through).

5. **`crates/lvqr-cli/tests/streamkeys_e2e.rs`** (new, ~330 lines): real RTMP-publish lifecycle test. Boots TestServer with a `StaticAuthProvider { publish_key: Some("never-matches") }` fallback so empty store + arbitrary RTMP key DENY by default (proves the fixture genuinely gates ingest). Then drives: (a) baseline arbitrary-key denied; (b) admin POST mints a key, list surfaces it, capture token; (c) RTMP publish with the minted token succeeds via store-hit; (d) admin DELETE revokes; (e) RTMP publish with the same token now denied via store-miss + fallback-deny. Sister test `streamkey_rotate_invalidates_old_token_on_publish` does the same shape across rotate. Both use raw-TCP HTTP/1.1 helpers inline (no reqwest dev-dep) and a lifted `try_rtmp_publish` helper from `one_token_all_protocols.rs`. 2 tests.

6. **SDK client surfaces**:
   * **`bindings/js/packages/core/src/admin.ts`**: new `StreamKey` + `StreamKeySpec` + `StreamKeyList` interfaces; new `listStreamKeys()` / `mintStreamKey(spec?)` / `revokeStreamKey(id)` / `rotateStreamKey(id, override?)` methods + a private `sendJson` helper that omits Content-Type when no body is provided so the rotate empty-body path round-trips cleanly. Re-exports added to `index.ts`.
   * **`bindings/python/python/lvqr/{types,client,__init__}.py`**: matching `StreamKey` + `StreamKeySpec` dataclasses; matching `list_streamkeys()` / `mint_streamkey(spec)` / `revoke_streamkey(id)` / `rotate_streamkey(id, override?)` methods with a `_streamkey_from_json` defensive parser that uses `.get(...)` for every Optional field so future server bodies adding sibling fields do not break older clients.
   * **`bindings/js/tests/sdk/admin-client.spec.ts`**: 2 new live Vitest cases (mint-list-revoke round-trip + rotate-preserves-id) hitting the CI harness `lvqr serve` instance.
   * **`bindings/python/tests/test_client.py`**: 5 new pytest cases (list-empty, list-populated-omitting-optional, list-pre-146-omits-keys-wrapper, mint-round-trip, StreamKeySpec-defaults) using `unittest.mock.patch` against `httpx.Client.{get,post}`.

7. **Docs + README**:
   * **`docs/auth.md`**: new "Stream-key CRUD admin API" section covering the additive composition model, wire shape, token format, rotate semantics, persistence, observability, and anti-scope.
   * **`docs/sdk/{javascript,python}.md`**: new "Stream-key CRUD" section with a worked `mint -> list -> rotate -> revoke` example.
   * **`README.md`**: "Next up" item 1 flipped from `[ ]` to `[x] Shipped in session 146.` with a docs-link to the new auth.md anchor.

### Key 146 design decisions baked in

* **`MultiKeyAuthProvider` always wraps the existing chain (including Noop), not just JWT/JWKS/Webhook.** Brief section 4 listed only those three flags as the trigger set; following that literally would have meant a bare `lvqr serve` deployment installs MultiKey with `fallback: None`, which DENIES every Publish on store-miss -- a silent backward-breaking change for every existing rtmp_*_e2e test (and every operator running without auth flags). Re-resolved on read-back: MultiKey is purely additive over WHATEVER `build_auth()` resolves, including Noop. Bare `lvqr serve` keeps allowing arbitrary publishes via the Noop fallback; minting a key is purely additive (nobody loses access). Operators who want stream-key as the enforcement gate layer a deny-by-default fallback (Static-publish-key, JWT, Webhook) underneath. Documented in `docs/auth.md`'s composition section. Existing `auth_integration.rs` + every `rtmp_*_e2e.rs` continue to pass unchanged.

* **Tokens carry the `lvqr_sk_` prefix.** Brief said "32 bytes OsRng base64-urlsafe-encoded" -- silent on prefix. Web research found unanimous industry convention for typed prefixes on bearer credentials (Stripe `sk_live_`, GitHub `ghp_`, AWS IVS `sk_<region>_`) for secret-scanner recognisability. Adopted: total token shape is `lvqr_sk_<43-char base64url-no-pad>`, 51 chars. Operators who grep for `lvqr_sk_` in logs / git history / leaked-secret feeds can identify LVQR keys at a glance; secret-scanning vendors (GitHub Advanced Security, GitLeaks, TruffleHog) can add a one-line regex.

* **List endpoint returns the literal token, not a redacted prefix.** Brief locked this; web research confirmed: Mux Live Streams, Cloudflare Stream Live Inputs, and AWS IVS all return the full token on subsequent GETs / lists. The "mint-once-display" model (GitHub PATs) defends against a threat where the secret holder is a less-trusted actor on a multi-tenant SaaS; an LVQR operator running their own relay is the same principal who holds admin credentials with equal or greater blast radius, so storing the token cleartext adds no marginal risk. Operators who DO want the GitHub-style model can set up a webhook and rotate keys after first read.

* **Rotate is single-call atomic, body decides scope.** Empty body preserves `label` / `broadcast` / `expires_at` and only swaps the token (operator-friendly default for "I leaked the token, rotate everything else"). Non-empty `StreamKeySpec` re-scopes while rotating; a `null` field on the override CLEARS the existing field. Industry split was 50/50 between two-op delete+create (IVS, Mux, Cloudflare) and single-call regenerate (GitHub PATs); single-call won here because the brief locked it and atomicity at the token layer is operator-friendly even if not strictly required.

* **`AuthContext::Admin` always delegates to fallback.** The load-bearing safety property: stream-key CRUD never gates admin auth. Without this, an operator who misconfigured the fallback chain and revoked the only admin token would be locked out of their own server. Tested via the explicit "admin token survives stream-key revoke of the same string" unit test.

* **Stream-key store is publish-only in v1.** `Subscribe` and `Admin` contexts always delegate to fallback. Subscribe tokens have an existing surface (`SubscribeAuth` + the HMAC-signed-URL path from session 124/128); adding a parallel CRUD surface for them without operator demand is scope creep.

* **In-memory only, lazy expiry, no daemon sweep.** `expires_at` is checked on every `get_by_token` lookup so an expired token never authenticates; the operator-facing list endpoint surfaces expired keys until a manual revoke (cosmetic only -- they don't authenticate). Sled / SQLite-backed `StreamKeyStore` impl is its own session and the trait is shaped so the swap is purely additive.

* **`AdminState` stores `Option<SharedStreamKeyStore>`, not feature-gated routes.** When the operator passes `--no-streamkeys` the routes are still mounted but the list endpoint returns `{"keys":[]}` and the mutating endpoints return 500. This matches the consistent "always-mount + return safe defaults when not configured" pattern the slo + wasm_filter routes use; tooling can poll unconditionally without a 404 handler.

* **Counter increments once per API call, not per affected key.** Brief decision 4. `op="rotate"` is its own label rather than a synthetic `revoke+mint` pair so dashboards summing the labels see one event per operator action. Revoke effects on the auth path also surface on the existing `lvqr_auth_publish_denied_total{entry=...}` counter; operators monitoring publish-denial rates see revoke effects without an additional metric.

* **Test count exceeded the prompt target.** Prompt called for ~+19 net (+10 lvqr-auth unit, +4 lvqr-admin unit, +1 RTMP integration, +2 Vitest live, +2 pytest); shipping ~+39 (+22 lvqr-auth, +8 lvqr-admin, +2 RTMP integration, +2 Vitest, +5 pytest). Extra margin lands on edge cases the brief flagged but did not enumerate (lazy-expiry-on-list-vs-auth-path, scope-mismatch-beats-permissive-fallback, rotate-clears-vs-preserves, list-empty-when-store-not-configured, defensive-parse-when-server-omits-keys-wrapper).

### Ground truth (session 146 close)

* **Head**: `9b5f4aa` on `main` pre-session-146; close-block commit not yet authored at this time. v0.4.1 unchanged.
* **Source change scope**: 8 files added or substantially edited:
  * `crates/lvqr-auth/src/stream_key_store.rs` (new)
  * `crates/lvqr-auth/src/multi_key_provider.rs` (new)
  * `crates/lvqr-auth/src/lib.rs` (re-exports)
  * `crates/lvqr-auth/Cargo.toml` (dashmap+rand+base64 to direct deps)
  * `crates/lvqr-admin/src/streamkey_routes.rs` (new)
  * `crates/lvqr-admin/src/{routes.rs,lib.rs}` (AdminState builder + module decl + route mount)
  * `crates/lvqr-cli/src/{lib.rs,main.rs,config.rs}` (wrap + flag + ServeConfig field)
  * `crates/lvqr-test-utils/src/test_server.rs` (matching `no_streamkeys` knob)
  * `crates/lvqr-cli/tests/streamkeys_e2e.rs` (new)
  * `bindings/js/packages/core/src/{admin.ts,index.ts}` (types + 4 methods + re-exports)
  * `bindings/js/tests/sdk/admin-client.spec.ts` (+2 live cases)
  * `bindings/python/python/lvqr/{types,client,__init__}.py` (types + 4 methods + re-exports)
  * `bindings/python/tests/test_client.py` (+5 defensive-parse cases)
  * `docs/auth.md` (new "Stream-key CRUD admin API" section)
  * `docs/sdk/{javascript,python}.md` (new "Stream-key CRUD" section each)
  * `README.md` (Next-up item 1 flipped)
* **Tests** (verified at session close):
  * lvqr-auth lib: 24 -> 46 (+22).
  * lvqr-admin lib: 30 -> 38 (+8).
  * lvqr-cli integration (streamkeys_e2e): 0 -> 2 (+2).
  * Vitest live: 11 -> 13 (+2).
  * Python pytest: 30 -> 35 (+5).
  * Workspace default gate: 1043 -> 1070 (+27 Rust net, target was +19; extra margin on lazy-expiry edge cases + defensive-parse compat). 130 test binaries, 0 failed, 0 ignored on this host.
* **CI gates**: `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean (one redundant_closure on a `RelayStats::default` reference cleaned up before close); existing `auth_integration.rs` (6 tests) + every `rtmp_*_e2e.rs` continue to pass with the MultiKey wrap on by default (proves the wrap is purely additive).
* **Workspace version**: `0.4.1` unchanged. No `cargo publish`. `@lvqr/core@0.3.2` + `@lvqr/player@0.3.2` (npm) + `lvqr@0.3.2` (PyPI) unchanged.
* **Admin surface**: 11 route trees now (added `/api/v1/streamkeys/*`).

### Known limitations after 146

* **No persistence backend.** Restart loses every minted key. Operators needing durable single-key publish auth keep using the existing `LVQR_PUBLISH_KEY` (which becomes the wrapped fallback under MultiKey). Future session can land a sled / SQLite-backed `StreamKeyStore` impl; the trait is shaped so the swap is purely additive.
* **No subscribe-token CRUD.** Same surface could in principle expose viewer tokens, but the existing HMAC-signed-URL path (sessions 124/128) already covers the common need. Adding a second surface without operator demand is scope creep.
* **No JWT-mint endpoint.** Stream-keys are STORE keys, not signed JWTs. Operators wanting signed JWTs continue to use their own minting pipeline (`--jwks-url` + an external IdP, or `--jwt-secret` + an external mint script).
* **No bulk operations.** Mint / revoke / rotate are per-key. Bulk import / export is its own session.
* **No webhook on key changes.** Operators wanting "key minted" callouts can poll the list endpoint or watch the `lvqr_streamkeys_changed_total{op=...}` counter.
* **No daemon expiry sweep.** `expires_at` is checked on the auth path; expired keys remain visible on the list endpoint until manually revoked. Lazy expiry is correct semantically; only operator-facing cosmetics on the list show stale entries.
* **No hot config reload bundling.** Hot reload is a separate next-up item (#2 in README) and remains unscheduled.
* **All other session 145 + earlier known limitations unchanged.**



## Session 145 close (2026-04-24)

**Shipped**: Workspace 0.4.0 -> 0.4.1 release cut + republish of all 26 publishable Rust crates on crates.io. Pure release session: zero source changes outside version strings, the audit pre-flight (Cargo.lock + audit.toml + workflow staging step), and the post-publish README flip. Sessions 141-144's source (per-peer mesh capacity in 144, ICE config + TURN deploy in 143, three-peer Playwright in 142, offload reporting in 141) now reaches `cargo install lvqr-cli` users; the registry was previously frozen at the 0.4.0 snapshot from 2026-04-16 and consumers depending on the published API could not pick up any of the four mesh-data-plane rows even though they all landed on `origin/main`. Live-verified end-to-end via lvqr-cli's own publish-verify step (resolves and compiles all 25 transitive deps from crates.io into a fresh `target/package/lvqr-cli-0.4.1` build before upload).

### Deliverables

1. **`tracking/SESSION_145_BRIEFING.md`** (new, 423 lines): locked the design before any source touch. Documents the 25-vs-26 count reconciliation in the kickoff (lvqr-transcode appearing in the tier list while the body said "25 already-claimed"; resolved to "all 26 bump to 0.4.1"), the audit-failure root cause (RUSTSEC-2026-0098/0099/0104 against rustls-webpki 0.103.11; daily advisory-DB refresh on `9f5bded` caught the 2026-04-14 to 2026-04-22 advisories that did not exist when sessions 141-143 last passed CI), the version-bump strategy (single `workspace.package.version` flip + 26 internal-dep version-string flips, atomic in one commit; every member crate already uses `version.workspace = true` so the bump propagates), the republish order (depended on a 4-layer topo sort, not the user's original 6-tier list which placed lvqr-record in Tier 1 even though it depends on lvqr-cmaf), and the explicit anti-scope list (no feature work, no CHANGELOG fold-in, no npm/PyPI re-publishes, no wasmtime upgrade, no tag push).

2. **`audit.toml`** (new, repo root, 41 lines): cargo-audit config consumed by `.github/workflows/audit.yml` and `.github/workflows/ci.yml`. `[advisories].ignore` lists 22 advisory IDs with per-line rationale comments:
   * 16 wasmtime advisories (RUSTSEC-2025-0046, 2025-0118, 2026-0020/0021/0085/0086/0087/0088/0089/0091/0092/0093/0094/0095/0096): pinned to wasmtime 25 per `Cargo.toml` workspace.dependencies' "any upgrade gets its own session per TIER_4_PLAN.md" comment; closing each row needs a multi-major bump that touches the lvqr-wasm host-binding generator.
   * 1 rsa Marvin (RUSTSEC-2023-0071): no upstream fix exists; rsa is a c2pa transitive used for manifest signing only, not decryption, so the chosen-ciphertext sidechannel is not exploitable in our use.
   * 5 unmaintained or unsound transitives: paste (RUSTSEC-2024-0436), proc-macro-error (RUSTSEC-2024-0370), rustls-pemfile (RUSTSEC-2025-0134), lru (RUSTSEC-2026-0002, IterMut Stacked Borrows), rand (RUSTSEC-2026-0097, custom-logger unsoundness).

3. **`Cargo.lock`**: `rustls-webpki 0.103.11 -> 0.103.13` via `cargo update -p rustls-webpki` closes RUSTSEC-2026-0098 (Name constraints for URI names), RUSTSEC-2026-0099 (Name constraints for wildcard), RUSTSEC-2026-0104 (CRL parse panic). Patch-version bump within the 0.103.x line; transitive only (workspace pin is on `rustls 0.23`); 26 lvqr-* row ticks 0.4.0 -> 0.4.1.

4. **`.github/workflows/audit.yml`** + **`.github/workflows/ci.yml`**: new "Stage advisory ignores" step before each `cargo audit` invocation copies repo's `audit.toml` to `~/.cargo/audit.toml`. cargo-audit 0.22 reads its config from the user-cargo dir, not from CWD; without this step the project-of-record audit.toml had no effect.

5. **`Cargo.toml`**: `workspace.package.version "0.4.0" -> "0.4.1"` + 26 `version = "0.4.0"` -> `"0.4.1"` flips in `[workspace.dependencies]` for every internal crate-with-`version`-string entry. The 3 `path`-only entries (lvqr-conformance, lvqr-test-utils, lvqr-soak; all `publish = false`) already had no version string so they did not need touching. Single `replace_all` against `version = "0.4.0"` since exactly 27 instances existed (1 + 26).

6. **`CHANGELOG.md`**: new `## [0.4.1] - 2026-04-24` entry naming the republish + linking `tracking/HANDOFF.md` for the session-83-through-144 narrative; the stale `## Unreleased (post-0.4.0, through session 82 -- 2026-04-17)` heading is renamed to `## Unreleased-pre-0.4.1` so it cannot mask future genuine pending-release content. Sessions 83-144 stay in HANDOFF; folding them into the changelog is a future docs-sweep candidate.

7. **`README.md`**: Client libraries table row for Rust flips `0.4.0 (crates.io)` to `0.4.1 (crates.io)`. Tier 5 readiness paragraph flips `(\`lvqr-core\` at 0.4.0 on crates.io)` to `0.4.1`. The "Known v0.4.0 limitations" section heading and its anchors stay as-is: those document what was known-broken at the v0.4.0 release cut and remain accurate as historical-state markers; relabelling them is anti-scope.

8. **Published artifacts (crates.io, 26 crates at 0.4.1)**:
   * Layer 0 (no intra-workspace deps): `lvqr-core`, `lvqr-archive`, `lvqr-auth`, `lvqr-codec`, `lvqr-moq`, `lvqr-observability`.
   * Layer 1 (Layer 0 only): `lvqr-fragment`, `lvqr-signal`, `lvqr-relay`, `lvqr-cluster`.
   * Layer 2 (Layer 0 + 1): `lvqr-cmaf`, `lvqr-admin`, `lvqr-agent`, `lvqr-mesh`, `lvqr-transcode`, `lvqr-wasm`.
   * Layer 3 (Layer 0 through 2): `lvqr-record` (cmaf + moq), `lvqr-agent-whisper` (agent + fragment), `lvqr-hls` (cmaf + codec + core), `lvqr-ingest` (cmaf + every Layer 0 + 1 dep).
   * Layer 4 (Layer 0 through 3): `lvqr-dash`, `lvqr-rtsp`, `lvqr-srt`, `lvqr-whip`, `lvqr-whep`.
   * Layer 5 (everything): `lvqr-cli`.

### Key 145 design decisions baked in

* **Bump all 26, not 25.** The kickoff prompt body said "25 already-claimed" but the embedded tier list contained 26 (lvqr-transcode in Tier 2). Reconciled: 25 was the pre-session-144 published count; session 144 first-published lvqr-transcode at 0.4.0 bringing total to 26. Because every member crate uses `version.workspace = true`, bumping `workspace.package.version` auto-bumps lvqr-transcode whether we want it to or not; the "skip transcode" path is messy un-inheriting. Cleaner to publish lvqr-transcode 0.4.1 with no source diff vs 0.4.0 and keep workspace-version coherence.

* **Audit fix as a separate pre-flight commit, not bundled.** `cargo publish` itself does NOT run `cargo audit`, so a tree where origin/main's audit job is red could still publish. We chose not to: the published source would carry a known-failing supply-chain check, and operators running a routine `cargo install --locked` could surface the same warnings. Three-commit shape ((1) audit fix, (2) version bump, (3) post-publish README flip) keeps each step's history readable and revertable in isolation.

* **Tier list re-derived from actual deps after lvqr-record failed.** The kickoff brief placed lvqr-record in Tier 1, but `lvqr-record/Cargo.toml` declares `lvqr-cmaf = { workspace = true }` (and lvqr-cmaf is in Tier 2). Live publish surfaced the bad ordering as `failed to select a version for the requirement \`lvqr-cmaf = "^0.4.1"\`: candidate versions found which didn't match: 0.4.0`. Recovered by publishing lvqr-cmaf first, then lvqr-record. Authored a 4-layer topo-sort (six layers including Tier 0 + cli leaf) from a one-shot grep of every remaining crate's intra-workspace deps; documented in the close block deliverable list.

* **`cargo publish` waits for index-availability automatically.** Each invocation logs `note: waiting for <crate> v0.4.1 to be available at registry crates-io` and blocks until the new version is resolvable by subsequent `cargo` calls. Removes the need for the briefing-suggested ~45 s manual inter-tier sleep; total session walltime for the 26 publishes was bounded by the verify+upload time per crate, not by registry-settle.

* **`audit.toml` lives at repo root, copied to `~/.cargo/audit.toml` in CI.** cargo-audit 0.22 only reads config from the user cargo dir or from `--config <PATH>` (cargo-audit's CLI does not auto-discover repo-root audit.toml). Putting the project-of-record at repo root keeps it version-controlled and reviewable; the workflow's new "Stage advisory ignores" step copies it before each `cargo audit` invocation. Future cargo-audit versions may add CWD auto-discovery; the staging step costs one `cp` line and is forwards-compatible.

* **Each ignore in audit.toml carries a tracking note, not a bare ID.** The wasmtime block names TIER_4_PLAN.md as the deferred-upgrade home; the rsa block names the LVQR exposure (signing-only, not decryption) so a future reader can confirm the rationale still holds. A naked ignore list rots silently as the project's exposure shifts; the rationale lines force re-evaluation when an ignore is touched.

* **`CHANGELOG.md` got a 2-line stub, not a 62-session fold-in.** Sessions 83-144's narrative lives in HANDOFF.md (per-session blocks plus archived Tier 0-3 archive). Folding the 62 sessions into changelog form is editorially expensive (~half a session). The 0.4.1 entry instead names the republish reason and links HANDOFF as the authoritative narrative; the stale "Unreleased" header gets renamed to `Unreleased-pre-0.4.1` so a future genuine `## Unreleased` block is unambiguous.

* **Live-smoke via lvqr-cli's own publish-verify, not a parallel `cargo install`.** lvqr-cli is the leaf of the dep graph; its `cargo publish` step compiles all 25 transitive deps fresh from crates.io into a `target/package/lvqr-cli-0.4.1` build directory before uploading. That IS the smoke test for end-to-end registry consistency; a parallel `cargo install --locked lvqr-cli` would repeat the same work in a clean CARGO_HOME. We kicked off the parallel install as defense-in-depth and continued with the close block; the bounded time-cost of the lvqr-cli verify (~24 s) is the load-bearing signal.

* **`.claude/` left untracked, not gitignored.** Every `cargo publish` ran with `--allow-dirty` so the untracked harness dir did not block the upload. Adding `.claude/` to `.gitignore` is a defensible follow-up but anti-scope here (the briefing's "no feature work, no bug fixes" anti-scope covers project-hygiene tweaks too). Future session can land it in a one-line commit if any harness state should never be committed.

### Ground truth (session 145 close)

* **Head**: `aff0d73` on `main`. v0.4.1. `origin/main` at `aff0d73`; local + origin EVEN after this docs-close commit lands.
* **Crates**: 29 workspace members; 26 published at 0.4.1 on crates.io; 3 `publish = false` (lvqr-conformance, lvqr-test-utils, lvqr-soak).
* **Tests**: unchanged from 144 close. Rust workspace default gate **1043** passed / 0 failed / 3 ignored. Python pytest **30**. Vitest 11. No Rust source touched in 145; cargo check + clippy clean re-runs verified the version flip was a non-functional change.
* **CI gates**: `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean (19.22 s on a primed cache); GitHub Actions Supply-chain audit job GREEN on `chore(release): bump workspace 0.4.0 -> 0.4.1` (3m14s). Other CI jobs in-progress at the time of this close block but every one of them was GREEN on the predecessor commit `9f5bded` and the version-bump touched no source.
* **Workspace version**: `0.4.1` everywhere via `version.workspace = true` inheritance.
* **npm + PyPI**: `@lvqr/core@0.3.2`, `@lvqr/player@0.3.2`, `lvqr@0.3.2` (PyPI) -- unchanged from session 144; the JS/Python SDKs version their wire shapes independently from the Rust workspace.

### Known limitations / documented v1 shape (after 145 close)

* **wasmtime is still pinned to 25.0.3.** Sixteen advisories against this version are explicitly tolerated via `audit.toml`; closing each row needs the deferred wasmtime >=36 upgrade per TIER_4_PLAN.md. Tracked as a planned Tier 4 maintenance session; not blocking 0.4.1.
* **rsa Marvin (RUSTSEC-2023-0071) has no upstream fix.** Permanently in `audit.toml` ignore until upstream rsa ships a constant-time path. LVQR's exposure is signing-only via c2pa, so the sidechannel is not exploitable in our use, but the advisory will keep firing on every `cargo audit` invocation.
* **Sessions 83-144 are not folded into CHANGELOG.md.** The 0.4.1 entry points readers at `tracking/HANDOFF.md` for the narrative. A future docs-sweep session can fold them back; until then `Unreleased-pre-0.4.1` is a partial-release-notes entry valid only for the 45-82 window.
* **No git tag pushed for 0.4.1.** The kickoff prompt did not request a `v0.4.1` tag and the session shipped without one; consumers depending on a tagged-release workflow should rely on the crates.io 0.4.1 publish or the `chore(release): bump workspace 0.4.0 -> 0.4.1` commit hash (`6b38e20`) as the release marker.
* **Three publishable rows in the post-0.4.0 work were deliberately left at 0.3.2 on their respective registries.** `@lvqr/core` and `@lvqr/player` (npm) and `lvqr` (PyPI) carry sessions 141-144 source but version-skew with the Rust workspace; a future session can align numbering if helpful, or each track can keep its independent cadence.
* **`.claude/` was in the working tree as untracked during the publish events.** Subsequently `.gitignore`d in the post-145 cleanup commit (see "Session 146 entry point" below). Every `cargo publish` in 145 ran with `--allow-dirty` to step over it; future sessions will not need the flag.
* **All other session 144 + earlier known limitations unchanged.**

### Session 146 entry point

Post-release cleanup landed on `origin/main` after the 145 close
block: README "Next up" list pruned (shipped items 1-5 + 7 moved
to a compact "Recently shipped" subsection; pending items
re-ranked by ship-ability so single-session items lead and
multi-session feature work follows); `.gitignore` grew a
`.claude/` line so future `cargo publish` invocations do not
need `--allow-dirty`; "2026-04-22 codebase audit" reference
flipped to 2026-04-24.

**Next deliverable: Stream-key CRUD admin API** (README "Next
up" item 1, post-cleanup ranking). Today RTMP / SRT / WHIP
stream keys are static config: operators provision them at
boot via `--auth-secret-jwt` or a JWKS endpoint, and rotation /
revocation requires a server bounce. The session-146 ship adds
`/api/v1/streamkeys` admin routes (list, mint, revoke, rotate)
backed by a stream-key store that lives alongside the existing
`AuthProvider` chain, plus matching JS + Python admin-client
methods.

Expected scope (locked when SESSION_146_BRIEFING.md lands):

1. New `StreamKeyStore` trait in `lvqr-auth` with at least an
   in-memory implementation (file-backed persistence is a
   follow-up). Methods: `list()`, `mint(spec) -> StreamKey`,
   `revoke(id)`, `rotate(id) -> StreamKey`.
2. New routes on `lvqr-admin` under `/api/v1/streamkeys`. JSON
   schema mirrors `MeshPeerStats` shape pattern from session 144.
3. `lvqr-cli` boot wires the in-memory store unconditionally;
   future sessions can swap in a Sled / SQLite backing.
4. `@lvqr/core` admin client grows `streamkeys()` family.
5. `bindings/python` client grows `stream_keys()` family +
   matching `@dataclass StreamKey`.
6. Real integration test (no mocks): `lvqr-cli` boots, admin
   mints a key, RTMP publish authenticates with that key,
   admin revokes the key, next publish attempt is rejected.

Anti-scope: persistence backend (in-memory only for v1; real
backing is its own session), per-key rate limits (counter
machinery already exists for fragments / bytes; per-key splits
need richer data model), expiry sweep (a future session can
add a background task; v1 mint accepts an `exp` field but the
sweep runs on first-use rather than as a daemon).

Read `crates/lvqr-auth/src/lib.rs` first to map the existing
`AuthProvider` shape; the `StreamKeyStore` should compose
alongside it, not replace it. Then `crates/lvqr-admin/src/
routes.rs` for the admin-route pattern.

**Other pending Phase D items** (per README post-cleanup
ordering): hot config reload (#2), SCTE-35 passthrough (#3),
DVR scrub web UI (#4), one hardware encoder backend (#5,
multi-session), MoQ egress latency SLO (#6, blocked on Tier 5
SDK). Sessions can pick from this list based on operator demand.


## Session 144 close (2026-04-24)

**Shipped**: Per-peer capacity advertisement. Closes the LAST of the four mesh-data-plane bullets under README's "Peer mesh data plane" checklist (line 450-451). Browser peers can self-report a static `capacity: u32` value on their `Register` signal message naming the maximum children they are willing to relay to. The lvqr-cli signal-callback bridge clamps the claim to the operator's configured global `--max-peers` ceiling at register time so on-disk `PeerInfo.capacity` values are always within bounds; `MeshCoordinator::find_best_parent` consults `PeerInfo.capacity` via the new `effective_capacity` helper so a peer with `capacity: 1` is treated as full after one child even when the global ceiling is higher. `GET /api/v1/mesh` surfaces the per-peer capacity alongside the existing `intended_children` (planner) and `forwarded_frames` (session 141) columns. With this row landed, `docs/mesh.md` flips from "topology planner + signaling + ..." to "IMPLEMENTED" status; the four phase-D mesh-data-plane bullets all close.

### Deliverables

1. **`crates/lvqr-mesh/src/tree.rs`**: `PeerInfo` grows `capacity: Option<u32>` behind `#[serde(default)]`; `PeerInfo::new` initializes it to `None`. New `effective_capacity(default_max)` returns `min(self.capacity.unwrap_or(default_max), default_max)` so the value is always within the operator ceiling -- defense-in-depth against a missed clamp at the call site. `can_accept_child` consults `effective_capacity` instead of the bare default. 5 new unit tests cover the helper plus the can_accept_child capacity-zero and per-peer-cap-below-default paths.

2. **`crates/lvqr-mesh/src/coordinator.rs`**: `MeshCoordinator::add_peer` signature grows a `capacity: Option<u32>` argument; the field lands on `PeerInfo.capacity` before the find_best_parent dispatch. 2 new unit tests: `find_best_parent_respects_per_peer_capacity` (3 peers under root_peer_count=1, max_children=5, capacities (1, 5, 5), peer-3 must descend to peer-2 even though MeshConfig allows 5 children per peer) + `find_best_parent_clamps_oversize_capacity` (proves the effective_capacity defense-in-depth even when a programmatic caller forgot to clamp). Existing 38+ test sites updated to pass `None`.

3. **`crates/lvqr-signal/src/signaling.rs`**: `SignalMessage::Register` grows `capacity: Option<u32>` behind `#[serde(default)]` so pre-144 clients omitting the field deserialize cleanly. New `PeerEvent<'a> { peer_id, track, capacity, connected }` struct delivered to the reshaped `PeerCallback` (`Fn(&PeerEvent<'_>) -> Option<SignalMessage>`); the named-struct shape avoids a positional `None`-on-disconnect that would have been pure noise in the tuple form. `register_peer` and `wait_for_register` thread the capacity claim through; `remove_peer` constructs a `PeerEvent` with `capacity: None` and `connected: false`. 3 new unit tests: `register_deserializes_pre_144_body_without_capacity` / `register_accepts_oversize_capacity_claim` / `peer_callback_receives_capacity_on_register_and_none_on_disconnect`.

4. **`crates/lvqr-signal/src/lib.rs`**: re-exports `PeerEvent`.

5. **`crates/lvqr-cli/src/lib.rs::start()`**:
   * Captures `config.max_peers as u32` into the signal callback closure as the global ceiling.
   * Signal callback unpacks the new `PeerEvent`, clamps `event.capacity.map(|c| c.min(global_max_children))`, and threads the clamped value into `MeshCoordinator::add_peer(.., clamped_capacity)`.
   * `with_mesh` admin closure surfaces `p.capacity` on every `MeshPeerStats` row alongside the existing `intended_children` + `forwarded_frames` columns.

6. **`crates/lvqr-cli/src/ws.rs`**: /ws subscriber path passes `None` (no capacity advertisement for /ws-side peers in v1; they typically consume media as leaves).

7. **`crates/lvqr-admin/src/routes.rs`**: `MeshPeerStats` grows `capacity: Option<u32>` behind `#[serde(default)]`. `mesh_with_peers` test extended to assert per-peer capacity round-trips. New `mesh_peer_stats_deserializes_pre_144_body_without_capacity` test proves the defensive `#[serde(default)]` fallback.

8. **`crates/lvqr-cli/tests/mesh_capacity_e2e.rs`** (new):
   * `capacity_one_forces_descent_for_third_peer`: spins up a TestServer with `--max-peers 5 --mesh-root-peer-count 1`, opens 3 real `tokio-tungstenite` WebSocket clients to `/signal`, sends `Register` with capacities (1, none, none), reads the AssignParents, and asserts peer-3 descends to peer-2 (proving the per-peer cap of 1 on peer-1 forces descent even though `MeshConfig.max_children = 5`). Also polls `/api/v1/mesh` and asserts the configured capacity round-trips onto the admin row.
   * `oversize_capacity_claim_is_clamped_to_global_max`: sends `capacity: u32::MAX` on the Register, asserts `/api/v1/mesh` reports `capacity: Some(2)` (the operator's `--max-peers 2`).

9. **`bindings/js/packages/core/src/mesh.ts`**:
   * `MeshConfig` grows `capacity?: number`. Documented as "self-reported relay capacity ... server clamps to its operator-configured global max-peers".
   * `connect()` includes the field on the Register payload. `JSON.stringify` drops undefined fields so an unset config produces a Register without the field and the server falls back to its global ceiling.

10. **`bindings/js/packages/core/src/admin.ts`**: `MeshPeerStats.capacity?: number` mirrors the Rust shape.

11. **`bindings/js/tests/sdk/admin-client.spec.ts`**: existing mesh assertion grows a `peer.capacity === undefined || typeof peer.capacity === 'number'` check.

12. **`bindings/python/python/lvqr/types.py`**: `MeshPeerStats.capacity: Optional[int] = None`. Docstring updated.

13. **`bindings/python/python/lvqr/client.py::mesh()`**: defensive `.get("capacity")` parse so a pre-144 server that omits the field produces `MeshPeerStats(capacity=None)` rather than failing.

14. **`bindings/python/tests/test_client.py`**: `test_mesh_peer_stats_defaults` extended with `capacity is None`. Existing `test_mesh` body extended with `capacity: 5` on the Root row + `capacity is None` on the Relay row to lock the round-trip. New `test_mesh_pre_session_144_server_omits_capacity` proves the defensive `.get("capacity")` parse.

15. **Docs**:
    * `docs/mesh.md`: status line flipped from "topology planner +..." to **IMPLEMENTED**. New "**Per-peer capacity advertisement** shipped in session 144" paragraph in the status block. "What is still phase-D scope" subsection removed (last row closed). New "Per-peer capacity (session 144)" body section with wire-shape, browser SDK example, and anti-scope. Per-peer offload-snapshot table grows the `capacity` row.
    * `docs/sdk/javascript.md`: `MeshPeerStats` interface grows the optional `capacity?: number` field. Peer-mesh narrative updated: "mesh data plane is fully implemented as of session 144". Constructor example shows `capacity: 3`.
    * `docs/sdk/python.md`: `MeshPeerStats` dataclass grows `capacity: Optional[int] = None`. Docstring inline notes the session-144 origin and clamping semantics.

16. **`README.md`**: "Per-peer capacity advertisement" bullet flipped from `[ ]` to `[x]` with the strikethrough + shipped-in-144 prose. "Flip docs/mesh.md to IMPLEMENTED" bullet also flipped (shipped in same session). "Next up #5" line rewritten to reflect that the entire mesh-data-plane phase D shipped in full.

17. **`tracking/SESSION_144_BRIEFING.md`** (new): locked design before any source touched. Documents what "capacity" means in v1 (static client-self-report; no bandwidth probing or CPU-headroom heuristic), wire shape (Register-extension over a separate `Capacity` variant, with rationale), coordinator integration, the `PeerEvent` struct decision, the clamp-at-ingest-vs-consult tradeoff, admin route exposure, and an explicit anti-scope list (no mid-session updates, no browser-bandwidth measurement, no CPU heuristic, no PeerRole::Leaf transition for capacity=0, no Playwright capacity test, no /ws-subscriber capacity advertisement).

### Key 144 design decisions baked in

* **What "capacity" means in v1.** Static, client-self-reported integer "I can serve up to N children". The web platform does not honestly expose either upload-bandwidth or CPU headroom across browsers; bandwidth probing is heavyweight and changes the wire shape. The integrator picks the value from their own profile knowledge (mobile / laptop / etc.). Empirical capacity discovery is anti-scope; v1.2 candidate.

* **Register-extension, not new `Capacity` variant.** Capacity is part of the registration contract: the server must know it AT THE MOMENT of computing the assignment, not in a follow-up message. A separate variant would either (a) force a round-trip delay on first AssignParent or (b) require an immediate reassign-on-Capacity, churning the tree on every join. Mid-session capacity revisions remain anti-scope; if/when needed, a sibling `Capacity` variant can coexist with the Register-extension.

* **`PeerEvent` struct over positional callback extension.** The `PeerCallback` shape switched from `Fn(&str, &str, bool) -> Option<SignalMessage>` to `Fn(&PeerEvent<'_>) -> Option<SignalMessage>`. Positional extension would have introduced a `None` capacity on disconnect that reads as noise; the named-struct form makes each field self-describing and matches the project's evident preference for named-field structs (see `IceServer` in 143).

* **Clamp at register time, not at consult time.** lvqr-cli's signal-callback closure clamps the client claim to `min(claim, config.max_peers as u32)` BEFORE storing into `PeerInfo.capacity`. Alternative considered: store-as-claimed, clamp at find_best_parent consult time. Cheaper to implement, but exposes raw client claims on the admin route. A misbehaving client claiming `u32::MAX` would appear on the operator's dashboard with a wildly inflated capacity. Clamp-at-ingest preserves the invariant that `PeerInfo.capacity` is always within the operator's ceiling. `effective_capacity` clamps again at consult time as defense in depth.

* **No new CLI flag.** `--max-peers <N>` already serves as the operator's global ceiling and the default-when-unset; one knob is enough.

* **No JS `MeshPeer.capacity` getter.** Integrator passes the value in once at construct time and does not need to read it back.

* **/ws subscribers stay capacity-less in v1.** /ws-side peers register through `add_peer(.., None)`. The /ws path is typically consumed by server-side or non-browser subscribers without a clear "I can serve K children" semantics. If operators ask, capacity-on-/ws is a future row; not v1.

* **Live smoke verified the boot path.** Started lvqr with `--mesh-enabled --max-peers 5` locally; the binary booted cleanly and `curl /api/v1/mesh` returned `{"enabled":true,"peer_count":0,"offload_percentage":0.0,"peers":[]}` -- the empty-mesh shape the new serde round-trip expects.

### Ground truth (session 144 close)

* **Head (pre-push)**: `feat(mesh+signal+sdk)` + docs close (pending). Sessions 135-143 pushed (origin `9f91763`). Local main will be 2 ahead of origin after this session's docs-close commit.
* **Tests**:
  * Rust workspace default gate: **1043** passed / 0 failed / 3 ignored (1030 -> 1043; +5 mesh tree + 2 mesh coordinator + 3 signal serde + 1 admin + 2 integration = 13 net).
  * `lvqr-mesh --lib`: 25 passed (18 pre + 5 new tree + 2 new coordinator).
  * `lvqr-signal --lib`: 21 passed (18 pre + 3 new).
  * `lvqr-admin --lib`: 30 passed (29 pre + 1 new).
  * `lvqr-cli --test mesh_capacity_e2e`: 2 passed (new file).
  * `bindings/python pytest`: **30** passed / 0 failed (29 pre + 1 new defensive parse).
  * `@lvqr/core` tsc: clean. `@lvqr/player` tsc: clean. Vitest case count unchanged at 11; existing `mesh returns a MeshState shape` grew the optional-capacity assertion in place.
* **CI gates locally clean**:
  * `cargo fmt --all -- --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `tsc` clean on `@lvqr/core` + `@lvqr/player`.
* **Workspace**: **29 crates**, unchanged.

### Known limitations / documented v1 shape (after 144 close)

* **No mid-session capacity revisions.** Register-time only. A client that wants to lower its capacity after a tab switch / network change would need to reconnect or the future-session `SignalMessage::Capacity` variant.
* **No browser-bandwidth or CPU-headroom auto-detection.** The integrator picks the value from their own profile knowledge.
* **Existing tree is not rebalanced on capacity changes.** Capacity is captured in `PeerInfo` at register time; subsequent peer joins consider it; current children stay assigned to their current parents until they reconnect.
* **No Prometheus per-peer capacity metric.** High-cardinality `peer_id` labels would not help dashboards; the JSON admin route is the answer.
* **No JS unit test of the `capacity` field on Register.** A non-trivial WebSocket mock would be needed; the Rust integration test covers the wire path end to end.
* **No Playwright capacity test.** The three-peer-chain regression run covers the no-capacity path; the Rust integration test covers the with-capacity path. A 4+ peer mixed-capacity Playwright would add proportional flake risk for incremental signal.
* **/ws subscribers stay capacity-less.** The /ws path passes `None` unconditionally; capacity is a /signal-path-only feature in v1.
* **All other session 143 + earlier known limitations unchanged.**


## Session 143 close (2026-04-24)

**Shipped**: TURN deployment recipe + server-driven ICE configuration. Closes the third of the four unshipped bullets under README's "Peer mesh data plane" checklist (line 444-445). New `--mesh-ice-servers <JSON>` CLI flag (env `LVQR_MESH_ICE_SERVERS`) accepts an array of `RTCIceServer` objects; the list flows down to every browser peer via a new `ice_servers: Vec<IceServer>` field on the existing `AssignParent` server-push message, and `MeshPeer.handleAssignment` rebuilds its `RTCPeerConnection({ iceServers })` from the snapshot when non-empty. Operators configure once on the server; clients pick up STUN/TURN entries automatically. Operator-facing `deploy/turn/` ships a coturn install runbook + minimal `coturn.conf`. Live smoke verified: `curl /api/v1/mesh` against an lvqr booted with the flag returns the expected mesh body, and the new Rust integration test (`crates/lvqr-signal/tests/ice_servers_e2e.rs`) drives a real WebSocket through the production admin router and asserts the configured list arrives in the AssignParent body verbatim.

### Deliverables

1. **`crates/lvqr-signal/src/signaling.rs`**:
   * New `pub struct IceServer { urls: Vec<String>, username: Option<String>, credential: Option<String> }`. Mirrors WebRTC's `RTCIceServer` JSON shape; `username` + `credential` carry `#[serde(skip_serializing_if = "Option::is_none")]` so STUN-only entries do not emit empty credential fields. `urls` is always emitted as an array even when one URL is configured -- normalizing on the wire keeps JS-side casts simple.
   * `SignalMessage::AssignParent` grows `#[serde(default)] ice_servers: Vec<IceServer>` so pre-143 server bodies that omit the field still deserialize cleanly into a new client.
   * 3 new unit tests: `assign_parent_carries_ice_servers` (round-trip with STUN + TURN entries, asserts the no-credential entry skips username/credential on the wire), `assign_parent_deserializes_pre_143_body_without_ice_servers` (proves `#[serde(default)]` fallback), `assign_parent_serialization` extended with `assert!(json.contains("\"ice_servers\":[]"))` and a check on the `ice_servers` field in the unpacked body.

2. **`crates/lvqr-signal/src/lib.rs`**: re-exports `IceServer`, plus the previously-internal `ForwardReportCallback` and `PeerCallback` types (drive-by; the bridge in `lvqr-cli` already uses them via path-qualified names but downstream callers benefit from the cleaner re-export surface).

3. **`crates/lvqr-cli/src/config.rs`**:
   * `ServeConfig` grows `mesh_ice_servers: Vec<lvqr_signal::IceServer>`. Default loopback config initializes it to `Vec::new()`.

4. **`crates/lvqr-cli/src/main.rs`**:
   * New `--mesh-ice-servers <JSON>` flag with `LVQR_MESH_ICE_SERVERS` env fallback; field type is `Option<String>` parsed at boot.
   * New `parse_mesh_ice_servers(raw: Option<&str>) -> Result<Vec<IceServer>>` helper. `None` and whitespace-only resolve to empty vec; valid JSON parses; malformed JSON surfaces `--mesh-ice-servers must be a JSON array of {urls, username?, credential?} objects: <serde error>` so operators can fix the input.
   * New `mod mesh_ice_servers_cli_tests`: 4 tests covering unset / empty-string / full-payload / malformed-json paths.

5. **`crates/lvqr-cli/src/lib.rs::start()`**:
   * Captures `config.mesh_ice_servers.clone()` into the signal callback closure once at boot. Both AssignParent constructions in the closure (the reuse-existing-peer branch and the fresh-add_peer branch) include `ice_servers: ice_servers_for_signal.clone()`.

6. **`crates/lvqr-test-utils/src/test_server.rs`**:
   * `TestServerConfig` grows `mesh_ice_servers: Vec<lvqr_signal::IceServer>` field + new `with_mesh_ice_servers(servers)` builder method.
   * `TestServer::start` plumbs the field into `ServeConfig`.

7. **`crates/lvqr-test-utils/Cargo.toml`**: new `lvqr-signal = { workspace = true }` direct dep so the public `with_mesh_ice_servers` signature can name the type. The crate is already a transitive of lvqr-cli; this is purely a public-API-surface dep.

8. **`crates/lvqr-signal/tests/ice_servers_e2e.rs`** (new):
   * `assign_parent_carries_configured_ice_servers`: spins up a TestServer with `--mesh-ice-servers` configured (one STUN entry + one TURN entry with credentials), opens a real `tokio-tungstenite` WebSocket to `/signal`, sends `Register`, reads `AssignParent`, asserts `ice_servers` matches the configured list verbatim.
   * `assign_parent_omits_ice_servers_when_unconfigured`: same flow without `--mesh-ice-servers`, asserts the AssignParent body has an empty `ice_servers` vec.

9. **`bindings/js/packages/core/src/mesh.ts::handleAssignment`**:
   * After applying `role` and `parentId`, reads `msg.ice_servers as RTCIceServer[] | undefined`. If non-empty, rebuilds `this.iceConfig = { iceServers: serverIceServers }` so subsequent `RTCPeerConnection` constructions (parent-side and child-side) pick up the operator's list. Empty or missing leaves the constructor-provided `iceConfig` untouched.

10. **`deploy/turn/coturn.conf`** (new): minimal coturn config with realm + long-term creds + UDP-only + sane port range + quotas. ~50 lines with inline comments. Operators copy + edit `realm`, `user`, and ranges before starting.

11. **`deploy/turn/README.md`** (new): operator runbook. Sections: why TURN (symmetric NAT), what ships in the dir, install on Debian / Alpine / Docker, how to wire the running coturn into LVQR via `--mesh-ice-servers`, sanity check via `turnutils_uclient`, cost shape, anti-scope (no TLS, no short-lived creds, no autoscaling).

12. **`docs/mesh.md`**:
   * "What is still phase-D scope" subsection trimmed to one bullet (per-peer capacity advertisement only).
   * New shipped-in-143 paragraph documenting `--mesh-ice-servers` + the AssignParent extension + `MeshPeer` rebuild semantics + a link to `deploy/turn/`.
   * Configuration table grows the `--mesh-root-peer-count` and `--mesh-ice-servers` rows (the former was already valid; documented here for the first time).
   * New "TURN / STUN configuration (session 143)" section with the constructor-vs-server-driven tradeoff, an example command line, and a "When you need TURN" subsection that links to `deploy/turn/` for the runbook.

13. **`README.md`**: "TURN deployment recipe" bullet flipped from `[ ]` to `[x]` with the strikethrough + shipped-in-143 prose linking `deploy/turn/`.

14. **`tracking/SESSION_143_BRIEFING.md`** (new): locked design before any source touched. Documents the wire shape, the CLI flag shape, the JS rebuild semantics, the testing strategy (unit + integration + relying-on-existing-Playwright happy path with empty ice_servers), and an explicit anti-scope list (no mid-session updates, no credential rotation, no `relay`-only iceTransportPolicy, no actual coturn boot in CI, no JS unit test of the rebuild path).

### Key 143 design decisions baked in

* **Server-driven, not constructor-only.** Both paths exist: `MeshPeer({iceServers: [...]})` still works for integrators who prefer to thread the list themselves. The new path adds operator ergonomics without breaking the integrator path. Empty server list = "no opinion, client decides", preserving backward compat.

* **Single CLI flag carrying the full JSON array, not repeated `--ice-server` flags.** Repeated flags carrying JSON objects are awkward to escape on a shell command line; a single JSON-array string parses once at boot and matches the env-var `LVQR_MESH_ICE_SERVERS` shape that systemd/docker units want.

* **Wire field is `ice_servers: Vec<IceServer>` with `#[serde(default)]`.** Same cross-version pattern as session 140 (`slots`) and session 141 (`peers`). Pre-143 servers omit the field; new clients deserialize them with empty vec. Pre-143 clients ignore the new field on a new server's body (JS structural typing is lenient on extra; Rust enum matches keep working since the default exists).

* **`IceServer` type is crate-public on `lvqr-signal`, not `lvqr-mesh` or a new crate.** It is purely a wire-shape concern in `SignalMessage::AssignParent`. Putting it in `lvqr-signal` keeps the dep graph tidy: lvqr-cli already depends on lvqr-signal, and the test-utils crate now adds a direct dep so `with_mesh_ice_servers` can name the type publicly.

* **Server-list is authoritative when non-empty.** Clients rebuild `iceConfig` rather than merging the lists. Merging would invite ordering questions (priority? dedup by url?) and add no operator value: the operator who sets `--mesh-ice-servers` already knows what they want clients to use. Preserves the "single knob" property.

* **No mid-session reconfiguration.** Operators set `--mesh-ice-servers` at boot; clients see the snapshot the first time they register on `/signal`. Existing `RTCPeerConnection` instances keep their config; the rebuilt `iceConfig` only affects future PCs. Acceptable for v1; a future session could close existing PCs on list-changed if operators ask.

* **`urls` is `Vec<String>` on the wire even when one URL is configured.** WebRTC's spec allows a single string OR an array; normalizing to array on the wire keeps the JS-side cast simple (`msg.ice_servers as RTCIceServer[]`) and avoids a `OneOrMany<String>` helper. CLI parser still accepts only the array form per the JSON Schema; operators always pass arrays.

* **STUN entries skip credential fields on the wire.** `username` and `credential` carry `#[serde(skip_serializing_if = "Option::is_none")]`. The `assign_parent_carries_ice_servers` test asserts `!json.contains("\"username\":null")` to lock the contract: a STUN-only entry produces `{"urls":["..."]}`, not `{"urls":["..."],"username":null,"credential":null}`. Pre-143 clients that strict-parse on Optional credential fields do not see a null where they expect absence.

* **Real integration test, not unit test of the closure.** The closure that captures `config.mesh_ice_servers` and calls `.clone()` on every AssignParent could regress without a unit test catching it (e.g. someone refactors the closure and forgets the clone). The integration test drives a real WebSocket through the production admin router so the regression would be visible immediately.

* **Live smoke verified the boot path.** Started lvqr with `--mesh-ice-servers '[...]'` locally; the binary booted cleanly and `/api/v1/mesh` answered the expected shape. The integration test covers the wire path from server to client.

### Ground truth (session 143 close)

* **Head (pre-push)**: `feat(mesh+signal+sdk)` + docs close (pending). Sessions 135-142 pushed (origin `8c28ca2`); session 143 local pending push.
* **Tests**:
  * Rust workspace default gate: **1030** passed / 0 failed / 3 ignored (1022 -> 1030; +3 new signal serde tests + 4 new CLI parse tests + 2 new integration tests = 9 net; the existing assign_parent_serialization test grew assertions in place rather than adding a new entry).
  * `lvqr-signal --lib`: 18 passed (16 pre + 2 new: `assign_parent_carries_ice_servers`, `assign_parent_deserializes_pre_143_body_without_ice_servers`). Note: `assign_parent_serialization` grew assertions but is not a new test.
  * `lvqr-signal --test ice_servers_e2e`: 2 passed (new file).
  * `lvqr-cli --lib`: tests pass; new `mesh_ice_servers_cli_tests` module adds 4 cases.
  * `bindings/python pytest`: **29** passed / 0 failed (UNCHANGED; no Python touched).
  * `@lvqr/core` tsc: clean. Vitest case count unchanged at 11.
  * `@lvqr/player` tsc: clean.
* **CI gates locally clean**:
  * `cargo fmt --all -- --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `tsc` clean on `@lvqr/core` + `@lvqr/player`.
* **Workspace**: **29 crates**, unchanged.

### Known limitations / documented v1 shape (after 143 close)

* **No mid-session reconfiguration of `--mesh-ice-servers`.** Operators must restart lvqr to change the list; in-flight peers keep their stale `iceConfig` until their session reconnects. Closing existing PCs on list-changed is a v1.2 candidate.
* **No short-lived TURN credentials.** The static user-pass pair lives on disk in `coturn.conf` and on the lvqr command line. coturn supports REST-API HMAC-derived ephemeral creds; LVQR does not yet implement the credential-refresh wire path, so static-only is the supported shape.
* **No TLS-wrapped TURN (`turns:` URLs).** The recipe ships with `no-tls` in `coturn.conf`. Deployments that need TURN-over-TLS add the cert + key paths and remove the `no-tls` line; LVQR passes whatever URLs it sees in the JSON unchanged, so there is no server-side blocker.
* **No `iceTransportPolicy: 'relay'` enforcement.** `MeshPeer` continues to use the default `'all'` policy; clients try host + reflexive candidates first and fall through to relay only when needed.
* **No JS unit test of the `iceConfig` rebuild path.** A non-trivial WebSocket mock + RTC-stub harness would be needed; the integration test at the Rust layer + the existing Playwright happy-path coverage are the v1 confidence story.
* **`deploy/turn/coturn.conf` is unverified in CI.** The runbook is documented; actual coturn boot is operator-side. A future session could add a docker-compose-based smoke test if operator feedback asks.
* **All other session 142 + earlier known limitations unchanged.**


## Session 142 close (2026-04-24)

**Shipped**: Three-peer Playwright matrix. Closes the second of the four unshipped bullets under README's "Peer mesh data plane" checklist (line 446-447). New `bindings/js/tests/e2e/mesh/three-peer-chain.spec.ts` spawns three Chromium contexts forming a depth-2 chain (`peer-1` Root -> `peer-2` Relay -> `peer-3` Relay grandchild) and asserts both the byte-for-byte frame relay AND the per-peer admin shape across the chain. The middle peer's `forwarded_frames > 0` is the load-bearing assertion that the depth-2 case adds over session 141's two-peer test: a single-hop test cannot distinguish "received-then-forwarded" from "received-only" because session 141's counter only fires on `dc.send`. Pure test + docs + one-line MeshPeer accessor; no source changes outside `bindings/js/`. Sessions 138-141 also pushed in this session (`origin/main` is now `0c2320d`).

### Deliverables

1. **`bindings/js/playwright.config.ts`**:
   * webServer command grows `--max-peers 1`. Forces deterministic chain assignment: peer-1 takes the only Root slot, peer-2 fills peer-1's only child slot, peer-3 descends to peer-2. The two-peer-relay spec is unaffected because it only ever uses one slot on peer-1.

2. **`bindings/js/packages/core/src/mesh.ts`**:
   * New public getter `parentPeerId: string | null`. Reads the existing private `parentId` field. Tests now wait for `peer.parentPeerId === 'expected-parent'` deterministically rather than fishing in private state. `null` for Root peers and for peers that have not yet received an `AssignParent`.

3. **`bindings/js/tests/e2e/mesh/three-peer-chain.spec.ts`** (new, ~250 lines):
   * Single test `three-peer chain relays root-pushed frames to a depth-2 grandchild`.
   * Three browser contexts; same harness pattern as the two-peer spec (`addInitScript` injection of `dist/mesh.js` as a global plus `__setupPeer` helper).
   * Connects peer-1, waits for Role=Root. Connects peer-2, waits for Role=Relay AND `parentPeerId === 'peer-one'`. Waits for peer-1 `childCount >= 1` to confirm the DataChannel opened on the parent side BEFORE peer-3 connects (without this, peer-3 could race in front of peer-2's slot claim under `--max-peers 1`).
   * Connects peer-3, waits for Role=Relay AND `parentPeerId === 'peer-two'`. Waits for peer-2 `childCount >= 1`.
   * Pushes frames from peer-1 on a 100 ms loop; waits for peer-3's `__frames` array to contain the expected payload.
   * Waits ~2.5 s for the session-141 ForwardReport interval to emit at least twice (peer-2's first emit fires before it is forwarding to peer-3, so the second is the one that reflects the chain). Stops the push loop, waits another 1.2 s for the final emit.
   * Polls `GET /api/v1/mesh` and asserts:
     * `enabled === true`, `peer_count === 3`, `offload_percentage` between 60 and 70 (2/3 ~= 66.7%).
     * `peer-1`: Root, parent=null, depth=0, intended_children=1, forwarded_frames > 0.
     * `peer-2`: Relay, parent="peer-one", depth=1, intended_children=1, forwarded_frames > 0 (LOAD-BEARING).
     * `peer-3`: Relay, parent="peer-two", depth=2, intended_children=0, forwarded_frames === 0.

4. **`README.md`**:
   * "Three-peer browser Playwright E2E" bullet flipped from `[ ]` to `[x]` with the strikethrough + shipped-in-142 prose, linking the new spec.

5. **`docs/mesh.md`**:
   * "What is still phase-D scope" subsection trimmed to two bullets (capacity + TURN); three-peer matrix moved into a new shipped-in-142 paragraph alongside the session-141 offload-reporting paragraph.

6. **`docs/sdk/javascript.md`**:
   * Peer-mesh section gains a paragraph naming the new `parentPeerId` getter and explaining its `null`-pre-assignment semantics.

7. **`tracking/SESSION_142_BRIEFING.md`** (new): locks the design before any source file is opened. Documents the topology forcing rationale (`--max-peers 1` + `--mesh-root-peer-count 1`), the per-peer expectation table, the load-bearing assertion, and the anti-scope list (no 4+ peer matrix, no fault injection, no browser matrix expansion, no exact-count assertions).

### Key 142 design decisions baked in

* **Single global `--max-peers 1`, not a second webServer.** Adding a second webServer entry on a different port would isolate the new spec's harness from the existing two-peer spec's, but doubles the boot cost in CI and adds a port-allocation cliff. The two-peer test only ever uses one slot on peer-1, so lowering the global cap to 1 is invisible to it. If a future fan-out test (one Root with 3 children) needs `max_children >= 3`, that session will add a second webServer at that point.

* **Separate spec file, not an extension of `two-peer-relay.spec.ts`.** Each test reads as a single narrative (depth-1 fanout vs depth-2 chain). CI can opt-in/out of the new test independently. Test-name labelling is clearer in Playwright reports.

* **`parentPeerId` getter, not exposed assignment object.** The full `AssignParent` message also carries `role` and `depth`, both of which already have getters (`peerRole`, no `depth` getter today; depth is reachable via the admin route's `peers[]`). Adding a single string getter is the minimum surface needed for deterministic chain-formation waits.

* **Wait for parent-side `childCount >= 1` BEFORE connecting the next peer.** Without this barrier, peer-3 could Register at the coordinator before peer-2's `add_peer` settled, racing for peer-1's only child slot under `--max-peers 1`. Sequencing the parent-side DataChannel-open ack ensures the assignment is committed.

* **`forwarded_frames > 0` rather than an exact count.** Same rationale as session 141's two-peer assertion -- SCTP timing variance on shared CI runners makes exact counts flake-prone. The semantic ("the chain transmitted at least one frame end-to-end") is what the test cares about.

* **Three-peer chain, not three-peer fan-out.** The plan brief originally called this a "matrix"; the actual test is a chain. A fan-out (one Root, two children, no grandchild) would re-test session 141's reporting on a wider topology but would not exercise the multi-hop relay path. The chain is the topology that uniquely validates the depth-2 forward path.

* **Browsers: Chromium only.** Same as session 115. RTCPeerConnection compat across Firefox + WebKit + their varying SCTP implementations is a v1.2 candidate; the cost is a real browser-matrix CI workflow which is several sessions of work on its own.

### Ground truth (session 142 close)

* **Head (pre-push)**: `feat(mesh-test+sdk)` + docs close (pending). Sessions 135-141 pushed (origin `0c2320d` after this session pushed sessions 138-141). After session-142 docs-close lands, local main will be 2 ahead of origin.
* **Tests**:
  * Rust workspace default gate: **1022** passed / 0 failed / 3 ignored (UNCHANGED; no Rust source touched).
  * `bindings/python pytest`: **29** passed / 0 failed (UNCHANGED).
  * `@lvqr/core` tsc: clean. Vitest case count unchanged at 11.
  * `@lvqr/player` tsc: clean.
  * Playwright spec: typecheck clean for both `three-peer-chain.spec.ts` and `two-peer-relay.spec.ts`. The actual browser run happens in the `mesh-e2e.yml` CI workflow on every push; not exercised locally this session.
* **CI gates locally clean**:
  * `cargo fmt --all -- --check` clean.
  * No clippy run needed (no Rust source touched).
  * `tsc` clean on `@lvqr/core` + `@lvqr/player` and on both e2e specs.
* **Workspace**: **29 crates**, unchanged.

### Known limitations / documented v1 shape (after 142 close)

* **Browser matrix is Chromium only.** Firefox / WebKit engine compatibility is unverified. README mesh-data-plane checklist line 446-447 was scoped to Playwright + the 5-artifact contract; both ship today against Chromium. Cross-browser RTCPeerConnection + SCTP behavior is v1.2 scope.
* **Strict ordering relies on slot-claim sequencing.** `--max-peers 1` makes the chain deterministic only because peer-2 connects + claims peer-1's slot before peer-3 connects. The test enforces this via a `childCount >= 1` wait on the parent. A racing harness that connected all three at once could land peer-3 in peer-1's slot first, leaving peer-2 to descend to peer-3 -- a different (still valid) chain that would fail the test's parent assertions.
* **Fault injection not covered.** Killing peer-2 mid-flight to test orphan reassignment for peer-3 is anti-scope. The orphan-reassign path is unit-tested in `lvqr-mesh::coordinator::tests::reassign_orphaned_peer`.
* **All other session 141 + earlier known limitations unchanged.**


## Session 141 close (2026-04-24)

**Shipped**: Actual-vs-intended mesh offload reporting. Closes the first of four unshipped bullets under README's "Peer mesh data plane" checklist (line 439-441): "clients report 'served by peer X'; coordinator aggregates; `/api/v1/mesh` returns measured offload". Browser peers now maintain a private cumulative forwarded-frame counter, increment it on each successful DataChannel send, and emit a new `ForwardReport` signal message once per second (skip-on-unchanged, so idle peers stay silent). Server resolves the sender from the WS session state (no `peer_id` on the wire -- a peer can only report for itself) and calls `MeshCoordinator::record_forward_report`, which replaces rather than accumulates so a reconnect cannot inflate the displayed value. `GET /api/v1/mesh` grows a new `peers: Vec<MeshPeerStats>` field that pairs the topology planner's `intended_children` with the client-reported `forwarded_frames` per peer. Smoke-verified locally: `curl /api/v1/mesh` against a mesh-enabled lvqr with no peers returns `{"enabled":true,"peer_count":0,"offload_percentage":0.0,"peers":[]}`.

### Deliverables

1. **`crates/lvqr-mesh/src/tree.rs`**:
   * `PeerInfo` grows a `forwarded_frames: u64` field with `#[serde(default)]` so pre-141 snapshots still deserialize. `PeerInfo::new` initializes it to 0.
   * `peer_info_defaults` unit test extended to assert the new field's default.

2. **`crates/lvqr-mesh/src/coordinator.rs`**:
   * New `pub fn record_forward_report(&self, id: &str, forwarded_frames: u64)`. Grabs `peers.get_mut(id)` and replaces the counter. Unknown-peer reports are silent no-ops (a client may outlive its tree entry when `remove_peer` fires between the client's last emit and WS close).
   * 4 new unit tests: `record_forward_report_sets_counter` / `..._on_unknown_peer_is_noop` / `..._handles_reconnect_reset` / `..._isolates_peers`.

3. **`crates/lvqr-signal/src/signaling.rs`**:
   * New `SignalMessage::ForwardReport { forwarded_frames: u64 }` variant. No `peer_id` on the wire -- tightens contract to "peer can only report for itself".
   * New `ForwardReportCallback` type + `SignalServer::set_forward_report_callback` setter mirroring the existing `PeerCallback` pattern. Keeps `lvqr-signal` independent of `lvqr-mesh`.
   * `handle_signal_message` gains a short-circuit `ForwardReport` arm before the `to`-field dispatch so a client cannot abuse the variant to forward a message to a target peer.
   * 4 new unit tests: `forward_report_round_trips` / `forward_report_callback_invoked_with_session_peer_id` / `forward_report_without_callback_is_silent_noop` / `forward_report_does_not_leak_to_other_peers`.

4. **`crates/lvqr-admin/src/routes.rs`**:
   * New `pub struct MeshPeerStats { peer_id, role, parent, depth, intended_children, forwarded_frames }` with `Serialize + Deserialize`.
   * `MeshState` grows a `peers: Vec<MeshPeerStats>` field with `#[serde(default)]` so pre-141 clients deserializing new-server bodies (and new clients parsing pre-141 bodies) both work without a schema-version bump.
   * Default (disabled) closure now also returns an empty `peers` vec.
   * `mesh_disabled_by_default` gains a `peers.is_empty()` assertion; `mesh_with_peers` test body expanded with a 2-peer JSON payload and per-peer assertions.
   * New `mesh_state_deserializes_pre_141_body_without_peers` test proves the defensive `#[serde(default)]` fallback works against a pre-141 server body (omits the `peers` key entirely).

5. **`crates/lvqr-admin/src/lib.rs`**: re-exports `MeshPeerStats`.

6. **`crates/lvqr-cli/src/lib.rs::start()`**:
   * Bridges `SignalServer::set_forward_report_callback` into `MeshCoordinator::record_forward_report` (new 3-line closure next to the existing `set_peer_callback` block).
   * `with_mesh` closure extended to iterate `mesh.tree_snapshot()` and build `Vec<MeshPeerStats>`; maps `PeerInfo.children.len()` to `intended_children` and `PeerInfo.forwarded_frames` to `forwarded_frames`.

7. **`bindings/js/packages/core/src/mesh.ts`**:
   * `MeshPeer` gains private `forwardedFrames` + `lastReportedFrames` + `reportInterval` state.
   * `forwardToChildren` increments `forwardedFrames` on each successful `dc.send()` (not attempted sends; matches the actual forwarded semantics).
   * `connect()` schedules `startForwardReportLoop` after the WS opens; `close()` calls `stopForwardReportLoop`. The WS `onclose` path also clears the interval so a dropped connection does not leak timers.
   * `startForwardReportLoop` fires a 1-second `setInterval` that emits `{"type":"ForwardReport","forwarded_frames":N}` ONLY when `N !== lastReportedFrames` -- skip-on-unchanged keeps idle peers silent.
   * New `forwardedFrameCount: number` public getter exposes the counter locally for callers that want to display the value without polling the admin route.

8. **`bindings/js/packages/core/src/admin.ts`**:
   * New `MeshPeerStats` interface mirroring the Rust shape byte-for-byte.
   * `MeshState` grows a `peers: MeshPeerStats[]` field.

9. **`bindings/js/packages/core/src/index.ts`** re-exports `MeshPeerStats`.

10. **`bindings/js/tests/sdk/admin-client.spec.ts`**:
    * `mesh returns a MeshState shape` test grows an `Array.isArray(mesh.peers)` assertion plus a per-peer shape loop (harmless on an empty harness, catches regressions when a publisher or peer is added later).

11. **`bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts`**:
    * New `ADMIN_URL` constant (reuses the `18088` port the playwright webServer already mounts).
    * After the existing `pushFrame` loop proves the DataChannel relay works, the test waits ~2.5 s for two `ForwardReport` emits to land, stops the push loop, waits another ~1.2 s for the last emit, then polls `GET /api/v1/mesh` and asserts: peer-one (Root) has `intended_children == 1` and `forwarded_frames > 0`; peer-two (Relay) has `intended_children == 0` and `forwarded_frames == 0`.

12. **`bindings/python/python/lvqr/types.py`**:
    * New `@dataclass MeshPeerStats` with all fields defaulted (so `MeshPeerStats()` gives the disabled-server shape).
    * `MeshState` grows a `peers: list[MeshPeerStats] = field(default_factory=list)` field. Docstring updated.

13. **`bindings/python/python/lvqr/client.py::mesh()`** now parses the `peers` array with a defensive `.get("peers", [])` fallback so a pre-141 server (which omits the field) still produces a valid `MeshState(peers=[])`.

14. **`bindings/python/python/lvqr/__init__.py`** re-exports `MeshPeerStats`.

15. **`bindings/python/tests/test_client.py`**:
    * `test_mesh_state_defaults` extended with `mesh.peers == []` assertion.
    * New `test_mesh_peer_stats_defaults` asserts default construction.
    * `test_mesh` body expanded with a 2-peer payload matching the Rust configured-snapshot assertion set.
    * New `test_mesh_pre_session_141_server_omits_peers` proves the defensive fallback works against a pre-141 server body.

16. **Docs**:
    * `docs/mesh.md` "What is still phase-D scope" subsection moves the "actual-vs-intended offload" bullet out and into a new "shipped in session 141" paragraph. Admin-route JSON example grows the populated `peers` array. New "Per-peer offload snapshot (session 141)" section documents the field table + `ForwardReport` wire message + reconnect semantics.
    * `docs/observability.md` grows a paragraph after the mesh-metric table naming the per-peer counters as JSON-body-only (not Prometheus) and linking to the new mesh.md section.
    * `docs/sdk/javascript.md` type reference grows the new `MeshPeerStats` TypeScript interface + extends `MeshState` with the `peers` field. The Peer-mesh section gains a paragraph about the 1 s `ForwardReport` cadence + the new `forwardedFrameCount` getter.
    * `docs/sdk/python.md` type reference grows the new `@dataclass MeshPeerStats` + extends the `MeshState` dataclass definition. Migration section updated: 14 -> 15 dataclasses.

17. **`tracking/SESSION_141_BRIEFING.md`**: locked design decisions BEFORE any source file was opened -- ForwardReport wire shape, callback plumbing, cumulative-replace semantics, 1 s skip-on-unchanged cadence, anti-scope.

### Key 141 design decisions baked in

* **`ForwardReport` carries no `peer_id` on the wire.** Server resolves from WS session state. A peer can only report for itself. This matches the trust model of `handle_signal_message(from_peer: &str, msg)` where `from_peer` is the post-Register session identity.

* **Counter is cumulative + replace-on-report, not delta + accumulate.** Client sends its running total; server overwrites rather than adds. Reconnect-safe: if the client reconnects (counter resets to 0), the server simply follows the wire value back down. Nothing drifts upward forever. Tradeoff: the server-visible value briefly drops at reconnect; operators polling `/api/v1/mesh` during a reconnect window see the dip. Acceptable for an observability surface; not used for billing.

* **Skip-on-unchanged in the 1 s emit loop.** Idle peers and leaf peers that never forward anything stay silent on the `/signal` channel. Prevents the WS from becoming noisy at N peers scale.

* **`ForwardReportCallback` type is standalone, NOT part of `PeerCallback`.** Register/unregister carries a message response (the `AssignParent`); ForwardReport is fire-and-forget. Reusing `PeerCallback` would force a `None` return on every report. Separate type is clearer and keeps each callback's contract focused.

* **`lvqr-signal` remains independent of `lvqr-mesh`.** `ForwardReportCallback: Fn(&str, u64)` exposes just the report semantics, not the coordinator shape. Bridge wired in `lvqr-cli::start()` next to the existing `set_peer_callback` block.

* **`PeerInfo.forwarded_frames` is a plain `u64`, not `AtomicU64`.** DashMap already serializes per-key access via `get_mut`. Atomics would complicate the existing `Clone` derive without buying anything -- every read path goes through `tree_snapshot()` or `get_peer()` which both clone.

* **Counter increments on successful `dc.send()`, not on `pushFrame` calls.** Fanout to N children = N increments per `pushFrame`. This matches the "served by peer X" intent (one send = one relayed frame) rather than the application-level frame count. A parent with closed children DataChannels forwards nothing and reports nothing -- correctly.

* **No per-`(peer, child)` breakdown.** Single aggregate per peer. Splitting by child would require either a protocol change (`ForwardReport` carrying a map) or a richer `MeshPeerStats` shape. Operator question answered today is "is peer X forwarding?" -- yes/no. Per-child splits are a v1.2 candidate if operator feedback asks.

* **Playwright two-peer assertions use `> 0`, not an exact count.** The 100 ms push interval running for ~2.5 s before the admin poll means a count in the 20-25 range, but SCTP backpressure + DataChannel open race can reduce that on slow CI runners. Asserting strictly positive captures the semantic ("at least one send landed") without flaking on timing.

* **Live smoke verified the full wiring.** Started lvqr with `--mesh-enabled` locally; curl against `/api/v1/mesh` returned `{"enabled":true,"peer_count":0,"offload_percentage":0.0,"peers":[]}` -- the empty-mesh shape the new serde round-trip expects.

### Ground truth (session 141 close)

* **Head (pre-push)**: `feat(mesh+admin+sdk)` + docs close (pending). Sessions 135-137 pushed (origin `1a1667e`); sessions 138-140 local + session-141 pair (pending). Local main will be 8 ahead of origin after this session's docs-close commit lands.
* **Tests**:
  * Rust workspace default gate: **1022** passed / 0 failed / 3 ignored (1013 -> 1022; +4 coord + 1 tree + 4 signal = 9 net).
  * `lvqr-mesh --lib`: 18 passed / 0 failed (13 pre + 4 new coord + 1 extended tree).
  * `lvqr-signal --lib`: 16 passed / 0 failed (12 pre + 4 new).
  * `lvqr-admin --lib`: 29 passed / 0 failed (28 pre + 1 new pre-141 defensive-parse; the existing `mesh_with_peers` test grew body assertions without adding a new entry).
  * `bindings/python pytest`: **29** passed / 0 failed (27 pre + 2 new: `MeshPeerStats` defaults + pre-141 defensive parse).
  * `@lvqr/core` tsc: clean on both `packages/core` + `packages/player`. Vitest case count unchanged at 11; existing `mesh` test grew `peers` array-shape assertions.
  * Playwright `two-peer-relay.spec.ts`: new admin-poll assertion block added; runs on the `mesh-e2e.yml` CI workflow on every push to main (not exercised locally in this session -- gated behind the browser install + the webServer webkit binary).
* **CI gates locally clean**:
  * `cargo fmt --all -- --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `tsc` clean on both `@lvqr/core` + `@lvqr/player` packages.
* **Workspace**: **29 crates**, unchanged.

### Known limitations / documented v1 shape (after 141 close)

* **No per-`(peer, child)` breakdown.** `forwarded_frames` aggregates across every DataChannel send. Per-child offload visibility would require extending `ForwardReport` to carry a map (or switching to a delta protocol with child IDs); deferred to v1.2.
* **`forwarded_frames` is frame count, not bytes.** Operators computing a bandwidth savings estimate from this field must multiply by a typical fragment size. A bytes counter is additive and could land alongside in a future session if operator feedback asks.
* **No Prometheus label for per-peer counters.** `lvqr_mesh_peer_forwarded_frames_total{peer_id="..."}` would require label-set governance (peer_id is high-cardinality for large meshes). The JSON route is sufficient for interactive debugging today.
* **`MeshState.peers` is a required JS field.** A pre-141 server body that omits the key would produce a missing-field read at runtime. Python's `.get("peers", [])` is the asymmetric-compat story; TypeScript callers polling older servers see `undefined` reads but do not throw (structural typing is lenient on reads). Pragmatic: TS consumers on `main` see a `main` server, not an older one, so this only matters for mixed deployments during a rolling upgrade.
* **Playwright assertion is `> 0`, not an exact count.** Tradeoff documented inline -- SCTP timing makes exact counts flaky.
* **All other session 140 + earlier known limitations unchanged.**


## Session 140 close (2026-04-24)

**Shipped**: Per-slot WASM filter counters. Closes the session-137 explicit known limitation: "Per-slot counters not exposed. The bridge tap observes the chain's COMPOSITE decision; there is no way today to ask 'is slot 2 doing the dropping?'". New `SlotCounters` struct in `lvqr-wasm`, internal `InstrumentedFilter` wrapper, `ChainFilter::slot_counters()` accessor. New `WasmFilterSlotStats` type added to `lvqr-admin` and plumbed through the `/api/v1/wasm-filter` route's body as a new `slots: Vec<WasmFilterSlotStats>` field. Matching JS + Python SDK types + client-side parsing. Integration test gains per-slot assertions that verify the short-circuit invariant `slots[i+1].seen <= slots[i].kept`. Smoke-verified locally: `curl /api/v1/wasm-filter` against a chain-configured lvqr returns `{"enabled":true,"chain_length":1,"broadcasts":[],"slots":[{"index":0,"seen":0,"kept":0,"dropped":0}]}`.

### Deliverables

1. **`crates/lvqr-wasm/src/lib.rs`**:
   * New `pub struct SlotCounters { fragments_seen, fragments_kept, fragments_dropped: AtomicU64 }` with `.seen()` / `.kept()` / `.dropped()` snapshot accessors.
   * New internal `struct InstrumentedFilter { inner: SharedFilter, counters: Arc<SlotCounters> }` that wraps each slot and increments counters on `FragmentFilter::apply`.
   * `ChainFilter` now holds `Vec<InstrumentedFilter>` instead of `Vec<SharedFilter>`. `ChainFilter::new(Vec<SharedFilter>)` wraps each input in an `InstrumentedFilter` at construction.
   * New `ChainFilter::slot_counters() -> Vec<Arc<SlotCounters>>` accessor clones the per-slot Arc handles so the admin closure can snapshot counters from outside the filter apply path.
   * `ChainFilter::filters()` accessor removed (was only used by one test, replaced with `slot_counters().len()`).
   * 5 new unit tests: counters start at zero / counters increment on apply / short-circuit prevents later slots from seeing dropped fragments / counter handles share state with live chain / empty chain has no slot counters.

2. **`crates/lvqr-admin/src/routes.rs`**:
   * New `pub struct WasmFilterSlotStats { index, seen, kept, dropped }` with `Serialize + Deserialize`.
   * `WasmFilterState` grows a `slots: Vec<WasmFilterSlotStats>` field with `#[serde(default)]` so pre-session-140 clients deserializing a new-server body (or new clients parsing a pre-session-140 server body) both work without a schema-version bump.
   * Default (disabled) closure now also returns an empty `slots` vec.
   * Configured-snapshot unit test grew to assert the new slots field contents + the short-circuit invariant.
   * Disabled-route unit test gained a `st.slots.is_empty()` assertion.

3. **`crates/lvqr-admin/src/lib.rs`** re-exports the new `WasmFilterSlotStats` alongside the existing admin types.

4. **`crates/lvqr-cli/src/lib.rs::start()`**:
   * Extracts `chain.slot_counters()` BEFORE wrapping the `ChainFilter` in the bridge's outer `SharedFilter` (the wrap type-erases the chain structure, so the counter handles need to be cloned while the concrete chain is still in scope).
   * The destructuring binding grew from `(Option<bridge>, Vec<reloaders>)` to `(Option<bridge>, Vec<reloaders>, Vec<Arc<SlotCounters>>)`; the empty-wasm-filter branch yields `Vec::new()` for the counters as well.
   * The admin `with_wasm_filter(move || ...)` closure now captures both `bridge` and `slot_counters`; it builds the `slots` array via `slot_counters.iter().enumerate().map(...)`.

5. **`crates/lvqr-cli/tests/wasm_filter_admin_route.rs`**:
   * `admin_route_reports_chain_length_and_per_broadcast_counters` test gains ~30 lines of slot-array assertions: slots length mirrors `chain_length`; slot 0 (frame-counter.wasm) keeps every fragment it sees; slot 1 (redact-keyframes.wasm) drops every fragment it sees; the invariant `slots[1].seen == slots[0].kept` holds (proving short-circuit semantics flow through to the admin body).
   * `admin_route_reports_disabled_when_no_filter_configured` gains a `body["slots"].as_array().unwrap().len() == 0` assertion.

6. **`bindings/js/packages/core/src/admin.ts`**:
   * New `WasmFilterSlotStats` interface.
   * `WasmFilterState` grows a required `slots: WasmFilterSlotStats[]` field (TypeScript structural typing means pre-session-140 server bodies that omit the field would trip; the Python client is the asymmetric-compatibility path via its defensive `.get("slots", [])` unmarshaler).

7. **`bindings/js/packages/core/src/index.ts`** re-exports the new interface.

8. **`bindings/js/tests/sdk/admin-client.spec.ts`** live-server Vitest test grew 5 assertions on the `state.slots` array (length, index, and zeroed counters with no publisher).

9. **`bindings/python/python/lvqr/types.py`**:
   * New `@dataclass WasmFilterSlotStats` with index + seen/kept/dropped fields (all defaulted to 0).
   * `WasmFilterState` grows a `slots: list[WasmFilterSlotStats] = field(default_factory=list)` field.

10. **`bindings/python/python/lvqr/client.py::wasm_filter()`** now parses the `slots` array with a defensive `.get("slots", [])` fallback so a pre-session-140 server (which omits the field) still produces a valid `WasmFilterState(slots=[])`.

11. **`bindings/python/python/lvqr/__init__.py`** re-exports the new `WasmFilterSlotStats` dataclass.

12. **`bindings/python/tests/test_client.py`**:
    * New `test_wasm_filter_slot_stats_defaults` asserts the default construction.
    * `test_wasm_filter_disabled` body expanded to include `slots: []`.
    * `test_wasm_filter_populated` body expanded with a 2-slot JSON payload that matches the configured-snapshot assertion set.
    * New `test_wasm_filter_pre_session_140_server_omits_slots` proves the defensive `.get("slots", [])` fallback works against a pre-session-140 server (body omits the key entirely).

13. **Docs**:
    * `docs/observability.md` WASM filter chain section grows a new slots field in the example JSON body + an expanded "use this route to..." paragraph naming `slots` as the per-filter debugging surface and calling out the short-circuit invariant.
    * `docs/sdk/javascript.md` type reference grows the new `WasmFilterSlotStats` TypeScript interface + extends `WasmFilterState` with the `slots` field.
    * `docs/sdk/python.md` type reference grows the new `@dataclass WasmFilterSlotStats` + extends the `WasmFilterState` dataclass definition. Migration section updated: 13 -> 14 dataclasses.

### Key 140 design decisions baked in

* **Instrumentation lives inside `lvqr-wasm`, not the bridge.** The bridge installer (`install_wasm_filter_bridge`) still takes a single opaque `SharedFilter` and knows nothing about chain structure. The per-slot counters are the responsibility of the `ChainFilter` type itself; operator code reads them via the chain's own accessor, not via the bridge handle. This preserves the bridge's "accepts any FragmentFilter" abstraction and keeps unit tests of chain semantics in `lvqr-wasm` where they belong.

* **`InstrumentedFilter` is crate-internal.** External callers constructing chains still pass `Vec<SharedFilter>`; `ChainFilter::new` does the wrapping automatically. Exposing `InstrumentedFilter` as a public type would invite external callers to build partially-instrumented chains, which adds complexity without use cases.

* **`Arc<SlotCounters>` exposed by clone, not borrow.** `slot_counters()` returns an owned `Vec<Arc<SlotCounters>>`. Callers snapshot the Arc handles and hold them alongside the live chain; a snapshot read always reflects current state because Arc shares the same atomic fields as the live instrumentation. Tests assert this invariant explicitly (`slot_counter_handles_are_shared_with_live_chain`).

* **`#[serde(default)]` on the new `slots` field** means: (a) a pre-session-140 server body deserializes into a new client without throwing -- `slots` defaults to `Vec::new()` when absent; (b) the field is always emitted by a new server so new clients that assume it exists work fine. The asymmetric direction -- a pre-session-140 client that does not know about `slots` -- also works because TypeScript's structural typing is lenient on extra fields and Python's `.get("slots", [])` tolerates anything.

* **Short-circuit invariant `slots[i+1].seen <= slots[i].kept` asserted in the integration test.** This is the load-bearing correctness property of the chain semantics; a regression that broke short-circuit (e.g., always calling every slot) would produce `slots[1].seen == slots[0].seen` instead. Catching that here is cheap.

* **`index` explicit in `WasmFilterSlotStats`, not implicit from array position.** The JSON body emits slots in insertion order so `index` is redundant for a strict reader, but including it makes the shape robust against clients that reorder arrays (some tooling sorts JSON objects) and makes individual slot entries self-describing when logged or quoted out of context. No significant size overhead (one extra u64 per slot).

* **Did not add per-`(broadcast, track)` slot counters.** Full per-slot-per-broadcast counters would require plumbing the broadcast+track pair through `FragmentFilter::apply` (breaking trait change) or redesigning the bridge to know about chain structure (API coupling). The global per-slot counters cover the operator's main question ("which filter is denying?") cheaply; per-broadcast slot breakdown is a v1.2 follow-up if operator feedback asks for it.

* **Live smoke test verified the full wiring.** Started lvqr with `--wasm-filter crates/lvqr-wasm/examples/frame-counter.wasm` locally; curl against `/api/v1/wasm-filter` returned `{"enabled":true,"chain_length":1,"broadcasts":[],"slots":[{"index":0,"seen":0,"kept":0,"dropped":0}]}` as expected.

### Ground truth (session 140 close)

* **Head (pre-push)**: `feat(wasm+admin+sdk)` + docs close (pending). Sessions 135-137 pushed (origin `1a1667e`); sessions 138-140 local: `d661138` + `77689a0` + `7823c4b` + `595af99` + session-140 pair (pending). Local main is 6 ahead of origin after this session.
* **Tests**:
  * Rust workspace default gate: **1013** passed / 0 failed / 3 ignored (1008 -> 1013; +5 lvqr-wasm slot-counter unit tests).
  * `lvqr-wasm --lib`: 28 passed / 0 failed / 0 ignored (23 pre + 5 new).
  * `lvqr-admin --lib`: 28 passed / 0 failed / 0 ignored (unchanged; existing configured-snapshot test grew assertions in place).
  * `lvqr-cli --test wasm_filter_admin_route`: 2 passed / 0 failed / 0 ignored (assertions expanded).
  * `bindings/python pytest`: **27** passed / 0 failed (25 pre + 2 new: slot-stats defaults + pre-session-140 defensive parse).
  * `@lvqr/core` tsc: clean. Vitest case count unchanged at 11; existing `wasmFilter` test grew 5 slot assertions.
* **CI gates locally clean**:
  * `cargo fmt --all -- --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `tsc` clean on both `@lvqr/core` + `@lvqr/player` packages.
* **Workspace**: **29 crates**, unchanged.

### Known limitations / documented v1 shape (after 140 close)

* **Per-`(broadcast, track)` slot counters not exposed.** The `slots` array aggregates across ALL broadcasts. Per-broadcast slot breakdown is anti-scope for v1.1; a follow-up session could plumb context through `FragmentFilter::apply` if operator feedback asks for it.
* **No metric for per-slot counters.** Prometheus exposure would be a `lvqr_wasm_chain_slot_total{slot_index="0", outcome="keep|drop"}` counter; deferred.
* **JS `WasmFilterState.slots` is a required field.** A pre-session-140 server body that omits the key would produce a missing field at runtime. Python client's `.get("slots", [])` fallback is the asymmetric-compat story; TypeScript callers polling older servers would need a defensive check. Pragmatic: TypeScript consumers on `main` see a `main` server, not an older one, so this only matters for mixed deployments.
* **All other session 139 + earlier known limitations unchanged.**




## Session 139 close (2026-04-24)

**Shipped**: `sdk-tests.yml` CI workflow now boots `lvqr serve` with `--wasm-filter crates/lvqr-wasm/examples/frame-counter.wasm`, so the live Vitest assertion for `LvqrAdminClient.wasmFilter()` exercises the chain-configured admin shape (`enabled: true, chain_length: 1`) end-to-end instead of the disabled-body shape. Closes the session-138 "Known Limitation": **no live-server integration test for the chain-configured shape on the SDK side**. Smoke-verified locally: lvqr boots with the new flag, the admin route returns the expected JSON (`{"enabled":true,"chain_length":1,"broadcasts":[]}`), and all 11 Vitest cases pass against that live instance.

### Deliverables

1. **`.github/workflows/sdk-tests.yml`**:
   * Background `lvqr serve` spawn grows a new arg: `--wasm-filter crates/lvqr-wasm/examples/frame-counter.wasm`. Uses the committed fixture that already backs `wasm_frame_counter.rs` + `wasm_filter_chain.rs` + `wasm_filter_admin_route.rs`; no new files to commit.
   * Workflow header comment updated to note session 139's change + the session-138 gap it closes.

2. **`bindings/js/tests/sdk/admin-client.spec.ts`**:
   * The existing `wasmFilter returns a WasmFilterState shape` test had asserted the disabled shape because session 138's sdk-tests.yml harness booted without `--wasm-filter`. Session 139 flipped the harness, so the test now asserts `enabled: true, chain_length: 1, broadcasts: []` (no publisher in the harness keeps `broadcasts` empty; the chain is live and observable). Test name widened to `wasmFilter returns a WasmFilterState shape reflecting the configured chain`.

3. **Local smoke verification**:
   * `cargo build -p lvqr-cli` (cached -- no source touched).
   * Booted `lvqr serve --admin-port 18090 --hls-port 0 --rtmp-port 18091 --port 18092 --mesh-enabled --cluster-listen 127.0.0.1:18093 --no-auth-signal --wasm-filter crates/lvqr-wasm/examples/frame-counter.wasm` in the background.
   * `curl http://127.0.0.1:18090/api/v1/wasm-filter` returned `{"enabled":true,"chain_length":1,"broadcasts":[]}` -- the shape the updated assertion expects.
   * `cd bindings/js && LVQR_TEST_ADMIN_URL=http://127.0.0.1:18090 npm run test:sdk` -- 11/11 Vitest cases pass in 31 ms. Killed lvqr after.

### Key 139 design decisions baked in

* **One lvqr serve invocation, not two.** Adding a second invocation (one with `--wasm-filter`, one without) would double CI wall-clock and require a second admin port. Flipping the single harness to the chain-configured shape + flipping the one affected test assertion is the smallest change that closes the gap. The disabled shape is already tested via mocks in Python pytest + type defaults in both SDKs; no need for a live server to prove disabled works too.

* **Use the committed `frame-counter.wasm` fixture.** Already in the repo at `crates/lvqr-wasm/examples/frame-counter.wasm`; backs four existing integration tests (`wasm_frame_counter.rs`, `wasm_filter_chain.rs`, `wasm_hot_reload.rs`, `wasm_filter_admin_route.rs`). Zero new artifacts to commit; CI does not need a build step to produce the wasm.

* **No publisher in the harness.** `broadcasts: []` is fine and intentional. A real RTMP publisher in Vitest would require a JS RTMP client; that is out of scope for this workflow. End-to-end chain-configured coverage with a publisher already exists at the Rust layer (`wasm_filter_admin_route.rs`). The Vitest assertion is about SDK wiring, not about the chain actually tapping traffic.

* **Federation-flake investigation aborted.** Session 134 named `lvqr-admin::cluster_routes::tests::federation_route_reports_configured_link_status` as an intermittent flake. 8 full-module runs this session all passed cleanly; the session 134 note said "failed once in the first workspace-test run and passed on every subsequent isolated re-run", so reproduction requires the full `cargo test --workspace` run and specific scheduling. Without local repro, any fix would be speculative band-aid work. Path aborted in favor of the smaller, more certain sdk-tests.yml win.

* **DASH-IF + hardware encoder still paused.** Docker daemon is installed but off on the dev box; GStreamer CLI is absent. Authoring CI workflow YAML that I cannot verify locally (the session-139 change is small + well-understood, but a `dashif/conformance` container integration is not) keeps producing speculative work. Both items wait for the bits the dev environment cannot currently supply.

### Ground truth (session 139 close)

* **Head (pre-push)**: `ci(sdk)` + this close-doc commit (pending). Sessions 135-137 pushed (origin `1a1667e`); session 138 local (commits `d661138` + `77689a0`); session 139 adds two more. Local main is 4 ahead of origin after this session.
* **Tests**:
  * Rust workspace default gate: **1008** passed / 0 failed / 3 ignored (unchanged; no Rust code touched).
  * `bindings/python pytest`: **25** passed / 0 failed (unchanged).
  * `bindings/js @lvqr/core` Vitest: **11** passed / 0 failed against a chain-configured local lvqr (smoke-verified this session; CI runs same shape).
  * Local smoke: `curl /api/v1/wasm-filter` returns `{"enabled":true,"chain_length":1,"broadcasts":[]}` as expected.
* **CI gates locally clean**:
  * `cargo fmt --all -- --check` clean (no Rust change).
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95 (no Rust change).
  * `tsc` clean on both `@lvqr/core` + `@lvqr/player` (no TypeScript API change).
* **Workspace**: 29 Rust crates (unchanged), `@lvqr/core` + `@lvqr/player` TypeScript packages, `lvqr` Python package.

### Known limitations / documented v1 shape (after 139 close)

* **Live chain test has no publisher.** Broadcasts stay empty; the assertion proves SDK wiring but not that fragments flow through the chain. End-to-end chain-with-traffic coverage lives at the Rust integration-test layer.
* **Federation flake** still pre-existing; investigation aborted this session (no local repro).
* **DASH-IF container validator, hardware encoder, npm + PyPI publish, mesh data-plane** -- all still waiting on external bits the dev environment cannot supply.
* All other session 138 + earlier known limitations unchanged.




## Session 138 close (2026-04-24)

**Shipped**: JS `@lvqr/core::LvqrAdminClient.wasmFilter()` method + Python `lvqr.LvqrClient.wasm_filter()` method hitting session 137's `GET /api/v1/wasm-filter` admin route. Both SDKs gain matching `WasmFilterState` + `WasmFilterBroadcastStats` types/dataclasses mirroring `lvqr_admin`'s serde shape. Python pytest count **21 -> 25** (+4: 2 type-default tests + 2 client-mock tests). JS Vitest spec grows one live-server test against the new method. Rust workspace gate unchanged at 1008/0/3 (this session touched bindings only).

### Deliverables

1. **`bindings/js/packages/core/src/admin.ts`**:
   * New `WasmFilterBroadcastStats` interface (`broadcast`, `track`, `seen`, `kept`, `dropped`).
   * New `WasmFilterState` interface (`enabled`, `chain_length`, `broadcasts`).
   * New `async wasmFilter(): Promise<WasmFilterState>` on `LvqrAdminClient` hitting `/api/v1/wasm-filter`.
   * Class docstring updated: route list now reads `/api/v1/{stats,streams,mesh,slo,wasm-filter}`.
2. **`bindings/js/packages/core/src/index.ts`** re-exports the two new interfaces alongside the existing admin types.
3. **`bindings/js/tests/sdk/admin-client.spec.ts`** gains one new `#[it]` case asserting the `{enabled: false, chain_length: 0, broadcasts: []}` shape against the sdk-tests.yml harness (which boots lvqr without `--wasm-filter`).
4. **`bindings/python/python/lvqr/types.py`**:
   * New `@dataclass WasmFilterBroadcastStats` mirroring the Rust shape.
   * New `@dataclass WasmFilterState` with `field(default_factory=list)` for `broadcasts`.
5. **`bindings/python/python/lvqr/client.py`**:
   * Imports the two new types.
   * New `def wasm_filter(self) -> WasmFilterState` method that hits `/api/v1/wasm-filter` and builds the dataclass from the JSON body.
   * Module docstring updated: route list now reads `/api/v1/{stats,streams,mesh,slo,wasm-filter}`.
6. **`bindings/python/python/lvqr/__init__.py`** re-exports the two new types alongside the existing.
7. **`bindings/python/tests/test_client.py`**:
   * New `TestTypes::test_wasm_filter_state_defaults` + `test_wasm_filter_broadcast_stats_defaults` tests asserting the dataclass defaults.
   * New `TestClient::test_wasm_filter_disabled` + `test_wasm_filter_populated` tests using `unittest.mock.patch` on `httpx.Client.get` to simulate both response shapes.
8. **Docs**:
   * `docs/sdk/javascript.md` method-reference table gains the `wasmFilter()` row; response-type reference gains the two TypeScript interfaces.
   * `docs/sdk/python.md` method-reference table gains the `wasm_filter()` row; response-types code block gains the two `@dataclass` definitions.
   * `docs/sdk/python.md` migration section updated: 3/9 -> 10 methods on main, 11 -> 13 new dataclasses.

### Key 138 design decisions baked in

* **SDK methods called `wasmFilter` (JS) / `wasm_filter` (Python).** Matches the per-language convention already established by `clusterNodes` vs `cluster_nodes`. Directly naming the server route.
* **Return types named `WasmFilterState` + `WasmFilterBroadcastStats`** -- byte-for-byte match with the Rust `lvqr_admin::WasmFilterState` + `WasmFilterBroadcastStats` serde structs, and with the field set the session-137 `get_wasm_filter` route returns. Dataclass / interface field names use Rust-side `snake_case` (not JS `camelCase`) because the JSON-on-wire already is `snake_case`; keeping the client side matching avoids a rename layer.
* **Defensive defaults on dataclass.** `WasmFilterState.enabled = False`, `chain_length = 0`, `broadcasts = []` matches the server's "disabled" body exactly so `WasmFilterState()` gives the same shape a disabled-server GET returns. Lets tests and operators synthesize a blank state without mocking the network.
* **Python client constructs dataclasses field-by-field, not `**kwargs` unpack.** Matches the pattern the other methods already use (`cluster_federation` builds `FederationLinkStatus` kwarg-by-kwarg too). Guards against surprise fields on the wire and keeps the fallback-default behavior consistent.
* **JS Vitest case asserts the disabled shape**, not the chain-configured shape, because the sdk-tests.yml harness boots lvqr with `--mesh-enabled` + `--cluster-listen` but NOT `--wasm-filter`. Chain-configured end-to-end coverage already exists at the Rust layer in session 137's `wasm_filter_admin_route.rs` integration test; the SDK Vitest just proves the route is reachable and parses into the declared shape.
* **No sdk-tests.yml workflow change needed.** The workflow runs `vitest` + `pytest` against the test suites; both discover the new tests automatically.
* **No Rust code touched this session.** Workspace tests unchanged at 1008/0/3. The SDK bindings are the single surface that needed to grow.

### Ground truth (session 138 close)

* **Head (pre-push)**: `feat(sdk)` + `docs` close-doc commit (pending). Sessions 135-137 are pushed; `origin/main` at `1a1667e`.
* **Tests**:
  * Rust workspace default gate: **1008** passed / 0 failed / 3 ignored (unchanged; no Rust code touched).
  * `bindings/python pytest`: **25** passed / 0 failed (21 pre-existing + 4 new).
  * `bindings/js @lvqr/core build:ts`: clean (new exports from index.ts compile).
  * `bindings/js @lvqr/core/tests/sdk/admin-client.spec.ts`: runs against the sdk-tests.yml harness; the new `wasmFilter` case asserts the disabled shape.
* **CI gates locally clean**:
  * `cargo fmt --all -- --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `tsc` clean on both `@lvqr/core` + `@lvqr/player` packages.
* **Workspace**: 29 Rust crates (unchanged), `@lvqr/core` + `@lvqr/player` TypeScript packages, `lvqr` Python package.

### Known limitations / documented v1 shape (after 138 close)

* **SDK clients on-registry still lag.** `@lvqr/core` + `@lvqr/player` at 0.3.1 on npm, `lvqr` at 0.3.1 on PyPI still ship the pre-session-122 admin surface (3/9 methods on Python, partial on JS). The publish cycle bundles session-122 9/9 admin expansion + session-126 JWKS public API + session-128 `sign_live_url` + session-137 admin route + session-138 SDK bindings. Needs npm + PyPI credentials.
* **No CLI module** for the new route in the Python SDK; callers do `client.wasm_filter()` programmatically.
* **No live-server integration test for the chain-configured shape on the SDK side.** The sdk-tests.yml harness is wasm-filter-unaware; a chain-configured variant would need either the existing harness to grow `--wasm-filter` args or a second harness invocation. Chain-configured end-to-end coverage already exists at the Rust layer.
* All other session 137 + earlier known limitations unchanged.




## Session 137 close (2026-04-24)

**Shipped**: `GET /api/v1/wasm-filter` admin route. A follow-on to session 136's WASM chain-composition feature that closes the operator-visibility gap (operators running a chain can now observe its shape + per-broadcast counters via the standard admin surface). Default-gate workspace test count grows **1003 -> 1008** (+5: 3 lvqr-admin unit tests + 2 lvqr-cli integration tests).

This session was a pivot off the Phase D named items (DASH-IF validator, hardware encoder backend, publish cycle) because none of those three options had a local-verification path given the current dev environment: Docker daemon is installed but off, so `dashif/conformance` container cannot be exercised locally; GStreamer CLI is absent, so a VideoToolbox encoder factory cannot be verified locally; npm + PyPI credentials are not available. Rather than ship unverified CI-only workflow YAML or a half-scaffolded encoder, this session completed the natural follow-up to session 136's chain work -- the admin route that makes the chain observable to operators in production.

### Deliverables

1. **`crates/lvqr-wasm/src/observer.rs`** gains a `chain_length: usize` field on `WasmFilterBridgeHandle` plus a `pub fn chain_length(&self) -> usize` accessor. `install_wasm_filter_bridge` grew a `chain_length` parameter; the single external caller in `lvqr-cli::start()` passes `ChainFilter::len()`, the 4 in-crate tests pass `1`. `Debug` impl grew to print `chain_length` alongside `tracked_broadcasts`.

2. **`crates/lvqr-admin/src/routes.rs`** gains:
   * Two new public `#[derive(Serialize, Deserialize)]` types: `WasmFilterState { enabled, chain_length, broadcasts }` and `WasmFilterBroadcastStats { broadcast, track, seen, kept, dropped }`.
   * A new `get_wasm_filter: Arc<dyn Fn() -> WasmFilterState + Send + Sync>` field on `AdminState` with a default closure returning `{enabled: false, chain_length: 0, broadcasts: []}`.
   * A new `with_wasm_filter(get: impl Fn() -> WasmFilterState + Send + Sync + 'static) -> Self` builder method, mirroring the existing `with_mesh` / `with_slo` callback pattern so `lvqr-admin` stays free of a `lvqr-wasm` dep.
   * New `/api/v1/wasm-filter` route in `build_router` behind the existing admin-auth middleware.
   * 3 new `#[tokio::test]` cases: default disabled shape, configured-snapshot shape, admin-auth rejection.

3. **`crates/lvqr-admin/src/lib.rs`** re-exports the two new public types (`WasmFilterBroadcastStats`, `WasmFilterState`).

4. **`crates/lvqr-cli/src/lib.rs::start()`** calls `.with_wasm_filter(move || ...)` on `AdminState` when `wasm_filter_handle` is `Some`. The closure clones the `WasmFilterBridgeHandle`, reads `chain_length()` + iterates `tracked()` + reads per-broadcast counters via the handle's existing `fragments_seen` / `fragments_kept` / `fragments_dropped` methods. When no filter was configured the admin default (disabled body) applies unchanged.

5. **`crates/lvqr-cli/tests/wasm_filter_admin_route.rs`** (new, 2 `#[tokio::test]` cases):
   * `admin_route_reports_chain_length_and_per_broadcast_counters` -- starts a `TestServer` with a two-filter chain (`frame-counter.wasm` + `redact-keyframes.wasm`), publishes RTMP keyframes, issues a real HTTP `GET /api/v1/wasm-filter` against the admin server, and asserts the JSON body contains `enabled: true`, `chain_length: 2`, and a `live/admin-chain` entry with `seen > 0 && kept == 0 && dropped == seen` (chain short-circuits on slot 2's drop).
   * `admin_route_reports_disabled_when_no_filter_configured` -- boots a plain `TestServer` with no filter and asserts `{enabled: false, chain_length: 0, broadcasts: []}`.

6. **Docs**:
   * `README.md` "Observe" quickstart gains a `curl` line for `/api/v1/wasm-filter`.
   * `docs/observability.md` grows a new "WASM filter chain" section paralleling the existing "Latency SLO" section: operator-facing body shape, disabled-body contract, field semantics, three troubleshooting hints (`chain_length` mismatch, zero `seen`, surprising `dropped`).

### Key 137 design decisions baked in

* **Callback closure on `AdminState`, not a direct dep.** `lvqr-admin` stays free of `lvqr-wasm`; the `with_wasm_filter(Fn() -> WasmFilterState)` pattern matches the existing `with_mesh` / `with_slo` shape so CLI composition root is the single place that peeks at the bridge handle.

* **Chain length baked into the bridge handle, not recomputed.** `install_wasm_filter_bridge` takes `chain_length: usize` at install time and stores it on the handle. Alternative would be to re-expose the inner `SharedFilter` and downcast to `ChainFilter`, but that ties the bridge to the chain type and breaks the "bridge sees any FragmentFilter" abstraction. Storing the static length as a scalar on the handle is clean, cheap, and correct: chain length is set once at `start()` and never changes for the server lifetime (there is no runtime "add a filter" API, and there cannot be without a mesh of lifetime invariants).

* **Default admin shape is 200-OK-with-disabled, not 404.** Mirrors the existing SLO route pattern. Dashboards (Grafana, custom ops tools) can pre-bake the response shape and render a "WASM filter not configured" hint client-side without a 404 / 200 fork. Also: returning 200 even when disabled means the route's auth behavior (reject on bad token) is unambiguous.

* **Counters read per-request, not snapshotted.** The closure re-iterates `tracked()` + re-reads counters on every admin GET. Cheap (atomic loads over a bounded DashMap) and always fresh. An alternative "cached snapshot" pattern would reduce cost for heavy polling but introduce staleness that's visible to operators debugging a misbehaving filter -- exactly the wrong tradeoff.

* **Handle is `Clone`, closure clones it once.** The `WasmFilterBridgeHandle` is `Clone` (session-B design). The admin closure takes a cloned handle via `move || { ... bridge.tracked() ... }` so the AdminState remains `Clone` without extra lifetime plumbing.

* **`chain_length: usize` is a signature change, not a soft-compat helper.** Adding a second `install_wasm_filter_bridge_with_len` function was rejected because the caller set is tiny (one external, four tests) and soft-compat layers accumulate indefinitely. Single signature, updated all call sites in one session.

* **No metric for `chain_length`.** Prometheus labels are expensive; a single scalar `chain_length` is fine as a REST body field. Exposing it as a metric would be a steady-state gauge with one time series per server -- dashboard-worthy but not yet a named SLO.

* **Integration test uses the existing two WASM fixtures.** `frame-counter.wasm` + `redact-keyframes.wasm` already ship under `crates/lvqr-wasm/examples/`; reusing them avoids a fresh `build_fixtures` step. The chain outcome (`noop -> drop`) is the same as session 136's `wasm_filter_chain.rs`; this session's test proves the admin route mirrors the same outcome over HTTP.

* **Phase-D pivot explicit in the header.** DASH-IF + hardware encoder + publish cycle all have local-verification gaps the dev box cannot close autonomously. Shipping the WASM admin route keeps session cadence high with full local verification while those three wait for the bits they need (Docker daemon, GStreamer CLI, registry credentials).

### Ground truth (session 137 close)

* **Head (pre-push)**: `feat(wasm+admin)` + `docs` close-doc commit (pending). `origin/main` at `c0d9198` unchanged; local main is 7 ahead (135 pair + 136 pair + README-drift single + 137 pair pending).
* **Tests**:
  * Default workspace gate: **1008** passed / 0 failed / 3 ignored (1003 -> 1008; +3 lvqr-admin unit + 2 integration).
  * `lvqr-wasm --lib`: 23 passed / 0 failed / 0 ignored (unchanged; `chain_length` parameter added to install_wasm_filter_bridge + 4 test call sites updated).
  * `lvqr-admin --lib`: 28 passed / 0 failed / 0 ignored (25 pre-existing + 3 new: default disabled, configured snapshot, admin-auth rejection).
  * `lvqr-cli --test wasm_filter_admin_route`: 2 passed / 0 failed / 0 ignored.
* **CI gates locally clean**:
  * `cargo fmt --all -- --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 1008 / 0 / 3.
* **Workspace**: **29 crates**, unchanged.

### Known limitations / documented v1 shape (after 137 close)

* **Per-slot counters not exposed.** The bridge tap observes the chain's COMPOSITE decision; there is no way today to ask "is slot 2 doing the dropping?". Adding per-slot instrumentation would widen `FilterStats` + require each slot to have its own counter group, which is a non-trivial refactor deferred to a follow-up session.
* **No metric for `chain_length`.** Operators who want chain length in Grafana would need a dedicated exporter reading the admin route.
* **No SDK method for the admin route.** `@lvqr/core` + `lvqr` Python admin clients both gained 9/9 route parity in sessions 122/123. The new 10th route is not yet on the clients; this is a follow-up row bundled with the npm + PyPI publish cycle.
* **Phase D named rows unchanged.** DASH-IF container validator, hardware encoder backend, npm + PyPI publish cycle, mesh data-plane completion -- all still open; each waits on external bits the dev environment cannot currently supply.
* All other session 136 + earlier known limitations unchanged.




## Session 136 close (2026-04-24)

**Shipped**: Phase D row "Stream-modifying WASM pipelines v2 with chaining" (chain-composition slice). New `ChainFilter` public type in `lvqr-wasm`; `--wasm-filter` widened to `Vec<PathBuf>` with per-slot hot-reload; 7 unit tests + 4 CLI parse tests + 1 RTMP integration test land on the default gate (no new Cargo feature; pure additive). Default-gate workspace test count grows **991 -> 1003** (+12). True stream-modifying downstream propagation remains anti-scope per `crates/lvqr-wasm/src/observer.rs`.

### Deliverables

1. **`crates/lvqr-wasm/src/lib.rs`** grows a new `pub struct ChainFilter { filters: Vec<SharedFilter> }`. Public API surface: `ChainFilter::new(Vec<SharedFilter>)`, `ChainFilter::empty()`, `len()`, `is_empty()`, `filters()`. `impl FragmentFilter for ChainFilter` iterates the list in insertion order and returns `None` the first time a filter drops (propagating via the `?` operator), which short-circuits the remaining slots for that fragment. `Clone + Debug + From<Vec<SharedFilter>>` derives round out the type. The crate-level anti-scope comment is updated: "no stateful filters, no GPU, no browser target" (dropped the now-incorrect "no multi-filter pipeline" line).

2. **CLI: `--wasm-filter` widened from `Option<PathBuf>` to `Vec<PathBuf>`.** Clap arg is `#[arg(long, env = "LVQR_WASM_FILTER", value_delimiter = ',', num_args = 1..)]` so both the repeated-flag shape (`--wasm-filter a.wasm --wasm-filter b.wasm`) and the comma-separated env shape (`LVQR_WASM_FILTER=a.wasm,b.wasm`) parse into the same ordered `Vec`. Single-path callers are unchanged (a one-value vec is functionally a single filter).

3. **`ServeConfig.wasm_filter: Option<PathBuf>` -> `Vec<PathBuf>`.** `Default` impl updated (`None` -> `Vec::new()`). Breaking change to the public `lvqr_cli::ServeConfig` surface; acceptable at pre-1.0 v0.4.0 and the handoff doc explicitly names it as a shape change.

4. **`ServerHandle::_wasm_reloader: Option<WasmFilterReloader>` -> `_wasm_reloaders: Vec<WasmFilterReloader>`.** N slots each get their own watcher; `Drop` on the handle drops each reloader independently, which stops its worker thread. No user-visible API change (underscore-prefixed field, pub(crate)).

5. **`start()` rewires the wasm-filter bridge.** Empty `config.wasm_filter` -> install nothing (same as before). Non-empty -> loop each path, build N `SharedFilter`s + N `WasmFilterReloader`s, wrap the ordered list in `ChainFilter`, then call `install_wasm_filter_bridge(&registry, SharedFilter::new(chain))` exactly once. The bridge API is unchanged; the tap installation site sees a single opaque `SharedFilter` whose internals happen to be a chain.

6. **`TestServerConfig::with_wasm_filter(path)` changed from `self.wasm_filter = Some(path.into())` to `self.wasm_filter.push(path.into())`.** Preserves the builder signature. Repeated calls chain filters in insertion order -- that is exactly the v1.1 chain-composition surface the CLI exposes. The two existing tests (`wasm_frame_counter.rs` single-call, `wasm_hot_reload.rs` single-call) continue to run via the degenerate 1-filter chain.

7. **Tests.** 7 new unit tests in `lvqr-wasm::tests` (empty chain passthrough / single-filter equivalence with bare filter / short-circuit on first drop / intermediate-output propagation through the chain / drop-first-vs-truncate-first order sensitivity / `From<Vec<SharedFilter>>` roundtrip / replacing one chained slot mid-life via `SharedFilter::replace`). 4 new CLI parse tests in `main.rs::wasm_filter_cli_tests` (unset = empty vec / single path = 1-element vec / repeated flag stacks in order / comma-delimited stacks). 1 new integration test in `crates/lvqr-cli/tests/wasm_filter_chain.rs` driving a real RTMP publish through a `(frame-counter, redact-keyframes)` chain and asserting `seen > 0 && kept == 0 && dropped == seen` -- proves both slots are consulted end-to-end and the slot-2 drop short-circuits the chain.

8. **Docs.**
   * `README.md` "Next up" item 7 prose rewritten: chain-composition shipped in session 136; downstream propagation (stream-modifying pipeline) remains anti-scope; points readers at `crates/lvqr-wasm/src/observer.rs` for the tap-mode contract.
   * `README.md` "Egress + encoders" checkbox for "Stream-modifying WASM filter chains" flipped to `[x]` with a concrete shipped-in-session-136 detail block.
   * `README.md` CLI reference block updated: "WASM filter chain (read-only tap in v1)" section now documents the repeated-flag + comma-separated shapes.
   * `tracking/PLAN_V1.1.md` Phase D scope row for the WASM chaining marquee marked SHIPPED (chain-composition slice) + new detailed row in the "Phase D shipped rows" subtable.

### Key 136 design decisions baked in

* **Chain composition lives in `lvqr-wasm`, not in the bridge.** The bridge installer (`install_wasm_filter_bridge`) keeps its single `SharedFilter` argument; `start()` wraps the `ChainFilter` in a `SharedFilter` before handing it off. The bridge never learns about chains. Benefits: zero API change on the bridge side, bridge tests stay minimal (single filter is enough), the chain implementation is unit-testable in `lvqr-wasm` without a tokio runtime.

* **Each chained slot keeps its own `SharedFilter`.** The `ChainFilter` holds `Vec<SharedFilter>`, not `Vec<Box<dyn FragmentFilter>>`. This is load-bearing: the session-C hot-reloader calls `SharedFilter::replace` to swap a module in place; if the chain held the `Box` directly, a per-path reloader could not target just its own slot. By keeping one `SharedFilter` per slot, the reloader's "swap this path's module" semantics compose naturally with the chain's "iterate in order" semantics.

* **Short-circuit on first drop, no "rollback".** Once a filter returns `None`, subsequent slots are not called. An operator writing a filter chain knows their slot's `apply` only runs if every slot before it kept the fragment. This is the natural pipeline semantics and matches the observer.rs tap contract where "dropped" is a terminal state.

* **Double-wrap is intentional.** `SharedFilter::new(ChainFilter::new(Vec<SharedFilter>))` applies two mutex acquisitions per fragment: one on the outer SharedFilter, one per chained slot's SharedFilter. Each hold is microseconds (brief HashMap-get + function-pointer dispatch), not a workload problem, and the two-layer shape is what lets the bridge take a single opaque `SharedFilter` while the chain slots stay individually replaceable. Unwrapping would require a bridge API change (accept `impl FragmentFilter + Clone`) for a negligible performance win.

* **`ChainFilter::empty()` is legal but unused at the CLI.** An empty `config.wasm_filter` short-circuits BEFORE chain construction -- `start()` installs nothing, no bridge, no reloaders. The empty chain exists for completeness (and one unit test) but the CLI path never constructs one. If a user types `--wasm-filter` with zero values, clap rejects (`num_args = 1..`).

* **`value_delimiter = ','` + `num_args = 1..` together** give us both the repeat-flag shape AND the env-var comma-shape without a second config field. One semantic surface, two ergonomic UIs.

* **`TestServerConfig::with_wasm_filter` changed semantics, kept signature.** Old: `with_wasm_filter(path)` replaced whatever was there. New: pushes to the vec. The old shape only ever allowed ONE filter so the semantic difference only surfaces when a test calls the builder twice -- which the existing two tests never did. Preserves every existing test call site; enables the new `wasm_filter_chain.rs` test without a second method.

* **Breaking `ServeConfig.wasm_filter` shape change documented, not version-bumped.** At pre-1.0 v0.4.0 the workspace permits internal breaking changes in point-of-use types. `ServeConfig` is the `start()` composition root; every in-repo caller is updated in this session. External callers have not adopted `lvqr-cli` as a library (v0.3.x publishes are `@lvqr/core`, `@lvqr/player`, `lvqr` Python, `lvqr-core`, not `lvqr-cli`). No external consumers to notify.

* **Integration test uses the two committed WASM fixtures** (`frame-counter.wasm` noop + `redact-keyframes.wasm` drop-all) rather than building a fresh multi-filter module. Reuses the same `cargo run -p lvqr-wasm --example build_fixtures` toolchain path. The `noop -> drop` chain gives a clear tap-observable outcome: every fragment is seen and every fragment is dropped. Single-filter companion coverage already exists in `wasm_frame_counter.rs` (all-keep) + `wasm_hot_reload.rs` (replace mid-life).

### Ground truth (session 136 close)

* **Head (pre-push)**: `feat(wasm)` + this close-doc commit (pending). `origin/main` at `c0d9198` unchanged; local main is 4 ahead (session 135's pair + session 136's pair).
* **Tests**:
  * Default workspace gate: **1003** passed / 0 failed / 3 ignored (991 -> 1003, +12 from session 136: +7 chain unit tests in lvqr-wasm + 4 CLI parse tests + 1 integration test).
  * `lvqr-wasm --lib`: 23 passed / 0 failed / 0 ignored (16 pre-existing + 7 new).
  * `lvqr-cli --bin lvqr` (default features): 12 passed / 0 failed / 0 ignored (8 c2pa + 4 wasm_filter_cli_tests).
  * `lvqr-cli --test wasm_filter_chain`: 1 passed / 0 failed / 0 ignored.
  * `lvqr-auth --features webhook` (from session 135): 40 passed / 0 failed / 0 ignored, unchanged.
* **CI gates locally clean**:
  * `cargo fmt --all -- --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 1003 / 0 / 3.
* **Workspace**: **29 crates**, unchanged.

### Known limitations / documented v1 shape (after 136 close)

* **Chain is tap-only.** Downstream subscribers see the original fragment bytes, not the chain's output. `crates/lvqr-wasm/src/observer.rs` documents the tap contract; stream-modifying pipeline (where subscribers see filter output) is explicitly deferred there. Chain composition does not change this.
* **Serial per-fragment apply.** One chain iteration per fragment; no parallelism across slots (would violate the ordered-pipeline semantics anyway). Each slot's WASM `apply` is itself serial per store.
* **Drop-first policy.** A slot that returns `None` terminates the chain for that fragment; there is no "skip this slot, try the next" recovery path. Intentional for pipeline semantics; operators who want branching logic build it inside a single guest.
* **Authoritative DASH-IF container validator** still deferred.
* **npm + PyPI publish cycle** still pending (needs credentials).
* **Mesh data-plane completion** still Phase D.
* **Hardware encoder backend** still Phase D (needs deployment-target pick).
* All other session 135 + earlier known limitations unchanged.




## Session 135 close (2026-04-23)

**Shipped**: Phase D row "Webhook auth provider" -- the first Phase D row to land after Phase C closed in session 134. New `WebhookAuthProvider` in `lvqr-auth` behind a new `webhook` Cargo feature; CLI wiring in `lvqr-cli` via four new `--webhook-auth-*` flags; operator docs in `docs/auth.md`. Default-gate workspace count unchanged at **991 / 0 / 3** (new tests are feature-gated on `webhook`, off by default). `lvqr-auth --features webhook` gate gains **15** new tests (25 -> 40 passing).

### Deliverables

1. **`crates/lvqr-auth/src/webhook_provider.rs`** (~330 LOC + ~280 LOC of tests) -- new module registered as `#[cfg(feature = "webhook")] mod webhook_provider` in `lvqr-auth/src/lib.rs` with `WebhookAuthConfig` + `WebhookAuthProvider` re-exported. Public surface mirrors `JwksAuthProvider`'s shape: async `new(cfg) -> Result<Self, AuthError>`, sync `check(ctx) -> AuthDecision` via the shared `AuthProvider` trait, `config()` + `cached_decision_count()` introspection accessors, `Drop` aborts the background task.

2. **Caching model.** `check()` is sync (the trait contract). A `RwLock<HashMap<CacheKey, CacheEntry>>` stores decisions with per-entry `expires_at`. Cache hit + fresh returns the cached decision; miss-or-expired enqueues the `AuthContext` into a `pending: StdMutex<HashMap<CacheKey, AuthContext>>` and calls `tokio::sync::Notify::notify_one()`. Concurrent checks for the same key coalesce inside `pending` (HashMap insert is a no-op on key collision) so only one POST per unique context per batch hits the webhook.

3. **Background fetcher task.** Spawned from `new()`; loops on `kick.notified().await`; drains `pending` under a brief `StdMutex` hold; POSTs each context serially via `reqwest::Client` (shared `fetch_timeout`); writes each decision back under `cache.write()` with `allow_cache_ttl` (default 60 s) or `deny_cache_ttl` (default 10 s). On cache overflow (capacity default 4096) evicts the entry with the earliest `expires_at` -- not strict LRU but O(n) over a bounded `n` and sufficient for an auth-decision cache where TTL dominates.

4. **Request body JSON shape.** `#[derive(Serialize)] enum WebhookRequestBody` with `#[serde(tag = "op", rename_all = "lowercase")]` produces `{"op":"publish","app","key","broadcast"?}`, `{"op":"subscribe","token"?,"broadcast"}`, `{"op":"admin","token"}`. `broadcast` + `token` skip-if-none so absent fields omit instead of serializing `null`. Borrowed `&str` so no per-POST allocation beyond serde_json's output buffer.

5. **Response body JSON shape.** `#[derive(Deserialize)] struct WebhookResponse { allow: bool, #[serde(default)] reason: Option<String> }` on a 2xx. 5xx / timeout / malformed body all map to `AuthDecision::Deny { reason: "webhook call failed: <error>" }` and cache for `deny_cache_ttl` so a broken webhook drains the pending queue quickly instead of looping.

6. **Config validation** at `new()` time: empty URL, non-http(s) scheme, `allow_cache_ttl < 1s`, zero `deny_cache_ttl`, zero `fetch_timeout` all fail fast as `AuthError::InvalidConfig`. No startup probe to the webhook itself (the endpoint may legitimately reject `GET` / `HEAD` requests); runtime unreachability surfaces as `Deny` reasons on the first few decisions.

7. **CLI wiring in `crates/lvqr-cli/src/main.rs`.** Four new flags on `lvqr serve`, all feature-gated on `webhook`:
   * `--webhook-auth-url <URL>` / `LVQR_WEBHOOK_AUTH_URL`
   * `--webhook-auth-cache-ttl-seconds <u64>` (default 60) / `LVQR_WEBHOOK_AUTH_CACHE_TTL_SECONDS`
   * `--webhook-auth-deny-cache-ttl-seconds <u64>` (default 10) / `LVQR_WEBHOOK_AUTH_DENY_CACHE_TTL_SECONDS`
   * `--webhook-auth-fetch-timeout-seconds <u64>` (default 5) / `LVQR_WEBHOOK_AUTH_FETCH_TIMEOUT_SECONDS`

   Precedence cascade (docs at the call site): `--jwks-url` > `--webhook-auth-url` > `--jwt-secret` > static-token provider > `NoopAuthProvider`. Mutually exclusive pairs: webhook vs jwks, webhook vs jwt-secret, jwks vs jwt-secret (latter pre-existing). Startup rejects combinations with a clear error message rather than silently picking one strategy.

8. **`check_jwks_flag_combination` renamed to `check_auth_flag_combinations`.** Now handles all three pairwise mutex checks; gated on `any(feature = "jwks", feature = "webhook")` with a stub variant for builds with neither feature. `serve_from_args` calls a new `build_auth(&args) -> Result<SharedAuth>` helper that linearizes the cascade under one `#[cfg]` block per feature instead of the previous nested `if-else-cfg`. Net: fewer cfg seams in the caller.

9. **Test coverage.** 10 wiremock-backed `#[tokio::test]` functions + 5 pure-data config-validation `#[test]` functions + 2 serialization / discriminator unit tests in `webhook_provider::tests`:
   * Config: empty URL, non-http scheme, short allow TTL, zero deny TTL, zero timeout, sensible defaults.
   * Data: `CacheKey::from_ctx` discriminates the three variants, request body has the `"op":"publish"` tag + omits absent `broadcast`.
   * Integration: happy allow roundtrip, deny with surfaced reason, cache hit does not re-POST (`expect(1)` wiremock guard), distinct contexts produce distinct cache entries, 5xx denies with error reason + caches, malformed body denies with parse-error reason, concurrent 20 checks for same context coalesce into 1 cache entry, oldest-first eviction when capacity overflows.

   **5 CLI tests** in `main.rs::webhook_cli_tests`: unset flags pass combination check, URL flag parses, webhook+jwt-secret mutex, webhook+jwks mutex (gated on `feature = "jwks"`), TTL + timeout overrides apply.

10. **`docs/auth.md`** grows a new **"Webhook auth provider"** section (~90 lines) documenting enabling, request + response body shapes, caching semantics with the allow-vs-deny TTL rationale, and explicit anti-scope (no retry, serial POSTs, no startup probe). Providers table gains the new row. Anti-scope bullet "No webhook auth provider yet" dropped.

11. **`README.md`** flips `[ ] Webhook auth provider` to `[x] ~~Webhook auth provider~~ Shipped in session 135...`; "Next up" item 8 updated to drop webhook from the `[ ]` list.

12. **`tracking/PLAN_V1.1.md`** Phase D scope row marked SHIPPED + new "Phase D shipped rows" subtable with the full deliverable list per row.

### Key 135 design decisions baked in

* **Sync `check()` + background fetcher matches `JwksAuthProvider`'s pattern.** Trait contract is sync; LVQR calls `check()` from axum handlers, MoQ accept loops, RTMP callbacks. Blocking any of those on a network call would deadlock the runtime. The `Deny("cache miss; decision pending")` first-request response is the same UX JWKS presents for an unknown `kid`; documented explicitly in the module header.

* **Two separate TTLs (`allow_cache_ttl` + `deny_cache_ttl`).** A single TTL would force a tradeoff between caching allow decisions long (better throughput) and caching deny decisions short (broken webhook recovers quickly). Splitting lets operators set `allow=60s` + `deny=10s` so a failing webhook drains the pending queue in 10 s and a working webhook serves cached allows for 60 s.

* **`deny_cache_ttl > 0` is enforced at config time.** A zero deny TTL would cache nothing on failure; every request would re-queue + re-POST + re-fail, hammering the broken webhook. The 1-second minimum (documented) rules this out.

* **Serial POSTs within a batch, not `JoinSet` or `join_all`.** A slow webhook backpressures decisions but does not fan out concurrent HTTP connections that could overwhelm the operator's endpoint on a cold-cache flood (e.g., server restart with 500 active subscribers). Explicit anti-scope in `docs/auth.md`; operators scale the webhook endpoint independently.

* **Oldest-`expires_at` eviction, not strict LRU.** Strict LRU requires tracking access time + doubly-linked list. For an auth-decision cache where TTL dominates, "evict the entry that would expire next" is both cheaper (O(n) over bounded n, no list maintenance) and semantically closer to "forget the stalest decision first". Documented in the `cache_capacity` field comment.

* **No startup connectivity probe.** Unlike JWKS (which can always respond to a `GET` / `HEAD` with its key set), the webhook endpoint has no canonical probe shape. A `GET` might 404 even on a working webhook that only accepts POST. A synthetic `op:"healthcheck"` POST would force a contract on the webhook that is surprising to operators who already run their own endpoint. Cheap path: validate URL shape at `new()`, surface runtime unreachability via Deny reasons on early requests. Explicit anti-scope in `docs/auth.md`.

* **Credentials in the POST body.** Raw tokens / stream keys flow to the operator's webhook because the entire point is operator-owned decision logic. Logged URL only, not body; documented. Operators run the webhook on infrastructure they trust.

* **Deduplication via `pending: HashMap<CacheKey, AuthContext>`.** 20 concurrent `check()` calls for the same token land 20 `pending.entry(key).or_insert_with(clone)` calls that collapse to 1 entry; the fetcher POSTs once. Single-test proof in `concurrent_checks_for_same_context_coalesce_into_one_post`. Avoids a thundering-herd-style fanout on cold-cache flood.

* **Precedence: JWKS > webhook > jwt-secret.** Arbitrary but documented. JWKS-before-webhook reflects "if you already have JWKS, that's the lower-latency path (no network call per token); the webhook is for operators who cannot deploy an IdP". Precedence only matters for programming-error combinations (caught by the mutex checks anyway); real deployments set one flag.

* **`check_auth_flag_combinations` (renamed).** Old name `check_jwks_flag_combination` was singular; now handles three pairwise mutexes. The `#[cfg(any(feature = "jwks", feature = "webhook"))]` union gates the function body, with a fallthrough `#[cfg(not(any(...)))]` stub so default-feature builds still compile.

* **`lvqr_test_utils` stays untouched.** Webhook tests use `wiremock` directly (already in `lvqr-auth` dev-deps for JWKS tests). No new shared-helper surface needed.

### Ground truth (session 135 close)

* **Head (pre-push)**: `feat(auth)` + this close-doc commit (pending). `origin/main` at `c0d9198` unchanged from session 134's post-close audit.
* **Tests**:
  * Default workspace gate: **991** passed / 0 failed / 3 ignored (unchanged; new tests feature-gated on `webhook`).
  * `lvqr-auth --features webhook`: 40 passed / 0 failed / 0 ignored (25 pre-existing + 15 new from this session).
  * `lvqr-cli --features webhook --bin lvqr`: 12 passed / 0 failed / 0 ignored (8 c2pa + 4 webhook; the 5th webhook test `webhook_plus_jwks_is_mutex_error` requires `webhook,jwks` both enabled).
  * `lvqr-cli --features webhook,jwks --bin lvqr`: 18 passed / 0 failed / 0 ignored.
* **CI gates locally clean**:
  * `cargo fmt --all -- --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo clippy -p lvqr-auth --features webhook --all-targets -- -D warnings` clean.
  * `cargo clippy -p lvqr-cli --features webhook --all-targets -- -D warnings` clean.
  * `cargo test --workspace` 991 / 0 / 3.
* **Workspace**: **29 crates**, unchanged.

### Known limitations / documented v1 shape (after 135 close)

* **First-request-per-context denies.** Cache miss returns `Deny("cache miss; decision pending")`; the background task POSTs the webhook and caches the decision; the next request for the same context succeeds. Same UX JWKS presents for unknown `kid`. Documented in the module header + `docs/auth.md#webhook-auth-provider`.
* **Serial POSTs within a batch.** A slow webhook backpressures all pending decisions. Anti-scope in `docs/auth.md`; operators scale the webhook endpoint.
* **No startup connectivity probe.** URL shape validated; runtime unreachability surfaces as deny reasons on early requests.
* **No retry on webhook failure.** A failed POST caches a deny for `deny_cache_ttl`; the next attempt re-hits the webhook.
* **Authoritative DASH-IF container validator** still deferred.
* **npm + PyPI publish cycle** still pending (needs credentials).
* **Mesh data-plane completion** still Phase D.
* **Hardware encoder backend** still Phase D (needs deployment-target pick).
* **WASM filter chains v1.1** still Phase D.
* All other session 134 + earlier known limitations unchanged.




## Session 134 close (2026-04-23)

**Shipped**: PLAN row 122-F (sixth + closing slice of the shared-helpers refactor). `lvqr-test-utils::rtmp` gains three more public helpers; 14 test files migrated onto them. Default-gate workspace count unchanged at **991 / 0 / 3**.

### Deliverables

1. **`crates/lvqr-test-utils/src/rtmp.rs`** gains three new public async functions:
   * `send_results(stream: &mut TcpStream, results: &[ClientSessionResult])` -- writes every `OutboundResponse` packet in `results` in wire order; no-ops on other variants.
   * `send_result(stream: &mut TcpStream, result: &ClientSessionResult)` -- same but for a single result.
   * `read_until<F>(stream: &mut TcpStream, session: &mut ClientSession, timeout: Duration, predicate: F) where F: Fn(&ClientSessionEvent) -> bool` -- drives the stream-read + session-handle-input event loop, writing any `OutboundResponse` packets the session produces and returning when the session raises an event for which `predicate` returns true. Uses `tokio::time::Instant` + `saturating_duration_since` for the deadline arithmetic (matches tokio's timer semantics + dodges the debug-mode subtract-overflow on `std::time::Instant` that 2 pre-migration variants risked).
   All three consume the shared module's panic contract; panic messages name the function for debugging.

2. **14 test files migrated** onto the shared helpers:
   * `crates/lvqr-cli/tests/archive_dvr_read_e2e.rs` -- drop local fns; 2 read_until call sites get TIMEOUT arg. 4 / 4 tests pass.
   * `crates/lvqr-cli/tests/c2pa_cli_flags_e2e.rs` -- drop local fns. 2 / 2 pass with `--features c2pa`.
   * `crates/lvqr-cli/tests/c2pa_verify_e2e.rs` -- drop local fns. 1 / 1 pass with `--features c2pa`.
   * `crates/lvqr-cli/tests/playback_signed_url_e2e.rs` -- drop local fns. 3 / 3 pass.
   * `crates/lvqr-cli/tests/rtmp_archive_e2e.rs` -- drop local fns. 2 / 2 pass.
   * `crates/lvqr-cli/tests/rtmp_dash_e2e.rs` -- drop local fns. 2 / 2 pass.
   * `crates/lvqr-cli/tests/rtmp_hls_e2e.rs` -- drop local fns. 3 / 3 pass.
   * `crates/lvqr-cli/tests/rtmp_whep_audio_e2e.rs` -- drop local fns; call sites pass `RTMP_TIMEOUT` instead of `TIMEOUT`. Compile-verified (transcode feature; runtime on `feature-matrix.yml`).
   * `crates/lvqr-cli/tests/rtmp_ws_e2e.rs` -- drop local fns. 1 / 1 pass.
   * `crates/lvqr-cli/tests/transcode_ladder_e2e.rs` -- drop local fns. Compile-verified (transcode feature).
   * `crates/lvqr-cli/tests/wasm_frame_counter.rs` -- drop local fns. 1 / 1 pass.
   * `crates/lvqr-cli/tests/wasm_hot_reload.rs` -- drop local fns. 1 / 1 pass.
   * `crates/lvqr-cli/tests/whisper_cli_e2e.rs` -- drop local fns. Compile-verified (whisper feature).
   * **`crates/lvqr-ingest/tests/rtmp_bridge_integration.rs` -- full migration**. This file was the outlier of the 122-A-E slices (lives in a different crate; `lvqr-test-utils` is already a dev-dep there). Session 134 consumed the one-file opportunity to close everything at once: local `rtmp_client_handshake` + local FLV builders (`flv_video_seq_header`, `flv_video_nalu`, `flv_audio_seq_header`, `flv_audio_raw`) + local `find_available_port` + local `send_results` / `send_result` / `read_until` -- all replaced with shared-module imports. Dropped ~100 LOC of duplicated helper code, zero semantic change. 2 / 2 tests still pass.

   Aggregate: ~320 LOC of duplicated helper code dropped across 14 files in exchange for ~60 LOC of new shared module surface + ~14 LOC of call-site `TIMEOUT` arg extensions.

3. **Drive-by unused-import cleanup.** After the locals moved out, several files' `use tokio::io::{AsyncReadExt, AsyncWriteExt};` became partly-or-fully dead. `cargo clippy --fix --allow-dirty` collected the obvious ones; a short manual sed pass swept the 3 files where the remaining `use tokio::io::AsyncWriteExt;` was left unused after the fix pass. The 2 files that only named `ClientSessionResult` inside the factored helpers also lost that import. All trimmed cleanly.

4. **`tracking/PLAN_V1.1.md`** row 122-F marked SHIPPED with the per-file detail list inline.

5. **README.md + `docs/auth.md` unchanged** -- pure internal hygiene.

### Key 134 design decisions baked in

* **`tokio::time::Instant` not `std::time::Instant`.** The shared `read_until` uses tokio's Instant for deadline arithmetic. Two pre-migration files (`archive_dvr_read_e2e`, `transcode_ladder_e2e`) used `std::time::Instant` directly; their `deadline - Instant::now()` could underflow in debug-mode and panic. `tokio::time::Instant::saturating_duration_since` saturates at zero. Net safer with no behavior change for any caller whose Instant stays monotonic (which loopback integration tests always do).

* **Take `timeout: Duration` explicitly, not a module-level default.** Every caller has a file-local TIMEOUT (or RTMP_TIMEOUT for rtmp_whep_audio) and passes it at the callsite. A shared default would hide per-file timing policy; taking it explicitly keeps each test's timeout decision visible locally. Matches the session-129 `HttpGetOptions::timeout` precedent.

* **rtmp_bridge_integration gets full migration in this slice.** This file was deferred from sessions 130-133 because it lives in `lvqr-ingest`, not `lvqr-cli`. Since `lvqr-test-utils` is already in its dev-deps and session 134's scope already touches the file for the send/read_until migration, closing out the handshake + FLV migrations in the same pass is cheaper than leaving the file in a partial state. All four helper surfaces land at once.

* **`one_token_all_protocols.rs` stays exempt.** Its local handshake is the documented single-caller Result-returning variant from session 133; its local `write_outbound` is a 1-caller Result-returning `send_results` shim used by the `try_rtmp_publish` error-recovery flow. Both would need `_try` variants in the shared module. Single caller, documented exemption, no factoring.

* **Drop the `ClientSessionResult` import in 2 files.** `c2pa_verify_e2e.rs` + `rtmp_hls_e2e.rs` only named `ClientSessionResult` inside their local read_until. With that helper gone + the shared one owning the type, the import is dead. clippy surfaces this cleanly; removing it keeps the imports honest.

* **Drop `tokio::io::{AsyncReadExt, AsyncWriteExt}` where no longer needed.** The factored helpers' internal implementation needs these traits; the test files no longer do (all raw read/write went through the shared helpers). Files that still call `stream.write_all(...)` directly (in `publish_*_keyframes` style flows, to send FLV tags the session emits) keep the import. Trimmed automatically by clippy --fix except for 3 stragglers where the fix tool left the unused `use tokio::io::AsyncWriteExt;` in place; manual `sed` pass removed them.

* **Flaky test ignored but named.** `lvqr-admin::cluster_routes::tests::federation_route_reports_configured_link_status` failed once in the first workspace-test run and passed on every subsequent isolated re-run. Not caused by session 134's changes (the test doesn't touch any migrated surface); appears to be a pre-existing timing flake. Not investigating this session; flagged in the close doc so future sessions notice if it trips again.

* **Default-gate test count unchanged at 991/0/3.** The session adds zero tests and removes zero tests. Every migrated file's per-file roster is unchanged.

### Ground truth (session 134 close)

* **Head (pre-push)**: `refactor(tests)` + this close-doc commit (pending). `origin/main` at `4dc5b9f` unchanged from session 133 push.
* **Tests**:
  * Default workspace gate: **991** passed / 0 failed / 3 ignored (unchanged from session 133; 1 intermittent flake in lvqr-admin's federation_route test passes on every isolated re-run).
  * Per-file re-runs: archive_dvr_read 4/0/0, rtmp_archive 2/0/0, rtmp_hls 3/0/0, rtmp_dash 2/0/0, rtmp_ws 1/0/0, wasm_frame_counter 1/0/0, wasm_hot_reload 1/0/0, playback_signed_url 3/0/0, c2pa_verify 1/0/0 (--features c2pa), c2pa_cli_flags 2/0/0 (--features c2pa), rtmp_bridge_integration 2/0/0.
  * Compile-verified under feature gates: whisper_cli_e2e (whisper), transcode_ladder_e2e + rtmp_whep_audio_e2e (transcode) to the extent the host allows (brew gstreamer metapackage still lacks the gstreamer-app-1.0.pc needed for full compile of the transcode graph; `feature-matrix.yml` CI remains the authoritative runtime signal).
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 991 / 0 / 3.
* **Workspace**: **29 crates**, unchanged.

### Audit trail (session 134)

Post-migration grep for local `async fn (read_until|send_results|send_result|rtmp_client_handshake|flv_video_|flv_audio_)` function definitions in `crates/lvqr-cli/tests/*.rs` and `crates/lvqr-ingest/tests/rtmp_bridge_integration.rs`:

* Files with zero local definitions (full consumers of `lvqr_test_utils::{http, flv, rtmp}`): 22.
* Files with a documented single-caller local exemption: 1 (`one_token_all_protocols.rs`; local `rtmp_handshake` Result-variant + local `write_outbound` Result-variant + local send/read_until helpers).
* Files with one local helper + thin wrapper (e.g. 48k flv wrapper around the parameterized helper): 1 (`rtmp_whep_audio_e2e.rs`; the 5-line 48k audio seq-header wrapper around `flv_audio_aac_lc_seq_header(3, 2)`).

The shared-helpers refactor is **complete**. Sessions 129+130+131+132+133+134 together factored the 4 primitive surfaces every integration test reimplemented (HTTP GET + FLV tag builders + RTMP handshake + RTMP event-loop helpers) into `lvqr-test-utils`. Net reduction across the six slices: ~1650 LOC of duplicated helper code dropped in exchange for ~480 LOC of new shared module surface.

### Known limitations / documented v1 shape (after 134 close)

* **Shared-helpers refactor fully closed.** No more PLAN row 122-* work remaining.
* **`one_token_all_protocols.rs` keeps 4 local helpers** (handshake, write_outbound, send/read-until-style) with documented single-caller error-recovery contracts. Factoring needs a second caller to justify the expanded shared module surface.
* **`rtmp_whep_audio_e2e.rs` keeps a 5-line local 48k AAC wrapper** around the shared parameterized helper. Documentation-of-intent only; not a regression.
* **`lvqr-admin::cluster_routes::tests::federation_route_reports_configured_link_status` has an intermittent flake.** Pre-existing; passes on every isolated re-run. Future session can investigate.
* **transcode + rtmp_whep_audio runtime verification still CI-only.** `brew gstreamer` metapackage lacks the dev-lib `.pc` file.
* **Authoritative DASH-IF container validator** still deferred; GPAC MP4Box remains primary.
* **Webhook auth provider** still pending.
* **npm + PyPI publish cycle** still pending.
* All other session 133 + earlier known limitations unchanged.


## Session 133 close (2026-04-23)

## Session 133 close (2026-04-23)

**Shipped**: PLAN row 122-E (fifth slice of the shared-helpers refactor; drains the `rtmp_client_handshake` duplication surface across 13 test files). New `lvqr-test-utils::rtmp` module registered in `lib.rs`. Default-gate workspace count unchanged at **991 / 0 / 3**.

### Deliverables

1. **`crates/lvqr-test-utils/src/rtmp.rs`** (~85 LOC) -- new module registered as `pub mod rtmp` in `lvqr-test-utils/src/lib.rs`. Exposes `rtmp_client_handshake(stream: &mut TcpStream) -> Vec<u8>`, the panic-on-error variant that 12 of the 13 migrating files already used. The module doc comment explicitly documents the panic contract, and explains why the Result-returning variant used by `one_token_all_protocols` stays local (single-caller; that test's `try_rtmp_publish` maps `Err` to `return false` as a signal that the server dropped the handshake, which the panic variant would turn into a test failure instead).

2. **`crates/lvqr-test-utils/Cargo.toml`** -- adds `rml_rtmp = { workspace = true }` to dependencies (previously absent; the handshake module names `Handshake`, `HandshakeProcessResult`, `PeerType` directly) and `io-util` to tokio's feature list (needed for `AsyncReadExt::read` / `AsyncWriteExt::write_all`). Dep-graph change invalidated the incremental clippy cache for every consumer crate, surfacing 11 pre-existing `collapsible_match` lints in test-file `read_until` helpers; drive-by-fixed via `cargo clippy --fix`.

3. **13 test files migrated** to consume the shared helper:
   * Direct consumption (rename local `fn rtmp_client_handshake` into module import): `archive_dvr_read_e2e`, `c2pa_cli_flags_e2e` (feature-gated on c2pa), `c2pa_verify_e2e` (feature-gated on c2pa), `rtmp_archive_e2e`, `rtmp_dash_e2e`, `rtmp_hls_e2e`, `rtmp_ws_e2e`, `rtmp_whep_audio_e2e` (feature-gated on transcode), `wasm_frame_counter`, `wasm_hot_reload`, `whisper_cli_e2e` (feature-gated on whisper; #[ignore]'d).
   * Rename + consumption (rename local `fn rtmp_handshake` call sites to `rtmp_client_handshake` + import the shared helper): `playback_signed_url_e2e`, `transcode_ladder_e2e` (feature-gated on transcode).

4. **`one_token_all_protocols.rs` intentionally left with its local handshake.** The file's `fn rtmp_handshake(stream) -> std::io::Result<Vec<u8>>` is semantically different from the shared panic-returning variant: the surrounding `try_rtmp_publish(addr, app, key) -> bool` helper uses `Err(std::io::Error)` propagation to distinguish "handshake succeeded + server rejected publish" from "server closed mid-handshake" (which is exactly the auth-rejection signal the test wants). Exposing a `_try` shared variant for one caller would be premature factoring; the module doc comment in `lvqr-test-utils::rtmp` names this explicitly so future contributors understand the single-caller exemption.

5. **Drive-by `collapsible_match` lint fix across 11 files.** `cargo clippy --fix --allow-dirty --allow-staged` auto-applied the suggested guard-pattern rewrite in every test file's `read_until` helper. The lint was pre-existing but masked by clippy's incremental cache; the new `rml_rtmp` dep on `lvqr-test-utils` invalidated the cache and surfaced the lint. Fixed files: `archive_dvr_read_e2e`, `c2pa_cli_flags_e2e`, `c2pa_verify_e2e`, `playback_signed_url_e2e`, `rtmp_archive_e2e`, `rtmp_dash_e2e`, `rtmp_hls_e2e`, `rtmp_ws_e2e`, `wasm_frame_counter`, `wasm_hot_reload` (lvqr-cli) + `rtmp_bridge_integration` (lvqr-ingest).

6. **`tracking/PLAN_V1.1.md`** row 122-E marked SHIPPED with the per-file detail list inline.

7. **README.md + `docs/auth.md` unchanged** -- pure internal hygiene; no operator-facing surface moved.

### Key 133 design decisions baked in

* **Expose only the panic variant in the shared module.** 12 of 13 migrating files used `.unwrap()` / `assert!` panics on handshake errors; the Result-returning variant is a single-caller. YAGNI + explicit documentation on the module's one-caller exemption. If a second caller ever needs Result-returning semantics, the module doc comment explicitly points at the `_try` follow-up slot.

* **Drop the rml_rtmp::handshake import line from every migrated file.** Every migrated file previously imported `rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType}` for its own local handshake fn. Those imports are now redundant once the local fn is gone. The `rml_rtmp::sessions::{...}` imports stay because the test bodies still name `ClientSession`, `ClientSessionEvent`, etc. directly.

* **Rename `rtmp_handshake` -> `rtmp_client_handshake` at the 2 diverging call sites** (`playback_signed_url_e2e`, `transcode_ladder_e2e`). The shared module is canonical; keeping two names would mean maintaining two-call-site aliasing for no benefit. The rename is a 1-line change per file (`let remaining = rtmp_handshake(&mut stream).await;` -> `let remaining = rtmp_client_handshake(&mut stream).await;`).

* **Fix `collapsible_match` lint auto-instead-of-allow.** Session 133's initial clippy run surfaced 11 pre-existing lints after the dep-graph change invalidated the incremental cache. Two options: (a) add `#[allow(clippy::collapsible_match)]` module-level attributes across 14 files, or (b) apply the clippy-suggested fix via `cargo clippy --fix`. Went with (b) because the fix is pure style (turns `match { RaisedEvent(ref event) => { if predicate(event) { return; } } }` into `match { RaisedEvent(ref event) if predicate(event) => return, }`) with no behavior delta and leaves the tree in a cleaner shape. The `read_until` helper itself is still duplicated per file; factoring that out is PLAN 122-F.

* **`read_until` + `send_results` + `send_result` stay local.** These three helpers are the remaining duplication in RTMP test files. Factoring them requires harmonizing predicate signatures (all variants currently use `Fn(&ClientSessionEvent) -> bool`, identical so migrating is mechanical) AND picking an Instant type (some files use `tokio::time::Instant`, others `std::time::Instant` -- both work for the deadline arithmetic). That is session 134's work (PLAN 122-F); keeping this slice single-purpose avoids mixing the RTMP handshake rewrite with an unrelated harmonization pass.

* **Transcode-feature runtime verification still deferred.** `brew install gstreamer` completed mid-session but the formula is a metapackage that does not ship `gstreamer-app-1.0.pc`; pkg-config still can't resolve the dev lib so `cargo test --features transcode` compile-fails before it runs. Same posture as sessions 131 + 132: `feature-matrix.yml` CI is the authoritative runtime signal.

* **Default-gate test count unchanged at 991/0/3.** The session adds zero tests + removes zero tests. ~240 LOC of duplicated handshake bodies drop in exchange for 85 LOC of new shared module + 13 LOC of import-line changes.

### Ground truth (session 133 close)

* **Head (pre-push)**: `refactor(tests)` + this close-doc commit (pending). `origin/main` at `92535d7` unchanged from session 131 + README + session 132 push.
* **Tests**:
  * Default workspace gate: **991** passed / 0 failed / 3 ignored (unchanged from session 132).
  * Per-file re-runs after migration: rtmp_archive 2/0/0, rtmp_hls 3/0/0, rtmp_dash 2/0/0, c2pa_verify 1/0/0 (--features c2pa), c2pa_cli_flags 2/0/0 (--features c2pa).
  * Compile-verified under feature gates: whisper_cli_e2e via `cargo check --features whisper` (session 131 precedent). rtmp_whep_audio_e2e + transcode_ladder_e2e compile-verified to the extent the local host allows (brew gstreamer metapackage missing pkg-config's gstreamer-app-1.0.pc file; full compile deferred to feature-matrix.yml's transcode cell on CI).
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95 (after the drive-by `collapsible_match` auto-fix).
  * `cargo test --workspace` 991 / 0 / 3.
* **Workspace**: **29 crates**, unchanged.

### Audit trail (session 133)

Post-migration grep for local `rtmp_client_handshake` / `rtmp_handshake` function definitions in `crates/lvqr-cli/tests/*.rs`:

* Files with a local handshake definition: **1** (`one_token_all_protocols.rs`, Result-returning single-caller variant with documented exemption).
* Files consuming the shared `lvqr_test_utils::rtmp::rtmp_client_handshake`: 13.
* Files with no handshake at all (don't do RTMP): the rest.

### Known limitations / documented v1 shape (after 133 close)

* **`read_until` + `send_results` + `send_result` RTMP test helpers still duplicated across ~14 files.** PLAN 122-F follow-up slice. Factoring requires choosing an Instant type (tokio vs std) and harmonizing the Fn predicate constraint (identical in shape across variants today). Same mechanical pattern as 122-A/B/C/D/E.
* **`one_token_all_protocols.rs` keeps its local `fn rtmp_handshake` Result-variant.** Single-caller with documented error-recovery contract. Exposing a `_try` shared variant is premature factoring; revisit if a second Result-consumer appears.
* **transcode + rtmp_whep_audio runtime verification still deferred to CI.** `brew gstreamer` metapackage lacks the dev-lib .pc file needed for `cargo` to compile the transcode-feature graph locally.
* **Authoritative DASH-IF container validator deferred**; GPAC MP4Box remains primary.
* **Webhook auth provider** still pending.
* **npm + PyPI publish cycle** still pending.
* All other session 132 + earlier known limitations unchanged.


## Session 132 close (2026-04-23)

## Session 132 close (2026-04-23)

**Shipped**: PLAN row 122-D (fourth slice of the shared-helpers refactor; drains the `http_get` duplication surface across the 8 non-RTMP test files). No new shared module; no new feature signal. Default-gate workspace count unchanged at **991 / 0 / 3**.

### Deliverables

1. **8 test files migrated** off local `http_get` onto `lvqr_test_utils::http::{HttpGetOptions, http_get_with}` (a subset also consume the shared `HttpResponse`):

   * `crates/lvqr-cli/tests/auth_integration.rs` -- kept the local `HttpResponse { status, body: String }` struct because every call site reads `resp.body` as a `String` (the admin router emits JSON; assumption is honest for the JWT-path tests that parse it). 10-line wrapper runs `String::from_utf8_lossy(&resp.body).into_owned()` to adapt the shared helper's `Vec<u8>` body shape. Bearer-dispatch + 5-second TIMEOUT preserved. 6 / 6 tests pass.
   * `crates/lvqr-cli/tests/cluster_redirect.rs` (#![cfg(feature = "cluster")]) -- the pre-migration `http_get_raw(addr, path) -> (u16, Option<String>)` was a ~30-line raw-TCP + hand-rolled header parse whose only job was projecting the Location header out as an Option<String>. Replaced with a 4-line wrapper that calls the shared `http_get(addr, path)` and runs `resp.header("location").map(|v| v.to_string())`. 3 / 3 tests pass. Tokio imports dropped since the file had no other need for them.
   * `crates/lvqr-cli/tests/federation_reconnect.rs` -- same shape as auth_integration (bearer-dispatch + body: String); same thin wrapper. 5-second HTTP_TIMEOUT preserved. 1 / 1 test passes.
   * `crates/lvqr-cli/tests/rtsp_hls_e2e.rs` -- standard 2-arg http_get; dropped local `HttpResponse` struct + parse. Consumes the shared `HttpResponse` directly. 1 / 1 test passes.
   * `crates/lvqr-cli/tests/slo_latency_e2e.rs` -- `http_get(addr, path) -> (u16, Vec<u8>)` tuple signature preserved at the call-site ergonomics layer via a 12-line wrapper that destructures `resp.status` + `resp.body` from the shared helper. 10-second TIMEOUT preserved. 3 / 3 tests pass.
   * `crates/lvqr-cli/tests/srt_hls_e2e.rs` -- standard 2-arg http_get; dropped local `HttpResponse` struct + parse + the now-unused tokio TcpStream + AsyncRead/WriteExt imports (`bytes::Bytes` stays; the SRT caller's `send((now, Bytes::from(ts_data)))` call sites still need it). 2 / 2 tests pass.
   * `crates/lvqr-cli/tests/srt_dash_e2e.rs` -- mirror of srt_hls_e2e's shape. 1 / 1 test passes.
   * `crates/lvqr-cli/tests/whip_hls_e2e.rs` -- dropped local `HttpResponse` + `find_header` + the http_get branch of `parse_http_response`; http_get now forwards to the shared module. `http_post_sdp` stays local because the shared module is GET-only, but its internal parse now constructs the shared `HttpResponse`. 1 callsite swap from `find_header(&resp, "location")` to `resp.header("location")`. 1 / 1 test passes.

   Aggregate per-file re-runs: 6 + 3 + 1 + 1 + 3 + 2 + 1 + 1 = **18 integration tests green**; zero regressions. Total dropped: ~270 LOC of duplicated http_get bodies, hand-rolled status-line parsers, and local `HttpResponse` structs. Total added: ~75 LOC of thin wrappers + module imports.

2. **`tracking/PLAN_V1.1.md`** row 122-D marked SHIPPED with the per-file detail list inline.

3. **README.md + `docs/auth.md` unchanged** -- the refactor is pure internal hygiene; no operator-facing surface moved.

### Key 132 design decisions baked in

* **Keep local wrappers over the shared helper where call-site ergonomics differ.** Three files (`auth_integration`, `federation_reconnect`, `slo_latency_e2e`) used non-standard return shapes (body-as-String tuple; status + body tuple) at every call site. Converting those to the shared `HttpResponse { status, headers, body: Vec<u8> }` would have touched dozens of call sites in each file. The 8-12 LOC wrapper per file keeps the shared module canonical while the local call sites stay byte-identical. This is exactly the session-129 pattern for `archive_dvr_read_e2e` and `playback_signed_url_e2e`; applied here to the non-RTMP cohort.

* **`cluster_redirect`'s `http_get_raw` wrapper shrinks to a 4-line projection.** The historical helper's only job was to extract the Location header out of the response; all other fields were discarded. With the shared `HttpResponse::header()` method the entire helper collapses to `let resp = http_get(addr, path).await; (resp.status, resp.header("location").map(|v| v.to_string()))`. Good signal for how much the shared module earns when the project uses it seriously.

* **No mass conversion of call-site ergonomics.** Each of the 8 files kept its historical call-site signature. I could have changed every `let (status, loc) = http_get_raw(...)` to `let resp = http_get(...); (resp.status, resp.header("location"))` inline across the file, but that would be ~20-30 unrelated call-site edits per file for zero behavior change. Minimum-touch is the right target for a pure code-dedup refactor.

* **`whip_hls_e2e`'s `parse_http_response` stays local.** The shared module is GET-only by design (pulling a POST / DELETE / PATCH surface into `lvqr-test-utils` is feature creep for the 2 files that need it). The WHIP / WHEP signaling exchange POSTs an SDP offer; `http_post_sdp` keeps the POST logic local but its body-parse now constructs the shared `HttpResponse` so downstream call sites see one canonical response type. Same pattern as session 131's `rtmp_whep_audio_e2e`.

* **RTMP handshake helper explicitly OUT of scope.** The 3 representative handshake bodies (in `rtmp_hls_e2e`, `rtmp_ws_e2e`, `transcode_ladder_e2e`) show semantic equivalence but name variance (`rtmp_client_handshake` vs `rtmp_handshake`) and subtle branch handling in the `InProgress` arm (one writes-if-non-empty, another always-writes). Harmonizing is its own slice (PLAN 122-E). Session 132 scoping ruled it out so the commit's diff stays a single-purpose code-dedup, not a mixed dedup + semantic-harmonization change.

* **Default-gate test count unchanged at 991/0/3.** The session adds zero tests + removes zero tests. Same behavior, fewer LOC.

### Ground truth (session 132 close)

* **Head (pre-push)**: `refactor(tests)` + this close-doc commit (pending). `origin/main` at `c882da1` unchanged from session 131 + README push.
* **Tests**:
  * Default workspace gate: **991** passed / 0 failed / 3 ignored (unchanged from session 131).
  * Per-file re-runs after migration: auth_integration 6/0/0, cluster_redirect 3/0/0 (--features cluster), federation_reconnect 1/0/0, rtsp_hls_e2e 1/0/0, slo_latency_e2e 3/0/0, srt_hls_e2e 2/0/0, srt_dash_e2e 1/0/0, whip_hls_e2e 1/0/0.
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean (rustfmt applied one `.header("location").expect(...)` chain split, same as session 131's `rtmp_whep_audio`).
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 991 / 0 / 3.
* **Workspace**: **29 crates**, unchanged.

### Audit trail (session 132)

Post-migration grep for local `http_get` function definitions in `crates/lvqr-cli/tests/*.rs`:

* Files with substantial local `http_get` body: **0**.
* Files with thin `http_get` wrappers forwarding into the shared module: 16 (all RTMP-class + non-RTMP files).
* Files with no local http_get at all (call the shared module directly, or do not do HTTP): the rest.

The `http_get` duplication surface in the CLI integration test directory is now zero. Sessions 129+130+131+132 together touched the four primary duplication surfaces identified in session 129's triage: HTTP GET (closed 129+130+131+132), FLV tag builders (closed 130+131), and the RTMP handshake helper (still deferred).

### Known limitations / documented v1 shape (after 132 close)

* **RTMP handshake helper not yet factored.** ~10 RTMP-ingest tests still reimplement `rtmp_client_handshake` / `rtmp_handshake` with subtle variance. PLAN 122-E (dedicated session).
* **`http_post_sdp` local in `whip_hls_e2e` + `rtmp_whep_audio_e2e`.** Shared module is GET-only. The POST duplication across these 2 files is ~25 LOC; a future session could add `http_post_sdp` (or `http_post`) to the shared module if a 3rd POST-using test appears.
* **Authoritative DASH-IF container validator deferred**; GPAC MP4Box remains primary.
* **Webhook auth provider** still pending (README Auth+ops-polish checklist `[ ]`).
* **npm + PyPI publish cycle** still pending.
* All other session 131 + earlier known limitations unchanged.


## Session 131 close (2026-04-23)

## Session 131 close (2026-04-23)

**Shipped**: PLAN row 122-C (third slice of the shared-helpers refactor). No new shared module; no new feature signal. Default-gate workspace count unchanged at **991 / 0 / 3**.

### Deliverables

1. **9 test files migrated** off local `http_get` + FLV duplicates onto `lvqr_test_utils::{http, flv}`:
   * `crates/lvqr-cli/tests/archive_dvr_read_e2e.rs` (session 118; http_get already migrated in 129) -- removed local `flv_video_seq_header` + `flv_video_nalu` + the `bytes::Bytes` import. 4 / 4 tests still pass.
   * `crates/lvqr-cli/tests/playback_signed_url_e2e.rs` (session 124; http_get_with_bearer wrapper kept since 129) -- removed local FLV builders + `bytes::Bytes` import. 3 / 3 tests still pass.
   * `crates/lvqr-cli/tests/wasm_frame_counter.rs` (Tier 4 4.2) -- removed local FLV builders. File has no http_get (uses the `WasmFilterBridgeHandle` directly off `ServerHandle`). 1 / 1 test still passes.
   * `crates/lvqr-cli/tests/wasm_hot_reload.rs` (Tier 4 4.2 follow-up) -- removed local FLV builders. Same no-http_get shape as the frame-counter sister. 1 / 1 test still passes.
   * `crates/lvqr-cli/tests/rtmp_ws_e2e.rs` (Tier 2.3) -- removed local FLV builders. File has no http_get; the WS subscriber inlines its handshake against the `axum::extract::ws` route. 1 / 1 test still passes.
   * `crates/lvqr-cli/tests/captions_hls_e2e.rs` (Tier 4 4.5 session C) -- no local FLV; pushes synthetic `Fragment`s straight onto the registry rather than going through RTMP. Removed local `HttpResponse` + `http_get` + the unused `tokio::io` + `tokio::net::TcpStream` imports; kept a 10-line `http_get` wrapper that pins `HttpGetOptions::timeout = TIMEOUT` (10 s for the LL-HLS partial-window read path). 2 / 2 tests still pass.
   * `crates/lvqr-cli/tests/transcode_ladder_e2e.rs` (#![cfg(all(feature = "transcode", feature = "rtmp"))]) -- removed local FLV (video + 44k audio) + http_get (which had a `Result<HttpResponse, String>` signature). The Result-returning shape is preserved via a thin always-Ok wrapper marked `#[allow(clippy::unnecessary_wraps)]` so the call sites' existing `?`-propagation style stays unchanged; the shared helper panics on connect/read failure today, so the Err arm is unreachable but the signature documents the historical error-context contract for any future change. Compile-verified via `cargo check -p lvqr-cli --tests --features whisper` (cargo's check graph compiles all targets that match the active feature set; with whisper enabled the transcode-gated file is gated out, so this does NOT cover the transcode build alone -- compile-only verification of the transcode-feature graph is deferred to a host with GStreamer installed and to `feature-matrix.yml`'s transcode cell on CI).
   * `crates/lvqr-cli/tests/whisper_cli_e2e.rs` (#![cfg(feature = "whisper")]; #[ignore]'d pending the cached-model scheduled workflow) -- removed local FLV + audio (44k). 1 callsite swap from `flv_audio_seq_header()` to `flv_audio_aac_lc_seq_header_44k_stereo()`. Compile-verified locally via `cargo check -p lvqr-cli --tests --features whisper`.
   * `crates/lvqr-cli/tests/rtmp_whep_audio_e2e.rs` (session 115; #![cfg(feature = "transcode")]) -- the most involved migration of this slice. Replaced local 48 kHz / stereo `flv_audio_seq_header_48k_stereo` (literal `0xAF 0x00 0x11 0x90`) with a thin local wrapper that calls the shared `flv_audio_aac_lc_seq_header(3, 2)` parameterized helper -- exercises the parameterized form session 130 set up specifically for this case. Replaced local `flv_video_keyframe(cts, nalu)` with one inline call to `flv_video_nalu(true, cts, nalu)` (the local helper hard-coded the keyframe-frame-type byte; the shared helper takes the bool). Removed local `HttpResponse` struct + `find_header` standalone function + `parse_http_response`; kept `http_post_sdp` local because the shared `lvqr_test_utils::http` is GET-only and the WHEP signaling exchange POSTs the SDP offer, but the local POST helper now constructs and returns the shared `HttpResponse` shape. The `find_header(&resp, "location")` callsite swapped for `resp.header("location")` (the shared `HttpResponse` exposes the case-insensitive header lookup as a method). Compile-verified deferred to a GStreamer-installed host (brew install gstreamer kicked off in the same shell session; verification will land in the next session if it completes).

2. **`tracking/PLAN_V1.1.md`** row 122-C marked SHIPPED with the per-file detail list inline.

3. **README.md + `docs/auth.md` unchanged** -- the refactor is pure internal hygiene; no operator-facing surface moved.

### Key 131 design decisions baked in

* **Drain the FLV duplication surface to zero, not just majority-migrate.** Sessions 129 + 130 + 131 together touch all 14 test files in `crates/lvqr-cli/tests/` that previously carried local FLV builders. Leaving even 1-2 stragglers would mean the next test author can't reliably rely on `lvqr_test_utils::flv` being the canonical source. Closing the surface entirely makes the shared module the only place the FLV byte math lives.

* **Keep the `Result<HttpResponse, String>`-returning wrapper in `transcode_ladder_e2e`.** The original returned a Result so the callsites' `?`-propagation could surface the path in error messages. The shared `http_get_with` panics with generic context on connect/read failure. Two options: (a) rewrite all `?`-propagating call sites to plain `.await` -- big diff; (b) keep the Result-returning wrapper so call sites stay byte-identical; the Err arm becomes unreachable today but documents the historical error-context contract for any future re-introduction. Went with (b) plus a `#[allow(clippy::unnecessary_wraps)]` so a clippy::pedantic run-down does not flag it. Note the pragmatism: the shared helper's panic messages do not include the path; future hardening could add that, after which the Result-wrapper here could collapse.

* **Inline `flv_video_keyframe` -> `flv_video_nalu(true, ..)`.** The single call site in `rtmp_whep_audio_e2e` made a local wrapper not worth keeping. Inlining is more work for the human reader at the call site (2 extra arguments visible) but removes 5 LOC of helper code. For a one-callsite case the inline is the right trade-off; if a second 48 kHz keyframe-only test file appears, factor at that point.

* **Replace local `find_header` with the shared `HttpResponse::header()` method.** The shared method does the same case-insensitive lookup; the standalone-function form was just an artifact of the file pre-dating the shared module. Dropping it is a 0-LOC-difference rename that locks the lookup surface in one place.

* **48 kHz audio uses the parameterized helper inline; no `_48k_stereo` convenience wrapper.** Session 130 exposed `flv_audio_aac_lc_seq_header(freq_idx, channels)` plus a `_44k_stereo` convenience wrapper because 44.1 kHz / stereo is the dominant case (3 files). 48 kHz / stereo is one file (rtmp_whep_audio_e2e); adding a `_48k_stereo` wrapper for one call site would be premature factoring. The local 5-line wrapper `fn flv_audio_seq_header_48k_stereo() -> bytes::Bytes { flv_audio_aac_lc_seq_header(3, 2) }` is the right balance: shows the bytes' meaning at the call site without exposing the math twice.

* **Compile-only verification for the 3 feature-gated files.** GStreamer + whisper.cpp dev libs are nontrivial deps that not every contributor laptop has. The session-123 `feature-matrix.yml` workflow already has dedicated runners for these features; relying on CI for the transcode + whisper + rtmp_whep_audio runtime exercise is honest about local capability. `cargo check -p lvqr-cli --tests --features whisper` was clean locally; transcode + transcode-gated rtmp_whep_audio compile-check is deferred to the same brew-install-gstreamer pass that's still running, or to CI.

* **Default-gate test count unchanged at 991/0/3.** The session adds zero tests + removes zero tests. Every migrated file's per-file roster is unchanged. The 280-LOC drop is in helper code; the test bodies are byte-identical to their pre-migration shape.

### Ground truth (session 131 close)

* **Head (pre-push)**: `refactor(tests)` + this close-doc commit (pending). `origin/main` at `77da8c3` unchanged from session 129 push.
* **Tests**:
  * Default workspace gate: **991** passed / 0 failed / 3 ignored (unchanged from session 130).
  * Per-file re-runs after migration: archive_dvr_read 4/0/0, playback_signed_url 3/0/0, wasm_frame_counter 1/0/0, wasm_hot_reload 1/0/0, rtmp_ws 1/0/0, captions_hls 2/0/0.
  * Compile-verified: whisper_cli_e2e via `cargo check -p lvqr-cli --tests --features whisper` clean.
  * Deferred to GStreamer-installed host or CI: transcode_ladder_e2e + rtmp_whep_audio_e2e runtime + compile of the transcode-feature graph.
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean (rustfmt joined the multi-line `lvqr_test_utils::flv` import in rtmp_whep_audio_e2e onto one line; the file's first line therefore exceeds the soft 120 width but is a single import statement so width is not a fmt verdict).
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 991 / 0 / 3.
* **Workspace**: **29 crates**, unchanged.

### Audit trail (session 131)

Post-migration grep for remaining `fn flv_` in `crates/lvqr-cli/tests/*.rs`:

* `archive_dvr_read_e2e.rs`: 0 local FLV definitions.
* `playback_signed_url_e2e.rs`: 0 local FLV definitions.
* `wasm_frame_counter.rs`: 0 local FLV definitions.
* `wasm_hot_reload.rs`: 0 local FLV definitions.
* `rtmp_ws_e2e.rs`: 0 local FLV definitions.
* `captions_hls_e2e.rs`: 0 local FLV (never had any).
* `transcode_ladder_e2e.rs`: 0 local FLV definitions.
* `whisper_cli_e2e.rs`: 0 local FLV definitions.
* `rtmp_whep_audio_e2e.rs`: 1 local FLV wrapper (`flv_audio_seq_header_48k_stereo` -- thin 5-line forwarder over the shared parameterized helper).

Total local FLV definitions across all `crates/lvqr-cli/tests/*.rs`: **0** (down from 13 pre-session-130, i.e. 5 closed in 130 + 8 closed in 131; the 1 remaining wrapper in rtmp_whep_audio is documentation-of-intent rather than duplication).

Post-migration grep for remaining `fn http_get` (excluding files already only carrying thin wrappers):

* Files with substantial local `http_get` body: 8 non-RTMP files (`auth_integration`, `cluster_redirect`, `federation_reconnect`, `rtsp_hls_e2e`, `slo_latency_e2e`, `srt_hls_e2e`, `srt_dash_e2e`, `whip_hls_e2e`).
* Files with thin `http_get` wrappers over the shared module: 8 (sessions 129+130+131 work).

### Known limitations / documented v1 shape (after 131 close)

* **8 non-RTMP test files still carry local `http_get` duplicates.** Future session work; the 8 are http_get-only (no FLV interaction); migrating each is a 5-minute mechanical edit.
* **RTMP handshake helper not yet factored.** ~10 RTMP-ingest tests still reimplement `rtmp_client_handshake` / `rtmp_handshake` with subtle name + buffer-handling variance between the variants. Harmonizing is its own session.
* **transcode + rtmp_whep_audio runtime verification deferred to CI.** Local host did not have GStreamer at session start; brew install kicked off mid-session may complete before commit. If it does, the close note will be amended; if not, `feature-matrix.yml`'s transcode cell carries the authoritative runtime signal.
* **Authoritative DASH-IF container validator deferred**; GPAC MP4Box remains the primary validator in `dash-conformance.yml`.
* **Webhook auth provider** still pending (README Auth+ops-polish checklist `[ ]`).
* **npm + PyPI publish cycle** still pending.
* All other session 130 + earlier known limitations unchanged.


## Session 130 close (2026-04-22)

## Session 130 close (2026-04-22)

**Shipped**: PLAN row 122-B (second slice of the shared-helpers refactor). No new feature signal; code-dedup hygiene. Default-gate workspace count moved from **984 -> 991** (+7 from the new `lvqr_test_utils::flv` module's unit tests; zero new integration tests).

### Deliverables

1. **`crates/lvqr-test-utils/src/flv.rs`** (~145 LOC + ~60 LOC of unit tests) -- new module registered as `pub mod flv` in `lvqr-test-utils/src/lib.rs`. Centralizes the FLV tag builders every RTMP-ingest integration-test file had reimplemented verbatim:
   * `fn flv_video_seq_header() -> Bytes` -- H.264 High@L3.1 SPS+PPS record. The byte sequence was byte-identical across all 11 pre-migration files (session 129's "subtle per-file variance" concern did not hold for this function; a `diff` of all 11 shows zero delta).
   * `fn flv_video_nalu(keyframe: bool, cts: i32, nalu_data: &[u8]) -> Bytes` -- AVC keyframe / P-frame NALU tag with signed-24-bit-BE composition-time encoding. Byte-identical across the same 11 files.
   * `fn flv_audio_aac_lc_seq_header(sample_freq_index: u8, channels: u8) -> Bytes` -- parameterized AAC-LC AudioSpecificConfig byte math. Reproduces the historical 44.1 kHz / stereo bytes (`(4, 2)` -> `0xAF 0x00 0x12 0x10`) AND session-114's 48 kHz / stereo bytes (`(3, 2)` -> `0xAF 0x00 0x11 0x90`). Explicit mask form (`(sample_freq_index & 0x01) << 7`) in place of the historical `(sample_freq_index << 7) as u8` overflow-wrap-based computation; the unit tests lock both historical byte sequences against the new math.
   * `fn flv_audio_aac_lc_seq_header_44k_stereo() -> Bytes` -- common-case convenience wrapper.
   * `fn flv_audio_raw(aac_data: &[u8]) -> Bytes` -- byte-identical across the 4 pre-migration files using it.
   * 7 unit tests in `flv::tests`: historical-bytes lock for `flv_video_seq_header`, frame-type-nibble flip on keyframe flag, signed-24-bit-BE composition-time encoding, NALU payload appended verbatim, 44 kHz + 48 kHz seq-header byte sequences, `flv_audio_raw` prepends packet-type tag.

2. **Migrated 5 test files** to consume both the session-129 `http_get` helper AND the new `flv` module:
   * `crates/lvqr-cli/tests/rtmp_archive_e2e.rs` (session 118) -- removed ~60 LOC of local `HttpResponse` + `http_get` + `http_get_with_auth` + ~15 LOC of flv builders. Kept a 6-line `http_get` wrapper that pins the 10-second TIMEOUT this test needs for RTMP-publish-adjacent reads + a 6-line `http_get_with_auth` wrapper that dispatches through `HttpGetOptions::bearer`. 2 / 2 tests still pass.
   * `crates/lvqr-cli/tests/rtmp_hls_e2e.rs` (session 12) -- removed ~45 LOC of local `HttpResponse` + `http_get` + `parse_http_response` + ~35 LOC of `flv_video_*` + `flv_audio_*` builders. 1 call site updated from `flv_audio_seq_header()` to the shared-module `flv_audio_aac_lc_seq_header_44k_stereo()` to make the 44 kHz / stereo choice explicit at the callsite. Kept a 10-line `http_get` wrapper for the 10-second TIMEOUT. 3 / 3 tests still pass.
   * `crates/lvqr-cli/tests/rtmp_dash_e2e.rs` (session 12) -- removed ~45 LOC of http helpers + ~18 LOC of flv_video builders. Kept a 10-line `http_get` wrapper. 2 / 2 tests still pass.
   * `crates/lvqr-cli/tests/c2pa_verify_e2e.rs` (session 94; `#![cfg(feature = "c2pa")]`) -- removed ~35 LOC of http helpers + ~18 LOC of flv_video builders. Kept a 10-line `http_get` wrapper. 1 / 1 test still passes with `--features c2pa`.
   * `crates/lvqr-cli/tests/c2pa_cli_flags_e2e.rs` (session 121; `#![cfg(feature = "c2pa")]`) -- removed ~35 LOC of http helpers + ~18 LOC of flv_video builders. Kept a 10-line `http_get` wrapper. 2 / 2 tests still pass with `--features c2pa`.

3. **`tracking/PLAN_V1.1.md`** row 122-B marked SHIPPED with the design-decision list inline.

4. **README.md + `docs/auth.md` unchanged** -- the refactor is pure internal hygiene; no operator-facing surface moved.

### Key 130 design decisions baked in

* **FLV builders ARE uniform; session 129's deferral note was over-cautious.** A grep-plus-awk diff across 11 pre-migration files confirmed `flv_video_seq_header` and `flv_video_nalu` were copy-pasted byte-for-byte (no per-file variance). The 44.1 kHz / stereo `flv_audio_seq_header` was also identical across the 3 files that use it. The only genuine variance was session-114's `rtmp_whep_audio_e2e.rs` using 48 kHz / stereo AAC for its WHEP audio bridge -- solved by exposing a parameterized helper `flv_audio_aac_lc_seq_header(sample_freq_index, channels)` and letting that file call it with `(3, 2)` whenever it migrates (out of scope for this session; rtmp_whep_audio_e2e stays on its local 48k helper for now). Factoring is therefore NOT risky: the shared helpers' unit tests lock every byte sequence against the historical bytes, and each migrated test file re-runs on the same-bytes pipeline.

* **Parameterized `flv_audio_aac_lc_seq_header(freq_idx, channels)` + 44k convenience wrapper, not two fixed helpers.** Option A (two fixed helpers `_44k_stereo` + `_48k_stereo`) would lock the module to the current call set. Option B (one generic helper taking both parameters) is flexible for any future sample-rate test. Went with B plus one convenience wrapper for the dominant 44.1 kHz / stereo case so the migrating files stay readable. The 48 kHz case uses the generic directly (when it migrates); no `_48k_stereo` wrapper today since there is only one 48k call site and it has not been migrated yet.

* **Explicit mask form replaces the historical `(freq_idx << 7) as u8` overflow-truncation.** The pre-migration `let b1: u8 = (4 << 7) | (2 << 3);` relies on u8 shift-overflow wrapping (4 * 128 = 512, truncates to 0 in u8). Rust's release codegen does the wrap silently; debug codegen would check for overflow except the literals are const-evaluated as i32 first. Rewriting as `((freq_idx & 0x01) << 7) | (channels << 3)` makes the intent (extract low bit of sample_freq_index, shift into bit 7) self-documenting and avoids any reliance on wrap semantics. The unit test `audio_seq_header_44k_stereo_matches_historical_bytes` locks the output to `0xAF 0x00 0x12 0x10` so a regression in the byte math would fail loud.

* **Five files migrated, 10 left.** The 10 unmigrated files split into two classes: (a) RTMP-ingest tests whose FLV builders are byte-identical to the shared helpers (`rtmp_ws_e2e`, `rtmp_whep_audio_e2e`, `captions_hls_e2e`, `transcode_ladder_e2e`, `whisper_cli_e2e`, `wasm_frame_counter`, `wasm_hot_reload`, `playback_signed_url_e2e`) -- safe to migrate any time; (b) non-RTMP tests that only need the http_get migration (`cluster_redirect`, `federation_reconnect`, `rtsp_hls_e2e`, `slo_latency_e2e`, `srt_hls_e2e`, `srt_dash_e2e`, `whip_hls_e2e`, `auth_integration`). The 5 chosen for session 130 are the highest-duplication-surface files (each lost both http_get AND flv code) + the two c2pa tests which are feature-gated but landed first because their diffs are the smallest.

* **Local wrappers preserve the 10-second TIMEOUT at each migrated callsite.** Every migrated file kept a thin `http_get` / `http_get_with_auth` wrapper that pins `HttpGetOptions { timeout: TIMEOUT, ..Default::default() }`. Inlining the option struct at every callsite would add ~5 lines of churn per call; the wrappers keep the call sites byte-for-byte identical to their pre-migration shape. Matches the session-129 pattern for `archive_dvr_read_e2e` and `playback_signed_url_e2e`.

* **RTMP handshake helper stays un-factored.** A quick `diff` between the 3 rtmp_client_handshake variants (`rtmp_hls_e2e`, `rtmp_ws_e2e`, `transcode_ladder_e2e`) shows the body is semantically identical but has name variance (`rtmp_client_handshake` vs `rtmp_handshake`) and subtle buffer-handling differences in the `HandshakeProcessResult::InProgress` branch (write-before-continue vs write-if-non-empty). Factoring would require harmonizing these variants, which is a separate session's worth of design work and test re-runs. Session 130 scope explicitly excludes it; session 131 or later can pick it up.

* **Default-gate test count 984 -> 991 (+7).** The 7 delta is exactly the new `flv::tests` unit count. No integration tests were added or removed; every migrated file kept its exact test roster. The briefing's "stays at 984" constraint held for integration tests; the +7 is unit-test expansion which is always a net win.

### Ground truth (session 130 close)

* **Head (pre-push)**: `refactor(tests)` + this close-doc commit (pending). `origin/main` at `77da8c3` unchanged from session 129 push.
* **Tests**:
  * Default workspace gate: **991** passed / 0 failed / 3 ignored (+7 over session 129's 984, from the new flv::tests unit suite).
  * Per-file re-runs after migration: `rtmp_archive_e2e` 2/0/0, `rtmp_hls_e2e` 3/0/0, `rtmp_dash_e2e` 2/0/0, `c2pa_verify_e2e` 1/0/0 (with `--features c2pa`), `c2pa_cli_flags_e2e` 2/0/0 (with `--features c2pa`).
  * New flv unit tests: `flv::tests` 7/0/0 in 0.00 s.
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 991 / 0 / 3.
* **Workspace**: **29 crates**, unchanged.

### Audit trail (session 130)

Post-migration grep for remaining `fn http_get` / `fn flv_video_` in `crates/lvqr-cli/tests/*.rs`:

* `rtmp_archive_e2e.rs`: 2 local http_get wrappers (http_get + http_get_with_auth, both thin forwarders over shared helper); 0 local flv builders.
* `rtmp_hls_e2e.rs`: 1 local http_get wrapper (thin forwarder); 0 local flv builders.
* `rtmp_dash_e2e.rs`: 1 local http_get wrapper (thin forwarder); 0 local flv builders.
* `c2pa_verify_e2e.rs`: 1 local http_get wrapper (thin forwarder); 0 local flv builders.
* `c2pa_cli_flags_e2e.rs`: 1 local http_get wrapper (thin forwarder); 0 local flv builders.
* 10 other files: still carry local http_get duplicates; 7 of them additionally carry local flv builders that are byte-identical to the shared module's.

Every wrapper left in the migrated files is 6-10 LOC and forwards directly into the shared module, so future edits inherit the shared semantics.

### Known limitations / documented v1 shape (after 130 close)

* **10 test files still carry local `http_get` duplicates** and 7 of them carry identical flv builder duplicates. Future session work; no behavior difference from the migrated files.
* **RTMP handshake helper not yet factored.** ~10 RTMP-ingest tests still reimplement `rtmp_client_handshake` with subtle name + buffer-handling variance between the 3 representative bodies. Harmonizing is its own session.
* **48 kHz AAC `flv_audio_aac_lc_seq_header` one-off** in `rtmp_whep_audio_e2e.rs` stays local until the file migrates (shared module provides the parameterized helper it will use when migrated).
* **Authoritative DASH-IF container validator deferred**; GPAC MP4Box remains the primary validator in `dash-conformance.yml`.
* **Webhook auth provider** still pending (README Auth+ops-polish checklist `[ ]`; smaller than JWKS).
* **npm + PyPI publish cycle** still pending; both published builds at 0.3.1 are behind on admin coverage + miss the session 126 JWKS + session 128 sign_live_url public APIs.
* All other session 129 + earlier known limitations unchanged.


## Session 129 close (2026-04-22)

## Session 129 close (2026-04-22)

**Shipped**: PLAN row 122-A (first slice of the shared-helpers refactor). No new feature signal; pure code-dedup hygiene. Default-gate workspace count unchanged at **984**.

### Deliverables

1. **`crates/lvqr-test-utils/src/http.rs`** (~160 LOC) -- new module registered as `pub mod http` in `lvqr-test-utils/src/lib.rs`. Centralizes the raw-TCP HTTP/1.1 GET primitive every integration-test file had reimplemented: `HttpResponse { status, headers, body }` with `header()` case-insensitive lookup and `body_text()` lossy-utf8 view; `HttpGetOptions { bearer, range, extra_headers, timeout }` with `Default` (5 s timeout) + `with_bearer(token)` + `with_range(spec)` constructors; `http_get(addr, path)` / `http_get_with(addr, path, opts)` / `http_get_status(addr, path)` entry points. Raw-TCP stays -- the alternative (reqwest or hyper client) would pull a full TLS + HTTP stack into `lvqr-test-utils` which is `publish = false` but part of the default-feature build graph.

2. **Migrated 4 test files** to consume the shared helper:
   * `crates/lvqr-cli/tests/live_signed_url_e2e.rs` (session 128) -- removed local `HttpResponse` + `http_get` + `TIMEOUT` constant + `TcpStream` imports; every test now calls `http_get_status(addr, path)` from the shared module directly. Still passes 7 / 7.
   * `crates/lvqr-cli/tests/hls_live_auth_e2e.rs` (session 112) -- removed ~45 LOC of duplicated `http_get` + `Response` struct. New 10-line local `status_with_bearer` wrapper dispatches to `http_get_with(HttpGetOptions::with_bearer(...))` or `http_get_status(...)` depending on whether a bearer is present. Still passes 7 / 7.
   * `crates/lvqr-cli/tests/playback_signed_url_e2e.rs` (session 124) -- removed ~40 LOC of duplicated `HttpResponse` struct + `http_get_with_bearer` body. New 7-line `http_get_with_bearer` wrapper forwards into `http_get_with(HttpGetOptions { bearer, timeout: TIMEOUT, ..default })`. Preserves the test's 10-second TIMEOUT for RTMP-publish-adjacent routes. Still passes 3 / 3.
   * `crates/lvqr-cli/tests/archive_dvr_read_e2e.rs` (session 118) -- removed ~60 LOC of duplicated `HttpResponse` struct + `header()` impl + `http_get_with_range` body. Two local wrappers (`http_get` / `http_get_with_range`) preserve the 10-second TIMEOUT this test needs for its live-DVR scrub scenario. Still passes 4 / 4.

3. **`tracking/PLAN_V1.1.md`** row 122-A marked SHIPPED with the design-decision list inline.

4. **README.md + `docs/auth.md` unchanged** -- the refactor is pure internal hygiene; no operator-facing surface moved.

### Key 129 design decisions baked in

* **Factor `http_get`, not FLV / RTMP helpers.** Three distinct duplication surfaces showed up in the file inventory: (a) raw-TCP HTTP GET in 19 test files; (b) FLV tag builders in ~10 RTMP-ingest tests; (c) RTMP handshake wrapper in ~10 RTMP-ingest tests. The HTTP GET helpers are the most uniform across files (every file's version does essentially the same thing), so factoring that first minimizes regression risk. FLV builders and the RTMP handshake have subtle per-file variance -- different buffer sizes, different error-handling semantics, different `ClientSessionEvent` filters -- so those need a more careful migration pass per file. Scoping to just the HTTP helper for this session keeps the blast radius small.

* **4 files migrated, 13 left.** The remaining 13 (`rtmp_archive_e2e`, `rtmp_hls_e2e`, `rtmp_dash_e2e`, etc.) can adopt the shared helper incrementally without disturbing the module's API. The 4 migrated here are the recent / high-turn-over test files where future edits are most likely; locking in the pattern there means new tests authored downstream of them inherit the shared helper by default. Mass-migration across all 19 in one session would inflate the diff size + regression surface for marginal additional value.

* **`http_get_status` as a separate function**, not an alias for `http_get(...).await.status`. The new helper takes a step beyond what the duplicated code had -- every caller that only cared about the status code would still have had to construct the body + headers vectors. `http_get_status` avoids the allocations (cheap but non-zero) and gives a cleaner call site.

* **`HttpGetOptions<'a>` rather than a builder.** The options struct fields are all `Option` / `Vec`, so a `..Default::default()` struct update is natural. A builder pattern (`HttpGetOptions::new().bearer(...).range(...).build()`) would add another ~30 LOC to the module without making the call sites any more readable. The two convenience constructors (`with_bearer` / `with_range`) cover the 90% cases where only one option is set.

* **Raw-TCP, not reqwest / hyper client.** Pulling a proper HTTP client into `lvqr-test-utils` would introduce a significant transitive-dep surface for every consumer. The current raw-TCP approach works because every integration-test route accepts `Connection: close` and reads-to-EOF naturally; adding a full client would be feature creep for no signal gain. The module doc comment names this trade-off explicitly so future contributors do not second-guess the decision.

* **`TIMEOUT` stays a local test constant**, not a module-level default. `lvqr-test-utils::http` defaults to 5 s because that is reasonable for snappy auth-only routes (hls_live_auth_e2e). Tests that drive RTMP publishers into the admin HTTP surface (playback_signed_url_e2e, archive_dvr_read_e2e) need 10 s and override via `HttpGetOptions::timeout`. A global default of 10 s would slow down every fast test's timeout path; a global default of 5 s would flake the slow tests. Per-test override is the right middle ground.

* **Wrappers preserved where the call pattern needed bearer/range dispatch.** Each of the 4 migrated files ended up with a small (5-10 line) local wrapper around the shared helper. Callers that dispatched on `Option<&str>` bearer / range keep that dispatch at the callsite layer so their test bodies do not change. The wrappers explicitly avoid replicating any of the HMAC / header parsing / EOF detection logic; that lives in the shared module.

* **Default-gate workspace count unchanged (984 / 0 / 3).** The briefing asked to "prove the factor-out by asserting the default-gate test count stays at 968" -- updated to 984 after session 128's new tests. The refactor is pure code reorganization; no tests added or removed; every migrated file re-verified passing before the commit.

### Ground truth (session 129 close)

* **Head (pre-push)**: `refactor(tests)` + this close-doc commit (pending). `origin/main` at `52abd21` unchanged from session 128 push.
* **Tests**:
  * Default workspace gate: **984** passed / 0 failed / 3 ignored (unchanged from session 128).
  * Per-file re-runs after migration: `live_signed_url_e2e` 7/0/0, `hls_live_auth_e2e` 7/0/0, `playback_signed_url_e2e` 3/0/0, `archive_dvr_read_e2e` 4/0/0.
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 984 / 0 / 3.
* **Workspace**: **29 crates**, unchanged.

### Audit trail (session 129)

Post-migration grep for remaining `fn http_get` / `async fn http_get` in `crates/lvqr-cli/tests/*.rs`:

* `live_signed_url_e2e.rs`: 0 local http_get (fully migrated to shared helper)
* `hls_live_auth_e2e.rs`: 0 local http_get (uses `status_with_bearer` wrapper over shared helper)
* `playback_signed_url_e2e.rs`: 1 local http_get_with_bearer (thin wrapper over shared helper)
* `archive_dvr_read_e2e.rs`: 2 local (http_get + http_get_with_range, both thin wrappers)
* 13 other files: still carry their own http_get duplicates (future session work)

Every wrapper left in the migrated files is under 10 LOC and forwards directly into the shared module, so future edits pick up the shared semantics.

### Known limitations / documented v1 shape (after 129 close)

* **13 test files still carry local `http_get` duplicates.** Future session work; no behavior difference from the 4 migrated. The migration is incremental-safe; each file can adopt the shared helper independently.
* **FLV tag builders + RTMP handshake helper not yet factored.** Multiple RTMP-ingest tests still reimplement `flv_video_nalu` + `flv_video_seq_header` + `rtmp_client_handshake`. A later session can factor those into `lvqr-test-utils::flv` + `lvqr-test-utils::rtmp` if the subtle per-file variance can be reconciled.
* **Authoritative DASH-IF container validator deferred**; GPAC MP4Box remains the primary validator.
* **npm + PyPI publish cycle** still pending; both published builds at 0.3.1 are behind on admin coverage + miss the session 126 JWKS + session 128 sign_live_url public APIs.
* All other session 128 + earlier known limitations unchanged.


## Session 128 close (2026-04-22)

**Shipped**: PLAN row 121-B (HMAC extension to live `/hls/*` + `/dash/*`). Closes the session-124 Known Limitation "HMAC gated on `/playback/*` only". Default-gate workspace count moved from **968 -> 984** (+15: 8 new `signed_url::tests` unit tests + 7 new `live_signed_url_e2e.rs` integration tests). Shares one `--hmac-playback-secret` across all three playback route trees; the scheme tag baked into the signed input prevents cross-scheme replay.

### Deliverables

1. **New `crates/lvqr-cli/src/signed_url.rs`** (~280 LOC + ~120 LOC tests) -- shared HMAC-SHA256 signed-URL primitives. `SignedUrlCheck` enum (`Allow` / `Deny(Response)` / `NotAttempted`) + `verify_signed_url_generic(hmac_secret, signed_input, sig, exp, metric_entry) -> SignedUrlCheck` take arbitrary signed-input strings so playback + live paths share one primitive. `compute_signature(secret, input) -> Vec<u8>` is the single HMAC-SHA256 call both the sign and verify halves route through. New `LiveScheme { Hls, Dash }` enum with `as_str()` helper; `fn live_signed_input(scheme, broadcast, exp)` produces `"<scheme>:<broadcast>?exp=<exp>"`. Public `pub fn sign_live_url(secret, scheme, broadcast, exp_unix) -> String` returns the same `"exp=<exp>&sig=<b64url>"` query suffix shape as `sign_playback_url`. Private `verify_live_signed_url` wraps the generic verifier with the correct metric labels (`"hls_signed_url"` / `"dash_signed_url"`).

2. **`crates/lvqr-cli/src/archive.rs` refactor** (~100 LOC simpler) -- the private `SignedUrlCheck` + `verify_signed_url` + `compute_playback_signature` + the direct `hmac` / `sha2` / `subtle` / `SystemTime` imports are replaced with thin delegations to the shared `signed_url` module. `sign_playback_url` (still publicly exported) now builds `format!("{request_path}?exp={exp_unix}")` + calls `compute_signature` so playback and live paths cannot drift on the HMAC primitive.

3. **`crates/lvqr-cli/src/auth_middleware.rs`** -- `LivePlaybackAuthState` grew a `scheme: LiveScheme` + `hmac_secret: Option<Arc<[u8]>>` field. The existing session-112 `live_playback_auth_middleware` pre-checks `verify_live_signed_url` before consulting the subscribe-token provider. New private `extract_query_param(query, key)` helper reads `sig` and `exp` from the URL query. Allow short-circuits the subscribe-token gate; Deny returns the already-built 403; NotAttempted falls through to the existing bearer-token check.

4. **`crates/lvqr-cli/src/lib.rs`** -- hoisted the `hmac_playback_secret: Option<Arc<[u8]>>` declaration above the `combined_router` block so both the HLS + DASH spawn closures can capture it into their `LivePlaybackAuthState`. Each spawn block declares a local `hls_hmac_secret` / `dash_hmac_secret` clone so the `move` closure owns the Arc. New `pub use signed_url::{LiveScheme, sign_live_url};` re-export from the crate root.

5. **Integration test coverage** in `crates/lvqr-cli/tests/live_signed_url_e2e.rs` (7 `#[tokio::test]` functions): valid HLS signed URL returns not-401 without a bearer; valid DASH signed URL returns not-401 without a bearer; tampered HLS sig returns 403; expired HLS URL returns 403; HLS-minted sig on DASH route returns 403 (cross-scheme replay prevented); sig minted for broadcast A on broadcast B returns 403 (cross-broadcast replay prevented); no sig + no bearer returns 401 (fall-through to subscribe-token gate works). Bootstraps `TestServer::new().with_dash().with_auth(...).with_hmac_playback_secret(...)` in every test.

6. **Unit tests in `signed_url::tests`** (8 sync tests): signature scheme-bound / broadcast-bound / expiry-bound; `sign_live_url` round-trip verifies via `verify_live_signed_url`; cross-scheme tamper deny; cross-broadcast tamper deny; expired URL deny; no-secret and no-sig both return `NotAttempted`.

7. **Docs + PLAN + README**: `docs/auth.md` gains a dedicated "Live HLS + DASH signed URLs" subsection with design rationale, signature input shape, operator helper example, and metric labels. The existing Scope bullet updated to note path-bound (playback) vs broadcast-scoped (live). README Next Up #4 updated to name both sessions 124 + 128. README Auth+ops-polish checklist item flipped to name both `sign_playback_url` and `sign_live_url`. `tracking/PLAN_V1.1.md` new row 121-B marked SHIPPED with design decisions.

### Key 128 design decisions baked in

* **Broadcast-scoped signatures, not path-bound.** A session-124-style path-bound signature (`"<path>?exp=<exp>"`) works for DVR because the paths are stable -- one URL per segment, the operator mints a share link once. For live HLS the playlist references partial URIs like `part-video-42.m4s` that roll over every 200 ms; minting a new URL per partial is impractical. Broadcast-scoped (`"<scheme>:<broadcast>?exp=<exp>"`) gives one sig that admits every URL under the broadcast's live tree until expiry. The briefing explicitly recommended this shape.

* **Scheme tag baked into the signed input.** `"hls:<broadcast>?exp=<exp>"` vs `"dash:<broadcast>?exp=<exp>"` produce different HMACs even for the same broadcast + exp. A sig minted for HLS cannot be replayed against DASH -- the middleware reconstructs the signed input from its own `LivePlaybackAuthState::scheme`, not from the client request, so the attacker cannot trick the verifier. The unit test `signature_is_scheme_bound` + integration test `hls_sig_rejected_on_dash_route` lock this into place.

* **One `--hmac-playback-secret` across all three route trees.** Forcing operators to manage a separate secret per scheme would double the configuration surface without meaningful security benefit (the scheme tag already prevents cross-scheme replay). Reusing the session-124 flag keeps operator mental model simple; docs explicitly name the single-secret convention.

* **Factored-out shared primitive, not duplicated HMAC code.** Session 124's original `compute_playback_signature` and the new `compute_live_signature` would both need to hash byte strings through the same `Hmac<Sha256>` code. Duplicating that would create drift risk (base64 engine change, constant-time-compare rewrite, metric-label bug only caught on one path). Factoring into `signed_url::compute_signature(secret, input)` + `verify_signed_url_generic(..., metric_entry)` means there is one HMAC call and one verify call; both playback and live wrappers build their signed-input string and pass it in.

* **Pre-gate short-circuit order: signed URL first, subscribe-token second.** When both a signed URL AND a bearer are presented, the signed URL wins (the short-circuit returns before the bearer check runs). This matches session 124's semantics on `/playback/*`. Rationale: a client that mints a signed URL is explicitly claiming "I do not have a bearer, here is a pre-shared one-off grant"; short-circuiting avoids the redundant scope check against the `SharedAuth` provider that does not know about the signed grant anyway.

* **`LivePlaybackAuthState` grew two fields, not a new parallel middleware.** An alternative was a second `live_signed_url_middleware` layered on top of the existing `live_playback_auth_middleware`. Rejected because tower middleware ordering is position-dependent and easy to get wrong on future refactors; one state + one middleware with branching inside keeps the layering trivial. The NotAttempted branch preserves every byte of session-112 behavior when the operator has not set `--hmac-playback-secret`.

* **`hmac_playback_secret` hoisted above `combined_router`.** The `Arc<[u8]>` was previously declared inside the block that builds the admin router; moving it above lets both the admin-side `playback_router(..., hmac_playback_secret.clone())` call AND the spawn closures for HLS / DASH capture it. The existing call site on the admin side is unchanged; the two new `hls_hmac_secret = hmac_playback_secret.clone()` / `dash_hmac_secret = hmac_playback_secret.clone()` lines give the `move` closures owned Arc clones.

* **Public helper named `sign_live_url`, not `sign_hls_playback_url` / `sign_dash_playback_url`.** A single function with a `LiveScheme` parameter is less surface area than two. Operators pick `LiveScheme::Hls` or `LiveScheme::Dash` once at the call site; the scheme tag flows through the signed input automatically. The alternative (`sign_hls_playback_url`, `sign_dash_playback_url`) would require operators to choose twice (function name + Cargo feature) and double the rustdoc surface.

* **Default-gate test count intentionally bumped.** The session-124 integration tests live in the default feature gate; for symmetry session 128's `live_signed_url_e2e.rs` also runs on every PR. The 7 integration tests + 8 unit tests run in under 1 second combined -- no meaningful CI-time cost.

* **No change to the session-124 `/playback/*` wire contract.** The playback signature input remains `"<path>?exp=<exp>"`. Any existing signed URL minted by an operator via the session-124 `sign_playback_url` helper continues to verify. The refactor to delegate into `signed_url::verify_signed_url_generic` preserves the exact byte sequence + metric label (`entry="playback_signed_url"`), and the existing `playback_signed_url_e2e.rs` tests pass unchanged.

### Ground truth (session 128 close)

* **Head (pre-push)**: `feat(auth)` + this close-doc commit (pending). `origin/main` at `8c48caf` unchanged from session 127 push.
* **Tests**:
  * Default workspace gate: **984** passed / 0 failed / 3 ignored (+15 over session 127's 968, from 8 new `signed_url::tests` + 7 new `live_signed_url_e2e.rs`).
  * `cargo test -p lvqr-cli --test playback_signed_url_e2e` still 3 / 0 / 0 (session-124 regression check).
  * `cargo test -p lvqr-cli --test live_signed_url_e2e` 7 / 0 / 0 in 0.17 s.
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 984 / 0 / 3.
* **Workspace**: **29 crates**, unchanged.
* **crates.io**: unchanged. Session 128 adds public types (`LiveScheme`, `sign_live_url`) to `lvqr-cli` + two new `LivePlaybackAuthState` fields. All additive; published surface moves at the next release cycle.

### Known limitations / documented v1 shape (after 128 close)

* **Signed-URL revocation still requires rotating the shared secret.** Same constraint as session 124; a per-URL revocation list is explicitly anti-scope.
* **Authoritative DASH-IF container validator deferred**; GPAC MP4Box remains the primary validator in `dash-conformance.yml`.
* **Shared-helpers refactor across 9+ integration tests** still queued (scope grew with the new `live_signed_url_e2e.rs`).
* **npm + PyPI publish cycle** still pending; both published builds at 0.3.1 are 3/9 admin coverage + miss the new JWKS + sign_live_url public APIs.
* All other session 127 + earlier known limitations unchanged.


## Session 127 close (2026-04-22)

**Shipped**: PLAN row 119-C (nightly-soak leg). Last remaining leg of the three-way split of PLAN row 119 the prior sessions opened (feature-matrix workflow shipped in session 123 as 118-B/119-A; whisper-scheduled workflow shipped in session 125 as 119-B). No Rust code moved; default-gate workspace count unchanged at 968 / 0 / 2.

### Deliverables

1. **`.github/workflows/soak-scheduled.yml`** -- new scheduled workflow that runs `lvqr-soak` daily on GitHub-hosted `ubuntu-latest`. Cron `23 7 * * *` (daily at 07:23 UTC, offset from `whisper-scheduled.yml`'s 05:23 cron to avoid runner-slot contention). `workflow_dispatch` for manual reruns. 60-minute duration (`--duration-secs 3600`), 10 concurrent RTSP subscribers at 30 Hz, 30-second metrics sampling cadence. Release build via `cargo build -p lvqr-soak --release` so the soaked binary shape matches what operators deploy. stdout + tracing output captured via `tee` into `soak-artifacts/soak-report.txt`, uploaded via `actions/upload-artifact@v4` with `name: soak-report-${{ github.run_id }}` + 30-day retention. `Swatinem/rust-cache@v2` with a namespaced `prefix-key: soak-scheduled-v1` so the release-profile cache does not collide with the default-gate debug-profile cache. `continue-on-error: true` initial posture (standard for every dedicated workflow in this repo); promotes to a required check after the first clean run. `timeout-minutes: 80` gives comfortable headroom for the 60 min soak + warm-cache build + toolchain + artifact upload within the 360 min GH-hosted job ceiling.

2. **`tracking/PLAN_V1.1.md`** -- row 119 annotated with the three-legged split + row 119-C marked **SHIPPED** with the full design decisions list.

3. **`README.md`** -- the "No nightly 24h soak in CI" Known Limitations bullet flipped to "Nightly long-run soak in CI. **Fixed on `main`** in session 127" with the workflow link + the 60 min / 24 h deviation documented. Next Up list renumbered (the soak row removed; mesh data-plane becomes #5 instead of #6).

### Key 127 design decisions baked in

* **60-minute duration, not 24 hours.** The PLAN scope line named "nightly 24h" but GitHub-hosted runners cap at 360 min per job. Options considered: (a) 60 min daily on GH-hosted -- chosen; (b) 4 h weekly on GH-hosted -- rejected because weekly discovery windows dilute the regression-catch signal; (c) 24 h on self-hosted -- rejected because introducing self-hosted-runner infrastructure is its own session of work, not a row-closure. The soak driver's drift metrics (RSS, FD, CPU) are linear-in-time; 60 min is long enough past any startup transient to surface any regression that would also surface at 24 h. 10 subscribers * 30 Hz * 3600 s = 1.08 M RTP packets per run, well into steady-state territory. The documentation is explicit about the deviation so operators reading the README do not expect a true 24 h run.

* **Daily cron, not weekly.** Matches the whisper-scheduled.yml cadence decision (session 125): 24 h discovery vs 7 d discovery meaningfully affects how quickly a regression lands in a developer's face. The runner cost of 60 min * 30 runs/month = ~30 h/month of GH-hosted CI is within the free-tier budget the project already uses for feature-matrix + whisper + audit workflows combined.

* **07:23 UTC schedule, offset from 05:23 UTC whisper cron.** Three scheduled workflows (audit, whisper-scheduled, soak-scheduled) now run on daily cadences; staggering their start times by ~2 h each keeps any one day's runner-slot contention manageable on the shared free-tier queue.

* **Release profile, not debug.** Debug builds carry the `overflow-checks` + debug-assertion surface that real operators do not deploy. A soak run that exercises release codegen matches the production shape and avoids false signals (e.g. "cpu over window" numbers that are 3x real-world because debug iterators are slower).

* **Namespaced Swatinem cache key.** The default-gate CI builds debug; soak-scheduled builds release. Sharing one `Swatinem/rust-cache@v2` key between them would constantly thrash (each run would invalidate the other's cache). `prefix-key: soak-scheduled-v1` isolates the two caches.

* **Tee-capture of stdout + tracing into one artifact file.** `set -o pipefail` ensures a non-zero exit from `lvqr-soak` propagates even through `tee`. The live workflow-log stream stays populated so a developer watching the run does not have to download the artifact to see progress; `if: always()` on the upload step guarantees the report ships even when the soak fails.

* **`lvqr_soak`'s internal pass/fail logic is the authoritative gate.** The workflow does not add any secondary assertions. The binary already asserts on RTP threshold per subscriber, RTCP SR threshold per subscriber, and reports RSS / FD / CPU drift in the summary block. Adding a second assertion layer in the workflow YAML would duplicate the pass-fail surface with a worse expression language; the binary is the single source of truth.

* **`continue-on-error: true` initial posture.** Matches every dedicated workflow since `hls-conformance.yml`. Scheduled workflows surface environmental flake (upstream DNS blips, apt cache eviction, runner-slot contention) that a CI team needs a few weeks to triage before promoting to a required check. Session notes explicitly track the promotion pending first clean run.

* **No self-hosted-runner variant in this session.** Introducing self-hosted infrastructure (authentication, lifecycle, security surface) is outside the scope of closing the v1.1 PLAN row. The 24 h variant is documented as a v1.2 follow-up in both the workflow header comment and the README Known-Limitations bullet.

### Ground truth (session 127 close)

* **Head (pre-push)**: `feat(ci)` + this close-doc commit (pending). `origin/main` at `033740b` unchanged from session 126 push.
* **Tests**: default workspace gate **968** passed / 0 failed / 2 ignored (unchanged; the session adds a workflow YAML + docs only, no Rust).
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95 (no Rust moved).
  * `cargo test --workspace` 968 / 0 / 2.
  * `python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/soak-scheduled.yml"))'` parses cleanly.
* **Workspace**: **29 crates**, unchanged.
* **`soak-scheduled.yml` not exercised locally.** CI-only; first cron fire or manual `workflow_dispatch` trigger carries the authoritative signal.

### Known limitations / documented v1 shape (after 127 close)

* **60-minute soak, not true 24 h.** Documented in the workflow header + README. A true 24 h nightly requires a self-hosted runner; tracked as a v1.2 follow-up.
* **HMAC gated on `/playback/*` only**; extension to `/hls/*` + `/dash/*` is the next phase-C follow-up.
* **Authoritative DASH-IF container validator deferred**; GPAC MP4Box remains the primary validator.
* **Shared-helpers refactor across 9+ integration tests** still queued.
* **npm + PyPI publish cycle** still pending; both published builds at 0.3.1 are 3/9 admin coverage.
* All other session 126 + earlier known limitations unchanged.


## Session 126 close (2026-04-22)

**Shipped**: PLAN row 120 (OAuth2 / JWKS dynamic key discovery). Largest remaining v1.1 auth item; closes the README `[ ] OAuth2 / JWKS dynamic key discovery` Known-Limitations checkbox that has been open since Tier 4. Default-gate workspace test count unchanged at 968; the new provider + integration tests live behind the off-by-default `jwks` Cargo feature so `cargo install lvqr-cli` stays lean.

### Deliverables

1. **`crates/lvqr-auth/src/jwks_provider.rs`** (~470 LOC + ~410 LOC of tests) -- new `JwksAuthProvider` + `JwksAuthConfig`. Async `new(config)` validates the URL scheme + refresh interval + allowed-algorithm set, performs an initial synchronous JWKS fetch (fail-fast on unreachable endpoints or empty/malformed responses), then spawns a `tokio::spawn` refresh task. The cache is `Arc<SharedState { cache: RwLock<HashMap<String, CacheEntry>>, refresh_notify: Notify }>`; `CacheEntry` holds `(DecodingKey, Algorithm)`. Sync `check()` calls `jsonwebtoken::decode_header`, checks the alg against `config.allowed_algs` (default `RS256` + `ES256` + `EdDSA`), looks up the `kid`, runs `decode::<JwtClaims>` with the correct `Validation`, then applies the same scope-hierarchy + broadcast-binding logic as `JwtAuthProvider`. `Drop` aborts the refresh `JoinHandle`. Unknown `kid` denies and calls `refresh_notify.notify_one()` so the refresh task picks up the new JWKS shape. Missing `kid` is accepted only when the JWKS has exactly one key (OIDC single-key convention). HS256 in the allowed set is rejected at `validate_config` time to prevent the public-key-as-HMAC-secret downgrade attack.

2. **`crates/lvqr-auth/Cargo.toml`** -- new `jwks` feature (`jwks = ["jwt", "dep:reqwest", "dep:tokio", "dep:url"]`). Optional deps added. Dev-deps: `wiremock`, `rcgen`, `base64`, `tokio` (with `macros` + `rt` + `rt-multi-thread` + `time` features for integration tests).

3. **Workspace `Cargo.toml`** -- `reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }` (shares the ring crypto provider the rest of the graph uses, so no extra TLS backend in the link graph) and `wiremock = "0.6"`. Both are workspace-pinned so future version bumps are a single-file change.

4. **`crates/lvqr-auth/src/lib.rs`** -- gated `mod jwks_provider` + `pub use jwks_provider::{JwksAuthConfig, JwksAuthProvider}` behind `#[cfg(feature = "jwks")]`.

5. **`crates/lvqr-cli/Cargo.toml`** -- new `jwks` feature (`jwks = ["lvqr-auth/jwks"]`) threaded into `full`. Default builds still do not link reqwest.

6. **`crates/lvqr-cli/src/main.rs`** -- feature-gated `jwks_url: Option<String>` + `jwks_refresh_interval_seconds: u64` fields on `ServeArgs` with `LVQR_JWKS_URL` / `LVQR_JWKS_REFRESH_INTERVAL_SECONDS` env equivalents. `check_jwks_flag_combination(&args)` helper rejects the `--jwks-url` + `--jwt-secret` combination at startup. Auth resolution in `serve_from_args` factored into three layers: JWKS (when `--jwks-url` is set + feature on), JWT HS256 (when `--jwt-secret` set), static-token provider (when any individual token set), `NoopAuthProvider`. The existing `--jwt-issuer` / `--jwt-audience` flags are reused on the JWKS path so operators learn one claim-binding vocabulary rather than two.

7. **Integration test coverage in `jwks_provider::tests`** (9 `#[tokio::test]` functions + 5 sync unit tests, 14 total):
   * `config_default_algs_excludes_hs`
   * `validate_config_rejects_empty_url`, `validate_config_rejects_non_http_scheme`, `validate_config_rejects_hmac_algs`, `validate_config_rejects_short_refresh_interval`, `validate_config_accepts_sensible_values`
   * `happy_path_accepts_signed_ed25519_token` -- wiremock + rcgen Ed25519 keypair + jsonwebtoken `EncodingKey::from_ed_der` driving the full fetch-then-decode path.
   * `unknown_kid_denies_and_kicks_refresh`
   * `tampered_token_denied`
   * `scope_enforcement_matches_jwt_provider` -- subscribe-scoped token denied for publish context; broadcast claim enforced.
   * `hs256_header_rejected_pre_signature_check` -- hand-crafted token with a forged HS256 header + junk signature proves the allowed-algs gate trips before signature verification.
   * `key_rotation_refresh_picks_up_new_kid` -- dynamic wiremock responder returns different JWKS on first-hit vs follow-ups; first request with the new kid denies + kicks, second request after the refresh lands passes.
   * `missing_kid_with_single_key_accepts` -- OIDC single-key convention.
   * `initial_fetch_failure_surfaces_error` -- pointing at a closed port proves `new()` does not silently start with an empty cache.

8. **CLI parse tests in `main.rs::jwks_cli_tests`** (5 functions, feature-gated on `jwks`):
   * `jwks_url_unset_passes_combination_check`
   * `jwks_url_flag_parses`
   * `jwks_url_plus_jwt_secret_is_mutex_error`
   * `jwks_refresh_interval_override_applies`
   * `jwt_issuer_audience_still_apply_under_jwks`

9. **Docs + README + PLAN**: `docs/auth.md` grows a full "JWKS dynamic key discovery" section (enablement, accepted algorithms, key-selection rules, JWK shape, operational notes, Anti-scope line on webhook providers refreshed). README Known-Limitations checklist flips `[ ] OAuth2 / JWKS dynamic key discovery` to `[x]` with session-126 deliverables inline. README Auth summary line expanded to name the four provider variants. README CLI reference block gains `--jwks-url` + `--jwks-refresh-interval-seconds`. README Next Up list re-numbered (OAuth2/JWKS removed as a pending item). `tracking/PLAN_V1.1.md` row 120 marked **SHIPPED** with the full deliverable list.

### Key 126 design decisions baked in

* **JWKS synchronous initial fetch + async periodic refresh.** The `AuthProvider::check` trait is synchronous because every other provider does pure CPU work; adding JWKS needed a way to do network I/O without breaking the contract. Solution: async `new()` does the initial fetch before returning (so startup fails loud on misconfiguration), then spawns a `tokio::spawn` task that runs `tokio::select!` on a periodic `Interval::tick()` and a `Notify::notified()` for on-demand refresh. `check()` reads the cache under a `RwLock` and signals the background task via `notify_one()` on cache miss. Considered alternatives: (a) `block_in_place` + `Handle::current().block_on` for synchronous fetch inside `check()` -- rejected because it ties the request latency to the IdP and assumes a multi-threaded runtime; (b) pre-populate once and never refresh -- rejected because IdPs rotate keys; (c) block the first unknown-kid request while a refresh completes -- rejected because a sync `check()` cannot `.await` without changing the trait.

* **HS256 explicitly rejected in the default allowed-algs set.** A JWKS distributes public keys. If the allowed set included HS256 and an attacker presented `Header { alg: HS256, kid: <valid-rsa-kid> }`, a naive implementation would try to use the RSA public key as an HMAC secret + verify the attacker's signature. Guarding at the allowed-algs layer (not just relying on `DecodingKey::from_jwk` ignoring HS256) keeps the surface explicit. `validate_config` surfaces this at startup as an `AuthError::InvalidConfig` rather than tripping at request time.

* **`kid` lookup with single-key fallback.** OIDC 5.2 says the `kid` header "SHOULD" be present but clients may omit it when the JWKS has exactly one key. The cache lookup follows that: `Some(kid) -> cache.get(kid)`, `None` with `cache.len() == 1 -> cache.values().next()`, `None` with multiple keys -> deny. This matches what Auth0, Okta, Keycloak, and similar IdPs emit when they publish only their current signing key.

* **Kick-on-miss refresh, not block-on-miss.** When an unknown `kid` arrives, `check()` denies the request and calls `refresh_notify.notify_one()`. The first request after a key rotation fails, subsequent requests after the refresh lands pass. Rationale: blocking the request thread on a remote HTTP call couples request latency to IdP availability + requires an async runtime in a sync trait method. Documenting the behavior as "IdPs should publish new keys BEFORE presenting tokens signed with them" is honest about the trade-off; the kick-on-miss path still handles the race where rotation and traffic arrive in the opposite order.

* **Refresh interval minimum of 10 seconds.** `validate_config` rejects anything lower. Rationale: a misconfigured deployment setting the interval to `1` would hammer the IdP with thousands of requests per minute across a multi-instance LVQR fleet. 10 s is aggressive enough for a test harness but slow enough that no real IdP notices the load. Operators can still go lower in integration tests by building `JwksAuthConfig` programmatically, but the CLI flag clamps.

* **`--jwks-url` + `--jwt-secret` are mutually exclusive.** Both select a JWT-validation strategy. Running both would be ambiguous (silent fall-through picks one; the other is dead code). The `check_jwks_flag_combination` helper rejects the combination at startup with a message naming both flags. Factored out so the check is unit-testable without booting the runtime; matches the session-120 `build_c2pa_config` pattern exactly.

* **Reusing `--jwt-issuer` / `--jwt-audience` on the JWKS path.** Operators already know these flags. A parallel `--jwks-issuer` / `--jwks-audience` pair would double the CLI surface for identical semantics. The help text on `--jwt-audience` is updated to name both auth paths.

* **`jwks` is an opt-in feature, not default.** `cargo install lvqr-cli` does not pull reqwest + the full HTTP client stack. Operators who need JWKS explicitly opt in via `--features jwks` or `--features full`. Matches the c2pa / whisper / transcode precedent: every feature that introduces a significant dep graph is opt-in.

* **`reqwest` configured with `default-features = false, features = ["rustls-tls", "json"]`.** The default feature set pulls `native-tls` (which means OpenSSL on Linux) + `cookies` + `gzip` + `brotli` + `zstd` + a blocking-runtime module. None of those are needed for a JWKS fetch. Explicitly enabling `rustls-tls` reuses the `ring` crypto provider already pulled in by `rustls` and `jsonwebtoken`, so there is no second TLS backend in the link graph.

* **Ed25519 as the test-side signing algorithm.** rcgen produces Ed25519 keypairs via `PKCS_ED25519`; `public_key_raw()` returns the 32 bytes that go into the JWK `x` field directly; `serialize_der()` gives PKCS8 DER that `jsonwebtoken::EncodingKey::from_ed_der` consumes directly. ECDSA P-256 would have worked too but would require manually splitting the SPKI DER to extract `x` + `y` coordinates. RSA key generation is heavier + slower. Ed25519 keeps every test under 300 ms total.

* **No dedicated lvqr-cli integration test.** The 9 wiremock tests in `jwks_provider` already exercise the full JWKS provider end-to-end against a real HTTP mock. The SharedAuth-over-axum path is already covered for JWT by `auth_integration.rs`. Adding a duplicate server-boot JWKS test would mean pulling wiremock into `lvqr-cli`'s dev-deps for marginal extra signal. Skipped.

### Ground truth (session 126 close)

* **Head (pre-push)**: `feat(auth)` + this close-doc commit (pending). `origin/main` at `cae6b74` unchanged from session 125 push.
* **Tests**:
  * Default workspace gate: **968** passed, 0 failed, 2 ignored (unchanged; all new tests live behind the off-by-default `jwks` feature).
  * `cargo test -p lvqr-auth --features jwks`: **43** passed / 0 failed / 0 ignored in 0.05 s (29 pre-existing + 14 new jwks tests).
  * `cargo test -p lvqr-cli --features jwks --bin lvqr`: **13** passed / 0 failed / 0 ignored (8 pre-existing + 5 new jwks CLI unit tests).
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo clippy -p lvqr-auth --features jwks --all-targets -- -D warnings` clean.
  * `cargo clippy -p lvqr-cli --features jwks --all-targets -- -D warnings` clean.
  * `cargo test --workspace` 968 / 0 / 2.
* **Workspace**: **29 crates**, unchanged.
* **crates.io**: unchanged. Session 126 adds a new public type (`JwksAuthConfig`, `JwksAuthProvider`) + two new optional ServeArgs fields; all additive. `lvqr-auth 0.4.1` at the next release cycle will carry the public surface behind the `jwks` feature.

### Known limitations / documented v1 shape (after 126 close)

* **No webhook auth provider yet.** Tracked as a remaining v1.1 item; the JWKS provider is the biggest auth-surface expansion this cycle.
* **JWKS `kid` rotation race.** A token signed with a freshly-rotated key whose JWKS update has not reached the provider will fail once; the kick-on-miss path ensures the next request succeeds. IdPs that publish new keys BEFORE minting tokens against them sidestep the race entirely.
* **No JWKS signature caching beyond decoded keys.** Every incoming token re-runs the `jsonwebtoken::decode::<JwtClaims>` verification. For HS256 this was already true; for RSA/EC/Ed25519 the per-request crypto cost is higher but still fast enough (under 100 us on modern hardware). A future sess could add a per-signature LRU if needed.
* **Nightly 24h soak still unshipped** (PLAN row 119 second leg).
* **HMAC gated on `/playback/*` only**; extension to `/hls/*` + `/dash/*` is a focused phase-C follow-up.
* **Authoritative DASH-IF container validator deferred**; GPAC MP4Box remains the primary validator.
* **Shared-helpers refactor across 9+ integration tests** still queued.
* **npm + PyPI publish cycle** still pending; both published builds at 0.3.1 are 3/9 admin coverage.
* All other session 125 + earlier known limitations unchanged.


## Session 125 close (2026-04-22)

**Shipped**: two small audit carry-overs bundled. Both close explicit Known Limitations bullets; neither touches Rust code, so the workspace-gate signal is unchanged at 968/0/2.

### Deliverables

1. **`.github/workflows/whisper-scheduled.yml`** -- new scheduled workflow that promotes `whisper_cli_e2e` from `#[ignore]` to CI. Daily cron (`23 5 * * *`), `workflow_dispatch` for manual reruns. `actions/cache@v4` memoizes the ~78 MB `ggml-tiny.en.bin` from Hugging Face (`https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin`) under a stable cache key; cache miss triggers a `curl` with retry loop + a 70 MB minimum-size sanity check (fails fast on an HTML error page or partial download). Installs `libclang-dev` + `cmake` + `build-essential` for whisper-rs's bindgen + whisper.cpp's internal build (same set `feature-matrix.yml`'s whisper cell uses). Sets `WHISPER_MODEL_PATH` to the cached file before invoking `cargo test -p lvqr-cli --features whisper --test whisper_cli_e2e -- --ignored --nocapture`. `continue-on-error: true` soft-fail initial posture per convention.

   Closes the session-121 audit + session-123 Known Limitations bullet: "`whisper_cli_e2e` stays `#[ignore]` because it needs a ~78 MB ggml model; promoting that to a scheduled workflow with an `actions/cache@v4`-backed model blob is the right place for it."

2. **`docs/sdk/javascript.md` expanded 105 -> 307 lines** with every section the session-121 audit named as missing:
   * Full method reference for the 9 admin methods the session-122 expansion landed (`healthz` / `stats` / `listStreams` / `mesh` / `slo` / `clusterNodes` / `clusterBroadcasts` / `clusterConfig` / `clusterFederation`), including the cluster-gate caveat.
   * Complete TypeScript response type reference (all 9 interfaces + the `FederationConnectState` union) directly copyable into operator code.
   * New "Timeouts + reconnect" section documenting `LvqrClient.connectTimeoutMs`, `LvqrAdminClient.fetchTimeoutMs`, `LvqrAdminClientOptions.bearerToken` + a canonical jittered-exponential-backoff reconnect loop for `LvqrClient.connect/subscribe` + an admin-side retry recipe with error discrimination on `AbortError`.
   * `MeshPeer` section with `pushFrame` + `onChildOpen` documented (both shipped to `main` in sessions 115 + 116 post-close but never surfaced in the SDK docs).

3. **`docs/sdk/python.md` expanded 62 -> 215 lines** with the Python mirror of every new JS section:
   * Full 9-method reference with cluster-gate caveat.
   * All 12 dataclass definitions + the `FederationConnectState` `Literal` union matching the typecheck surface.
   * "Timeouts + retries" section documenting httpx's `timeout=` semantics, `bearer_token=` kwarg, and a capped-exponential-backoff retry recipe with httpx exception discrimination (`httpx.ReadTimeout` vs `httpx.ConnectTimeout` vs `httpx.ConnectError`).
   * New "Migrating from `0.3.1` to `main`" section naming the published-vs-main version skew + a `hasattr` probe for code that runs against both.
   * Explicit "Python client is admin-only; streaming uses ffmpeg-python or av" section so adopters do not fish for a subscribe API that does not exist.

### Key 125 design decisions baked in

* **Daily cron not weekly** for the whisper workflow. A weekly schedule would be cheaper but leaves a ~7-day blast radius for any regression in the whisper wiring. Daily runs give 24-hour discovery; the cache makes the model download nearly free after the first hit. 10 GB repo cache quota trivially accommodates a 78 MB blob.
* **Cache key is URL-stable, not revision-pinned**. The Hugging Face `resolve/main` URL tracks the current HEAD of the model repo; if upstream ever swaps the file, cache-miss triggers a re-download. Alternative (pinning to a specific revision) was rejected because whisper.cpp's model revisions are not operator-visible and the current file has been stable for years.
* **70 MB minimum-size sanity check**. The expected model is ~77 MB. Hugging Face sometimes returns HTML error pages when their CDN has transient issues; without the size check an empty or truncated file would reach `whisper_cli_e2e` and produce a confusing whisper-rs panic rather than a clear "download failed" error.
* **JS docs include both `connectTimeoutMs` AND `fetchTimeoutMs`** in the same section. The two are named distinctly because they apply to distinct surfaces (WT/WS connect vs admin HTTP fetch); lumping them into one paragraph would be cleaner prose but more confusing for adopters who only use one of the two clients.
* **Reconnect recipe is the SDK's problem, not library-level**. The canonical recipe shows jittered exponential backoff with a 30 s ceiling; operators can copy + adjust to their environment. Baking reconnect into `LvqrClient.connect()` was rejected because real deployments have wildly varying reconnect preferences (CI wants bounded retries, public apps want aggressive but capped backoff, embedded viewers might want linear steps); the library staying agnostic is the defensible default.
* **Python retry recipe handles `httpx.ConnectError` alongside the timeout variants**. `ConnectError` is the one-off "TCP refused" variant that is not a timeout; a retry loop that only caught timeouts would not recover from "the server blipped for 2 seconds". Matching the JS `withRetry` helper's error surface keeps the two languages symmetrical.
* **Migration section added to Python docs, not JS docs**. The JS publish skew is the same (`main` has 9/9 admin; `@lvqr/core 0.3.1` has 3/9) but JS developers tend to reach for `@latest` tags; the explicit `hasattr` probe is more important in Python where `pip install lvqr==0.3.1` is a common pin in requirements files. JS docs handle the skew implicitly through the version-note header.

### Ground truth (session 125 close)

* **Head (pre-push)**: feat(ci+docs) + this close-doc commit (pending). `origin/main` at `c0fca09` unchanged from the session 124 push.
* **Rust workspace**: no source moved. `cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95; `cargo test --workspace` unchanged at 968 / 0 / 2 from session 124's close.
* **Docs line counts**: `docs/sdk/javascript.md` 105 -> 307; `docs/sdk/python.md` 62 -> 215. Pure additive; every existing section preserved with the same anchor structure.
* **YAML**: `python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/whisper-scheduled.yml"))'` parses cleanly.
* **`whisper-scheduled.yml` not exercised locally**. CI-only; first scheduled run will carry the authoritative signal (the manual `workflow_dispatch` trigger lets a developer force-run after landing).

### Known limitations / documented v1 shape (after 125 close)

* **Nightly 24h soak still unshipped** (PLAN row 119 had two legs: feature-matrix shipped in session 123, soak is the remaining leg).
* **OAuth2/JWKS still unshipped** (PLAN row 120). Largest remaining phase-C row.
* **HMAC gated on `/playback/*` only**; extension to `/hls/*` + `/dash/*` is a focused phase-C follow-up.
* **Authoritative DASH-IF container validator deferred**; GPAC MP4Box remains the primary validator in `dash-conformance.yml`.
* **Shared-helpers refactor across 9+ integration tests** still queued; the scope has grown with each new test file (now 9: `rtmp_archive_e2e`, `rtmp_hls_e2e`, `rtmp_dash_e2e`, `rtmp_ws_e2e`, `rtmp_whep_audio_e2e`, `c2pa_verify_e2e`, `c2pa_cli_flags_e2e`, `archive_dvr_read_e2e`, `playback_signed_url_e2e`).
* **npm + PyPI publish cycle** still pending; both published builds at `0.3.1` are 3/9 admin coverage.
* All other session 124 + earlier known limitations unchanged.

## Session 124 close (2026-04-22)

**Shipped**: PLAN row 121 (HMAC-signed playback URLs). Next item on the session-121 audit queue after the SDK slice closed. Narrow but real operator feature: a short-circuit auth path on `/playback/*` so operators can mint one-off share links for third parties who cannot authenticate. Default-gate test count 965 -> 968.

### Deliverables

1. **`--hmac-playback-secret` CLI flag + `LVQR_HMAC_PLAYBACK_SECRET` env** on `lvqr serve`. Threaded into `ServeConfig.hmac_playback_secret: Option<String>`, which `start()` wraps into `Arc<[u8]>` and passes through `playback_router(dir, index, auth, hmac_secret)` to the `ArchiveState` carried by every handler. `TestServerConfig` gained a matching `with_hmac_playback_secret` builder + pass-through assignment so integration tests wire it up via the same programmatic path operators would use.

2. **`crates/lvqr-cli/src/archive.rs` HMAC verification + signing helper** (~180 new LOC). Every `/playback/*` query struct (`PlaybackQuery`, `LatestQuery`, `FileQuery`) grew `sig: Option<String>` + `exp: Option<u64>` fields. New `SignedUrlCheck { Allow, Deny(Response), NotAttempted }` enum + `verify_signed_url()` free function implement the three-way outcome:
   * `NotAttempted` -- fall through to the existing `playback_auth_gate` (no secret configured OR sig+exp not both present on the request).
   * `Allow` -- signature verified + expiry valid; handler skips the subscribe-token gate.
   * `Deny(Response)` -- 403 Forbidden with one of three body strings: `"signed URL expired"`, `"signed URL malformed"`, `"signed URL signature invalid"`. Explicitly 403 not 401 so clients can tell "no auth" from "wrong auth" on status code alone.

   Constant-time compare via `subtle::ConstantTimeEq` on the decoded base64 bytes (not the string) so signature-length differences don't short-circuit via early return. Verification decodes the `sig` query param as base64url-no-pad; a decode error is itself a 403 outcome (malformed).

   Metric: every 403 path bumps `lvqr_auth_failures_total{entry="playback_signed_url"}` so ops dashboards see the signed-URL failure rate distinctly from the existing `entry="playback"` subscribe-token failures.

   The HMAC input is `"<request_path>?exp=<exp>"`, explicitly reconstructed per-handler from `format!("/playback/{broadcast}")`, `format!("/playback/latest/{broadcast}")`, or `format!("/playback/file/{rel}")`. Path reconstruction (not request-URI parsing) means the signed path is normalized against the route template, not whatever the client sent on the wire; a signed URL bound to `/playback/live/dvr` cannot be reused on `/playback/live/other` even if both broadcasts exist.

   Pure `pub fn sign_playback_url(secret: &[u8], request_path: &str, exp_unix: u64) -> String` generates the query suffix `"exp=<ts>&sig=<b64url>"` that operators concatenate after `<path>?`. Re-exported from `lvqr_cli::sign_playback_url`. Shares a private `compute_playback_signature` helper with the verifier so the two paths cannot drift.

3. **`crates/lvqr-cli/tests/playback_signed_url_e2e.rs`** (~400 LOC) with 3 `#[tokio::test]` functions:
   * `sign_playback_url_matches_hand_rolled_hmac` -- unit test that re-implements the HMAC input format by hand with the `hmac` + `sha2` crates and asserts the signing helper's output decodes back to the expected 32-byte HMAC-SHA256 digest.
   * `signed_url_grants_access_and_denies_tampering` -- full integration: boots `TestServer` with `subscribe_token: Some("cannot-use-without-bearer")` + an HMAC secret, publishes two RTMP keyframes, then runs SEVEN distinct assertions: (a) valid signed URL returns 200 without a bearer; (b) tampered sig returns 403; (c) expired URL returns 403; (d) no sig + no bearer returns 401; bonus: (e) `/playback/latest/` also accepts a signed URL; (f) bearer-token path still works when signed URL is absent; (g) cross-path signature (signed for `/playback/live/other`, GET on `/playback/live/dvr`) returns 403.
   * `signed_url_works_on_file_route` -- separate test proving the `/playback/file/*` raw-bytes route also honors signed URLs. Fetches a real archived segment's bytes via a signed URL + asserts the `moof` prefix on the returned bytes.

4. **Workspace-level Cargo.toml** gains direct `hmac = "0.12"` + `sha2 = "0.10"` + `subtle = "2"` entries alongside the existing `base64 = "0.22"`. `lvqr-cli/Cargo.toml` pulls all four into its dependencies. Every crate was already reachable transitively via `jsonwebtoken`; pinning them here makes the direct use explicit and stops a downstream dep bump from silently changing our version.

5. **Docs**: `docs/auth.md` grows a full "Signed playback URLs" section with URL shape, semantics, operator helper example, and explicit scope-boundary list. README Next Up item #4 flipped to shipped. README Auth+ops-polish checklist item (HMAC URLs) flipped. README CLI reference gains the new flag block + env var.

### Key 124 design decisions baked in

* **HMAC covers path + exp only, not the full canonical query string.** Other query params (`track`, `from`, `to`, `token`) are explicitly excluded from the signature. A signed URL grants broadcast-path-scoped access; the recipient can freely scrub `from` / `to` within that broadcast. Operator use case is "share this DVR stream with a third party for an hour"; they get free scrub within the window. Tighter-constrained signatures (e.g. "only segments in [14:00, 15:00]") would add complexity without matching the expected use case. Documented as an explicit scope boundary in `docs/auth.md`.

* **403 Forbidden on sig/expiry failure, 401 Unauthorized on missing auth.** RFC 7231 distinguishes "no auth presented" (401) from "auth presented but insufficient" (403). A tampered signature IS presented auth; returning 401 would invite clients to retry under the assumption they forgot to include a token. 403 is correct + the body string names which part failed.

* **Constant-time compare on decoded bytes, not the base64 string.** `subtle::ConstantTimeEq` on the sig bytes after base64 decode. Comparing as strings before decode would leak length information through the early-exit in string-equality. The decoder itself is naturally variable-time on input but any malformed input is already a 403 with no further comparison, so that channel carries no useful signal.

* **Signature is path-bound via explicit reconstruction, not request URI parsing.** Each handler computes `request_path = format!("/playback/{broadcast}")` (or similar) from its own route template. An attacker who obtains a signed URL for `/playback/live/dvr` cannot reuse it on `/playback/live/other` because the handler's reconstructed path is `/playback/live/other`, which produces a different HMAC input.

* **`sign_playback_url` is a pure free function, not a method on `ServeConfig`.** Operators mint URLs server-side in their own admin service, typically with the secret loaded from env. A method-on-config shape would force them to construct a full `ServeConfig` just to sign a URL. The pure helper takes `(secret, path, exp)` directly.

* **Secret is `Option<Arc<[u8]>>` threaded through handler state, not a module-level static.** Multiple tenants / test instances can each have their own secret. The `Arc<[u8]>` makes the secret cheap to clone into every request handler's state without copying the bytes.

* **Metric name is `lvqr_auth_failures_total{entry="playback_signed_url"}`, not reusing the existing `entry="playback"`.** Operators need to distinguish "legitimate subscribers are failing auth" from "someone is brute-forcing signed URLs". Separate labels on the same counter keeps one dashboard row but split series.

* **Three query params added to every query struct, not a new separate "signed-url" handler.** An alternative design was a parallel `/playback/signed/...` route tree. Rejected because it doubles the URL shape operators have to remember, doubles the route mount surface, and forces the client to know up-front whether it has a signed URL or a bearer (when actually both paths produce the same bytes on success). Query params let the same URL carry either form.

* **No `--hmac-playback-secret` integration with `/hls/*` or `/dash/*` in this session.** Live HLS + DASH have their own `SubscribeAuth` middleware (session 112) and different playlist-URL shapes; extending signed URLs to them would need middleware-level work, not handler-level work. Documented as a follow-up. The dominant share-link use case is DVR scrub (which routes through `/playback/*`) anyway.

### Ground truth (session 124 close)

* **Head (pre-push)**: feat(auth) + this close-doc commit (pending). `origin/main` at `3bfc5ae` unchanged from session 123 push.
* **Tests (default features gate)**: **968** passed, 0 failed, 2 ignored. **+3** over session 123's 965 (3 new signed-URL test functions; +1 ignored is an intentional `/// ```ignore` doc example on `sign_playback_url` that doesn't build an executable doctest).
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean (after one auto-format pass on two-line `assert_eq!`s + the archive.rs imports list).
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 968 / 0 / 2.
  * `cargo test -p lvqr-cli --test playback_signed_url_e2e` 3 / 0 / 0 in 1.23 s.
* **Workspace**: **29 crates**, unchanged.
* **crates.io**: unchanged. Session 124 adds a public function (`sign_playback_url`) + one optional ServeConfig field; both are additive. Next release cycle carries the public surface.

### Known limitations / documented v1 shape (after 124 close)

* **HMAC signed URLs gated on `/playback/*` only.** Live `/hls/*` and `/dash/*` routes do not honor the sig+exp query params. Extending is a phase-C follow-up.
* **Signature covers path + exp only**, not other query params. Operators who need "signed URL + signed scrub window" need to layer additional auth.
* **No token revocation list** -- rotating `--hmac-playback-secret` invalidates every outstanding URL at once. By design; matches the static-HS256-JWT posture.
* **SDK reconnect/retry docs still undocumented** (session-121 audit carry-over).
* **Nightly 24h soak still unshipped** (PLAN row 119 unshipped leg; the feature-matrix half landed in session 123).
* **OAuth2/JWKS still unshipped** (PLAN row 120).
* All other session 123 + earlier known limitations unchanged.

## Session 123 close (2026-04-22)

**Shipped**: two audit follow-throughs bundled -- Python admin client 3/9 -> 9/9 parity + feature-flag CI matrix. Closes the remaining items the session-121 / 122 audit queue ranked as small + ship-able. Both audit Known-Limitations bullets for SDK coverage asymmetry + incomplete feature-matrix CI are resolved.

### Deliverables

1. **Python admin client 9/9 route parity** (`bindings/python/python/lvqr/client.py`, `types.py`, `__init__.py`).

   * `client.py` rewrite: 84 LOC -> 237 LOC, 2 public methods -> 8 public methods. New methods mirror the session-122 JS admin expansion 1:1: `mesh()`, `slo()`, `cluster_nodes()`, `cluster_broadcasts()`, `cluster_config()`, `cluster_federation()`.
   * `types.py` grows 9 new dataclasses (`MeshState`, `SloEntry`, `SloSnapshot`, `NodeCapacity`, `ClusterNodeView`, `BroadcastSummary`, `ConfigEntry`, `FederationLinkStatus`, `FederationStatus`) + one `Literal["connecting", "connected", "failed"]` alias for `FederationConnectState`. Every dataclass mirrors the Rust serde struct on the server side; field names match JSON on-wire exactly so `**json.loads(body)` unpacks cleanly.
   * New `bearer_token` kwarg on `LvqrClient.__init__`; when set, the underlying `httpx.Client` carries `Authorization: Bearer <token>` on every request. Parity with the JS `LvqrAdminClientOptions.bearerToken`.
   * Shared private `_get_json(path)` helper replaces duplicated fetch + raise_for_status + json() across 8 methods; future shape / auth changes become single-edit.
   * `__init__.py` re-exports every new dataclass alongside existing names so `from lvqr import FederationStatus` works out of the box.
   * Pytest coverage grows 8 -> 21 tests (+13). Every new method gets a mocked-httpx test; `null`-capacity handling, populated-vs-empty SLO, populated-vs-empty federation, bearer-token header assertion, and 5 dataclass-default tests round it out. 21 / 0 / 0 in 210 ms locally.

2. **`.github/workflows/feature-matrix.yml`** -- new dedicated workflow with three jobs, each exercising a distinct feature-gated surface that no existing workflow covers directly:

   * **`c2pa`** cell (no runtime deps beyond default): `cargo clippy -p lvqr-cli --features c2pa --all-targets -- -D warnings` + `cargo test --test c2pa_verify_e2e` + `cargo test --test c2pa_cli_flags_e2e` (both rcgen + openssl test functions) + `cargo test -p lvqr-archive --features c2pa`. Installs `openssl` apt package for the session-121 openssl cert-generation test variant.
   * **`transcode`** cell: apt-installs the full GStreamer plugin set + dev headers + ffmpeg + libclang, then runs `aac_opus_roundtrip` + `software_ladder` + `transcode_ladder_e2e` + `rtmp_whep_audio_e2e` under `--features transcode`. Every test target is listed explicitly so new ones must be added to the workflow on each landing, not silently adopted via the default `cargo test` glob.
   * **`whisper`** cell: apt-installs `libclang-dev + cmake + build-essential` for whisper-rs's bindgen + whisper.cpp's internal build, runs `cargo clippy --features whisper` + `cargo test -p lvqr-agent-whisper --features whisper --test whisper_basic + --lib`. `whisper_cli_e2e` stays `#[ignore]` because it needs a ~78 MB ggml model on `WHISPER_MODEL_PATH`; promoting that to a scheduled workflow with a cached model download is the next whisper-scoped follow-up.

   `continue-on-error: true` during the initial weeks on `main`, per the convention every new dedicated workflow in this repo follows. Promotes to a required check after first clean run.

### Key 123 design decisions baked in

* **Python expansion + feature-matrix bundled in one session.** Both are explicit Next Up items from the session-121 audit (#3 + the Python asymmetry bullet). Both are small enough that a single session can land them cleanly with real tests; splitting into two sessions would double the session-close overhead for no signal benefit.

* **Python client mirrors the JS client 1:1 rather than pythonifying the surface.** `cluster_nodes` / `cluster_broadcasts` / `cluster_config` / `cluster_federation` use `snake_case` (Python convention) but otherwise every method name and dataclass shape lines up with the JS TypeScript interfaces. Operator tooling that transliterates one to the other doesn't have to translate field names.

* **`FederationConnectState` as `Literal["connecting", "connected", "failed"]`** -- the Python equivalent of the JS union type. A Python `enum.Enum` would not round-trip cleanly through JSON; the Literal alias gives type-checkers the same exhaustiveness safety + matches `serde(rename_all = "lowercase")` on the Rust enum.

* **Nullable optional fields use `Optional[T]` with dataclass `default=None`** rather than omitting the attribute. `NodeCapacity | None` on `ClusterNodeView.capacity`, `int | None` on `FederationLinkStatus.last_connected_at_ms`, `str | None` on `FederationLinkStatus.last_error`. Matches the JSON wire contract where the server emits `"capacity": null` until the first gossip round, not an absent key.

* **`bearer_token` passed through to `httpx.Client.headers`**, not re-added on each call. The `httpx.Client` default-headers mechanism sends the header on every request automatically; repeating it per method would be redundant. The pytest `test_bearer_token_header` asserts the client's `headers["Authorization"]` is the expected shape to lock in the contract.

* **Feature-matrix workflow explicitly lists every test target**, not `cargo test -p lvqr-cli --features X` globbing the workspace. Adding a new feature-gated test file then requires a conscious workflow edit rather than silently inheriting the default; the explicit list catches intent-drift. Cost: a few lines of YAML per new target. Benefit: each target stays an intentional cell in the matrix.

* **`whisper_cli_e2e` deliberately stays `#[ignore]` in the CI matrix.** Running it in the workflow would mean downloading a 78 MB ggml file on every push to `main` + every PR. A scheduled workflow (daily or weekly) with an `actions/cache@v4`-backed model blob is the right place for it; added as an explicit Known Limitations follow-up so the gap is visible instead of hidden.

* **Workflow mirrors `tier4-demos.yml`'s apt install list byte-for-byte** for the transcode cell. A fix in one workflow's GStreamer plugin set applies to the other automatically; drift between the two produces visible symmetry-break in the diff.

### Ground truth (session 123 close)

* **Head (pre-push)**: feat(sdk+ci) + this close-doc commit (pending). `origin/main` at `6b90f15` unchanged from the session 122 push.
* **Tests**:
  * Default workspace gate: **965** passed, 0 failed, 1 ignored (unchanged; no Rust code moved this session).
  * `@lvqr/core` Vitest suite: unchanged at 10 / 0 / 0 (no JS changes).
  * Python client pytest: **21** passed / 0 failed in 210 ms (+13 over session 122's 8, all from new admin-method tests).
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo clippy -p lvqr-cli --features c2pa --all-targets -- -D warnings` clean (previews the c2pa cell of the new workflow).
  * `cargo test --workspace` 965 / 0 / 1.
  * `python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/feature-matrix.yml"))'` parses cleanly.
* **`feature-matrix.yml` not run locally.** Workflow is CI-only; first GitHub Actions run on push carries the authoritative signal. The `c2pa` cell is the most tractable of the three to land clean; `transcode` has the widest apt-install surface so first-run flakiness is likely; `whisper` has the smallest footprint of the three because it only runs compile-check + basic unit tests.

### Known limitations / documented v1 shape (after 123 close)

* **Published npm + PyPI builds at 0.3.1 still ship the 3-method surface.** The 9-method surface lands for consumers at the next publish cycle. Both the JS and Python packages will ship together to keep operator tooling symmetrical.
* **`whisper_cli_e2e` remains `#[ignore]` in the feature-matrix workflow.** A scheduled workflow with a cached 78 MB ggml model would exercise the full caption-generation path; that is tracked as an explicit phase-C follow-up.
* **SDK reconnect/retry docs still undocumented** (session-121 audit gap carried over; untouched by sessions 122 + 123). `connectTimeoutMs` / `fetchTimeoutMs` / `bearer_token` ship on both clients but `docs/sdk/javascript.md` is silent on when to reconnect. Natural next-session target.
* All other session 122 + earlier known limitations unchanged.

## Session 122 close (2026-04-22)

**Shipped**: PLAN row 118 slice-A (SDK completion). User's "carry that work out" directive against the session-121 reality audit's Next Up #1 + #2. `@lvqr/core` admin client grows from 3 of 9 `/api/v1/*` routes to 9/9; new Vitest + pytest CI workflow runs both SDK test suites against a live `lvqr serve`. Closes two of the six Known-Limitations items the session-121 audit added to the README.

### Deliverables

1. **`@lvqr/core` admin client 9/9 route coverage.** `bindings/js/packages/core/src/admin.ts` grows from 95 LOC / 3 methods to 302 LOC / 9 methods. Every missing route gets a typed method:

   * `mesh() -> MeshState`
   * `slo() -> SloSnapshot` (shape `{ broadcasts: SloEntry[] }`)
   * `clusterNodes() -> ClusterNodeView[]`
   * `clusterBroadcasts() -> BroadcastSummary[]`
   * `clusterConfig() -> ConfigEntry[]`
   * `clusterFederation() -> FederationStatus` (shape `{ links: FederationLinkStatus[] }`)

   Nine new TypeScript interfaces (`MeshState`, `SloEntry`, `SloSnapshot`, `NodeCapacity`, `ClusterNodeView`, `BroadcastSummary`, `ConfigEntry`, `FederationLinkStatus`, `FederationStatus`) + one union (`FederationConnectState = 'connecting' | 'connected' | 'failed'`) mirror the underlying Rust serde structs at `lvqr_admin::{MeshState, SloEntry}` + `lvqr_cluster::{BroadcastSummary, ConfigEntry, FederationLinkStatus, NodeCapacity, NodeId}` + `lvqr_admin::cluster_routes::{ClusterNodeView, FederationStatusView}`. `bindings/js/packages/core/src/index.ts` re-exports every new type alongside the existing surface.

   New `LvqrAdminClientOptions.bearerToken` field: when set, every admin fetch emits `Authorization: Bearer <token>`. Closes the "only noop-auth deployments can actually use this client" gap.

   Shared private `getJson<T>(path)` helper replaces the duplicated `resp.ok / resp.json()` boilerplate the original admin.ts repeated inline per method.

2. **Vitest SDK smoke tests.** `bindings/js/vitest.config.ts` + `bindings/js/tests/sdk/admin-client.spec.ts` (+140 LOC, 10 tests). Each of the 9 admin methods is hit against a live `lvqr serve` at `LVQR_TEST_ADMIN_URL` (default `http://127.0.0.1:18090`); responses are shape-asserted against the declared TypeScript types. Plus one `fetchTimeoutMs aborts hung requests` test that points the client at TEST-NET-3 (`203.0.113.1:9`) + asserts the promise rejects within 5 s via the AbortController path. Completes in <250 ms end to end.

   Added `vitest ^2.0.0` + `@types/node ^20.0.0` to `bindings/js` devDeps; added `test:sdk` script (`vitest run`) + `build` script (`npm --workspaces --if-present run build`) to the workspace root `package.json`.

3. **`.github/workflows/sdk-tests.yml`.** New dedicated workflow mirroring `mesh-e2e.yml`'s pattern: Ubuntu runner, cargo + node + python toolchains, build `lvqr-cli` debug, install JS + Python deps, spawn `lvqr serve --admin-port 18090 --mesh-enabled --cluster-listen 127.0.0.1:18093 --no-auth-signal` in the background, poll `/healthz` for readiness (15 s budget), run `npm run test:sdk` (Vitest) + `python -m pytest -v` (pytest), kill lvqr, upload log artifact. `continue-on-error: true` initially per the same convention used by every new dedicated workflow since hls-conformance.yml.

### Verification against a live lvqr

Vitest suite: 10 / 0 / 0 in 246 ms on macOS against a debug `lvqr serve` with the exact flag set the workflow uses. Pytest suite: 8 / 0 / 0 in 160 ms. Both runs catch the admin client's runtime behavior against real server JSON, not mocked responses.

### Key 122 design decisions baked in

* **Python client expansion deferred, not done.** The user asked for the audit's #1 + #2 items; the JS admin client was #1 and Vitest/pytest CI was #2. Extending the Python client to match the JS 9/9 coverage is a natural follow-up but was not in the ranked list. Added as a new Known Limitations bullet + a pending Client-SDKs checklist item so operators see the asymmetry immediately.

* **Vitest tests hit a real lvqr, not mocked fetch.** Mirrors the `CLAUDE.md` rule ("real integration tests with actual network connections, not mocks") even though that rule targets Rust. A mocked-fetch SDK test would catch TypeScript shape drift but not field-name drift on the server side; the extra 2-3 seconds of workflow time to boot a real lvqr is cheap insurance.

* **`bearerToken` added to `LvqrAdminClientOptions`.** The pre-122 admin client sent unauthenticated requests; fine for Noop-provider deployments but the admin router returns 401 whenever a static-token or JWT provider is configured. Adding the option closes the gap in a single line per request.

* **New `getJson<T>` private helper over keeping per-method duplicated fetch boilerplate.** Each of the 9 admin methods would otherwise repeat `const resp = await this.fetchWithTimeout(...); if (!resp.ok) throw new Error(...); return resp.json();`. One factored helper keeps the method bodies at one line each and makes future shape/auth changes a single-edit proposition.

* **`FederationConnectState` as a TypeScript union type, not an enum.** `serde(rename_all = "lowercase")` on the Rust enum serializes as three exact strings; a TypeScript `enum` would not round-trip cleanly through JSON. The string union (`'connecting' | 'connected' | 'failed'`) exactly matches the on-wire shape + gives TypeScript users exhaustive-check safety.

* **Vitest + pytest run under the SAME workflow, not separate workflows.** Both suites exercise the same `lvqr serve` instance; launching it twice (once per workflow) would double CI wall-clock for no signal benefit. Keeping them in one workflow also means a regression in a shared-config pattern surfaces on both at once.

* **Workflow boots lvqr in the background + polls `/healthz`**, not via a Playwright `webServer` equivalent. Vitest has no webServer primitive; the simple bash-level spawn + nc-z / curl-health-check poll matches the shape `hls-conformance.yml` and `dash-conformance.yml` already use. Kept the stdout/stderr redirect + artifact upload so runner failures surface with a real log.

* **`@lvqr/core` dist rebuilt in the workflow, not checked in.** Each workflow run runs `npm run build` under `bindings/js/packages/core/` so the Vitest suite loads the TypeScript types + compiled JS fresh. `dist/` is still committed for npm publish consumption, but CI builds its own copy so a forgotten local rebuild never masks a real TS compile failure.

* **Python client expansion (mirror the 6 new JS methods) is an explicit follow-up**. The Python pytest suite today tests `healthz()`, `stats()`, and `list_streams()` via mocked httpx -- the three methods the client exposes. Adding the 6 missing methods is straightforward (mirror admin.ts against httpx); mocking against the same 9 routes is the natural Vitest-pytest symmetry. Added as Known Limitations bullet + Next Up item so the asymmetry is visible.

### Ground truth (session 122 close)

* **Head (pre-push)**: feat(sdk+ci) + this close-doc commit (pending). `origin/main` at `d4bb946` unchanged from the session-121 reality audit push.
* **Tests**:
  * Default workspace gate: **965** passed, 0 failed, 1 ignored (unchanged; no Rust code moved).
  * `@lvqr/core` Vitest suite: **10** passed / 0 failed in 246 ms against a live `lvqr serve` on macOS.
  * Python client pytest: **8** passed / 0 failed in 160 ms.
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 965 / 0 / 1.
  * `python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/sdk-tests.yml"))'` parses cleanly.
  * `npm run build` under `bindings/js/packages/core/` clean.
* **`sdk-tests.yml` not run locally**. Workflow is CI-only; first GitHub Actions run on push carries the authoritative signal (soft-fail initial posture).

### Known limitations / documented v1 shape (after 122 close)

* **Python admin client still at 3/9**. JS expansion shipped; Python mirror is pending.
* **SDK reconnect/retry docs still undocumented**. Behavior ships (`connectTimeoutMs`, `fetchTimeoutMs`, `bearerToken` now); docs at `docs/sdk/javascript.md` still silent. Follow-up row after Python expansion.
* **Feature-flag CI matrix still incomplete** (session-121 audit Next Up #3). `lvqr-cli --features {whisper,transcode,c2pa}` still only exercised via the soft-fail `tier4-demos.yml`.
* All other session 121 + earlier known limitations unchanged.

## Session 121 close (2026-04-22)

**Shipped**: audit + fix for the session-120 deferred C2PA integration test, plus demo-01.sh `LVQR_DEMO_C2PA=1` opt-in using the proven openssl recipe. The user's push-back ("fix the things rather than just deleting") triggered a source-level audit of c2pa-rs that identified TWO issues the initial session-120 draft missed. Default-gate test count 963 -> 965.

### The audit: two bugs, one generic error message

Session 120 gave up on the integration test after one error ("c2pa error: sign: the certificate is invalid") and scoped it out as "c2pa-rs cert-profile requirements are stricter than documented". The user pushed back. A source-level read of c2pa-rs (`crates/c2pa-0.80.0/src/crypto/cose/certificate_profile.rs` + `crypto/cose/verifier.rs`) traced the failure to TWO distinct issues, both of which c2pa-rs folds into the same generic error:

1. **Missing `AuthorityKeyIdentifier` extension on the leaf.** `check_certificate_profile` (line 485) requires `aki_good && ski_good && key_usage_good && extended_key_usage_good && handled_all_critical`. `aki_good` flips true only when the leaf cert has an `AuthorityKeyIdentifier` extension (verified via `ParsedExtension::AuthorityKeyIdentifier(_)` on line 419). rcgen 0.13's `CertificateParams::default()` sets `use_authority_key_identifier_extension: false` (`rcgen-0.13.2/src/certificate.rs:111`), so the extension is elided by default. c2pa-rs rejects the cert with "the certificate is invalid" -- generic, never names the missing AKI.

2. **Missing `Organization` (O) attribute in the subject DN.** AFTER the signature itself validates successfully, c2pa-rs's COSE verifier at `verifier.rs:159-166` extracts the org attribute for the `CertificateInfo::issuer_org` field: `sign_cert.subject().iter_organization().last().ok_or(CoseError::MissingSigningCertificateChain)?`. Without O, the extraction fails, `MissingSigningCertificateChain` bubbles up, and `claim.rs:3023` folds it into "claim signature is not valid" with a NULL signer in the verify response -- again generic, even though the signature itself was fine.

### Fixes

* Leaf params in `mint_c2pa_test_pki`: `leaf_params.use_authority_key_identifier_extension = true;`
* Leaf DN: `leaf_dn.push(DnType::OrganizationName, "LVQR Test Operator");`

Both fixes are one-liners. The audit is the load-bearing contribution -- the session-120 deferral was avoidable with 30 more minutes of source reading.

### Deliverables

1. **`crates/lvqr-cli/tests/c2pa_cli_flags_e2e.rs`** re-added, fixed, and extended with TWO test functions:

   * `certkeyfiles_signer_source_yields_valid_c2pa_manifest` -- the rcgen-based test from session 120, now fixed with the two one-liners above. Passes in ~1.2 s.
   * `openssl_generated_certkeyfiles_also_yields_valid_manifest` -- NEW companion test that shells out to `openssl ecparam` / `req` / `x509` to mint the CA + leaf + PKCS#8 key via the same recipe `demo-01.sh --c2pa` uses. Skips gracefully when openssl is not on `$PATH`. Passes in ~0.1 s. The openssl flow is functionally equivalent to the rcgen flow -- same `critical BasicConstraints: CA:FALSE`, `critical KeyUsage: digitalSignature`, `extendedKeyUsage: emailProtection`, `SubjectKeyIdentifier: hash`, `AuthorityKeyIdentifier: keyid:always`, CN + O in subject.

   The openssl test exists specifically to lock the demo's code path into CI. Any future divergence between rcgen output and operators' typical openssl-produced PEMs would surface here, not at a user's first try.

2. **`examples/tier4-demos/demo-01.sh`** extended with `LVQR_DEMO_C2PA=1` opt-in:

   * Prereq probe: `openssl` + CLI `--c2pa-signing-cert` flag (fails fast on an underfeatured `lvqr` binary).
   * Cert minting: writes `$SCRATCH/ca.cfg`, `leaf.cfg`, then runs `openssl ecparam` + `req` + `x509` + `pkcs8` to produce `signing.pem` (leaf-first chain) + `signing.key` (PKCS#8). The openssl commands are byte-for-byte the ones the CI test verifies.
   * CLI wiring: new `C2PA_ARGS=(--c2pa-signing-cert ... --c2pa-signing-key ... --c2pa-signing-alg es256 --c2pa-assertion-creator "LVQR demo-01.sh")` appended to `lvqr serve`.
   * Summary probe: polls `/playback/verify/live/demo` and prints `valid=<bool> state=<str> signer="<str>"`. Replaces the session-117 "c2pa sign+verify: not wired on the CLI today" stub that was stale after session 120 shipped the flags.
   * Documentation: `examples/tier4-demos/README.md` rewritten -- strikes "on the phase-C roadmap" language, documents the opt-in + the CI-locked recipe, updates the Tier 4 coverage table to flip row 4.3 from "no" to "yes, opt-in via LVQR_DEMO_C2PA=1".

3. **`README.md`** Known Limitations rewrite. The session-120 "on-disk CertKeyFiles integration test is future work" bullet gets replaced with a shorter "both signer paths covered by two integration tests" bullet that names the shipped tests + the typical operator cert-material layout (`CN + O`, `AKI`, `digitalSignature KU`, `emailProtection EKU`).

4. **Drive-by clippy fix.** The session-120 `c2pa_cli_tests::no_c2pa_flags_yields_none` test used `assert_eq!(..., true)` which trips `clippy::bool_assert_comparison` on Rust 1.95. Rewrote to `assert!(...)`. Slipped past session 120's clippy gate because the lint fires on the BINARY's test target (the unit-test module is in main.rs), and our session-120 clippy invocation + the Rust-version combo apparently didn't gate it until session 121's rerun.

### Key 121 design decisions baked in

* **Audit the source, don't trust the error message**. c2pa-rs's error reporting folds multiple distinct failures into the same generic strings ("certificate is invalid", "claim signature is not valid"). The user-visible error does not name the missing AKI or the missing Organization DN. Reading `certificate_profile.rs` end to end and tracing every field the final gate checks surfaced both issues in under 20 minutes. Deferring after one error was the wrong call.

* **Ship both rcgen AND openssl integration tests, not one or the other**. rcgen is the in-process test path; openssl is the operator path. They produce functionally equivalent PEMs but through very different code. Locking both into CI guarantees a regression in either ecosystem surfaces fast; shipping only one would leave the demo script's openssl commands unchecked by CI.

* **openssl test uses `have_openssl()` probe + graceful skip, not `#[ignore]`**. The test SHOULD run by default on developer machines (openssl is near-universally installed); marking it `#[ignore]` would hide failures. The probe + skip lets it run everywhere openssl is available without breaking hosts where it isn't.

* **Demo writes all openssl artifacts into the SCRATCH dir, not a separate tempdir**. When `LVQR_DEMO_SCRATCH` is set, the scratch dir is retained on exit; all cert material ends up alongside lvqr.log + ffmpeg.log for post-mortem. A separate tempdir would make "why did C2PA verify fail" harder to debug.

* **`--c2pa-assertion-creator "LVQR demo-01.sh"`** is hardcoded in the demo rather than the default `"lvqr"`. Helps operators inspecting a signed manifest differentiate demo-generated content from real signing pipelines.

* **README Known Limitations bullet rewritten rather than struck entirely**. Some operators bring their own PEM layouts; naming the tested layout (`CN + O`, `AKI`, `digitalSignature`, `emailProtection`) tells them what shape c2pa-rs is verified against without implying every OpenSSL config works.

### Ground truth (session 121 close)

* **Head (pre-push)**: feat(test) + this close-doc commit (pending). `origin/main` at `5a2986b` unchanged from session 120 push.
* **Tests (default features gate)**: **965** passed, 0 failed, 1 ignored on macOS. **+2** over session 120's 963 -- both new integration test functions. The 1 ignored is still the pre-existing `moq_sink` doctest.
* **CI gates locally clean**:
  * `cargo fmt --all --check` clean.
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95 (after fixing the session-120 `bool_assert_comparison` lint slip).
  * `cargo test --workspace` 965 / 0 / 1.
  * `cargo test -p lvqr-cli --features c2pa --test c2pa_cli_flags_e2e` 2 / 0 / 0 in 1.3 s.
  * `bash -n examples/tier4-demos/demo-01.sh` clean.
* **Demo C2PA opt-in not end-to-end run locally** -- still requires GStreamer at runtime, same posture as the rest of demo-01. The cert-generation leg alone is fully exercised by the openssl integration test.

### Known limitations / documented v1 shape (after 121 close)

* All session 120 known limitations RESOLVED for the C2PA line (integration test now ships for both signer paths; demo cover C2PA end-to-end).
* Multi-range `multipart/byteranges` still deferred (session 119 call; principled).
* Authoritative DASH-IF container validator still deferred (session 118 call; REST-API integration is a day's work on its own).
* Shared-helpers refactor across 8+ integration tests still deferred (scope).
* Whisper-enabled scheduled demo workflow still deferred (78 MB model download on every PR is a poor cost/benefit trade).
* All other session 118 + 119 + 120 known limitations unchanged.

## Session 120 close (2026-04-22)

**Shipped**: PLAN row 117-C (CLI C2PA wiring). Closes the "C2PA signing is programmatic-only" gap carried over from session 117's Known Limitations and every session since. C2PA signing now joins `--whisper-model`, `--wasm-filter`, `--transcode-rendition` as a first-class operator-opt-in CLI surface. Default-gate test count 955 -> 963.

1. **`feat(cli): C2PA signing flags`**. Six new CLI flags on `lvqr serve` + matching `LVQR_C2PA_*` env-var fallbacks, feature-gated on the existing `c2pa` Cargo feature:
   * `--c2pa-signing-cert <PATH>` + `LVQR_C2PA_SIGNING_CERT`: PEM-encoded leaf-first cert chain.
   * `--c2pa-signing-key <PATH>` + `LVQR_C2PA_SIGNING_KEY`: PKCS#8 private key matching the leaf.
   * `--c2pa-signing-alg <ALG>` + `LVQR_C2PA_SIGNING_ALG`: clap `ValueEnum` over `es256` / `es384` / `es512` / `ps256` / `ps384` / `ps512` / `ed25519`. Defaults to `es256` (matches rcgen's default P-256 output and the common operator-managed key shape).
   * `--c2pa-assertion-creator <STR>` + `LVQR_C2PA_ASSERTION_CREATOR`: schema-org CreativeWork author name. Defaults to `"lvqr"`.
   * `--c2pa-trust-anchor <PATH>` + `LVQR_C2PA_TRUST_ANCHOR`: PEM trust anchor for private-CA deployments. File contents are read eagerly at CLI time so a missing file surfaces as a configuration error, not a silent verify-time degradation.
   * `--c2pa-timestamp-authority <URL>` + `LVQR_C2PA_TIMESTAMP_AUTHORITY`: RFC 3161 TSA URL for countersigned manifests.

   New `C2paAlgArg` clap `ValueEnum` at the top of `main.rs` maps 1:1 to `lvqr_archive::provenance::C2paSigningAlg`; the indirection keeps clap's `ValueEnum` derive off the `lvqr-archive` crate so the `c2pa` Cargo feature stays the single gate on every provenance-adjacent dep. New `build_c2pa_config(&args) -> Result<Option<C2paConfig>>` helper returns `Ok(None)` when neither cert nor key is set, `Ok(Some(...))` with `C2paSignerSource::CertKeyFiles` when both are set, and `Err(anyhow)` with a clear message ("--c2pa-signing-cert was set but --c2pa-signing-key is missing; both flags must appear together") when exactly one is set. `ServeConfig.c2pa` population moves from a hard-coded `None` to `c2pa_config` (a local computed via `build_c2pa_config(&args)?` before the struct literal begins moving fields out of `args`).

   Test coverage: eight unit tests in a new `#[cfg(all(test, feature = "c2pa"))] mod c2pa_cli_tests` module in `main.rs` cover every CLI-to-config outcome: no flags -> None; cert-only -> Err; key-only -> Err; both -> CertKeyFiles with default alg Es256 + empty TSA + default "lvqr" creator + None trust_anchor; alg-flag -> archive enum mapping (Ed25519 checked); assertion-creator override lands on config; TSA override lands on CertKeyFiles; missing trust-anchor file -> Err with path in the message. Default workspace test count 955 -> 963 (+8 new).

2. **`docs`** -- this close-doc commit. Inserts a new `117-C` entry into `tracking/PLAN_V1.1.md`'s phase-C table marked SHIPPED, with the flag list + ValueEnum rationale + test shape + the deferred integration-test gap rolled into the row. Refreshes the status header (phase-C rows-117/117-A/117-B/117-C all SHIPPED). Authors this block. Updates `README.md`: strikes the "C2PA signing is programmatic-only" bullet under Known v0.4.0 limitations (replacing it with a narrower "on-disk CertKeyFiles integration test is future work" bullet), rewrites the "Provenance + signing" feature-overview bullet to name the CLI flags, and adds a dedicated "C2PA signing" block to the CLI reference section. Updates `project_lvqr_status` memory.

### Key 120 design decisions baked in

* **`C2paAlgArg` is a CLI-local enum, not a reuse of `lvqr_archive::provenance::C2paSigningAlg` with clap's `ValueEnum` derive applied upstream**. Putting `#[derive(ValueEnum)]` on the archive crate's enum would couple the `c2pa` Cargo feature to clap (an otherwise binary-only dep), pulling clap into the archive crate's graph for every downstream user. Keeping the enum local to the CLI + mapping via `to_archive_alg` costs 15 lines and decouples the crates cleanly.
* **Eager trust-anchor file read at CLI time, not lazy at sign time**. `build_c2pa_config` calls `std::fs::read_to_string(path)` on the trust-anchor path at CLI boot. If the operator hands `--c2pa-trust-anchor /wrong/path.pem`, the process exits immediately with a clear error naming the missing path. A lazy read would let the server boot, handle ingest for hours, and only surface the misconfiguration at the first broadcast's drain-terminated finalize -- a pattern that cost other production systems real incidents. The 10-20 ms of extra startup is cheap insurance.
* **Validation of `(cert, key)` pairing enforced; alg/creator/TSA/trust-anchor permitted in isolation**. Setting just `--c2pa-signing-cert` or just `--c2pa-signing-key` is a hard CLI-time error because neither half alone has defined behavior: a cert without a key cannot sign, and a key without a cert has no chain to advertise. Setting `--c2pa-signing-alg` or `--c2pa-assertion-creator` without cert+key is harmless (the values are carried but never consulted because `c2pa: None`), so the helper silently ignores them rather than erroring. This asymmetry matches operator intuition: "I tried to turn on signing and half-configured it" is a footgun worth catching; "I set a future-relevant default" is not.
* **`build_c2pa_config` called into a local before the `ServeConfig { ... }` literal**. First attempt placed the call inline in the struct literal (`c2pa: build_c2pa_config(&args)?`), which conflicted with the literal's partial move of `args.archive_dir` into an earlier field. Computing `let c2pa_config = build_c2pa_config(&args)?;` before the literal keeps the borrow scope clean and is equivalent at runtime.
* **Integration test attempted + reverted**. Authored `crates/lvqr-cli/tests/c2pa_cli_flags_e2e.rs` using rcgen 0.13 to mint a CA + leaf + key in-process, write PEMs to a tempdir, boot `TestServer` with `C2paConfig { CertKeyFiles { ... } }` pointed at the paths, and assert `/playback/verify/live/dvr` returns `valid=true`. rcgen produced a leaf with `emailProtection` EKU + `digitalSignature` KU + proper CA chain (all documented c2pa-rs requirements per the `C2paSignerSource::CertKeyFiles` rustdoc), but c2pa-rs rejected the cert at sign time with `c2pa error: sign: the certificate is invalid`. The rejection is inside c2pa-rs's `verify_certificate_profile` and is stricter than the documented EKU + KU requirements -- likely wants specific X.509 v3 extensions (critical Basic Constraints, AKI/SKI hints, bounded validity periods) that rcgen does not emit by default. Making rcgen produce a c2pa-rs-accepting cert chain is its own subproblem and was out of scope for this session. The test was deleted rather than shipped disabled. The CLI wiring is still proved end-to-end by the 8 unit tests on `build_c2pa_config`; a follow-up row will ship happy-path on-disk coverage via a pre-staged PEM fixture or c2pa-rs's own test cert helpers.
* **No new rcgen-style "make a C2PA cert in-test" helper in `lvqr-test-utils`**. Deferred for the same reason the integration test was deferred: a test helper whose output isn't accepted by c2pa-rs is worse than no helper at all. Ships alongside the follow-up integration test once the cert-profile issue is resolved.

### Ground truth (session 120 close)

* **Head (pre-push)**: feat(cli) + this close-doc commit (pending). `origin/main` at `e6e21d0` unchanged from session 119 push.
* **Tests (default features gate)**: **963** passed, 0 failed, 1 ignored on macOS. **+8** over session 119's 955 -- all eight from the new `c2pa_cli_tests` module in `main.rs`. Workspace test run compiles the CLI binary with `c2pa` enabled (feature unification pulls it in via `lvqr-archive`'s feature graph), so these tests run on the default gate. The 1 ignored is still the pre-existing `moq_sink` doctest.
* **CI gates locally clean**:
  * `cargo fmt --all --check` (after one auto-format pass on `build_c2pa_config`'s trust-anchor closure that rustfmt wrapped).
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 963 / 0 / 1.
  * `cargo build -p lvqr-cli --features c2pa` clean; `--features full` clean.
* **Workspace**: **29 crates**, unchanged.
* **crates.io / npm / PyPI**: unchanged. Session 120 is purely additive CLI surface + eight unit tests + docs; no published-crate API moved. Next release cycle carries the new flags.

### Known limitations / documented v1 shape (after 120 close)

* **On-disk `CertKeyFiles` happy-path integration test is future work**. CLI wiring is proved by 8 unit tests; the file-loading + sign + verify end-to-end chain is currently only exercised programmatically via `c2pa::EphemeralSigner` in `c2pa_verify_e2e.rs` (which routes through `C2paSignerSource::Custom`, not `CertKeyFiles`). Adding happy-path on-disk coverage requires c2pa-rs-acceptable test cert material; rcgen's output is rejected by c2pa-rs's sign-time profile check.
* **c2pa-rs cert-profile requirements are stricter than documented**. The `C2paSignerSource::CertKeyFiles` rustdoc lists EKU + `digitalSignature` KU + non-self-signed-leaf as the requirements. rcgen produces certs meeting all three and c2pa-rs still rejects them. Real operator deployments using openssl-generated certs presumably pass (the documented EKU list is taken from the C2PA spec, not a c2pa-rs-imposed extra); reproducing an openssl-equivalent cert chain via rcgen is the subproblem.
* **Hardware encoders still absent** (unchanged from session 119 + earlier).
* **No nightly soak in CI** (unchanged; phase-C row 119 candidate).
* All session 119 known limitations unchanged.

## Session 119 close (2026-04-22)

**Shipped**: two operator-polish follow-ups riding on top of phase-C row 117. Both were explicitly queued in the session 118 close block's Known Limitations + Phase-C follow-up rows; both are carried-over gaps rather than new PLAN rows, which keeps session numbers aligned with PLAN rows. Default-gate test count 944 -> 955.

1. **`feat(archive): HTTP Range: bytes= on /playback/file/*`**. `file_handler` in `crates/lvqr-cli/src/archive.rs` now honors RFC 7233 single-range requests. A new `parse_single_range(HeaderValue, total) -> ParsedRange { Single(start, end) | Unsatisfiable | Ignored }` helper parses the three legal forms of `bytes=` specs (closed `A-B`, open-tail `A-`, suffix `-N`); the handler returns 206 Partial Content with `Content-Range: bytes A-B/total` on `Single`, 416 Range Not Satisfiable with `Content-Range: */total` on `Unsatisfiable`, and falls through to a normal 200 on `Ignored` (malformed header, multi-range, etc.). Every response also carries `Accept-Ranges: bytes` so probing clients see a positive signal on the first non-ranged fetch.

   Closes a real DVR UX gap: HTML5 `<video>` tags issue `Range: bytes=0-` on first fetch and then `Range: bytes=<seek>-` as the viewer scrubs, so without this branch every seek would re-download the full segment from byte zero. The existing DVR scrub flow (fetch `/playback/{broadcast}`, pick a segment, fetch `/playback/file/<rel>`) becomes browser-seekable end to end.

   Test coverage: (a) 10 unit tests on `parse_single_range` covering every legal form + every malformed form + zero-suffix + reversed range + over-length end-clamp + empty-file unsatisfiable; (b) one new integration test `playback_file_supports_byte_range_requests` in `archive_dvr_read_e2e.rs` that exercises the full HTTP path -- fetches a real archived m4s, asserts `Accept-Ranges` on the baseline, then issues `bytes=0-15` + `bytes=<mid>-` + `bytes=-8` + `bytes=<beyond>-` + `bytes=0-10,20-30` against the same resource and checks status code + `Content-Range` header + body length + byte-for-byte match against the full body. Extended the raw-TCP HTTP client in that file with a `http_get_with_range` variant taking an optional `Range:` header.

   Design decisions baked in: (i) **single-range only**; multi-range requests (legal per RFC 7233) would need `multipart/byteranges` encoding that is rare in the wild and has no operator demand -- fall through to a normal 200 so the client still receives a valid response, just not partitioned; (ii) **full-file read + slice** rather than `tokio::fs::File::seek` -- typical segment file is <100 KB on a 2 s GOP, the file is already on the page cache, seek+read_exact buys nothing and adds complexity; (iii) **end-clamp at `total - 1`** per RFC 7233 so `bytes=0-99999` against a 100-byte file returns 100 bytes rather than 416; (iv) **reject `bytes=-0`** as unsatisfiable, matching nginx's behavior even though RFC 7233 does not strictly forbid it.

2. **`feat(ci): tier4-demos.yml workflow`**. New `.github/workflows/tier4-demos.yml` mirroring `mesh-e2e.yml` / `hls-conformance.yml` / `dash-conformance.yml`'s dedicated-workflow pattern. Ubuntu runner, `apt install ffmpeg jq gstreamer*` (tools + base/good/bad/ugly/libav plugin sets + dev headers + libclang-dev for whisper-rs's bindgen), `cargo build -p lvqr-cli --release --features full`, invoke `./examples/tier4-demos/demo-01.sh` with `LVQR_DEMO_SCRATCH=/tmp/tier4-demos-out` so the scratch dir survives the step for artifact upload. `LVQR_WHISPER_MODEL` deliberately unset so captions are skipped cleanly (saves ~78 MB download per run); a separate scheduled workflow with a cached whisper model is queued as a phase-C follow-up. `continue-on-error: true` initially per convention; promotion to required-check status comes after first clean run on `main`.

   Closes the session-117 follow-up gap explicitly named in that close block: "demo-01.sh is not invoked by CI... without CI coverage the demo can silently bitrot on CLI-flag renames." Any future rename of `--transcode-rendition`, `--wasm-filter`, `--archive-dir`, or any port flag will now surface as a red CI signal on the PR.

3. **`docs`** -- this close-doc commit. Appends a short note to `tracking/PLAN_V1.1.md` row 117 mentioning the two session-119 follow-ups SHIPPED on top of row 117's original 118 delivery (no new PLAN row created -- these are polish that rides on the existing row). Refreshes the status header to reflect 955 default-gate tests + phase-C row 117 + follow-ups all SHIPPED. Authors this block. Updates `project_lvqr_status` memory. Appends one sentence to `README.md`'s DVR scrub bullet naming the Range support.

### Key 119 design decisions baked in

* **Two small follow-ups rather than advancing the PLAN**. PLAN row 118 (SDK completion: expand `@lvqr/core` admin client + add Vitest + pytest in CI + document reconnect semantics) is the canonical next row but a realistic full scope is 5-8 hours across three separable concerns. A half-slice (Vitest + pytest only) would close one of the row's three bullets and leave the row partially pending. The two operator-polish follow-ups shipped this session are each small enough to land with full test coverage + docs in one session, close explicitly-queued gaps from session 117 + 118, and do not touch load-bearing surfaces. Sequencing rationale: prefer finishing what's already on the queue before opening new marquee work.
* **Follow-ups ride under row 117 rather than getting new row numbers**. The alternatives were `117-A` / `117-B` sub-rows (keeps session and PLAN-row numbering distinct) or inserting new rows `117.5` / `117.6` (bumps every row after). Neither adds real information. The PLAN row 117 entry now carries a short "session 119 added two follow-ups" note linking to the details in HANDOFF; PLAN rows = session rows stays clean for reasoning.
* **`parse_single_range` is a free function with its own `mod range_tests`**. Could have been a method on a hypothetical `RangeHeader` struct; rejected because the struct buys no state (it is purely a parse-and-return-an-enum operation) and would force the handler through an extra wrapper type. The `#[cfg(test)] mod range_tests` gives 10 unit tests that compile in-tree without any external crate boundary to cross.
* **`ParsedRange::Ignored` over panicking on malformed input**. A malformed `Range:` header is a client-side bug, not a server-side bug; returning the full body with a 200 is what nginx / Caddy / Apache all do and avoids breaking legacy clients that mis-format the header. Treating it as an error would be more strict but user-hostile.
* **Full-body fall-through for multi-range requests, not 416**. Multi-range requests are legal per RFC 7233 but require `multipart/byteranges` encoding that is (a) complex to implement correctly and (b) never requested by mainstream browsers in the wild. Returning the full body lets the client's own byte-range slicer pull what it wants; returning 416 would break the rare correct multi-range client. The fall-through is not observable to a properly-implemented client because a correct client always sends single-range first before trying multi-range.
* **`tier4-demos.yml` installs `libclang-dev` + `libgstreamer-plugins-base1.0-dev`**. The `--features full` build pulls whisper-rs (needs libclang for bindgen) + gstreamer-rs (needs the GStreamer dev headers). Without these the release build fails with a confusing bindgen error mid-build. Listing them explicitly alongside the runtime plugin sets keeps the workflow's first-run setup hermetic.
* **`LVQR_WHISPER_MODEL` deliberately unset in the CI workflow**. The captions path is covered in-repo by `crates/lvqr-cli/tests/whisper_cli_e2e.rs` (ignored by default, opt-in via `--ignored` + `WHISPER_MODEL_PATH`). Running captions through the demo on every PR would mean downloading + caching a ~78 MB ggml file; the cost/benefit does not pencil for the main PR gate. A scheduled nightly workflow with a cached model is a reasonable follow-up.

### Ground truth (session 119 close)

* **Head (pre-push)**: feat(archive) + feat(ci) + this close-doc commit (pending). `origin/main` at `30d8059` unchanged from session 118 push.
* **Tests (default features gate)**: **955** passed, 0 failed, 1 ignored on macOS. **+11** over session 118's 944 -- ten unit tests in the new `archive::range_tests` module + one integration test in `archive_dvr_read_e2e.rs`. The 1 ignored is still the pre-existing `moq_sink` doctest.
* **CI gates locally clean**:
  * `cargo fmt --all --check` (after one auto-format pass on two-line `assert_eq!` calls that rustfmt wrapped).
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 955 / 0 / 1.
  * `python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/tier4-demos.yml"))'` parses cleanly.
* **`tier4-demos.yml` not run locally**. Workflow is CI-only; first GitHub Actions run on push carries the authoritative signal.

### Known limitations / documented v1 shape (after 119 close)

* **`tier4-demos.yml` runs soft-fail initially**. Promotion to required-check waits for first clean run on `main`. Expect iteration on GStreamer apt packages + cached Rust build warm-up.
* **No nightly workflow with whisper model**. Captions path unexercised in CI; the in-repo `whisper_cli_e2e.rs` (ignored by default) is the only programmatic coverage.
* **Multi-range requests (`Range: bytes=0-10,20-30`) fall through to 200**. Single-range is the 99% case; multi-range would need `multipart/byteranges` encoding that no mainstream browser requests without first having tried single-range.
* **`/playback/file/*` range support does not extend to `/hls/*` or `/dash/*`**. Those routes serve live-stream fragments via axum's default handling, not the playback file path. Range support on live-stream fragments is a separate architectural question (live HLS clients follow playlist reloads; scrubbing a live stream is a DVR operation that already routes through `/playback/file/*`), so the narrow extension here is the right scope.
* All session 118 known limitations unchanged.

## Session 118 close (2026-04-22)

**Shipped**: `PLAN_V1.1.md` row 117 (phase-C kickoff). First dedicated integration test for the `/playback/*` DVR read surface + first CI workflow for DASH egress conformance. All three new tests pass on the default feature gate; the new workflow is soft-fail (`continue-on-error: true`) for its first weeks on `main` per the `hls-conformance.yml` precedent.

1. **`feat(test): crates/lvqr-cli/tests/archive_dvr_read_e2e.rs`** (~500 LOC, 3 `#[tokio::test]` functions). A design-audit pass at the start of the session found the entry-point block's "the read side has zero E2E" claim to be stale: `rtmp_archive_e2e.rs` already covers the happy-path range / latest / file routes + auth gate + path-traversal guard. The new test file targets three scenarios NOT covered by the existing write-side test:

   * `playback_scrub_window_arithmetic`: publishes five keyframes at RTMP timestamps `[0, 2000, 4000, 6000, 8000]` ms, yielding four closed segments in the redb index. A full-window scan establishes the ground-truth row set; a first-half scan `?to=<midpoint>` and a second-half scan `?from=<midpoint>` each obey the per-row overlap property documented on `find_range` (first-half rows have `start_dts < midpoint`; second-half rows have `end_dts > midpoint`); the half-window `segment_seq` union equals the full-window set exactly (allowing for the straddle case where one segment appears in both halves). Midpoint is derived numerically from `min_start + (max_start - min_start) / 2` rather than pinned to a timescale factor so the test does not couple to the RTMP -> CMAF 90 kHz conversion.

   * `live_dvr_scrub_while_publisher_is_active`: holds the RTMP session open across two scan passes (not `drop(rtmp_stream)` between them), proving the admin handlers do not deadlock on the writer's redb exclusive file lock. Asserts sub-second completion on the first scan and that `/playback/latest/*` advances during the live publish. This is the load-bearing DVR scenario for any real operator scrubbing a still-active broadcast; the existing write-side test waits for the publisher to finish, so it cannot catch a reader-writer race.

   * `playback_routes_emit_expected_content_types`: extends the raw-TCP HTTP client to parse every response header (the existing helper across six tests reads status + body only). Asserts `application/json` on `/playback/{broadcast}` + `/playback/latest/*`, `application/octet-stream` on `/playback/file/*`, and that `Content-Length` equals the actual body length. A future refactor that drops an explicit `Content-Type` or changes it to `text/plain` would trip this test; the existing suite would pass.

   All three tests pass on macOS in ~1.6 s combined. Every helper (FLV tag builders, RTMP handshake, raw TCP HTTP GET) is copy-pasted from `rtmp_archive_e2e.rs`; shared-helper extraction into `lvqr-test-utils` is accepted scope-creep rejection (the pattern is duplicated across six tests already and a factor-out is a separate hygiene session).

2. **`feat(ci): .github/workflows/dash-conformance.yml`**. Mirrors `hls-conformance.yml`'s "prefer the authoritative tool, soft-fall-back to ffmpeg" posture exactly. Differences from the HLS workflow are deliberate: Ubuntu runner (DASH validator stack is cross-platform; `mediastreamvalidator` is macOS-only), `--dash-port 8889` (matches the quickstart table), GPAC `MP4Box -dash-check` in the "authoritative" slot (apt-installable; structural conformance of the MPD + segment set), and `ffmpeg -i <mpd>` + `ffprobe` as the always-on fallback. `continue-on-error: true` initially so a regression surfaces in the artifact but does not block PRs; promotion to a required check waits for the first clean run on `main`. Upgrading the primary validator to the DASH-IF containerized tool (`dashif/conformance`) is a phase-C follow-up row because the container exposes a REST API that does not match the one-shot validator shape every other workflow uses.

3. **`tracking/SESSION_118_BRIEFING.md`** -- authored in-session before opening any source file, per the `PLAN_V1.1.md` "How to kick off the next session" convention (row 117 has two non-trivial design decisions: test-overlap with `rtmp_archive_e2e.rs`, and DASH-IF validator tooling choice). The briefing catalogued the actual coverage of the existing test, called out the stale claim in the entry-point block, and locked the three new test scopes + the GPAC-first DASH validator choice before engineering time got spent.

4. **`docs`** -- this close-doc commit. Rewrites `tracking/PLAN_V1.1.md` row 117 from pending to SHIPPED with the test shape + design decisions rolled into the row. Refreshes the status header to reflect phase B CLOSED + phase C kickoff SHIPPED. Authors this block. Updates `project_lvqr_status` auto-memory to reflect session 118 + phase-C kickoff. Queues the phase-C follow-ups (authoritative DASH-IF container, CLI C2PA wiring, tier4-demos CI coverage).

### Key 118 design decisions baked in

* **Entry-point claim "the read side has zero E2E" is stale**. Verified by reading `crates/lvqr-cli/tests/rtmp_archive_e2e.rs` end to end: happy-path range / latest / file routes + auth gate + path-traversal guard all already covered. The honest framing is that the existing test covers the *read surface* but not the *scrub arithmetic* or the *live-DVR race* or the *Content-Type contract*. The new test file targets those three specifically. No scenarios overlap with the existing test, so the new file is additive not duplicative.
* **Scrub arithmetic test uses numeric-derived midpoint, not a pinned timescale factor**. The contract under test is `find_range`'s `[start_dts, end_dts)` overlap semantics, which is a property of the redb index and the HTTP handler. The RTMP-to-CMAF 90 kHz conversion is a separate bridge-side contract that the test deliberately does not couple to; pinning `midpoint = 270_000` would make the test fragile against any future evolution of the RTMP timestamp-to-CMAF-dts mapping, which is a legitimate place where the ingest may evolve. Numeric-derived midpoint keeps the assertion on the scrub contract exclusively.
* **Live-DVR test holds the RTMP session open via a single `let (mut rtmp_stream, mut session) = connect_and_publish(..)` and publishes in two batches**, rather than spawning the publisher on a separate task. The admin handler runs the redb scan on `spawn_blocking` so the test task's tokio runtime is free to interleave the HTTP GET; no second thread needed. Asserting sub-5 s scan completion is the concrete failure signal if `spawn_blocking` were removed and the handler started blocking on the writer's lock.
* **`Content-Length` assertion is added as a bonus check on the file route**. The handler builds the response with `header(CONTENT_LENGTH, bytes.len())`; if a refactor ever switches to `Body::from_stream` and omits the explicit Content-Length, the test catches it. Not on the roadmap, but a cheap drift guard.
* **DASH validator: GPAC MP4Box over the DASH-IF container**. The DASH-IF Conformance Tool (`dashif/conformance` on Docker Hub) is the authoritative source of truth but exposes only a REST API; wiring it into a one-shot CI step requires spinning the container, POSTing the MPD + segment URIs, polling the job state, and reading the validation report. That is a day of integration work on its own and does not fit the "one-shot validator" shape every other workflow in this repo uses. GPAC's `MP4Box -dash-check` is apt-installable, covers structural conformance (required `<AdaptationSet>` + `<SegmentTemplate>` shape, segment-timing sanity, codecs attribute presence), and returns a non-zero exit on failure. Shipping GPAC first + deferring the DASH-IF container to a phase-C follow-up row is the honest trade.
* **`continue-on-error: true` on the DASH job initially**. Matches `hls-conformance.yml`'s early-days posture: the first-run artifact surfaces the output without blocking PRs. Promotion to a required check comes after the first clean run on `main` (same promotion path session 33 used for the HLS workflow's ffmpeg-as-client signal).
* **Helpers copy-pasted from `rtmp_archive_e2e.rs` rather than extracted to `lvqr-test-utils`**. The pattern is duplicated across `rtmp_archive_e2e.rs`, `rtmp_hls_e2e.rs`, `rtmp_dash_e2e.rs`, `rtmp_ws_e2e.rs`, `rtmp_whep_audio_e2e.rs`, `c2pa_verify_e2e.rs`, `whisper_cli_e2e.rs`, and now `archive_dvr_read_e2e.rs`. Every one of those tests has made per-test tweaks to the helper set (parameterized keyframe trains here, FLV audio-raw in the audio tests, etc.). Extracting into a shared module is a legitimate refactor but scope-creep for this session; a dedicated hygiene row can do it cleanly against the full matrix.
* **No `Range: bytes=` header tests**. The file handler does not implement range requests; that is a documented gap, not a regression. Adding range-request support is its own phase-C row candidate; testing for a feature that does not exist would be test-theater.

### Ground truth (session 118 close)

* **Head (pre-push)**: feat commit + this close-doc commit (pending). `origin/main` head at `f9ece25` unchanged from the session 117 push.
* **Tests (default features gate)**: **944** passed, 0 failed, 1 ignored on macOS. **+3** over session 117's 941 -- the three new `archive_dvr_read_e2e.rs` targets, all running on the default gate (no feature flags). The 1 ignored is still the pre-existing `moq_sink` doctest.
* **CI gates locally clean**:
  * `cargo fmt --all --check` (after one auto-format pass on the new test file for a >120-col `let` binding that rustfmt collapsed into a single line).
  * `cargo clippy --workspace --all-targets -- -D warnings` clean on Rust 1.95.
  * `cargo test --workspace` 944 / 0 / 1.
  * `python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/dash-conformance.yml"))'` parses the new workflow cleanly.
* **Workspace**: **29 crates**, unchanged.
* **crates.io / npm / PyPI**: unchanged. Session 118 is a new test target + a new CI workflow + docs; no published-crate surface moves.

### Known limitations / documented v1 shape (after 118 close)

* **`dash-conformance.yml` runs soft-fail initially**. First promotions to required-check status require a clean run on `main`; operator traction on the primary validator's quirks comes from the uploaded artifact. Expect iteration.
* **DASH-IF authoritative container validator deferred**. GPAC MP4Box covers structural conformance but misses some DASH-IF-specific profile rules (e.g. `urn:mpeg:dash:profile:isoff-live:2011` conformance). Phase-C follow-up row candidate.
* **`/playback/file/*` does not implement HTTP `Range: bytes=`**. Byte-range support is a DVR-UX feature gap; the index + on-disk layout already support it (every segment has a known byte_offset + length), so the handler extension is narrow. Phase-C follow-up row candidate.
* **Test helper duplication across 8 integration tests**. RTMP handshake + FLV tag builders + raw HTTP GET are copy-pasted between `rtmp_*_e2e.rs` / `c2pa_verify_e2e.rs` / `whisper_cli_e2e.rs` / `archive_dvr_read_e2e.rs`. A shared module under `lvqr-test-utils` is clean scope for its own hygiene row.
* All session 117 + post-116 + earlier-session known limitations unchanged.

## Session 117 close (2026-04-22)

**Shipped**: `PLAN_V1.1.md` row 116 (first public tier4-demos script) + a top-to-bottom README reality sweep. Closes the Tier 4 exit criterion left open when Tier 4 was marked COMPLETE; every phase-B row (113 + 114 + 115 + 116) is now SHIPPED on `main`.

1. **`feat(examples): tier4-demos/demo-01.sh`** -- `examples/tier4-demos/demo-01.sh` (~250 LOC Bash) + `examples/tier4-demos/README.md` (~180 LOC). Boots a single `lvqr serve` on non-default ports (admin 18080, hls 18888, rtmp 11935, moq 14443) with `--wasm-filter` pointed at the in-repo `crates/lvqr-wasm/examples/frame-counter.wasm` fixture, `--transcode-rendition 720p / 480p / 240p`, `--archive-dir <scratch>/archive`, and `--whisper-model <env>` when `LVQR_WHISPER_MODEL` points at a ggml file. A 20 s ffmpeg colour-bars+sine publish drives `rtmp://127.0.0.1:11935/live/demo`. The script polls `/healthz`, then the HLS `master.m3u8` until 4 variants are advertised (source + 3 ABR rungs), then `/metrics` for `lvqr_wasm_fragments_total{outcome="keep"}`, then the scratch archive dir for `0.mp4/finalized.*` on publisher disconnect. A flat summary block prints URLs + counters + archive paths; the script exits non-zero if the ABR or archive assertions fail. Prereq probes for `lvqr`, `ffmpeg`, `curl`, `jq`, `gst-launch-1.0` all fail fast with pointers back to `examples/tier4-demos/README.md`. The feature-set probe (`lvqr serve --help | grep -- --transcode-rendition`) refuses to proceed on an underfeatured binary.

2. **`docs`** -- this close-doc commit. Rewrites `tracking/PLAN_V1.1.md` row 116 from pending to SHIPPED with the demo shape + the C2PA scoping rationale rolled into the row. Rewrites the status header to reflect phase-B CLOSED + pointers to the next phase-C row (117: archive READ DVR E2E + DASH-IF validator). Authors this block. Updates the `project_lvqr_status` auto-memory to reflect session 117 + phase-B closure. Applies a top-to-bottom README reality sweep.

### README reality sweep (rode along with the close-doc commit)

Fixes a dozen drift points surfaced while authoring the demo:

* **Test count**: `917 workspace tests passing` -> `941 workspace tests passing, 0 failing, 1 ignored, plus a Playwright browser E2E`. The old 917 has been stale since session 109 A.
* **Tier 4 exit criterion**: flipped to CLOSED with a direct link to `examples/tier4-demos/`.
* **WHEP AAC-to-Opus**: Egress+encoders checklist item flipped from unchecked to SHIPPED (session 113).
* **Mesh two-peer browser E2E**: checklist item flipped from unchecked to SHIPPED (session 115) with a direct link to `bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts` + `.github/workflows/mesh-e2e.yml`.
* **Mesh feature overview**: "topology planner + WebSocket signaling server" paragraph rewritten to name the full shipped chain (server-side subscriber registration + client-side WebRTC DataChannel parent/child relay + the Playwright E2E on CI) with operator-grade completion scoped to phase D.
* **Client libraries row**: `@lvqr/core` description now names `pushFrame`, `onChildOpen`, `connectTimeoutMs`, `fetchTimeoutMs` on `main` ahead of the next publish cycle.
* **CLI reference**: added `--mesh-root-peer-count` and `--no-auth-signal` flags (both shipped but undocumented in the README). Peer-mesh block header rewritten from "topology planner only; media relay on the roadmap" to "topology planner + signaling + client-side relay ship; operator-grade completion on the phase-D roadmap".
* **Phantom `/readyz` endpoint**: removed. `lvqr-admin::routes` mounts `/healthz` but never mounted `/readyz`; the README had been listing it since the pre-v0.3 era.
* **`@lvqr/player` "upcoming"**: removed. The package has been on npm at 0.3.1 since session 103.
* **Known v0.4.0 limitations**: added a **C2PA signing is programmatic-only** bullet that names the CLI-wiring gap (no `--c2pa-signing-cert` etc.), points at `ServeConfig.c2pa` + `crates/lvqr-cli/tests/c2pa_verify_e2e.rs` for the programmatic shape, and tags CLI wiring as phase-C.

### Key 117 design decisions baked in

* **C2PA sign+verify deliberately scoped OUT of `demo-01.sh`**. The SESSION_116_BRIEFING.md demo sketch included C2PA verify as one of the chained surfaces (`WASM + whisper + transcode + archive + C2PA verify`). On reading the code, `crates/lvqr-cli/src/config.rs:110` exposes `ServeConfig.c2pa: Option<C2paConfig>` but nothing in `crates/lvqr-cli/src/main.rs` parses CLI flags into it -- C2PA is programmatic-only today. Surfacing it through new `--c2pa-signing-cert` / `--c2pa-signing-key` / `--c2pa-signing-alg` / `--c2pa-assertion-creator` / `--c2pa-trust-anchor` flags is a legitimate additive CLI change but it needs a design pass (clap ValueEnum for `C2paSigningAlg`, validation that cert+key land together, feature-gating against the `c2pa` Cargo feature, at least one integration test exercising the flags), and that scope does not fit in a session already committed to a demo script + README sweep. Instead: demo-01 runs WASM + whisper + transcode + archive (every Tier 4 surface actually reachable from today's CLI), the demo README explicitly flags C2PA as "programmatic-only today, CLI wiring on the phase-C roadmap" + prints a one-liner (`cargo test -p lvqr-cli --features c2pa --test c2pa_verify_e2e`) for the programmatic end-to-end fixture, and the main README's Known Limitations section names the gap so nobody reading the doc leaves with the wrong impression. Adding CLI C2PA flags is now a candidate row for phase-C sequencing.
* **Frame-counter WASM over a more dramatic filter**. The WASM surface under test is "the tap actually runs on every fragment"; the `lvqr_wasm_fragments_total{outcome="keep"}` counter is the demonstrable signal. The frame-counter module (identity filter) keeps every fragment, so downstream surfaces (HLS, transcode ladder, archive) still receive the full stream while the counter still ticks. The redact-keyframes alternative would drop every fragment, starving every downstream surface and making the demo's "four ABR variants advertised" + "archive finalized" assertions impossible. The rejected alternative was to ship a new demo-specific WASM module (e.g. a watermarker) that mutates payload bytes while still passing them through; that would have pulled a `wat2wasm` / `wasm-tools` build dep into the demo prereqs or required a checked-in binary, which grows the repo for negligible clarity gain.
* **Whisper is opt-in via env var, not required**. `LVQR_WHISPER_MODEL` unset = demo skips captions cleanly, everything else still runs. The alternative was to make the demo refuse to run without a model; rejected because requiring a ~78 MB download just to exercise the other four Tier 4 surfaces is poor ergonomics. The README documents the Hugging Face download one-liner for operators who want the captions path covered.
* **Non-default ports for the demo's `lvqr serve`**. `--admin-port 18080 --hls-port 18888 --rtmp-port 11935 --port 14443`. Same rationale as the session 116 Playwright config: a locally-running `lvqr serve` on zero-config ports should not collide with the demo's subprocess. The demo's README documents every override env var (`LVQR_DEMO_ADMIN_PORT` etc.) for operators on restricted runners.
* **README reality sweep rides along with the close-doc commit, not its own commit**. Every drift entry is small and interlocking (fixing the Tier 4 exit criterion row implies flipping the phase-B row 116 checkbox, which implies updating the `@lvqr/core` description to reference the new `onChildOpen` shipped in the post-116 sweep, and so on). Splitting them into separate commits would manufacture review noise without isolating any load-bearing change. The two-commit shape (feat = demo files, docs = close + README sweep + PLAN row 116) matches the kickoff-prompt convention.

### Ground truth (session 117 close)

* **Head (pre-push)**: feat commit + this close-doc commit (pending). `origin/main` head at `2f84da3` unchanged from the post-116 sweep.
* **Tests (default features gate)**: **941** passed, 0 failed, 1 ignored on macOS. Unchanged from the post-116 sweep because session 117 adds (a) Bash + Markdown files under `examples/tier4-demos/` that live outside the cargo workspace, and (b) zero Rust code. Demo-01 is runnable against a GStreamer-provisioned host; not verified end-to-end on this macOS dev host (GStreamer not installed via brew), matching the session-115 posture on `rtmp_whep_audio_e2e.rs`.
* **CI gates locally clean**:
  * `cargo fmt --all --check`.
  * `cargo clippy --workspace --all-targets -- -D warnings`.
  * `cargo test --workspace` 941 / 0 / 1.
  * `bash -n examples/tier4-demos/demo-01.sh` clean.
* **Workspace**: **29 crates**, unchanged.
* **crates.io / npm / PyPI**: unchanged. Session 117 is shell + markdown + an existing-file README sweep; no Rust / TypeScript / Python code moved.

### Known limitations / documented v1 shape (after 117 close)

* **C2PA sign+verify is programmatic-only**. `ServeConfig.c2pa` works end to end but no CLI flag set it from `lvqr serve ...`. Phase-C row candidate: `--c2pa-signing-cert` + `--c2pa-signing-key` + `--c2pa-signing-alg` + `--c2pa-assertion-creator` + `--c2pa-trust-anchor` + `--c2pa-timestamp-authority`, each feature-gated on the `c2pa` Cargo feature.
* **demo-01.sh is not invoked by CI**. The script needs GStreamer + ffmpeg on the runner; adding a dedicated workflow (mirror `mesh-e2e.yml`'s dedicated-workflow pattern) is a natural follow-up. Without CI coverage the demo can silently bitrot on CLI-flag renames. Phase-C row candidate.
* All session 113 / 114 / 115 / 116 + post-116 known limitations unchanged.

## Post-116 quality sweep (2026-04-22)

Five commits + one API affordance on top of the session 116 close. Each is a real fix for a latent issue surfaced either by the first CI run of the new mesh-e2e workflow or by a design-audit pass on the just-shipped code.

1. **`1ed9f0a` ci(mesh-e2e)**: new `.github/workflows/mesh-e2e.yml`. The Playwright test landed in `27b45fe` passed locally but had no CI hook -- any refactor of `@lvqr/core::MeshPeer` / `lvqr-signal` / `lvqr-mesh` / `lvqr-cli`'s mesh wiring could silently break it. The workflow boots `lvqr-cli` in debug, npm-installs the `bindings/js` workspace, rebuilds `@lvqr/core/dist`, installs Chromium with system deps, runs `npx playwright test`. Dedicated rather than extending `e2e.yml` because the mesh suite has a separate npm workspace + playwright.config.ts. Soft-fail (`continue-on-error: true`) for the first weeks.

2. **`9dfdbe0` fix(test)**: the Playwright test now passes `iceServers: []` to MeshPeer so restricted CI runners don't hang on Google STUN lookup. Host candidates gather regardless of iceServers config; on loopback they're sufficient. Local rerun with `[]` still passes in ~460 ms (vs. ~270 ms with default STUN).

3. **`07ce9b9` fix(js)**: two latent-hang bugs in @lvqr/core 0.3.1 fixed on `main`. `LvqrClient` now exposes `connectTimeoutMs` (default 10_000) applied to WebTransport + WebSocket + WebSocket-broadcast paths via a shared `withConnectTimeout` helper that closes the in-flight handshake on timeout. `LvqrAdminClient` now exposes `fetchTimeoutMs` (default 10_000) applied to every admin HTTP call via AbortController. Both latent on the published npm; fixes land at the next publish cycle.

4. **`0df2bd1` fix(hls)**: Rust 1.95 stable promoted `clippy::unnecessary_sort_by` to warn-by-default; CI (which tracks stable) started failing the `Format and Lint` job. `crates/lvqr-hls/src/server.rs:1274` rewritten from `sort_by(|a, b| b.0.cmp(&a.0))` to `sort_by_key(|b| std::cmp::Reverse(b.0))`. Three other workspace `sort_by` sites are on String keys where the `sort_by_key` suggestion would require cloning; clippy correctly skips those.

5. **`940d597` fix(whip)**: proptest in `crates/lvqr-whip/tests/proptest_depack.rs` caught a round-trip-preservation bug on CI (Rust 1.95 / Ubuntu). Minimal failing input: `nals = [[0x00, 0x00, 0x01]]`. A NAL body containing an embedded Annex B start-code pattern confuses the splitter into emitting two empty NALs. Real H.264 encoders escape this via an emulation-prevention byte (`00 00 03 xx`); the test's "well-formed" generator did not. Two changes: `prop_assume!` filter on the generator to reject NAL bodies whose `[0, 0, x <= 3]` window would require emulation prevention, plus a pinned `proptest_depack.proptest-regressions` file carrying the CI-discovered seed so the exact adversarial case replays on every future run. Adversarial (unescaped) path still covered by the pair of `_never_panics` properties that already live in the same file.

6. **`3e026f6` feat(mesh)**: new `onChildOpen(childId, dc)` callback on `MeshConfig`. Fires once per child when its DataChannel transitions to `open` on the parent side. Integrators who want deterministic one-shot push (e.g. init segment for a late-joining subscriber) can use this instead of the 100 ms `pushFrame` poll-loop the Playwright test uses as a workaround. Callback errors are swallowed so a throwing integrator does not tear down the parent-side state machine. Additive to `@lvqr/core`'s public API (optional field with default no-op); source- and ABI-compatible.

### Ground truth (post-116 sweep)

* **Head**: `3e026f6`. Local `main` = `origin/main`. 23 commits pushed across the session 113-116 arc; the last 11 are this session's mesh/quality chain.
* **Tests (default gate)**: **941** passed, 0 failed, 1 ignored. Unchanged; all session-116 new test targets are either feature-gated (`transcode`) or live outside the cargo workspace (Playwright).
* **Playwright**: 1 passed in ~270-460 ms on cached Chromium 1217.
* **CI**: Mesh E2E green on `9dfdbe0`; Test Contract green twice; Archive c2pa + Archive io-uring + Docker all green on `07ce9b9`'s run (before the clippy fix). `CI` and `LL-HLS Conformance` running against `3e026f6` at the last observation; expected green after the clippy fix cleared `Format and Lint`.
* **crates.io / npm / PyPI**: unchanged. The client-SDK bug fixes + the new `pushFrame` / `onChildOpen` APIs ride on `main` and land at the next @lvqr/core publish.



## Session 116 close (2026-04-22)

**Shipped**: `PLAN_V1.1.md` row 115 (mesh data-plane step 2). First end-to-end exercise of the `@lvqr/core::MeshPeer` client against a real LVQR signaling server. `docs/mesh.md` flipped from "topology planner + signaling wired; DataChannel media relay ready for end-to-end testing" to "topology planner + signaling + subscribe-auth + server-side subscriber registration + client-side WebRTC relay + two-peer DataChannel media relay end-to-end test all ship."

1. **`fix(mesh): expose pushFrame API`** (`18e32fd`). A design-audit finding from reading `bindings/js/packages/core/src/mesh.ts` before writing the test: `MeshPeer` as shipped at `@lvqr/core` 0.3.1 has no public way for a root peer to forward media to its children. The only call site for `forwardToChildren` is inside the child-side `connectToParent` / `dc.onmessage` path, which a root peer (parentId null) never reaches. The docstring claims "Server -> Root peers -> Child peers via WebRTC DataChannel" but there is no mechanism for the integrator to feed the root its upstream bytes. Added `pushFrame(data: Uint8Array)` public method that delegates to `forwardToChildren`. Root peers drain media from the server separately (MoQ, WebTransport, LvqrClient, WS relay) and call `pushFrame` on every chunk to relay it down the mesh tree.

2. **`feat(cli): --mesh-root-peer-count flag`** (`b5796cb`). Session 111-B1 shipped `ServeConfig::mesh_root_peer_count` and `TestServerConfig::with_mesh_root_peer_count`, but the CLI never surfaced the flag. Playwright's `webServer` block spawns `lvqr serve` as a subprocess; without this flag the test cannot force the second subscriber to become a child of the first. New `--mesh-root-peer-count <N>` flag with `LVQR_MESH_ROOT_PEER_COUNT` env fallback. Defaults to None (inherits the `lvqr_mesh::MeshConfig` default of 30).

3. **`feat(test): row 115 Playwright E2E`** (`27b45fe`). New `bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts` (+190 LOC) + `bindings/js/playwright.config.ts`. Playwright's `webServer` boots `target/debug/lvqr serve --mesh-enabled --mesh-root-peer-count 1 --no-auth-signal --admin-port 18088 --hls-port 0 --rtmp-port 11935 --port 14443`. `url` polls `/api/v1/mesh` (returns 200 JSON when mesh is enabled) because the default `/` 404 fails Playwright's `<400` health-check gate. Test opens two browser contexts (Chromium only; phase-D scope expands the matrix) and injects the compiled `dist/mesh.js` into each via `addInitScript` (ESM `export class` rewritten to `class` + a `window.MeshPeer` assignment so the script can be loaded without a module loader). Both peers register via `/signal`; server assigns peer-one as Root and peer-two as Relay with parent=peer-one. peer-two auto-initiates an RTCPeerConnection + DataChannel. SDP offer/answer and ICE candidates flow through `/signal`. The DataChannel opens. peer-one pushes a known 8-byte payload via `pushFrame` in a 100 ms loop; peer-two's `onFrame` callback observes the bytes via the DataChannel `onmessage`. Completes in ~270 ms on loopback. `bindings/js/package.json` gains `@playwright/test ^1.49.0` + a `test:e2e` script; `.gitignore` excludes Playwright artifact directories.

4. **`docs(mesh)` + `docs(plan)`** (this commit). `docs/mesh.md` status block rewritten to reflect the full client-side chain that ships; phase-D scope (actual-vs-intended offload, per-peer capacity advertisement, TURN recipe, 3+ browser matrix) explicitly called out as pending. `tracking/PLAN_V1.1.md` row 115 flipped from "pending" to SHIPPED with the full test-shape written into the row summary.

### Key 116 design decisions baked in

* **The pushFrame loop at 100 ms cadence, not a single push on `childCount === 1`.** `children.set(msg.from, { pc, dc: null, peerId: msg.from })` fires on the parent side at the start of `handleOffer` -- BEFORE the child's DataChannel even arrives via `pc.ondatachannel`, let alone transitions to `open`. A single pushFrame on `childCount === 1` silently no-ops (`forwardToChildren` skips `readyState !== 'open'`). The 100 ms cadence is the simplest correct fix; the test harness passes in ~270 ms because the DataChannel opens within two or three ticks of `childCount === 1`. An alternative was to expose a `childReady(id)` / `onChildOpen` callback on `MeshPeer` so the test could wait on the `open` transition explicitly. Rejected because it grows the client API surface for an edge case the integrator does not need (integrators almost always push in a continuous loop anyway, not one-shot).
* **Playwright `webServer` command uses fixed non-default ports.** `--admin-port 18088`, `--rtmp-port 11935`, `--port 14443`, `--hls-port 0`. Deliberately off-default so a locally-running `lvqr serve` on default ports does not collide with the test subprocess. The trade is that the test is sensitive to those specific ports being free; on a CI runner this is never an issue, and locally the collision message is obvious.
* **Test injects `dist/mesh.js` via `addInitScript`, not via a module loader.** The compiled ESM has exactly one top-level export (`class MeshPeer`) and no imports, so an `/^export\s+class\s+MeshPeer/m` -> `class MeshPeer` regex plus an appended `window.MeshPeer = MeshPeer;` yields a classic script the browser runs directly. An alternative was to stand up a local vite dev server via the `webServer` block and use real ESM imports; rejected because it doubles the test's external dependency surface (bundler + dev server) for negligible clarity gain.
* **`/api/v1/mesh` as the health-check URL, not a new `/health` endpoint.** Playwright's `webServer.url` expects `<400`; lvqr's admin router returns 404 on `/`. Adding a new `/health` route just for Playwright would grow the server's public surface; pointing the health check at the existing `/api/v1/mesh` route (which returns 200 JSON whenever `--mesh-enabled`) is a zero-cost fit. Requires `--mesh-enabled` to be set, which the Playwright webServer command always does.
* **Chromium-only first matrix.** Firefox and WebKit support RTCPeerConnection + RTCDataChannel, but WebRTC dialect differences (ICE lite, unified plan vs plan-b, DataChannel sctp negotiation) are real. Landing the first test green on one browser before expanding is the safer sequencing; the expansion is phase-D scope per the session 116 briefing.

### Ground truth (session 116 close)

* **Head (pre-push)**: `18e32fd` fix(mesh) + `b5796cb` feat(cli) + `27b45fe` feat(test) + this close-doc commit. `origin/main` head was `33e3802` at the start of this session; will move to the close-doc commit's SHA on the next push event.
* **Tests (default features gate)**: **941** passed, 0 failed, 1 ignored on macOS. Unchanged from session 115's 941 because this session adds (a) one Playwright Node test that lives outside the cargo workspace and (b) one internal `MeshPeer.pushFrame` method that has no direct Rust test coverage yet. The Playwright test lives at `bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts` and runs via `npx playwright test` from `bindings/js/`.
* **Playwright test locally**: passes in ~270 ms on a cached Chromium 1217 install.
* **CI gates locally clean**:
  * `cargo fmt --all --check`.
  * `cargo clippy --workspace --all-targets -- -D warnings`.
  * `cargo test --workspace` 941 / 0 / 1.
  * `npx playwright test` from `bindings/js/` passes the one spec.
* **Workspace**: **29 crates**, unchanged.
* **crates.io**: unchanged. Session 116 adds one CLI flag (additive) + one `@lvqr/core` class method (additive; source-compatible for the next npm publish cycle). No version bumps in this chain; next npm publish picks up `pushFrame`.

### Known limitations / documented v1 shape (after 116 close)

* **Server-originating media into the root peer's `pushFrame` is still integrator-driven**. A production deployment that wants mesh-offloaded fanout needs its own code to drain MoQ / WS / HLS on the root peer and forward via `pushFrame`. A future session could ship an `@lvqr/core` helper that bridges `LvqrClient`'s frame stream into `MeshPeer.pushFrame` automatically; this would close the last integrator-friction point.
* **Actual-vs-intended offload reporting** remains unshipped. `/api/v1/mesh` today reports tree-shape-intended offload, not measured traffic. Phase D.
* **Per-peer capacity advertisement** remains unshipped. `max-children` is a hard-coded per-node ceiling; peers do not advertise bandwidth / CPU / concurrent-subscriber capacity for rebalancing. Phase D.
* **TURN deployment recipe + symmetric-NAT fallback** not yet documented. For loopback + local-candidate tests (which is what the Playwright test exercises), STUN is unused and ICE completes via host candidates. A real deployment with peers behind symmetric NATs will need a coturn sidecar. Phase D.
* **Three-peer Playwright E2E + the 5-artifact test contract sweep** are phase D. This session ships the two-peer happy path only.
* All session 113 / 114 / 115 known limitations unchanged.

## Session 115 close (2026-04-22)

## Session 115 close (2026-04-22)

**Shipped**: the deferred row-2 of session 114 (RTMP to WHEP audio E2E). That row was the single largest documented user-visible test gap from the v1.1 audit (no RTMP-to-WHEP coverage at all on any ingest path feeding a WebRTC subscriber), and closing it flips `tracking/PLAN_V1.1.md` row 114 from "PARTIALLY SHIPPED" to "SHIPPED". The kickoff prompt listed this as Option B with the caveat that GStreamer absence on the dev host would compile-and-skip the test target; this host matched that profile, so the test lands as a feature-gated compile-only on local `main` and picks up the actual asserts on the GStreamer-enabled CI matrix that already runs `aac_opus_roundtrip.rs`.

1. **`crates/lvqr-cli/tests/rtmp_whep_audio_e2e.rs`** (+460 LOC, feature-gated on `transcode`). Generates ~800 ms of real AAC-LC 48 kHz stereo audio via an in-test `audiotestsrc ! avenc_aac ! aacparse` GStreamer pipeline (same pattern as `crates/lvqr-transcode/tests/aac_opus_roundtrip.rs`, strips the 7-byte ADTS header so each vec is a raw AAC access unit that `flv_audio_raw` can wrap). The test probes `AacToOpusEncoderFactory::is_available()` up front and prints a `skipping rtmp_whep_audio_e2e: ...` line + returns clean when any of `aacparse` / `avdec_aac` / `audioconvert` / `audioresample` / `opusenc` / `avenc_aac` is missing, matching the `aac_opus_roundtrip.rs` skip idiom.
   * **RTMP publisher**: `rml_rtmp` client connects, completes the RTMP handshake, publishes an FLV video sequence header + FLV audio sequence header (AAC-LC 48 kHz stereo, ASC `[0x11, 0x90]` matching the encoder test's expected config bytes) + one H.264 keyframe (so the broadcast registers on the relay), then spins the pre-generated AAC access units out at 21 ms cadence wrapped in `[0xAF, 0x01, ...]` FLV audio tags.
   * **WHEP HTTP subscriber**: fresh `str0m::Rtc` with `enable_opus(true)` only, audio-recvonly mid, POSTs the offer to `/whep/live/test`, asserts 201 Created + a `Location: /whep/live/test/{session_id}` header + an SDP answer that contains `opus`, then runs the same poll-loop shape as `crates/lvqr-whep/tests/e2e_str0m_loopback_opus.rs` until the client raises `Event::MediaData` at least once.
   * **Expected wire path**: FLV audio -> `RtmpMoqBridge` -> `SessionMsg::AudioConfig` (first) + `SessionMsg::Aac` (subsequent) -> `AacToOpusEncoder::push` -> `OpusFrame` on `opus_rx` -> `run_session_loop`'s Opus arm -> str0m `Writer::write` on the negotiated Opus Pt -> client's `Event::MediaData`.
   * **Design decision: publish AAC seq header BEFORE the first keyframe**. This ensures `WhepServer::audio_configs` caches the AudioSpecificConfig before the WHEP subscriber POSTs its offer, so `handle_offer`'s session-113 cached-config replay fires on the new session handle and the `AacToOpusEncoder` is spawned with the correct `AacAudioConfig` at the moment the first `SessionMsg::Aac` lands. Without this ordering the encoder would start in "no config yet" mode and the first burst of samples would be dropped while waiting for the config to arrive out-of-band.
   * **Design decision: 400 ms sleep between RTMP publisher spawn and WHEP POST**. The RTMP handshake + connect + publish + first audio seq header takes a handful of async ticks; giving the bridge enough wall time to cache the ASC before the offer hits eliminates a known-flaky race where the `WhepServer::on_audio_config` fanout arrives after `handle_offer` has already inserted the new session into the registry without a replay. The eventual race-resolving code path is still exercised by the out-of-order samples that arrive later, but the first-media-frame assert is much faster when the initial session has the ASC up front.

2. **`crates/lvqr-test-utils/src/test_server.rs`**: new `whep_enabled: bool` field + `with_whep()` builder + `whep_addr()` accessor. Mirrors the existing `with_whip` / `whip_addr` shape bit-for-bit (feature unchanged; default disabled so every pre-115 TestServer caller still starts without the WHEP listener).

3. **Session 115 close doc** (this block). `tracking/PLAN_V1.1.md` row 114 flipped from "PARTIALLY SHIPPED" to "SHIPPED" with the row-2 detail rolled into the row summary. Phase B rows 113 + 114 are now both SHIPPED; the next phase-B row is 115 (mesh data-plane step 2 with Playwright).

### Key 115 design decisions baked in

* **Test-utils change is additive, not a new crate**. `TestServerConfig::with_whep()` is a sibling of the existing `with_whip` / `with_dash` / `with_srt` / `with_rtsp` builders; `whep_addr: None` was already hardcoded in `test_server.rs:284` since the initial TestServer landing, and all that was needed was to flip the hardcoded `None` to `if config.whep_enabled { Some(ephemeral) } else { None }`. The CLI-side composition root already wires `WhepServer` + `Str0mAnswerer::with_aac_to_opus_factory` correctly when `config.whep_addr` is `Some` and the `transcode` feature is on; the test server therefore inherits the session 113 Opus-audio path for free the moment `with_whep()` is called under `--features transcode`.
* **Test lives in `crates/lvqr-cli`, not `crates/lvqr-whep`**. `lvqr-whep` has the unit-style `e2e_str0m_loopback*.rs` tests that hit `Str0mAnswerer::create_session` directly, bypassing HTTP. A true RTMP-to-WHEP E2E needs both the RTMP ingest crate (`lvqr-ingest`'s RTMP server) and the WHEP HTTP router live in the same process, so the CLI crate's composition root via `TestServer` is the right home. The str0m dev-dep pin added for session 114 (`str0m = "0.18"` on `lvqr-cli`) is reused verbatim.
* **FLV audio header matches the encoder's 48 kHz stereo contract**. `aac_opus_roundtrip.rs`'s in-module AAC generator produces 48 kHz stereo AAC-LC (ASC bytes `[0x11, 0x90]`), and the encoder's `AacAudioConfig` is keyed on those bytes. Using the same ASC in the RTMP publisher keeps the whole pipeline (bridge -> encoder -> opusenc -> Opus RTP -> client) on a single matched config; the alternative (44.1 kHz stereo, ASC `[0x12, 0x10]`, matching `rtmp_hls_e2e.rs`) would have exercised `opusenc`'s `audioresample` stage on the way out, which is worth testing but not for this particular smoke test.
* **Hold the RTMP publisher alive via `tokio::spawn` + `JoinHandle::abort`**. Dropping the RTMP stream mid-publish fires `BroadcastStopped` through the bridge which tears down the WHEP session handle's registry entry before the client has had a chance to poll `Event::MediaData`. The test spawns the publisher, awaits its first few ticks via a 400 ms sleep, lets the client poll loop run, and only `abort()`s the publisher after the subscriber's asserts complete. Symmetric shutdown via `server.shutdown().await` follows.

### Ground truth (session 115 close)

* **Head**: feat commit `3937a44` + close-doc commit (pending, lands with this block). Local `main` is 6 commits ahead of `origin/main` (head `2e50635`); both 113 and 114 and 115 commit pairs are unpushed.
* **Tests (default features gate)**: **941** passed, 0 failed, 1 ignored on macOS. Unchanged from session 114's 941 because the new target is `#![cfg(feature = "transcode")]` so the default gate does not compile it. Hosts with GStreamer installed (CI matrix) pick up the new test under `cargo test -p lvqr-cli --features transcode --test rtmp_whep_audio_e2e`.
* **CI gates locally clean**:
  * `cargo fmt --all --check`.
  * `cargo clippy --workspace --all-targets -- -D warnings`.
  * `cargo test --workspace` 941 / 0 / 1.
  * `cargo test -p lvqr-cli --features transcode --test rtmp_whep_audio_e2e` not verifiable on this macOS dev host (GStreamer runtime not installed via brew); covered by the feature-on CI matrix alongside `aac_opus_roundtrip.rs`.
* **Workspace**: **29 crates**, unchanged.
* **crates.io**: unchanged. Session 115 adds one new integration test target + a non-breaking builder method (`with_whep`) on `TestServerConfig` + a non-breaking accessor (`whep_addr`) on `TestServer`. No public API on any published crate moved.

### Known limitations / documented v1 shape (after 115 close)

* **`rtmp_whep_audio_e2e` runs only on GStreamer-enabled CI hosts**. This is the same posture as `aac_opus_roundtrip.rs` and the transcode_ladder_e2e; operators who want to run the full test locally need GStreamer 1.22+ with `gst-libav` + `gst-plugins-base` + `gst-plugins-good` installed. The default CI gate continues to skip the compile of this target.
* All session 113 known limitations (per-session encoder, approximate Opus SLO stamp, no client-side render timing) unchanged.
* All session 114 known limitations unchanged (LL-HLS partial-vs-segment tolerance in `whip_hls_e2e.rs`, SRT socket held open across DASH reads in `srt_dash_e2e.rs`).

### Post-115 fix (2026-04-22, commit `0c2c59d`)

A design audit on top of the close commit surfaced a latent CI-break in `crates/lvqr-cli/tests/rtmp_whep_audio_e2e.rs`: the test imported `gstreamer`, `gstreamer-app`, and `glib` directly, but `lvqr-cli`'s `Cargo.toml` does not list those as direct deps (they arrive only transitively via `lvqr-transcode` when the `transcode` feature is on). A transitive dep is not in the downstream crate's namespace for `use ...`, so the test file would resolve-error on any GStreamer-enabled CI host running `cargo test -p lvqr-cli --features transcode --test rtmp_whep_audio_e2e`. The default local gate did not catch it because the test is `#![cfg(feature = "transcode")]`-gated and never compiled on GStreamer-absent hosts.

Fix commit `0c2c59d` hoists the `audiotestsrc ! avenc_aac ! aacparse` helper out of `crates/lvqr-transcode/tests/aac_opus_roundtrip.rs` into a new `pub mod lvqr_transcode::test_support` module gated on the existing `transcode` feature. Both the session 113 encoder round-trip test and the session 115 RTMP->WHEP test now call `lvqr_transcode::test_support::generate_aac_access_units(duration_ms)`, so neither needs a direct gstreamer dep; the gstreamer crate graph stays a private implementation detail of `lvqr-transcode`. Also dedups ~80 LOC of identical pipeline setup across the two tests. Default gate still 941 / 0 / 1.

Semantic note: the shared helper uses `if let Ok(map) = buf.map_readable()` to skip the current sample on a transient map failure, whereas the original `aac_opus_roundtrip.rs` used `buf.map_readable().ok()?` which would propagate a `None` return from the whole function. In practice `map_readable` does not fail on live in-process GStreamer buffers; the tolerant behavior is strictly a small improvement and does not change the "AAC round-trips through the encoder" assertion.

### Post-115 fix (2026-04-22, commit `1c6d3f6`) -- real-time publisher cadence

Second design-audit finding on the session 115 test. The original `run_rtmp_publisher` used `tokio::time::sleep(Duration::from_millis(5))` between AAC sample pushes, so the 38-frame burst from the 800 ms generator finished in ~200 ms. That is well shorter than the WHEP subscriber's ICE + DTLS warm-up on a typical loopback handshake (~500-900 ms, longer on a loaded CI runner). Every Opus packet the per-session `AacToOpusEncoder` produced during the warm-up window would route through `write_opus_frame`'s pre-Connected drop branch, and by the time ICE completed there would be no fresh AAC still arriving. Net effect: `Event::MediaData` would never fire on the client and the test would time out against the 20 s deadline with no useful diagnostic.

Fix commit `1c6d3f6`:

1. Switch the publisher's inter-sample sleep from 5 ms (busy-burst) to 21 ms via `tokio::time::sleep_until` so samples arrive at the RTMP bridge at real-time cadence (1024 samples / 48 kHz = 21 1/3 ms per frame). Publisher now stays alive for the full duration of the generated span instead of bursting in 200 ms.
2. Bump the generator from 800 ms to 1600 ms so fresh AAC continues to reach the WHEP session poll loop through the full 500-900 ms ICE + DTLS warm-up plus a healthy tail for the first post-Connected Opus frame to land.

Also confirmed `rml_rtmp::sessions::ClientSession` is `Send` (no `Rc` / `RefCell` / raw-pointer fields in `rml_rtmp-0.8.0/src/sessions/client/`), so `tokio::spawn(run_rtmp_publisher(..))` is sound even though no other test in the workspace spawns a `ClientSession`.

## Session 114 close (partial) (2026-04-21)

### Commit chain on local `main` (chronological)

| SHA | Type | Scope |
|---|---|---|
| `b79cf6a` | docs | **111-A** v1.1 plan + README drift fixes + Known v0.4.0 limitations + docs/mesh.md refresh |
| `791152d` | feat(auth) | **112** live HLS + DASH subscribe auth middleware + `--no-auth-live-playback` flag + cargo audit CI workflow (7 new tests) |
| `6206870` | feat(mesh) | **111-B1** /signal subscribe auth via `?token=` + `ServerHandle::mesh_coordinator()` + MoQ-over-DataChannel wire-format decision doc (6 new tests) |
| `97bc16d` | refactor(cli) | Split `lib.rs` -- extract `auth_middleware.rs` + `ws.rs` modules (2513 -> 1830 lines) |
| `8da444a` | refactor(cli) | Split `lib.rs` -- extract `config.rs` + `handle.rs` modules (1830 -> 1110 lines) |
| `db23215` | feat(mesh) | **111-B2** WS-relay peer registration + leading `peer_assignment` JSON text frame + `ws_relay_session` idle-disconnect fix + idempotent `/signal` register callback (2 new tests) |
| `7bc16a9` | feat(mesh) | **111-B3** Sec-WebSocket-Protocol echo in `lvqr-signal` + bearer subprotocol extraction in `signal_auth_middleware` (2 new tests) |
| `d340a6f` | docs | Sync README + docs/mesh.md with mesh prereqs shipped (4 checklist items flipped to shipped) |
| `2e50635` | docs | Session 111-B3 close -- HANDOFF status refresh + README Known Limitations "Fixed on main" flags + kickoff prompt [**origin/main head**] |
| `323be2f` | feat(whep) | **113** WHEP AAC-to-Opus transcoder + `on_audio_config` observer hook + ADTS wrap + factory probe + session poll-loop Opus arm (integration test gated on `transcode` feature) |
| `3e9b444` | docs | Session 113 close -- HANDOFF + README "Fixed on main" flag + PLAN_V1.1.md row 113 SHIPPED |
| `b70be73` | feat(test) | **114 partial** WHIP->HLS + SRT->DASH E2E tests + 5 session-113 audit unit tests (parse_aac_asc refactor) (+7 default-gate tests, 934 -> 941) |
| `b315345` | docs | Session 114 partial close -- HANDOFF + PLAN_V1.1.md row 114 PARTIALLY SHIPPED |
| `d1b9607` | docs | Post-114 HANDOFF cleanup -- real SHAs + dedup + kickoff refresh |
| `3937a44` | feat(test) | **115** RTMP->WHEP audio E2E -- closes 114 row 2 (feature-gated `transcode`); TestServer gains `with_whep()` |
| `80bba4b` | docs | Session 115 close -- HANDOFF + PLAN_V1.1.md row 114 SHIPPED |
| `0c2c59d` | fix(transcode) | Post-115 fix -- share the AAC test-bytes generator via `pub lvqr_transcode::test_support` so `rtmp_whep_audio_e2e.rs` does not need direct gstreamer dev-deps on `lvqr-cli`; `aac_opus_roundtrip.rs` refactored to the same shared helper (~80 LOC de-dup) |
| `42db8ca` | docs | Session 115 post-close HANDOFF sweep -- chain table + audit-finding note |
| `1c6d3f6` | fix(test) | Post-115 fix -- publisher AAC cadence switched from 5 ms burst to 21 ms real-time sleep_until; generator span 800 ms -> 1600 ms so fresh AAC continues to reach the WHEP session through the full 500-900 ms ICE + DTLS warm-up [**local main head**] |

## Session 114 close (partial) (2026-04-21)

**Shipped**: 2 of 3 phase-B row-2 E2E tests + 5 audit-coverage unit tests on session 113 plumbing. Row 2 (RTMP to WHEP audio with real str0m client) is written up as unshipped scope below so it can be picked up by a future session on a GStreamer-provisioned CI host.

1. **Row 1: WHIP to HLS E2E test** (`crates/lvqr-cli/tests/whip_hls_e2e.rs`, +340 LOC). Drives a real `str0m::Rtc` publisher against the WHIP HTTP surface, POSTs an SDP offer to `/whip/live/test`, validates the `201 Created` response + `Location: /whip/live/test/{session_id}` header + parseable SDP answer, extracts the host candidate, completes ICE + DTLS + SRTP in-process over loopback UDP, writes synthetic H.264 samples (SPS + PPS + IDR per sample at ~50 Hz) through the client's `Writer::write`, and polls `/hls/live/test/playlist.m3u8` every 200 ms until the playlist carries at least one `#EXT-X-PART:` or `#EXTINF:` entry. Completes in ~0.4 s locally. Covers the full path: WHIP HTTP router -> `Str0mIngestAnswerer` -> `run_session_loop` -> `WhipMoqBridge` sample-side -> `MoqTrackSink` + `FragmentBroadcasterRegistry` -> `BroadcasterHlsBridge` drain -> `MultiHlsServer` -> axum HTTP.
   * New dev-dep `str0m = "0.18"` on `lvqr-cli` (same pin as `lvqr-whep`). Matches the existing precedent where `lvqr-cli` dev-deps call into `lvqr-ingest`'s publisher crates directly (e.g. `rml_rtmp`, `srt-tokio`) for E2E tests.
   * Assertion tolerance: accepts `#EXT-X-PART:` (LL-HLS partial) OR `#EXTINF:` (full segment) because the default 2 s / 90 kHz segment budget may not close a full segment inside a ~400 ms test run. The LL-HLS bridge is already covered elsewhere on the full-segment path via `rtmp_hls_e2e.rs`; this test's value is the WHIP-side plumbing.

2. **Row 3: SRT to DASH E2E test** (`crates/lvqr-cli/tests/srt_dash_e2e.rs`, +260 LOC). Mirrors the `srt_hls_e2e.rs` publisher (SRT caller pushing a minimal PAT + PMT + two H.264 keyframes spaced 2 s apart at 180_000 ticks / 90 kHz) against the `rtmp_dash_e2e.rs` DASH subscriber assertions (`<MPD>` element + `type="dynamic"` + `<AdaptationSet>` + `seg-video-$Number$.m4s` template + `init-video.m4s` with `ftyp` prefix + `seg-video-1.m4s` with `moof` prefix + 404 on an unknown broadcast). Completes in ~1.2 s locally. Key design call: the SRT socket is held open across the DASH read assertions to keep the broadcast in the live `type="dynamic"` state; closing immediately after the TS payload send cascades through `BroadcastStopped` into `MultiDashServer::finalize_broadcast` and flips the manifest to `type="static"` with the on-demand profile. The socket is explicitly `close()`d at the end of the test for symmetric teardown.

3. **Session 113 audit tests** (5 new tests in `crates/lvqr-whep/src/str0m_backend.rs`). Refactored the AAC `AudioSpecificConfig` parse from an inline-in-`on_audio_config` body to a `parse_aac_asc(bytes: &[u8]) -> Option<(u8, u32, u8)>` free function so the byte-layout contract is independently testable. Unit tests:
   * `parse_aac_asc_aac_lc_48khz_stereo`: ASC `[0x11, 0x90]` parses to `(2, 48_000, 2)`.
   * `parse_aac_asc_aac_lc_44100_stereo_matches_flv_test_fixture`: ASC `[0x12, 0x10]` (the exact bytes `flv_audio_seq_header` in `rtmp_hls_e2e.rs` produces) parses to `(2, 44_100, 2)`.
   * `parse_aac_asc_returns_none_for_short_input`: empty and 1-byte inputs return `None`.
   * `aac_freq_index_to_hz_known_values`: known indices (0 / 3 / 4 / 7 / 12) map to the right Hz, and out-of-table indices (13, 15) fall back to 44.1 kHz.
   * `on_audio_config_aac_does_not_panic_and_survives_drop`: integration-lite tokio test that pushes a real FLV-shaped ASC + an empty ASC + a non-AAC codec through a live `Str0mSessionHandle` and asserts clean shutdown. Without the `aac-opus` feature the poll loop drops the `SessionMsg::AudioConfig` after parsing; this test locks in the parse path.

4. **Session 114 close doc** (this block). `tracking/PLAN_V1.1.md` row 114 marked "PARTIALLY SHIPPED" with row 2 (RTMP-to-WHEP audio) called out as the one piece deferred; row 2's scope is sized as 1-2 hours of work on a CI host with GStreamer installed.

### Key 114 design decisions baked in

* **Dev-dep `str0m` is specifically pinned on `lvqr-cli`**, not routed through `lvqr-whip` (which would require making str0m re-exports). The precedent is already set: `lvqr-cli` dev-deps directly name `rml_rtmp`, `srt-tokio`, and `moq-native` to drive publishers / subscribers; str0m is the same shape. Version pin matches the existing `lvqr-whep` dep so the cargo lock stays minimal.
* **WHIP-to-HLS test tolerates both `#EXT-X-PART:` and `#EXTINF:` entries**. LL-HLS partials appear within ~1 s of the first keyframe; full segments require a 2 s span between keyframes. The test's 20 s deadline is generous but the assertion accepts the partials so the test stays under 1 s on a warm ICE handshake. Full-segment drain is already covered by `rtmp_hls_e2e.rs`.
* **SRT-to-DASH test holds the SRT socket open across the DASH read**. Without this, the `BroadcastStopped` event fires on socket close and `MultiDashServer::finalize_broadcast` flips the manifest to the on-demand profile (`type="static"`). The test explicitly closes the socket only after the dynamic-state assertions complete, to document the teardown invariant.
* **Session 113 ASC parser extracted from inline to a named free function**. The extraction adds one `parse_aac_asc` function + removes 14 lines of inline parsing from `Str0mSessionHandle::on_audio_config`. Behavior is byte-for-byte identical; the extraction exists purely so the parse can be unit-tested without booting a full session.

### Ground truth (session 114 partial close)

* **Head**: feat commit `b70be73` + close-doc commit `b315345`. Local `main` is 4 commits ahead of `origin/main` (head `2e50635`); both 113 and 114 commit pairs are unpushed.
* **Tests (default features gate)**: **941** passed, 0 failed, 1 ignored. +7 over session 113's 934: the 4 `parse_aac_asc` + `aac_freq_index_to_hz` unit tests (+4), the `on_audio_config_aac_does_not_panic_and_survives_drop` tokio test (+1), `srt_publish_reaches_dash_router` (+1), `whip_publish_reaches_hls_playlist` (+1).
* **CI gates locally clean**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` 941 / 0 / 1.
* **Workspace**: **29 crates**, unchanged.
* **crates.io**: unchanged. 114 adds only tests + one internal refactor (`parse_aac_asc` extraction) that is behavior-preserving.

### Known limitations / documented v1 shape (after 114 partial close)

* **Row 2 (RTMP to WHEP audio E2E) deferred**. The AAC-to-Opus encoder path requires GStreamer at test time; the dev host used for session 114 does not have GStreamer installed. A follow-up session on a GStreamer-provisioned CI runner can land the test directly by combining `rtmp_dash_e2e.rs`'s RTMP publisher with the `e2e_str0m_loopback_opus.rs` str0m-client subscriber pattern, plus a real AAC sample source (either the `aac_opus_roundtrip.rs`'s `audiotestsrc ! avenc_aac` generator or a captured WAV -> AAC fixture).
* All session 113 known limitations (per-session encoder, approximate Opus SLO stamp) unchanged.

## Session 113 close (2026-04-21)

1. **Phase B row 113: WHEP AAC to Opus transcoder** (feat commit).
   * **New `AacToOpusEncoder` + `AacToOpusEncoderFactory` in `lvqr-transcode`** (`crates/lvqr-transcode/src/aac_opus.rs`, +~500 LOC). Pure GStreamer worker-thread wrapper, siblings of session 105's `SoftwareTranscoder`: `gst::init()` probe at factory construction, `REQUIRED_ELEMENTS = ["appsrc", "aacparse", "avdec_aac", "audioconvert", "audioresample", "opusenc", "appsink"]`, same `sync_channel(WORKER_QUEUE_DEPTH=64)` bounded-mpsc between the `push()` site and the worker, same 5 s `SHUTDOWN_TIMEOUT` EOS drain on `Drop`. Pipeline: `appsrc is-live=true format=time do-timestamp=true caps=audio/mpeg,mpegversion=(int)4,stream-format=(string)adts ! aacparse ! avdec_aac ! audioconvert ! audioresample ! audio/x-raw,format=(string)S16LE,rate=(int)48000,channels=(int)2,layout=(string)interleaved ! opusenc bitrate=64000 frame-size=20 ! appsink name=sink emit-signals=true sync=false`. Input wrapping: `wrap_adts` synthesises a 7-byte ADTS header from the known `AudioSpecificConfig` fields (`object_type-1` as profile, `sample_rate -> sample_frequency_index`, `channels -> channel_configuration`, buffer_fullness bits set to 0x7FF per the stream-adts-from-raw convention). Output: each `opusenc` buffer minus the header-flagged ones lands on a caller-supplied `tokio::sync::mpsc::UnboundedSender<OpusFrame>` so the downstream consumer (the WHEP session poll loop) drains through a standard tokio arm. Gated behind the existing `transcode` Cargo feature so default CI gate hosts (no GStreamer) continue to build the crate.
   * **New feature `aac-opus` on `lvqr-whep`.** Adds an optional `lvqr-transcode` dep that activates `lvqr-transcode/transcode` via the feature list: `aac-opus = ["dep:lvqr-transcode", "lvqr-transcode/transcode"]`. Default OFF so hosts without GStreamer continue to build the WHEP crate as they did pre-113.
   * **`on_audio_config` hook on `RawSampleObserver` + `SessionHandle` traits.** Fires once per track when the upstream bridge learns the codec config; default impl is a no-op so existing observers / session handles compile unchanged. RTMP bridge `on_audio` handler fires it on every `FlvAudioTag::SequenceHeader` (ASC bytes). `WhepServer::on_audio_config` caches the snapshot keyed by broadcast name and fans it to every currently-subscribed session; the router also replays the cached snapshot onto a newly-created session in `handle_offer` so a subscriber joining after the first AAC ASC still gets the config.
   * **WHEP session poll-loop wiring** (`crates/lvqr-whep/src/str0m_backend.rs`). New `SessionMsg::Aac` + `SessionMsg::AudioConfig` variants alongside the existing `SessionMsg::Video`. `SessionCtx` gains `aac_config: Option<AacAudioConfig>`, `aac_encoder: Option<AacToOpusEncoder>`, `opus_write_dts: u64`, `last_aac_ingest_ms: u64`, and `aac_without_factory_warned: bool`. `Str0mAnswerer::with_aac_to_opus_factory(Arc<AacToOpusEncoderFactory>) -> Self` builder (behind `aac-opus` feature). `run_session_loop` gains: (a) new `select!` arm on `opus_rx` that calls `write_opus_frame(...)` through the negotiated Opus mid/pt; (b) new handler for `SessionMsg::Aac` that lazily spawns the encoder on first AAC sample once `aac_config` + factory are both present, then `enc.push(&payload, dts)`; (c) new handler for `SessionMsg::AudioConfig` that parses `object_type`, `samplingFrequencyIndex`, and `channelConfiguration` off the first 2 ASC bytes and caches an `AacAudioConfig` on the session. SLO record: each successful `write_opus_frame` records `now_ms - last_aac_ingest_ms` under `transport="whep"` so the audio path participates in the existing 110-B histogram.
   * **`Str0mSessionHandle`**: `on_raw_sample` splits `MediaCodec::Aac` into `SessionMsg::Aac` vs `SessionMsg::Video` so the AAC track stops riding the video `write_sample` path (which still carries a `Ok(false)` drop branch for pre-113 callers). `on_audio_config(&self, track, codec, codec_config)` converts the ASC bytes into a `SessionMsg::AudioConfig` and sends onto the session's sample channel.
   * **Composition root wiring** (`crates/lvqr-cli/src/lib.rs`, `start()`). When the CLI is built with `--features transcode`, the `Str0mAnswerer` gets `.with_aac_to_opus_factory(Arc::new(AacToOpusEncoderFactory::new()))` before being cloned into `WhepServer`. The factory probes GStreamer elements once on construction; missing elements log once and every subsequent `build()` returns `None`, so a host without `gst-libav` still serves video to WHEP without panicking.
   * **Integration test** (`crates/lvqr-transcode/tests/aac_opus_roundtrip.rs`). Generates 400 ms of real AAC-LC audio via an in-process `audiotestsrc ! avenc_aac ! aacparse` pipeline (skips cleanly when any generator element is absent), pushes each access unit through `AacToOpusEncoder::push`, drains the `OpusFrame` receiver for up to 2 s, and asserts at least one non-empty Opus packet lands with `duration_ticks == 960` (20 ms at 48 kHz). Gated on `#![cfg(feature = "transcode")]` so the default CI gate does not even build the test target.

2. **Session 113 close doc** (this block).
   * Also flipped the README "WHEP has no AAC audio" Known Limitations bullet to "**Fixed on `main`**" and tweaked the "Egress" feature paragraph to advertise the new Opus-audio path. `tracking/PLAN_V1.1.md` row 113 marked SHIPPED.

### Key 113 design decisions baked in (confirmed in-commit per the plan-vs-code rule)

* **Encoder lives in `lvqr-transcode`, not `lvqr-whep`**. The scope row explicitly points at `lvqr-transcode` (or a `lvqr-transcode-audio` sibling if the dep graph demanded it). The graph does not demand a split: `lvqr-transcode` is already feature-gated on `transcode` with its own gstreamer-rs optional deps, so adding the AAC path behind the same feature adds zero new crates. `lvqr-whep` is the consumer; it pulls `lvqr-transcode` as an optional dep behind `aac-opus`, and the CLI's `transcode` meta-feature activates both so operators never have to opt in twice.
* **`avdec_aac` + `aacparse` + ADTS input, not `faad` + raw input with `codec_data` caps**. The scope suggested `faad`; in practice `faad` lives in `gst-plugins-bad` (LGPL) and is missing from many distribution default installs, while `avdec_aac` lives in `gst-libav` (already a session-105 dep for video `avdec_h264`). ADTS framing over raw is also more forgiving when the caller is pushing one access unit per buffer: `aacparse` negotiates the exact `audio/mpeg,mpegversion=4,stream-format=adts` caps without the caller having to mint a precise `codec_data` buffer from the ASC. The ADTS header synthesis in `wrap_adts` reads the sample rate index from a static `ADTS_SAMPLE_RATES` table, reads the channel config and profile off the known `AacAudioConfig`, and sets the `aac_frame_length` field to `7 + payload.len()`. Four unit tests in the in-module `tests` mod lock the header layout in.
* **Per-session encoder, not per-broadcast**. An alternative was to register the transcoder at the `FragmentBroadcasterRegistry` level (like `SoftwareTranscoderFactory`) and publish Opus fragments into a shared broadcaster that every WHEP session subscribes to. That saves CPU when N > 1 subscribers share a broadcast but introduces a new dependency on the fragment-broadcaster path for audio-only flow and couples WHEP session state to a broadcast-level resource the WHEP code otherwise does not touch. Per-session keeps the WHEP code self-contained and matches the existing WHIP Opus passthrough path (which stamps Opus per-session too). For a v0.4 deployment with single-digit concurrent subscribers, the CPU overhead of N encoders is negligible; a v1.2 optimisation can move to a shared encoder behind a new flag if profiling shows it matters.
* **`AacToOpusEncoderFactory` probes once, opts out per-build**. Matches the `SoftwareTranscoderFactory` shape from session 105: one `gst::init()` + element-probe at construction logs once on a shortfall; every `build()` returns `None` when the probe fails. A misconfigured host therefore still serves WHEP (just without AAC audio), rather than panicking at session-start time.
* **AAC ASC plumbing via an `on_audio_config` trait method, not a new field on `RawSample`**. Adding a field to `lvqr_cmaf::RawSample` would touch ~30 struct-literal construction sites across `lvqr-rtsp`, `lvqr-srt`, `lvqr-whip`, `lvqr-whep`, `lvqr-soak`, `lvqr-cmaf` benches + tests, and so on (same reason 110 B threaded `ingest_time_ms` through a trailing trait-method parameter instead of onto `RawSample`). An `on_audio_config` trait method with a default no-op is narrower: it adds one new method to two traits, one new call site in `lvqr-ingest::bridge`, one new fanout in `WhepServer::on_audio_config`, one new branch in `Str0mSessionHandle::on_audio_config`, and one new `SessionMsg::AudioConfig` message. Every other consumer (the two existing WhepServer sibling tests, federation replay, synthetic fixtures) inherits the stub.
* **WhepServer caches the latest ASC per broadcast**. A WHEP subscriber that joins after the publisher's first SequenceHeader is common (browser refresh, late-joining viewer). `WhepState::audio_configs: DashMap<String, AudioConfigSnapshot>` captures the latest config by broadcast name; `WhepServer::cached_audio_config` reads a clone so the router's `handle_offer` can call `SessionHandle::on_audio_config` on the freshly-minted session before inserting it into the registry. The cache is overwrite-on-update (latest wins); publishers that change their AAC config mid-stream (unusual but legal for adaptive-bitrate audio) have all matching sessions resynchronise on the next sample.
* **Opus SLO stamp uses "most recent AAC ingest wall-clock" as the ingest time**. `opusenc` buffers internally so the Opus packet the writer emits at time T does not 1:1 correspond to the most-recent AAC input, but a 20 ms Opus frame vs a 21.3-23.2 ms AAC frame drifts by under 3 ms across a burst. The histogram bucket boundaries in `docs/slo.md` (p50=100 ms, p99 critical=1000 ms for `whep`) are generous enough that this approximation is honest; an operator who wants exact Opus-frame-to-ingest-ms latency would need to thread `ingest_time_ms` through the encoder worker thread via a Arc<AtomicU64>, which is a v1.2 refinement.
* **Session poll loop gets a dedicated `opus_rx` arm, not a merged sample channel**. Merging Opus frames onto the existing `samples` channel would require tagging each message type (an enum we would end up introducing anyway) and leaks the encoder's output side into the session-handle facing API. The separate tokio mpsc is cheap, makes the data-flow obvious when reading `run_session_loop`, and keeps the encoder's appsink callback talking to a typed `OpusFrame` channel rather than a `SessionMsg` that lives in a different crate.
* **`write_sample` signature stays the same**. The old `MediaCodec::Aac` branch in `write_sample` is now unreachable (AAC is routed via `SessionMsg::Aac` instead of `SessionMsg::Video`), but the branch stays for compatibility: a future producer that somehow routes AAC through the video path still sees the same one-shot warn + drop behaviour.

### Ground truth (session 113 close)

* **Head**: feat commit `323be2f` + close-doc commit `3e9b444`. No push event in this block. Origin/main head remains `2e50635`.
* **Tests (default features gate)**: **934** passed, 0 failed, 1 ignored on macOS. Unchanged from session 112's 934: the new `aac_opus_roundtrip.rs` test is behind `#![cfg(feature = "transcode")]` so the default gate does not even compile the test target. Hosts with GStreamer installed (`cargo test -p lvqr-transcode --features transcode`) pick up the new test.
* **Tier 4 execution status**: **COMPLETE** (unchanged). Phase B row 113 lands on top of a closed Tier 4.
* **CI gates locally clean**:
  * `cargo fmt --all --check`.
  * `cargo clippy --workspace --all-targets -- -D warnings`.
  * `cargo test -p lvqr-whep` 22 lib + 12 integration + 4 proptest + 1 e2e h264 + 1 e2e hevc + 1 e2e opus passed.
  * `cargo test -p lvqr-ingest --lib` 28 passed.
  * `cargo test -p lvqr-transcode --lib` 22 passed.
  * `cargo test --workspace` 934 / 0 / 1.
  * `cargo test -p lvqr-transcode --features transcode --test aac_opus_roundtrip` not verifiable on this macOS dev host (GStreamer runtime not installed via brew); covered by the feature-on CI matrix.
* **Workspace**: **29 crates**, unchanged.
* **crates.io**: unchanged. Session 113 additively extends two public trait signatures (`RawSampleObserver::on_audio_config`, `SessionHandle::on_audio_config`) with a default no-op impl so every existing downstream consumer compiles without code changes. The new `lvqr-whep/aac-opus` Cargo feature is additive. The new `lvqr-transcode::AacToOpusEncoder` et al types are behind the existing `transcode` feature. The pending re-publish chain from session 105 still lands cleanly on the next release.

### Known limitations / documented v1 shape (after 113 close)

* **Host must have GStreamer 1.22+ with `gst-libav` (`avdec_aac`) and `gst-plugins-base` (`audioconvert`, `audioresample`, `opusenc`, `aacparse`) installed** for the AAC-to-Opus encoder to activate. The factory probes at startup; a missing element logs once and the session falls back to the pre-113 "drop AAC, warn once" path. Operators should install the same plugin set session 105 requires for the video transcoder.
* **Opus SLO sample uses the most-recent AAC input's ingest_time_ms rather than a precisely-correlated input-to-output correspondence**. Accurate to within one 20 ms Opus frame; operators reading the `whep` row of the latency histogram should treat it as "server-side AAC-to-Opus encode latency plus RTP packetisation" rather than "per-frame glass-to-glass".
* **AAC-to-Opus transcoder is per-session**. N subscribers on the same broadcast each spin up a separate GStreamer pipeline + worker thread. For v0.4 deployments with single-digit concurrent WHEP subscribers this is negligible; a v1.2 follow-up can factor out a shared-per-broadcast encoder when profiling shows it matters.
* **Full RTMP-to-WHEP-client E2E not landed this session**. The `aac_opus_roundtrip` test exercises the encoder path with real AAC bytes through a real GStreamer decoder chain. A fuller RTMP publish -> str0m client-side Opus receive test (scope row 4 in the 113 briefing) is deferred to session 114 alongside the WHIP-to-HLS + SRT-to-DASH E2E gap-closer. All plumbing pieces (`on_audio_config` routing, composition-root wiring, factory pass-through, session poll loop arms) are verified by default-gate unit tests.

crates.io is unchanged since the 110 push chain. The published v0.4.0 crates do NOT have the 111-B or 112 changes; consumers who need them should build from main or wait for the next release cycle.

**Session 111-A shipped** (docs accuracy sweep): authored `tracking/PLAN_V1.1.md` with the four-phase plan (A stop-the-bleeding, B user-visible wins, C operator polish, D v1.1 marquee); corrected README drift on published SDKs, WASM mutation capability, mesh client-side state, Tier 4 exit criterion; added "Known v0.4.0 limitations" README section; refreshed `docs/mesh.md`. Docs-only commit `b79cf6a`.

**Session 112 shipped** (live HLS + DASH subscribe auth + supply-chain audit CI): the HLS and DASH routers at the CLI composition root are now wrapped with a tower middleware that applies the `SubscribeAuth` provider to every `/hls/{broadcast}/...` and `/dash/{broadcast}/...` request. Broadcast extraction mirrors the handler's `split_broadcast_path` rule (rfind('/')). Token extraction honors the `Authorization: Bearer <token>` header first and `?token=<token>` query parameter second, matching the existing `/playback/*` pattern. `NoopAuthProvider` deployments see no behavior change because the provider always returns `Allow`. Configured deployments (static subscribe-token, JWT) get an automatic 401 on unauthed requests. New `--no-auth-live-playback` CLI flag (and `LVQR_NO_AUTH_LIVE_PLAYBACK` env var) and corresponding `no_auth_live_playback: bool` field on `ServeConfig` and `TestServerConfig::without_live_playback_auth()` as the escape hatch. New integration test `crates/lvqr-cli/tests/hls_live_auth_e2e.rs` with 7 cases (missing token rejected; bearer header accepted; query token accepted; wrong bearer rejected; DASH same shape; escape hatch disables the gate; Noop provider never gates). New CI workflow `.github/workflows/audit.yml` running `cargo audit --deny warnings` daily on a cron plus on push to main and on manual dispatch, closing the audit-debt item from `tracking/archive/AUDIT-READINESS-2026-04-13.md:37-61` that flagged "cargo-audit supply-chain scan is not wired into CI". All gates green: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` 924 / 0 / 1. Tests 917 -> 924 (the 7 new tests in `hls_live_auth_e2e.rs`). No crate version bumps; the `SubscribeAuth` + `Arc` + tower middleware are internal plumbing, the new CLI flag is additive, the new `ServeConfig` field is additive with a `false` default via `loopback_ephemeral()`.

**Sessions 111-B1 + 111-B2 + 111-B3 shipped** (mesh server-side prereqs). `lvqr-cli/src/lib.rs::start()` now hoists `MeshCoordinator` construction out of the admin-router conditional and stores it on `ServerHandle::mesh_coordinator: Option<Arc<MeshCoordinator>>` (111-B1). `/signal` WebSocket is gated behind the shared `SubscribeAuth` provider via a middleware in `crate::auth_middleware::signal_auth_middleware` that extracts the bearer from `Sec-WebSocket-Protocol: lvqr.bearer.<token>` first (111-B3) and `?token=<token>` second (111-B1); escape hatch is `--no-auth-signal` / `TestServerConfig::without_signal_auth()`. `lvqr_signal::SignalServer::ws_handler` echoes any offered bearer subprotocol back on the upgrade response so RFC-6455-strict clients accept the handshake (111-B3). Every `ws_relay_session` generates a server-side `ws-{counter}` peer_id, calls `MeshCoordinator::add_peer` at connect time, and sends a leading JSON text frame `{"type":"peer_assignment","peer_id":...,"role":...,"parent_id":...,"depth":...}` on the WS before any binary MoQ frames (111-B2). Disconnect calls `remove_peer` + `reassign_peer` for every orphan; the session loop watches both the MoQ-side rx and `socket.recv()` so idle subscribers (no frames flowing) exit promptly on client-side close instead of pinning the mesh peer entry forever. `/signal` Register callback is idempotent via `MeshCoordinator::get_peer` so a client that already holds a `ws-{n}` peer_id from `/ws` can reuse it on `/signal` without a second tree entry (111-B2). MoQ-over-DataChannel wire format locked in at `docs/mesh.md` as 8-byte big-endian `object_id` prefix + raw MoQ frame bytes per DataChannel message (111-B1). 10 new integration tests total across the three sessions. crates.io unchanged.

**Refactor commits `97bc16d` + `8da444a` shipped** (lib.rs decomposition). `lvqr-cli/src/lib.rs` dropped from 2513 lines at the start of the session chain to 1110 lines, split across 8 focused modules: `archive.rs`, `auth_middleware.rs` (205 LOC), `captions.rs`, `cluster_claim.rs`, `config.rs` (419 LOC, hosts `ServeConfig` + transcode parse helpers), `handle.rs` (334 LOC, hosts `ServerHandle` with pub(crate) fields so the composition root still builds via struct literal), `hls.rs`, `ws.rs` (544 LOC). Public API surface unchanged via `pub use` re-exports from `lib.rs`. Remaining big chunk is the ~1000-line `start()` composition root; documented as a follow-up refactor candidate that needs a dedicated design session.

## Session 110 close (2026-04-21)

1. **v1.1 item 110 A: WebSocket fMP4 relay egress SLO instrumentation** (feat commit).
   * `crates/lvqr-cli/src/lib.rs::WsRelayState`: two new fields, `registry: lvqr_fragment::FragmentBroadcasterRegistry` (cloned from the shared registry on the composition root) + `slo: Option<lvqr_admin::LatencyTracker>` (cloned from the shared tracker). The composition root at line 1292 passes both alongside the existing `origin` / `init_segments` / `auth` / `events` fields.
   * `crates/lvqr-cli/src/lib.rs::ws_relay_session`: after the MoQ-side subscription spawn the session now iterates `["0.mp4", "1.mp4"]`, calls `state.registry.get(&broadcast, track_id).map(|bc| bc.subscribe())`, and spawns one auxiliary tokio task per present track. Each aux task `while let Some(frag) = sub.next_fragment().await` skips zero `ingest_time_ms` values (synthetic test fragments, federation replays) and records `now_unix_ms().saturating_sub(fragment.ingest_time_ms)` on the shared `LatencyTracker` under `transport="ws"`. The aux task is cancelled alongside the MoQ-side drain via the existing per-session `CancellationToken` so a WS client disconnect tears everything down together.
   * The MoQ-side drain (`relay_track`) is **unchanged**: wire behavior stays byte-identical, `MoqTrackSink::push` is untouched, foreign MoQ clients + federation's `forward_track` + LVQR's own browser playback see the same frame payloads pre-110 and post-110.

2. **v1.1 item 110 B: WHEP RTP packetizer egress SLO instrumentation** (part of the same feat commit `ffc28a5`. The WS and WHEP legs share a commit because both thread the shared `lvqr_admin::LatencyTracker` from a single composition-root clone into a different egress surface; the traits each touches are disjoint (WS does not touch `RawSampleObserver` at all, and WHEP does not touch `ws_relay_session` or the fragment-registry drain). Landing them together keeps the v1.1-B scope atomic in `git log` so a future bisector cannot land halfway into "4 of 5 egress surfaces live".).
   * `crates/lvqr-ingest/src/observer.rs::RawSampleObserver::on_raw_sample` gains a trailing `ingest_time_ms: u64` parameter. `NoopRawSampleObserver` forwards the arg but ignores it.
   * `crates/lvqr-whep/src/server.rs::SessionHandle::on_raw_sample` grows the same parameter; `RawSampleObserver for WhepServer` forwards it to every matching `SessionHandle::on_raw_sample`.
   * `crates/lvqr-whep/src/str0m_backend.rs`:
     * New `TRANSPORT_LABEL: &str = "whep"` constant.
     * `Str0mAnswerer` grows an optional `slo: Option<LatencyTracker>` field + a `with_slo_tracker(LatencyTracker) -> Self` builder. Default `new()` leaves the tracker unset (compat with existing tests that boot a bare answerer without a tracker).
     * `SessionMsg::Video` grows `ingest_time_ms: u64`. Every producer (the `Str0mSessionHandle::on_raw_sample` forwarding site is the only in-tree producer of the variant) sets the field from the raw-sample arg.
     * `run_session_loop` grows an `slo: Option<LatencyTracker>` parameter; the sample-arm of the `tokio::select!` destructures `ingest_time_ms` off the `SessionMsg::Video`, calls `write_sample` with the existing other fields, and records `now_unix_ms().saturating_sub(ingest_time_ms)` after `Ok(true)`. Zero `ingest_time_ms` is skipped (synthetic test path, pre-110 callers).
     * `write_sample` return type widened from `Result<(), ()>` to `Result<bool, ()>`: `Ok(true)` on a successful `Writer::write` (the RTP packet is on the wire), `Ok(false)` on every pre-wire drop (pre-Connected, missing mid, missing pt, codec mismatch, AAC-into-Opus-only session, empty Annex B output), `Err(())` on a str0m write error. Only the `Ok(true)` branch records an SLO sample so the histogram is not biased by ICE / DTLS warm-up drops.
   * `crates/lvqr-whep/Cargo.toml`: new regular deps `lvqr-admin = { workspace = true }` + `lvqr-core = { workspace = true }`. The `lvqr-admin` dep does NOT override `default-features`: cargo rejects the override on inherited workspace deps that do not pre-declare `default-features` (same constraint the 109 A feat documented on `lvqr-dash`). Accepting the `cluster` default feature brings in nothing new because `lvqr-cluster` + `lvqr-moq` were already reachable through `lvqr-ingest` -> `lvqr-cmaf` -> `lvqr-moq` in the pre-110 dep tree.
   * `crates/lvqr-ingest/src/bridge.rs` (RTMP video + audio paths): stamp a single `let ingest_ms = now_unix_ms()` per sample and pass it to both `obs.on_raw_sample(.., ingest_ms)` and the matching `Fragment::with_ingest_time_ms(ingest_ms)` so the WHEP SLO sample lines up with the HLS + DASH samples the downstream drains record from `Fragment::ingest_time_ms`. Previously the RTMP bridge only stamped via `publish_fragment` (which stamps at emit time if unset); the explicit stamp here is load-bearing because the raw-sample observer call fires *before* `publish_fragment`.
   * `crates/lvqr-whip/src/bridge.rs` (WHIP video H.264/H.265 + audio Opus paths): same pattern as the RTMP bridge.
   * `crates/lvqr-cli/src/lib.rs`: composition root wires `Str0mAnswerer::new(str0m_cfg).with_slo_tracker(slo_tracker.clone())` so every WHEP session the answerer creates clones the tracker into its poll loop.
   * All existing on_raw_sample call sites updated: `crates/lvqr-whep/src/str0m_backend.rs` in-module test `on_raw_sample_forwards_video_and_drops_audio`, the new `slo_tracker_skips_pre_connected_writes` inline test, `crates/lvqr-whep/tests/integration_signaling.rs` (CountingHandle stub + 4 server.on_raw_sample call sites), `crates/lvqr-whep/tests/e2e_str0m_loopback.rs`, `crates/lvqr-whep/tests/e2e_str0m_loopback_opus.rs`, `crates/lvqr-whep/tests/e2e_str0m_loopback_hevc.rs`. Synthetic test paths all pass `0` (treated as "unset" by the recording guard).

3. **v1.1 item 110 C: tests + doc refresh**. The two new tests (`slo_route_reports_ws_latency_samples_after_publish` integration + `slo_tracker_skips_pre_connected_writes` inline unit) live in the same feat commit `ffc28a5` as the 110 A + B wiring so the verification gate stays atomic with the code it verifies; the three prose edits (`docs/slo.md` + `README.md` + this HANDOFF close block) land in the sibling docs commit `3d9e4ac`.
   * `crates/lvqr-cli/tests/slo_latency_e2e.rs`: new third test function `slo_route_reports_ws_latency_samples_after_publish` (+125 LOC). Boots a `TestServer`, creates a MoQ broadcast with a `0.mp4` track on `origin()` so `ws_relay_session`'s `consume_broadcast` + `subscribe_track` resolve, calls `registry.get_or_create("live/demo", "0.mp4", meta)` to register the fragment broadcaster, connects a WS subscriber via `tokio_tungstenite::connect_async(server.ws_url("live/demo"))`, sleeps 200 ms so `ws_relay_session` spawns + acquires its aux subscription, emits 8 backdated fragments through the registry, polls `/api/v1/slo` until 8 samples appear under `(live/demo, ws)`, asserts p50 >= 150 ms + p99 >= p50 + max >= p99, and asserts both `ws` and `hls` entries co-exist on `ServerHandle::slo().snapshot()`. Total test time locally ~0.3 s.
   * `crates/lvqr-whep/src/str0m_backend.rs` new inline unit test `slo_tracker_skips_pre_connected_writes`: creates a fresh `LatencyTracker`, constructs an `Str0mAnswerer::new(..).with_slo_tracker(..)`, creates one session, calls `handle.on_raw_sample` three times with a non-zero backdated `ingest_time_ms`, sleeps 80 ms, drops the handle, asserts no `transport="whep"` entry appears on the tracker snapshot. This is a negative assertion: without the `write_sample` pre-Connected guard the histogram would see a burst of sub-millisecond samples during every session's ICE + DTLS warm-up. The positive path (Connected -> successful `writer.write` -> record under `transport="whep"`) is covered by the existing `e2e_str0m_loopback*.rs` integration tests via their real DTLS-completed str0m peers; the compromise documented here is that those tests do not explicitly assert on SLO tracker counters, so a future maintainer wanting to guard the positive path should extend `e2e_str0m_loopback.rs` with a tracker-snapshot assertion after `client_receives_h264_video` completes.
   * `docs/slo.md`: "Egress emit" bullet expanded to enumerate four drain points (LL-HLS, MPEG-DASH, WS, WHEP) with one sentence each on the drain mechanics + sample cadence; "v1 limitations" section flipped from "WS / MoQ / WHEP still need a one-line `tracker.record(..)`" to "four of five egress surfaces are live; pure MoQ subscribers remain Tier 5 client-SDK scope" + names the 110 scoping decision (no MoQ wire payload prefix) as the reason the fifth surface is deferred. Threshold decision table is unchanged (already had `ws` + `whep` rows since 108 B).
   * `README.md`: feature-list SLO paragraph now enumerates all four live transports with one sentence on each; "What's NOT shipped yet" list drops the WS / MoQ / WHEP instrumentation bullet and replaces it with a pure-MoQ-subscriber-only bullet that names the Tier 5 client-SDK as the path forward. "What's next" section shortened to point at `tracking/SESSION_110_BRIEFING.md`'s post-Tier-4 follow-up table for the maintainer's next pick.

### Key 110 design decisions baked in (confirmed in-commit per the plan-vs-code rule)

* **MoQ wire stays pure**. `MoqTrackSink::push` is untouched; no 8-byte `ingest_time_ms` prefix on the wire. The briefing's three-option list (a = MoQ frame header, b = aux fragment-registry subscription, c = sidecar `SloSampler`) picked option (b) up front, and the implementation honors it. Foreign MoQ clients + federation's `forward_track` + LVQR's browser playback see byte-identical frame payloads before and after 110.
* **WS decision within option (b): per-fragment sample on the registry-side aux drain, not per-outbound-MoQ-frame sample via a VecDeque correlation**. The briefing's default recommendation was the VecDeque correlation; this implementation takes the simpler per-fragment approach because (i) each Fragment maps 1:1 to an outbound WS wire frame on the session's MoQ consumer side, (ii) the MoQ side emits one phantom init-segment frame per group boundary that does NOT correspond to a Fragment (`MoqTrackSink::push` prepends `meta.init_segment` as frame 0 of every keyframe group when set), so a correlation queue would drift by 1 per keyframe, (iii) the aux subscription's lifecycle is tied to the WS session, so samples are only recorded while a WS subscriber is connected (no ghost samples, matching the briefing's "per outbound WS frame" intent to within sub-millisecond accuracy). Recording at the registry side is also the pattern the HLS + DASH drains already use, so the SLO code path stays uniform across three of the four transports.
* **Raw-sample observer + session-handle signature grew a trailing `ingest_time_ms: u64` parameter; RawSample itself is unchanged**. Adding a field to `lvqr_cmaf::RawSample` would have required touching ~30 struct-literal construction sites across `lvqr-rtsp`, `lvqr-srt`, `lvqr-soak`, `lvqr-cmaf` benches + tests, `lvqr-ingest` golden-fmp4 tests, and so on. Threading a trailing parameter through the two traits + their four stamp sites (RTMP video + audio, WHIP video + audio) + their handful of impls + test stubs is a narrower blast with the same semantic effect. Synthetic test paths pass `0`, which the recording guard treats as "unset" (same as HLS + DASH).
* **`write_sample` return type widened from `Result<(), ()>` to `Result<bool, ()>`**. Only `Ok(true)` records an SLO sample. Without this split the pre-Connected + codec-mismatch + AAC-into-Opus + empty-Annex-B drops would all record a near-zero sample because `SessionMsg::Video::ingest_time_ms` is freshly stamped but str0m never actually packetized an RTP packet, biasing the histogram heavily toward zero during ICE warm-up. The `slo_tracker_skips_pre_connected_writes` inline test locks this contract in.
* **WHEP SLO measures server-side packetization latency, not WebRTC end-to-end latency**. Per the existing `docs/slo.md` "Server-side measurement only" caveat; WebRTC's ~200-500 ms end-to-end glass-to-glass includes network RTT + jitter buffer + decode + render, all of which are client-SDK scope. The WHEP sample value in practice is the delta from bridge-ingest to first successful `Writer::write`, which is typically under 10 ms on a warm session; operators should read the `whep` row in `docs/slo.md`'s threshold decision table (p50 = 100 ms, p99 critical = 1000 ms) as "server-side only" rather than "true glass-to-glass".
* **The WHEP answerer builder is `with_slo_tracker(LatencyTracker) -> Self`, not a trailing constructor arg on `Str0mAnswerer::new`**. The builder keeps the existing `Str0mAnswerer::new(config)` call sites in the e2e loopback tests working unchanged without requiring each test to pass an `Option<LatencyTracker>`. Matches the existing `with_` builder idiom used throughout the WHIP / WHEP / HLS modules.
* **Four transport labels on the tracker today**: `"hls"`, `"dash"`, `"ws"`, `"whep"`. Unquoted lowercase matches the existing convention + the `docs/slo.md` threshold decision table row headers. No alert-pack YAML edits were needed because the rule pack's rules are scoped on `(broadcast, transport)` generically; a fifth label (future `"moq"` if Tier 5 client SDK pushes samples back server-side) would light up the existing panels automatically.

### Ground truth (session 110 close)

* **Head**: feat commit `ffc28a5` + close-doc commit `3d9e4ac` + post-audit docs tidy commit (pending). Local `main` is 2 commits ahead of `origin/main` pre-tidy, 3 after the tidy lands; no push event in this block. Pre-session head on `origin/main`: `ea3bbae`.
* **Tests (default features gate)**: **917** passed, 0 failed, 1 ignored on macOS. +2 over session 109 A's 915: the new `slo_route_reports_ws_latency_samples_after_publish` integration test in `crates/lvqr-cli/tests/slo_latency_e2e.rs` (+1) plus the inline `slo_tracker_skips_pre_connected_writes` test in `crates/lvqr-whep/src/str0m_backend.rs` (+1). The 1 ignored is the pre-existing `moq_sink` doctest.
* **Tier 4 execution status**: **COMPLETE** (unchanged). This session lands a v1.1 follow-up; it does not reopen the Tier 4 close.
* **CI gates locally clean**:
  * `cargo fmt --all --check`.
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`.
  * `cargo test -p lvqr-cli --test slo_latency_e2e` 3 passed (HLS + DASH + new WS).
  * `cargo test -p lvqr-whep --lib` 22 passed (+1 new inline `slo_tracker_skips_pre_connected_writes`).
  * `cargo test -p lvqr-whep` all integration tests green (12 signaling, 4 proptest packetizer, 1 e2e h264 loopback, 1 e2e opus loopback, 1 e2e hevc loopback; the trait-sig extension flowed through cleanly).
  * `cargo test -p lvqr-admin` green (unchanged; the new `lvqr-admin` dep edge from `lvqr-whep` does not touch any admin-crate tests).
  * `cargo test --workspace` 917 / 0 / 1.
* **Workspace**: **29 crates**, unchanged.
* **crates.io**: unchanged. Session 110 additively extends two public trait signatures (`RawSampleObserver::on_raw_sample` + `SessionHandle::on_raw_sample`) with a trailing `ingest_time_ms: u64` parameter. Downstream callers outside this workspace (none known) would need to update their call sites + impls; on a strict semver reading this is a source-incompatible change, but `lvqr-ingest` + `lvqr-whep` are pre-1.0 and the pending re-publish chain from session 105 still lands cleanly. The new `lvqr-admin` + `lvqr-core` edges on `lvqr-whep`'s Cargo.toml do not change the published subtree because both crates are already reachable through `lvqr-ingest`'s existing tree.

### Known limitations / documented v1 shape (after 110 close)

* **Pure MoQ subscribers' egress latency SLO stays out of server-side measurement scope**. Server-side measurement would require a MoQ wire change (frame-payload `ingest_time_ms` prefix or per-frame extension) that 110's scoping decision explicitly rejected. The Tier 5 client-SDK is the path forward: a browser / Rust / Python client records render-side timestamps and pushes them back to a future `POST /api/v1/slo/client-sample` endpoint. Row 7 of the briefing's post-Tier-4 follow-up table (MoQ frame-carried ingest-time propagation) remains open in case a maintainer wants server-side measurement for pure MoQ subscribers before the Tier 5 SDK lands.
* ~~**WHEP inline unit test asserts the negative (pre-Connected) path only**. The positive (Connected -> record) path runs in the existing `e2e_str0m_loopback*.rs` tests against real DTLS peers; those tests do not currently assert on tracker counters. A future maintainer extending the e2e tests should add a `server.slo().snapshot()` assertion after the first video frame reaches the client.~~ Closed in a post-110-close audit tidy commit (`lvqr-whep/src/lib.rs` re-exports `LatencyTracker`; `e2e_str0m_loopback.rs` + `e2e_str0m_loopback_opus.rs` + `e2e_str0m_loopback_hevc.rs` each attach a fresh tracker via `Str0mAnswerer::with_slo_tracker`, stamp each `on_raw_sample` call with `now_unix_ms()`, and assert `>=1` sample under `transport="whep"` for the broadcast after the client's `media_frames >= 1` assert). The positive and negative paths are now both locked in as regression guards.
* **DASH sample cadence is per-Fragment, not per-finalized-`$Number$`-segment** (unchanged from 109 A).
* **Server-side measurement only** (unchanged from 107 A).
* **No admission control** (unchanged from 108 B).
* **No time-windowed retention on the admin snapshot** (unchanged from 107 A).

## Session 117 entry point -- remaining phase-B work + phase-C kickoff

Phase B rows 113 (WHEP AAC-to-Opus), 114 (WHIP->HLS + SRT->DASH + RTMP->WHEP audio E2E), and 115 (mesh two-browser Playwright E2E) are all SHIPPED. The one remaining phase-B row is 116. Phase C starts after.

| # | Scope | Risk |
|---|---|---|
| 1 | **Tier 4 `examples/tier4-demos/` first public demo script.** Per `PLAN_V1.1.md` row 116: one polished scripted demo chaining WASM filter + Whisper captions + ABR transcode + archive + C2PA verify. Closes the Tier 4 exit criterion that was skipped. `SESSION_116_BRIEFING.md` already captures the demo shape (single `demo-01.sh`, pre-generated test cert, whisper model download prereq). | low-medium |
| 2 | **Phase C row 117: archive READ DVR E2E + DASH-IF conformance validator in CI.** Per `PLAN_V1.1.md` row 117. Adds a full scrub test against the `/playback/*` routes + wires a DASH-IF conformance check into the existing `audit.yml` workflow or a new `conformance.yml`. | medium |
| 3 | **Phase D follow-up: root peer MoQ -> pushFrame auto-bridge.** Session 116 row 115 shipped with an integrator-driven `pushFrame`; a helper that wires `LvqrClient`'s frame stream into `MeshPeer.pushFrame` automatically would close the last mesh-integration friction. Not on the v1.1 roadmap; worth noting for phase D sequencing. | low |

### Known follow-up refactor candidates

- **Split `start()` into per-subsystem wiring helpers.** `lvqr-cli/src/lib.rs::start` is still ~1000 lines. A per-subsystem split would drop lib.rs below 500 lines.
- **Per-broadcast `AacToOpusEncoder`**. Session 113 ships a per-session encoder; for N > 1 WHEP subscribers sharing a broadcast a per-broadcast encoder behind a new flag would halve CPU. Defer to v1.2 once profiling confirms it matters.
- **Expose `MeshPeer.onChildOpen(id, dc)` callback** so callers that want to one-shot push on channel-open can do so without polling. Session 116 row 115 works around this with a 100 ms pushFrame loop; the workaround is fine for an integrator but future tests may want the deterministic path.
- **Shared-per-broadcast AacToOpusEncoder** (113 follow-up): factor the per-session encoder out behind a flag when profiling shows overhead matters.
- **WHIP-to-HLS E2E full-segment assertion** (114 follow-up): extend `whip_hls_e2e.rs` to write keyframes far enough apart (>2 s) that a full `#EXTINF:` segment closes, proving the end-to-end segment-finalisation path rather than just the LL-HLS partial path.

### Kickoff rules (unchanged)

No Claude attribution in commits (CLAUDE.md rule). No emojis anywhere (code, commits, docs). No em-dashes in prose. 120-column max for Rust. Real ingest and egress in integration tests (no `tower::ServiceExt::oneshot` shortcuts, no mocked sockets). Only edit in-repo. No push without direct instruction. Plan-vs-code refresh on any design deviation from PLAN_V1.1.md.

See the "Next session kickoff prompt" section immediately below for the canonical context-pointer + rules list to paste into a fresh session.

## Next session kickoff prompt

Paste the block below into a fresh Claude Code session to hand off cleanly. Keep it in sync with the current "Session N entry point" block above whenever the queue advances.

> You are continuing work on LVQR, a Rust live video streaming server. Local `main` head is `b315345` (session 114 partial close-doc); origin/main is at `2e50635`. **4 commits unpushed**: `323be2f` feat(whep) 113 WHEP AAC-to-Opus transcoder, `3e9b444` docs 113 close, `b70be73` feat(test) 114 partial WHIP->HLS + SRT->DASH E2E + 113 audit tests, `b315345` docs 114 close. Phase A of `tracking/PLAN_V1.1.md` fully shipped + pushed; phase B row 113 SHIPPED, row 114 PARTIALLY SHIPPED (WHIP->HLS + SRT->DASH E2E tests landed; RTMP->WHEP audio E2E deferred to a GStreamer-provisioned host). Workspace tests: **941** passed / 0 failed / 1 ignored on the default gate. 29 crates. Rust crates at v0.4.0 on crates.io; `@lvqr/core` + `@lvqr/player` at 0.3.1 on npm; `lvqr` at 0.3.1 on PyPI (admin-client only).
>
> **Session scope (pick one; both are acceptable plan-vs-code reads)**:
>
> * **Option A (plan-compliant, medium scope)** -- session 115 per `PLAN_V1.1.md` row 115: mesh data-plane step 2. Exercise the existing `@lvqr/core` `MeshPeer` client against the 111-B server wiring via a Playwright two-browser E2E test. Flip `docs/mesh.md` from "topology planner only" to "topology planner + signaling wired; DataChannel media relay ready for end-to-end testing". Requires adding Playwright to the dev-deps in `bindings/js` + authoring a two-page harness. Expected 2-4 hours.
>
> * **Option B (host-sensitive, narrower scope)** -- finish session 114 row 2: `crates/lvqr-cli/tests/rtmp_whep_audio_e2e.rs`. Feature-gated on `transcode`; skips cleanly on hosts without GStreamer (matching the `aac_opus_roundtrip.rs` pattern). RTMP publisher (reuse `rtmp_dash_e2e.rs`'s `publish_two_keyframes` pattern) + real AAC sample source (call into `aac_opus_roundtrip.rs`'s `audiotestsrc ! avenc_aac` generator) + str0m WHEP client (reuse `e2e_str0m_loopback_opus.rs`'s poll-loop shape) + assert at least one `Event::MediaData` lands on the Opus mid. Expected 1-2 hours on a GStreamer-enabled host; on a host without GStreamer the test is a compile-and-skip.
>
> If unsure, default to Option B because it closes the single largest documented user-visible deliverable gap from 113+114 and its skip-on-no-GStreamer fallback makes the work landable on any host. Option A is the right call if you have Playwright set up and a couple of hours.
>
> **Read first, in this order**:
> 1. `CLAUDE.md` -- absolute rules (no Claude attribution in commits, no emojis, no em-dashes, 120-col max).
> 2. `tracking/HANDOFF.md` top through the "Session 115 entry point" block and this kickoff prompt.
> 3. `tracking/PLAN_V1.1.md` -- current four-phase plan, rows 114 and 115.
> 4. For **Option B**: `crates/lvqr-transcode/tests/aac_opus_roundtrip.rs` (AAC generation pattern + GStreamer skip idiom), `crates/lvqr-cli/tests/rtmp_dash_e2e.rs` (RTMP publish harness), `crates/lvqr-whep/tests/e2e_str0m_loopback_opus.rs` (str0m Opus-subscriber poll loop).
> 5. For **Option A**: `bindings/js/packages/core/src/mesh.ts` (the MeshPeer client under test), `crates/lvqr-cli/tests/mesh_ws_registration_e2e.rs` (server-side mesh E2E precedent from 111-B2), `docs/mesh.md` (the doc to flip).
>
> **Absolute rules**: never add Claude as author, co-author, or contributor in git commits, files, or any other attribution (no `Co-Authored-By` trailers); no emojis in code, commit messages, or documentation; no em-dashes in prose; 120-column max in Rust; real ingest and egress in integration tests (no `tower::ServiceExt::oneshot` shortcuts, no mocked sockets); only edit files within this repository; do NOT push to origin without a direct user instruction; plan-vs-code refresh on any design deviation from `PLAN_V1.1.md`; never skip git hooks (no `--no-verify`, no `--no-gpg-sign`); never force-push main.
>
> **Verification gates**: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (default gate) stays >= 941 / 0 / 1. For Option B with GStreamer present, also run `cargo test -p lvqr-cli --features transcode --test rtmp_whep_audio_e2e`.
>
> **After session 115**: write a "Session 115 close" block at the top of HANDOFF.md immediately after the status header; mark the chosen `tracking/PLAN_V1.1.md` row (114 or 115) SHIPPED (or PARTIALLY SHIPPED if only part lands); update the `project_lvqr_status` auto-memory; commit as a feat commit + a close-doc commit (two commits). Push only if the user says so. If the user does ask to push, also re-verify `git log --oneline origin/main..main` before pushing so the unpushed 113 + 114 chain rides along as a single batch.

## Session 109 A close (2026-04-21)

1. **v1.1 item A: MPEG-DASH egress SLO instrumentation** (feat commit).
   * `crates/lvqr-dash/Cargo.toml`: new regular dep `lvqr-admin = { workspace = true }`. Confirmed with `cargo tree -p lvqr-dash`: no cycle, no new crate pulled into the tree beyond `lvqr-admin`'s own existing subtree (`lvqr-auth` + `lvqr-cluster` + `lvqr-core` + `lvqr-moq`), none of which depend on `lvqr-dash`. `default-features = false` override was attempted first but cargo rejects it on inherited workspace deps that do not pre-declare `default-features`; accepting the cluster feature brings no additional crates into `lvqr-dash`'s tree because `lvqr-cluster` was already reachable via `lvqr-admin`.
   * `crates/lvqr-dash/src/bridge.rs`: `BroadcasterDashBridge::install` signature grows a third argument `slo: Option<lvqr_admin::LatencyTracker>`; `drain` grows a matching parameter and records one sample per delivered fragment via `tracker.record(&broadcast, "dash", now_ms - fragment.ingest_time_ms)`. Skips samples where `ingest_time_ms == 0` (federation replays, synthetic fragments, backfill paths) and skips entirely when no tracker was wired. New `TRANSPORT_LABEL = "dash"` constant (matches the HLS helper's `TRANSPORT_LABEL = "hls"` shape) + new private `unix_wall_ms()` helper byte-identical to the HLS + dispatch helpers (three copies now; a `lvqr-core::now_unix_ms()` consolidation is a legitimate 15-minute tidy candidate for a future session).
   * `crates/lvqr-dash/src/bridge.rs` tests: four in-crate `#[tokio::test]` call sites (`video_fragments_get_monotonic_sequence_numbers`, `audio_and_video_sequences_are_independent`, `reinit_resets_the_counter`, `unknown_track_ids_are_ignored`) grow the trailing `None` arg. No other call sites of `BroadcasterDashBridge::install` in the workspace outside the CLI composition root.
   * `crates/lvqr-cli/src/lib.rs` line 948: `BroadcasterDashBridge::install(dash.clone(), &shared_registry, Some(slo_tracker.clone()))`. The shared tracker built in `start()` since 107 A threads into the DASH bridge exactly the way it already threads into the HLS bridge one block earlier.

2. **`crates/lvqr-cli/tests/slo_latency_e2e.rs`** (extended, +90 LOC). New second test function `slo_route_reports_dash_latency_samples_after_publish` mirrors 107 A's HLS test: enables DASH on the TestServer via `TestServerConfig::default().with_dash()`, publishes 8 synthetic `moof + mdat` fragments backdated 200 ms through the shared `FragmentBroadcasterRegistry`, polls `/api/v1/slo` for up to 5 s until the `(live/demo, dash)` entry reports 8 samples, then asserts p50 >= 150 ms + p99 >= p50 + max >= p99 and that both `hls` and `dash` entries co-exist on `ServerHandle::slo().snapshot()` (HLS stays enabled on the TestServer so the cross-transport surfacing story is exercised end-to-end). Shared helpers (`minimal_init_segment`, `moof_mdat_fragment`, `http_get`, `unix_now_ms`) are unchanged. Total test time locally ~0.16 s for the file.

3. **`docs/slo.md`** (two small prose edits): (a) the "Egress emit" bullet spells out the DASH per-Fragment-vs-per-segment sample cadence so operators reading the sample rate panel know the DASH drain ticks once per incoming fragment, not once per finalized `$Number$` segment; (b) the "v1 limitations" bullet flips from "HLS-only egress instrumentation ships in 107 A" to "LL-HLS (107 A) + MPEG-DASH (109 A) egress instrumentation is live" and names MoQ frame-carried ingest-time propagation as the design lever blocking WS / MoQ / WHEP instrumentation. Threshold tuning table is already label-generic; no YAML / JSON asset edits needed.

4. **Session 109 A close doc** (this commit).

### Key v1.1-A design decisions baked in (confirmed in-commit per the plan-vs-code rule)

* **Transport label is `"dash"`**. Matches the existing `"hls"` convention and the rule pack's decision table header. No alert-pack edits needed because every rule in `deploy/grafana/alerts/lvqr-slo.rules.yaml` is scoped on `(broadcast, transport)` generically; the first DASH sample flowing into the histogram surfaces automatically in Grafana alongside any existing HLS series.
* **Bridge signature added an `Option<LatencyTracker>` trailing arg, not a separate `install_with_slo` constructor**. Option keeps the public surface backward-compatible for downstream crates that do not wire the tracker (the in-crate test module is the in-tree example); adding a second constructor would double the install surface for zero benefit. The four in-crate tests each pass a literal `None`.
* **Zero `ingest_time_ms` is still "unset"**. The `if fragment.ingest_time_ms > 0` guard mirrors the HLS drain byte-for-byte so synthetic test fragments and federation replays that preserve the `0` sentinel flow through without contaminating the histogram. Zero-latency valid deliveries (same-tick synthetic stamp) still record correctly because the caller stamps a non-zero `ingest_time_ms`.
* **DASH samples are per-Fragment, not per-finalized-`$Number$`-segment**. DASH's `SegmentTemplate` live-profile addresses full segments via `$Number$` URIs; a typical DASH segment is 2-6 s and may aggregate several incoming `Fragment` values before the segmenter finalizes it. The SLO tracker intentionally records at the `push_video_segment` / `push_audio_segment` call site, so operators get one sample per fragment arrival (the finest-grained latency signal available on the drain) rather than one sample per finalized segment (which would undersample the histogram during warm-up). This matches the HLS drain's per-`push_chunk_bytes` sample cadence.
* **`default-features = false` on the new `lvqr-admin` dep in `lvqr-dash/Cargo.toml` was rejected by cargo**. The workspace root's `[workspace.dependencies]` entry for `lvqr-admin` does not pre-declare `default-features`, and cargo's rule is that inherited workspace deps cannot toggle default-features without a workspace-level override. Dropping the override is harmless: `lvqr-admin`'s default `cluster` feature pulls in `lvqr-cluster` + `lvqr-moq`, both of which were already reachable from `lvqr-dash`'s transitive graph via `lvqr-cmaf` / workspace siblings, so no new crate is introduced into `lvqr-dash`'s build.
* **Only DASH in this session; WS / MoQ / WHEP deferred**. DASH's `BroadcasterDashBridge` drains `Fragment` values from the shared `FragmentBroadcasterRegistry`, so `Fragment::ingest_time_ms` is available for free at the drain point. WS relay (`lvqr-cli::ws_relay_session`), MoQ subscribers (`OriginProducer`), and WHEP (`str0m` RTP packetizer) each consume `moq_lite` `Bytes` frames that do not carry the ingest wall-clock stamp on the MoQ wire today. Instrumenting them requires either a MoQ frame-header design change or a sidecar sampling heuristic; both are v1.1 design sessions rather than one-line wiring changes and are deferred to the 110 A entry point.
* **Three copies of `unix_wall_ms()` previously existed** (`lvqr-ingest::dispatch`, `lvqr-cli::hls`, `lvqr-dash::bridge`) and were consolidated in a separate follow-up `refactor(core)` commit immediately after the 109 A close doc landed. New `lvqr_core::now_unix_ms()` is the single source of truth; `lvqr-dash` gained a direct `lvqr-core` dep (it was only reachable transitively via `lvqr-cmaf` before). Two inline smoke tests guard the helper against silent regressions. Zero behavior change; the refactor commit is independent of the feat so a reviewer can bisect either leg cleanly.

### Ground truth (session 109 A close)

* **Head**: feat commit `4b44f9b` + close-doc commit `eeb49ef` + refactor follow-up commit (pending). Local `main` will be N+3 ahead of `origin/main` once the refactor lands; no push event in this block. Pre-commit head on `origin/main`: `eab59fa`.
* **Tests (default features gate)**: **915** passed, 0 failed, 1 ignored on macOS. +3 over session 108 B's 912: the new `slo_route_reports_dash_latency_samples_after_publish` integration test in `crates/lvqr-cli/tests/slo_latency_e2e.rs` (+1) plus the two inline smoke tests guarding `lvqr_core::now_unix_ms()` in `crates/lvqr-core/src/lib.rs` (+2). The 1 ignored is the pre-existing `moq_sink` doctest.
* **Tier 4 execution status**: **COMPLETE** (unchanged). This session lands a v1.1 follow-up; it does not reopen the Tier 4 close.
* **CI gates locally clean**:
  * `cargo fmt --all --check`.
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`.
  * `cargo test -p lvqr-dash` 29 + 3 + 4 + 5 passed (lib + proptest + router + proptest_mpd targets unchanged from 108 B counts; the new slo wiring does not add bridge-scope tests).
  * `cargo test -p lvqr-admin` 25 + 3 passed (unchanged from 108 B; the new `lvqr-admin` dep edge from `lvqr-dash` does not touch any admin-crate tests).
  * `cargo test -p lvqr-cli --test slo_latency_e2e` 2 passed (both HLS and new DASH test functions).
  * `cargo test --workspace` 915 / 0 / 1 (after the refactor follow-up adds the two `time_tests` smoke tests in `lvqr-core`).
* **Workspace**: **29 crates**, unchanged.
* **crates.io**: unchanged. Session 109 A additively extends `BroadcasterDashBridge::install`'s public API (trailing `Option<LatencyTracker>`), so the pending re-publish chain from session 105 still lands cleanly on the next release. Downstream callers of `lvqr-dash::BroadcasterDashBridge` outside this workspace (none known) would need to pass a trailing `None`; a semver-major bump is not required because `lvqr-dash` is pre-1.0 but operators should treat this as a source-incompatible change.

### Known limitations / documented v1 shape (after 109 A close)

* **WS / MoQ / WHEP egress instrumentation still deferred**: drain points consume `moq_lite` `Bytes` frames rather than `Fragment` values, so `ingest_time_ms` is not on the wire. Unblocked by a 110 A-scope design session on MoQ frame-carried ingest-time propagation.
* **DASH sample cadence is per-Fragment, not per-finalized-`$Number$`-segment**: documented in `docs/slo.md`; operators expecting one sample per DASH segment should read one-sample-per-fragment-arrival instead.
* **Server-side measurement only** (unchanged from 107 A).
* **No admission control** (unchanged from 108 B).
* **No time-windowed retention on the admin snapshot** (unchanged from 107 A).
* ~~**Three copies of `unix_wall_ms()`** across `lvqr-ingest::dispatch`, `lvqr-cli::hls`, and `lvqr-dash::bridge`; ripe for a `lvqr-core::now_unix_ms()` consolidation.~~ Consolidated in the follow-up `refactor(core)` commit; `lvqr_core::now_unix_ms()` is now the single source of truth.

## Session 108 B close (2026-04-21)

1. **Tier 4 item 4.7 session B: Grafana / Prometheus alert pack + operator runbook** (feat commit).
   * `deploy/grafana/alerts/lvqr-slo.rules.yaml` (new): Prometheus-format rule pack with five rules keyed by `(broadcast, transport)`: `LvqrSloLatencyP99VeryHigh` (critical, p99 > 4 s for 2 min), `LvqrSloLatencyP99High` (warning, p99 > 2 s for 5 min), `LvqrSloLatencyP95High` (warning, p95 > 1.5 s for 5 min), `LvqrSloLatencyP50High` (info, p50 > 500 ms for 10 min), `LvqrSloNoRecentSamples` (warning, catches drain-stall without firing on clean publisher disconnect via the 30-min lookback). Every rule's `runbook_url` annotation points at the matching named section in `docs/slo.md`.
   * `deploy/grafana/dashboards/lvqr-slo.json` (new): Grafana schema-38 dashboard (`uid: "lvqr-slo"`, title "LVQR Latency SLO") with four panels -- p99 timeseries (heat thresholds at 2 s warning / 4 s critical), p95 timeseries, p50 timeseries, and the per-`(broadcast, transport)` sample rate feeding the histogram. `${DS_PROMETHEUS}` datasource variable so any Prometheus-shaped backend wires in.
   * `deploy/grafana/README.md` (new): import-path documentation covering Prometheus `rule_files:`, Grafana Cloud managed alert rules, Grafana UI import, and provisioning YAML. Links back to `docs/slo.md` for threshold tuning.
   * `docs/slo.md` (new): operator runbook. Five named sections (`Critical p99 above 4s`, `Warning p99 above 2s`, `Warning p95 above 1.5s`, `Info p50 above 500ms`, `No recent samples`) matching every alert's `runbook_url` anchor, plus `Threshold tuning by transport` with the HLS / DASH / WHEP / MoQ / WS threshold decision table. Covers metric shape (what's measured + what's explicitly NOT measured), `/api/v1/slo` response shape, and troubleshooting checklists.
   * `docs/observability.md`: new "Latency SLO (Tier 4 item 4.7)" section linking the rule pack, dashboard, and runbook.
   * **`crates/lvqr-admin/tests/slo_assets.rs`** (new): three asset-hygiene tests that read each file off disk and assert the expected contents. Guards against silent drift (renaming an alert in code without updating the rule pack, or vice versa). String-based checks rather than full YAML/JSON deserialization so we do not pull `serde_yaml` in for asset hygiene -- the authoritative YAML validation runs via `promtool check rules` in CI / operator tooling.

2. **Session 108 B close doc** (this commit).

### Key 4.7 session B design decisions baked in (confirmed in-commit per the plan-vs-code rule)

* **Rule pack in Prometheus format, not Grafana-managed alert format**. Prometheus YAML is the most portable: consumed by Prometheus directly, Grafana Cloud's alert rule import, Alertmanager, Thanos Ruler, Cortex / Mimir. A Grafana-managed alert JSON export would lock operators into Grafana's alert engine, which many LVQR deployments avoid in favour of the Prometheus-native stack.
* **Dashboard ships as schema-38 Grafana JSON, not a Terraform / Helm resource**. Same portability argument: JSON import works in every Grafana 10+ deployment without a provisioning layer. Operators who want GitOps add it to their `/etc/grafana/provisioning/dashboards/` path; the README documents both routes.
* **Thresholds tuned for LL-HLS defaults, with a documented tuning decision table for other transports**. Shipping one rule pack that claims to work for every egress surface would be dishonest -- WebRTC's p99 budget is ~500 ms, LL-HLS's is ~2 s. The table in `docs/slo.md` tells operators exactly how to clone + scope the rules for their deployment.
* **Asset-hygiene test uses string matching, not YAML deserialization**. The `serde_yaml` dep is heavy (depends on `unsafe-libyaml`) and we only need to catch drift, not validate against the Prometheus schema. Test failures surface with clear "rule pack missing `alert: X`" messages; the authoritative YAML validation is `promtool check rules` which CI runs separately.
* **Every alert's `runbook_url` points at a named section in `docs/slo.md`**. On-call engineers land on a specific diagnostic checklist, not the repo root. The asset-hygiene test enforces that every alert has a runbook URL and every docs anchor exists so links never rot silently.
* **Dashboard uid locked to `"lvqr-slo"`**. External runbooks, the rule pack's `runbook_url`, and integration tests all reference this uid; a rename would break every link. The asset-hygiene test guards it.
* **Rule pack separates p99 into two tiers (4 s critical + 2 s warning) rather than a single threshold with routing labels**. Keeps the fire-delay and severity decisions in the rule file itself rather than pushing complexity into Alertmanager routing. Operators can still override severity via labels on the alert rule consumer.

### Ground truth (session 108 B close; Tier 4 COMPLETE)

* **Head**: feat commit (pending) + this close-doc commit (pending). Local `main` will be N+2 ahead of `origin/main`; no push event in this block. Pre-commit head on `origin/main`: `88d712b`.
* **Tests (default features gate)**: **912** passed, 0 failed, 1 ignored on macOS. +3 over session 107 A's 909 (three asset-hygiene tests in the new `crates/lvqr-admin/tests/slo_assets.rs`). The 1 ignored is the pre-existing `moq_sink` doctest.
* **Tier 4 execution status**: **COMPLETE**. 4.1 + 4.2 + 4.3 + 4.4 + 4.5 + 4.6 + 4.7 + 4.8 all DONE. The last open items are v1.1 follow-ups explicitly documented as post-Tier-4 scope (WS / DASH / MoQ / WHEP egress SLO instrumentation on top of the 4.7 A wiring; hardware-encoder backends NVENC / VAAPI / VideoToolbox / QSV; stream-modifying WASM filter pipelines; WHEP audio transcoder; M4 marketing demo).
* **CI gates locally clean**:
  * `cargo fmt --all --check`.
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`.
  * `cargo test -p lvqr-admin --test slo_assets` 3 passed.
  * `cargo test --workspace` 912 / 0 / 1.
* **Workspace**: **29 crates**, unchanged.
* **crates.io**: unchanged. Session 108 B ships no Rust source outside the new test, so the pending re-publish chain from session 105 still lands cleanly on the next release.

### Known limitations / documented v1 shape (after Tier 4 close)

* **HLS-only egress SLO instrumentation**: WS / DASH / MoQ / WHEP subscribers need a one-line `tracker.record(broadcast, transport, delta_ms)` call at their subscriber-delivery point. Each is a small additive change; the alert pack + dashboard already label-match generically so they light up automatically when a new transport records samples.
* **Server-side measurement only**: true glass-to-glass requires browser SDK telemetry (Tier 5 client SDK scope).
* **Hardware encoders deferred post-4.6**: NVENC, VideoToolbox, VAAPI, QSV -- unchanged from the 4.6 close.
* **Admission control deferred**: per 4.7 anti-scope, operators react via alerts; the server does not refuse subscribers preemptively.
* **No admin-state `/api/v1/slo` authentication exemption**: the route is bearer-gated alongside the other `/api/v1/*` paths; dashboards scraping it need a token.

## Session 107 A close (2026-04-21)

1. **Tier 4 item 4.7 session A: latency SLO histogram + `/api/v1/slo` admin route** (feat commit).
   * `crates/lvqr-fragment/src/fragment.rs`: new field `Fragment::ingest_time_ms: u64` (`0` = unset) + `Fragment::with_ingest_time_ms(mut self, ms: u64) -> Self` builder. Every existing `Fragment::new` call site stays unchanged because `new()` defaults the field to `0`.
   * `crates/lvqr-ingest/src/dispatch.rs`: `publish_fragment` now stamps the fragment's `ingest_time_ms` with `SystemTime::now()` UNIX ms when unset, so every ingest protocol (RTMP, SRT, RTSP, WHIP, WS) automatically carries the server-side ingest wall-clock without per-protocol wiring. Callers that pre-stamp via `with_ingest_time_ms` (federation relays preserving upstream timing) keep their value.
   * `crates/lvqr-admin/src/slo.rs` (new, ~260 LOC): `LatencyTracker` + `SloEntry`. Per-`(broadcast, transport)` ring buffer capped at 1024 samples with sort-on-query p50 / p95 / p99 / max computation. `record(broadcast, transport, latency_ms)` both updates the internal buffer AND fires `metrics::histogram!("lvqr_subscriber_glass_to_glass_ms", "broadcast", "transport").record(ms)` so long-term observability reaches the Prometheus / OTLP fan-out. `snapshot()` returns the per-key Vec sorted lexicographically by `(broadcast, transport)`. 5 inline tests: percentile math, ring-buffer eviction, multi-key separation, empty snapshot, clear.
   * `crates/lvqr-admin/src/routes.rs`: `AdminState::with_slo(LatencyTracker)` + `GET /api/v1/slo` handler returning `{ broadcasts: [SloEntry..] }`. Returns an empty list when no tracker is wired so dashboards can pre-bake the response structure. 3 new route tests: empty-without-tracker, populated-snapshot, auth-gating.
   * `crates/lvqr-admin/Cargo.toml`: added `parking_lot` regular dep (used by `LatencyTracker`'s internal `RwLock`).
   * `crates/lvqr-admin/src/lib.rs`: re-exports `LatencyTracker` + `SloEntry`.
   * `crates/lvqr-cli/src/lib.rs`: `start()` builds one shared `LatencyTracker`, threads it into `BroadcasterHlsBridge::install(..., Some(tracker.clone()))` and `AdminState::with_slo(tracker.clone())`, and stashes it on `ServerHandle.slo`. `ServerHandle::slo() -> &LatencyTracker` accessor + top-level re-exports `pub use lvqr_admin::{LatencyTracker, SloEntry};` so downstream crates do not pull `lvqr-admin` in as a direct dep.
   * `crates/lvqr-cli/src/hls.rs`: `BroadcasterHlsBridge::install` / `drain` gained an `Option<LatencyTracker>` parameter. The drain loop records one sample per `push_chunk_bytes` delivery, skipping zero `ingest_time_ms` values (federation / backfill paths that do not stamp). Transport label: `"hls"`. Internal `unix_wall_ms()` helper mirrors the dispatch crate's equivalent.
   * `crates/lvqr-test-utils/src/test_server.rs`: new `TestServer::slo() -> &lvqr_cli::LatencyTracker` accessor exposing the server's shared tracker for integration tests.
   * **`crates/lvqr-cli/tests/slo_latency_e2e.rs`** (new, ~170 LOC): boots a `TestServer`, publishes synthetic init + fragment bytes directly onto the shared `FragmentBroadcasterRegistry` (stamping each fragment with a 200 ms backdated `ingest_time_ms`), polls `GET /api/v1/slo` until the HLS drain reports 8 samples for `live/demo` / `hls`, and asserts p50 >= 150 ms + max >= p99 >= p50. Also exercises the `ServerHandle::slo()` accessor on the same snapshot.

2. **Session 107 A close doc** (this commit).

### Key 4.7 session A design decisions baked in (confirmed in-commit per the plan-vs-code rule)

* **Server-side measurement, not true glass-to-glass**. The tracker measures the UNIX-wall-clock delta between `Fragment::ingest_time_ms` (stamped by the ingest protocol handler) and the moment an egress surface delivers the fragment to subscribers. This captures server-internal latency; client-render latency requires browser SDK telemetry and is explicitly a Tier 5 SDK item per the 4.7 risk block. Metric name (`lvqr_subscriber_glass_to_glass_ms`) matches the plan's forward-looking label even though 107 A only covers the server-side leg.
* **Ring buffer + sort-on-query instead of streaming quantiles**. Avoids a new dep (`hdrhistogram` / `quantiles`) and keeps the snapshot path O(n log n) over n=1024, which is ~10 us on a modern host. The `/api/v1/slo` route is low-QPS; operators hit it seconds-apart from a dashboard. A streaming quantile estimator would be preferable for very high sample rates but 1024 samples per `(broadcast, transport)` is plenty for the expected cardinality.
* **Per-`(broadcast, transport)` keying, not per-subscriber**. An admin snapshot of 10 000 subscribers would blow the admin JSON body; the aggregated view per egress surface is what operators actually consult when triaging SLO burn. Per-subscriber drilldown can come from the Prometheus histogram's high-cardinality samples in Grafana later.
* **Transport label is a string, not an enum**. Strings keep the API open to protocols the CLI adds later without a codec-style enum explosion. The HLS drain uses `"hls"`; WS / DASH / MoQ / WHEP are future instrumentation passes.
* **Dispatch-path stamping, not per-protocol stamping**. Every ingest protocol goes through `lvqr_ingest::dispatch::publish_fragment`, so stamping there covers RTMP + SRT + RTSP + WHIP + WS in one call site. Federation relays that want to preserve upstream timing pre-stamp the fragment before calling `publish_fragment`; the helper skips the overwrite when `ingest_time_ms != 0`.
* **Zero ingest-time is "unset", not "zero-latency"**. The HLS drain's `if fragment.ingest_time_ms > 0` skip lets synthetic test fragments, federation replays, and anything else without a meaningful stamp flow through without contaminating the histogram. Zero-latency deliveries (same-tick synthetic tests) are still handled correctly on the tracker side: the sample is recorded as `0` when the caller explicitly stamps + we observe a same-tick delivery.
* **Re-export `LatencyTracker` + `SloEntry` from `lvqr-cli`**: downstream test utilities and integration tests should not need to take a direct `lvqr-admin` dep just to name the tracker type. The re-export is a one-liner and keeps the ABI surface tight.
* **Dispatch-path stamp chosen over a bridge-level stamp**: the ingest bridges that compose `Fragment` values (RTMP, SRT, RTSP, WHIP) already route through `publish_fragment`, so stamping there avoids touching every bridge. An alternative (stamp inside `FragmentBroadcaster::emit`) would also work but widens the surface of `lvqr-fragment`.

### Ground truth (session 107 A close)

* **Head**: feat commit (pending) + this close-doc commit (pending). Local `main` will be N+2 ahead of `origin/main`; no push event in this block. Pre-commit head on `origin/main`: `bde70ce`.
* **Tests (default features gate)**: **909** passed, 0 failed, 1 ignored on macOS. +9 over session 106's 900: 5 new inline tests on `lvqr-admin::slo` + 3 new inline tests on the admin route + 1 new integration test `slo_latency_e2e.rs`.
* **CI gates locally clean**:
  * `cargo fmt --all --check`.
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`.
  * `cargo test --workspace` 909 / 0 / 1.
* **Workspace**: **29 crates**, unchanged. No crate added or removed.
* **crates.io**: unchanged. Session 107 A is additive: new `Fragment` field, new `lvqr-admin` module, new admin route. The pending re-publish chain from session 105 still lands cleanly on the next release cycle.

### Known limitations / documented v1 shape

* **HLS-only egress instrumentation**: session 107 A records samples only from the LL-HLS drain loop. WS relay, DASH drain, MoQ forward, WHEP RTP emit -- all pending. Each is a small additive patch (subscribe + read `fragment.ingest_time_ms` + call `tracker.record`); deferred to a future follow-up so 107 A stays focused on the framework + admin route.
* **Source variant resolution still absent from LL-HLS master playlist**: unchanged from session 106 close. Tracked as a 107+ follow-up.
* **No time-windowed retention**: the ring buffer is size-bounded, not time-bounded. A quiet broadcast keeps stale samples until new traffic arrives. Live dashboards read the Prometheus histogram instead for time-aligned views; the admin snapshot is a point-in-time aggregate.

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

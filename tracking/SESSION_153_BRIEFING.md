# Session 153 Briefing -- Dedicated DVR scrub web component

**Date kick-off**: 2026-04-25 (locked at end of session 152; actual
implementation session 153 picks up from here).
**Predecessor**: Session 152 (SCTE-35 ad-marker passthrough v1 --
both ingest paths + both egress wire shapes; splice_info_section
preserved verbatim, no semantic interpretation; vendor `rml_rtmp`
v0.8.0 patch adding `Amf0DataReceived`). Default-gate tests at
**824** lib / 0 / 0 (workspace lib slice, post-152 tally on this
machine; full default-features matrix 1129+/0/0 across 131 binaries),
admin surface at **12 route trees**, origin/main head `ba14809`. SDK
packages 0.3.2. Workspace 0.4.1 unchanged. The post-152 README
"Next up" list ranks the **Dedicated DVR scrub web UI** at #1 (item
#3 was strikethrough by SCTE-35 v1; the live ranking shifts the
remaining items up one).

## Goal

Today operators who want HLS DVR playback in the browser point any
HLS-aware player at the relay's live HLS endpoint
(`/hls/{broadcast}/master.m3u8`) with the `--hls-dvr-window-secs`
knob setting the sliding-window depth. hls.js renders the window and
DVR seek "works" -- but the seek-bar, live-edge indicator, paused-
vs-live mode toggle, and thumbnail-strip hover preview are all left
to the integrator. The native `<video controls>` track UI is generic
and unaware of the live edge; integrators end up either accepting
the generic look or building each affordance themselves.

After this session, a new `@lvqr/dvr-player` package ships in the
existing JS monorepo (`bindings/js/packages/dvr-player/`) as a
sister to `@lvqr/player`. The drop-in `<lvqr-dvr-player>` web
component embeds hls.js, swaps the native controls for a custom
seek bar with time-axis labels, surfaces a LIVE badge that tracks
the live-edge delta in real time, exposes a "Go Live" button for
the paused-vs-live transition, renders a hover thumbnail strip
sourced client-side from `HTMLVideoElement` seek + canvas
`drawImage`, and emits typed custom events on seek / live-edge
change / error. The component versions to **0.3.2** alongside the
rest of the SDK surface; `@lvqr/player` and `@lvqr/core` stay at
0.3.2 unchanged.

## Decisions (locked at brief read-back, 2026-04-25)

The eight decisions below were locked before opening source. The
key correction at read-back was decision 1 (the kickoff cited
`/playback/{broadcast}/master.m3u8` as the source URL; that
endpoint does not exist -- the v1 component points at
`/hls/{broadcast}/master.m3u8` instead, the live HLS endpoint with
`--hls-dvr-window-secs` driving the DVR window depth). Decision 2
was deepened from "vanilla `HTMLElement`" to "structured vanilla"
after researching how production media players ship: Mux Player +
Media Chrome are vanilla at v4.x scale; Vidstack's `lit-html +
Maverick signals` stack is being publicly retired in 2026 by its
own author. The Mux primitives stack would have been the
architecturally-cleanest path but was rejected on strategic-
dependency grounds (LVQR is a streaming-infrastructure peer of
Mux). All other decisions stand as drafted.

### 1. Wire shape: REUSE the existing live HLS endpoint, NOT `/playback/*`

The session-153 kickoff prompt and the README "Next up" both phrase
the source URL as `https://relay/playback/{broadcast}/master.m3u8`.
Reading `crates/lvqr-cli/src/archive.rs` confirms the actual
`/playback/*` surface is **JSON** (a list of `PlaybackSegment`
entries from the redb segment index) plus raw segment bytes via
`/playback/file/{*rel}` -- there is no `master.m3u8` rendered under
`/playback`. The DVR-via-HLS path that "works today" is the live
HLS endpoint:

```
GET /hls/{broadcast}/master.m3u8
```

served by `MultiHlsServer` in `lvqr-hls/src/server.rs` (path prefix
constant at line 87). The variant media playlists carry
`#EXT-X-MEDIA-SEQUENCE` walking back through the sliding window
sized by `--hls-dvr-window-secs` (configured in `lvqr-cli/src/lib.rs`
around line 344, where `max_segments = hls_dvr_window_secs /
target_duration`). When the broadcast finalizes, the retained window
gets `#EXT-X-ENDLIST` and becomes a VOD playlist (lvqr-cli/src/lib.rs
line 798 + lvqr-hls/src/server.rs line 319). hls.js handles both
forms transparently.

**Locked: the v1 `<lvqr-dvr-player src=...>` attribute points at
`/hls/{broadcast}/master.m3u8`.** No new server route, no change to
the `/playback/*` surface. The kickoff's `master.m3u8` URL example
is corrected in the docs to use the live HLS path.

A future session could add a thin `GET /playback/{broadcast}.m3u8`
that renders an HLS VOD playlist from the redb segment index for
archived (post-finalize) broadcasts that have aged out of the live
sliding window. That is **explicit anti-scope for v1**; the v1
component plays whatever the relay's live HLS endpoint serves,
which today encompasses both the live edge and the configured DVR
window depth.

### 2. Component mechanism: structured-vanilla `HTMLElement` (DIY, no framework deps)

`@lvqr/player` (`bindings/js/packages/player/src/index.ts`) is a
plain `class extends HTMLElement` with `attachShadow({ mode: 'open' })`,
`observedAttributes`, `connectedCallback` / `disconnectedCallback`
lifecycle hooks, and direct `customElements.define` registration at
module load. No Lit, no Stencil, no framework glue. Session 153
mirrors this at the API surface but adopts a more **structured**
internal pattern, because the dvr-player has ~8 reactive state
values (currentTime, seekableEnd, isAtLiveEdge, isDragging,
hoverPosition, thumbnailCache, errorState, controlsMode) versus
`@lvqr/player`'s ~1 (status string).

The structured-vanilla pattern, locked:

* **Tagged template literal HTML strings** for the shadow DOM:
  ``static getTemplateHTML(attrs) { return /*html*/ `<style>...</style><div>...</div>`; }``
  -- `/*html*/` is a comment, not a library call; some editors use
  it for syntax highlighting. The string is parsed once via
  `template.innerHTML = ...` and cloned per instance.
* **Small attribute helpers** in `src/internals/attrs.ts`:
  `getBooleanAttr`, `getNumericAttr`, `getStringAttr`, `setBooleanAttr`
  -- ~20 LOC, no deps, ours.
* **Typed event dispatch helper** in `src/internals/dispatch.ts`:
  `dispatchTyped<E>(el, name, detail)` -- ~15 LOC.
* **Pure-function arithmetic** in `src/seekbar.ts` -- time-to-x,
  x-to-time, percentile labels, threshold checks, time formatting.
  Unit-tested via Vitest with no DOM.
* **`attributeChangedCallback`-driven reactivity** -- per-property
  `_updateXxx()` methods called on attribute change.

Rejected (this turn): **Lit**. Adds a 5 KB runtime dep alongside
hls.js for syntactic sugar; the Vidstack 2026 retrospective ("Web
components felt like another framework you had to fight against";
their `lit-html + Maverick signals` stack is being publicly retired
by its own author) is a real signal against framework-on-framework
at this scope. Decorators force a `tsconfig` decision that would
need to apply consistently across the SDK monorepo. Single-
maintainer projects pay for every runtime dep in CVE / version-bump
overhead.

Rejected (this turn): **composition with Mux Media Chrome /
hls-video-element** (the architecturally-cleanest path). LVQR is a
streaming-infrastructure project; depending on a peer's UI
primitives library is a strategic posture LVQR avoids.

Rejected: Stencil (violates tsc-only build), Preact + custom-element
(adds two deps, JSX needed), Atomico / Hybrids (niche; risky for a
public SDK). Shadow DOM is non-negotiable -- the seek-bar styles
must not bleed into host pages, and the host page's CSS must not
leak in and re-skin the player.

The empirical signal supporting structured vanilla: Mux Player
(`@mux/mux-player`) at v3.12 ships **vanilla** custom elements and
serves a public 4.x release of Media Chrome at billions of requests.
Their `<media-time-range>` (the seek-bar component, exact analogue
of ours) is `class extends HTMLElement` with template-literal HTML
strings and attribute reflection. Vanilla scales to this complexity
when written with discipline; LVQR adopts the *pattern* without the
*dependency*.

### 3. Player engine: hls.js (direct dep, version-pinned)

The relay's live HLS endpoint is the only DVR-capable surface today
(`/hls/{broadcast}/master.m3u8`). DASH is supported in parallel
(`/dash/{broadcast}/manifest.mpd`) but the v1 component targets HLS
only; a future `<lvqr-dvr-player engine="dash">` mode could land in
v1.2 with Shaka Player as the engine, but is out of scope here.
hls.js (~150 KB minified gzipped) handles:

* LL-HLS partial-segment loading (the relay emits `#EXT-X-PART` per
  the LL-HLS spec).
* DVR seek through the sliding window (the relay emits
  `#EXT-X-MEDIA-SEQUENCE` walking back through `--hls-dvr-window-secs`
  worth of segments).
* `#EXT-X-ENDLIST` (VOD) transition when the broadcast finalizes.
* `#EXT-X-DATERANGE` SCTE-35 markers (session 152's surface; the
  component surfaces these via `lvqr-dvr-marker` events for
  integrators who want to visualize ad breaks on the seek bar; v1.1
  scope, NOT v1).

Native HLS in Safari (iOS / macOS desktop) remains a free fallback
path -- an integrator can point a plain `<video src="https://relay/hls/.../master.m3u8">`
at the same endpoint and get DVR via the OS-native player. The
`<lvqr-dvr-player>` component itself uses hls.js on every browser
including Safari (uniform seek-bar behavior across browsers
matters more than minimizing JS payload for the Safari path).

**Direct dep, version-pinned.** Initial pin: `"hls.js": "^1.5.0"`
in `dependencies` (NOT `peerDependencies`). A "drop-in" component
that demands integrators install a 150 KB peer dep separately
defeats the drop-in promise. Bundle size is documented in the
README (~150 KB hls.js + ~10 KB component + .d.ts). Integrators
who already ship hls.js in a different bundle and want
deduplication can vendor the package or rebuild against
peerDependencies; that path is documented but not the default.

Rejected: Shaka Player (~280 KB; broader format support is
unjustified when only HLS is in scope; DVR seek behavior is more
complex and would slow v1). Rejected: native MediaSource without
hls.js (re-implementing LL-HLS partial loading + DVR window walk
is multi-session work).

### 4. Thumbnail strip: client-side via canvas, no server change

On hover over the seek bar, the component:

1. Reads the cursor's x-position relative to the seek bar width.
2. Maps that to a `currentTime` within the seekable range
   (`videoEl.seekable.start(0)` to `videoEl.seekable.end(...)`).
3. Uses an off-screen `HTMLVideoElement` with the same `src` and
   the same hls.js instance (or a lightweight second hls.js
   instance bound to the same playlist URL but with
   `lowLatencyMode: false` and a larger backBufferLength for seek
   stability) to seek to that time.
4. Once the off-screen video reaches `readyState >= HAVE_CURRENT_DATA`
   at the seeked time, draws a frame onto an `OffscreenCanvas`
   (or fallback `HTMLCanvasElement`) and renders the canvas as the
   hover thumbnail.
5. Caches the canvas bitmap keyed by rounded-second `currentTime`
   so a second hover at the same position is instant.

Quality is whatever the browser decoder yields at seek points
(typically the nearest keyframe, which on the relay's GOP cadence
of ~2 s is acceptable for a hover preview). No bandwidth cost
beyond the existing segment fetch -- hls.js already pulls the
segments as part of normal DVR seek; the off-screen video reuses
the same segment bytes via the browser's HTTP cache.

Rejected: server-side WEBVTT image-stream sprite (`#EXT-X-IMAGE-STREAM-INF`).
Requires new thumbnailing in `lvqr-record` / `lvqr-archive`; the v1
brief's anti-scope explicitly forbids new ingest-side work. Sprite
support is a v1.2 candidate and would slot in behind a
`thumbnails="server-sprite"` attribute then.

Toggle: `thumbnails="enabled"` (default) / `thumbnails="disabled"`.
Disabled mode skips the off-screen video entirely, saving the
double-decode CPU cost on weak clients.

### 5. Seek bar UX: time-axis labels + LIVE badge + Go Live button

* **Time-axis.** The seek bar renders HH:MM:SS labels at five
  percentile points (0%, 25%, 50%, 75%, 100% of the seekable
  range), computed from `videoEl.seekable.start(0)` and
  `videoEl.seekable.end(...)`. For broadcasts longer than one hour
  the labels show HH:MM:SS; for shorter the leading hour is
  suppressed (MM:SS). This beats percentage-only for archives where
  an integrator wants to point a viewer at "the 14:30 mark".
* **LIVE badge.** A red `LIVE` pill in the top-right corner that
  toggles between `LIVE` (active) and `LIVE` (greyed-out) based on
  the delta between `videoEl.currentTime` and
  `videoEl.seekable.end(...)`. Threshold: less than
  `3 * #EXT-X-TARGETDURATION` (read from hls.js's `levelLoaded`
  event) registers as "at live edge"; greater registers as
  "behind". The threshold is configurable via the
  `live-edge-threshold-secs` attribute (default 6, matching the
  relay's typical `target_duration=2`).
* **Go Live button.** A small button next to the LIVE badge that,
  when clicked, calls `videoEl.currentTime = videoEl.seekable.end(...)`
  and resumes playback. Only rendered when the LIVE badge is in the
  greyed-out state. This is the explicit toggle path -- a viewer
  who paused at minute 12 of a 60-minute window does NOT
  automatically jump to live when they hit play; only the Go Live
  button does that. Implicit "resume snaps to live" was rejected
  because operators report that's surprising: viewers have a
  contextual reason to pause, and unpausing should not lose their
  position.

Rejected: percentage-only seek bar. Rejected: tick-mark live edge
indicator on the right of the bar (less discoverable than a badge).
Rejected: implicit live-snap on resume.

### 6. API shape: attributes + custom events

**Attributes** (mirror `@lvqr/player` conventions where applicable):

* `src` (required) -- master.m3u8 URL.
* `autoplay` -- start playback automatically.
* `muted` -- start muted (required for autoplay in most browsers).
* `token` -- bearer token forwarded to `/hls/*` requests via
  `Authorization: Bearer <token>` header (set on hls.js's
  `xhrSetup`). Falls back to a `?token=...` query param if the
  integrator prefers, matching the existing
  `/playback/*` token-extraction precedence.
* `dvr-window-secs` -- override the heuristic that derives the
  DVR depth from the playlist; default reads from
  `#EXT-X-TARGETDURATION` * `#EXT-X-MEDIA-SEQUENCE` walk via
  hls.js's loaded fragment list.
* `thumbnails` -- `"enabled"` (default) | `"disabled"`.
* `live-edge-threshold-secs` -- default 6.
* `controls` -- `"custom"` (default; the component renders its own
  seek bar) | `"native"` (fall back to the browser's `<video controls>`,
  useful for accessibility testing or a mobile-only fallback).

**Custom events** (all bubble + composed: false; detail shape
matches the `lvqr-` prefix convention used by `@lvqr/player`):

* `lvqr-dvr-seek` -- detail `{ fromTime: number, toTime: number,
  isLiveEdge: boolean, source: 'user' | 'programmatic' }`. Fires
  on seek-bar drag, Go Live button, or programmatic
  `videoEl.currentTime = ...`.
* `lvqr-dvr-live-edge-changed` -- detail `{ isAtLiveEdge: boolean,
  deltaSecs: number, thresholdSecs: number }`. Fires when the
  delta crosses the threshold (debounced to once per second).
* `lvqr-dvr-error` -- detail `{ code: string, message: string,
  fatal: boolean, source: 'hls.js' | 'component' }`. Fires on
  hls.js fatal errors (rebroadcast from the `Hls.Events.ERROR`
  hook) and on component-level errors (e.g. the off-screen
  thumbnail video failing to seek).

Programmatic API (instance methods on the element):

* `play()`, `pause()`, `seek(time: number)`, `goLive()`. Direct
  pass-through to the underlying `videoEl`; `goLive()` matches the
  Go Live button's behavior.

### 7. Build + publish target: ESM-only, tsc-only, 0.3.2

* Package directory: `bindings/js/packages/dvr-player/`.
* `package.json` mirrors `@lvqr/player`'s shape: `"type": "module"`,
  `"main": "dist/index.js"`, `"types": "dist/index.d.ts"`,
  `"files": ["dist"]`, ESM `exports` map, `"build": "tsc"`,
  `"prepublishOnly": "npm run build"`. License `MIT OR Apache-2.0`
  to match `@lvqr/player`. Author + repository identical.
* `tsconfig.json` mirrors `@lvqr/player`: ES2022, strict, `lib`
  `["ES2022", "DOM"]`, `outDir: "dist"`, `rootDir: "src"`.
* Dependencies: `"hls.js": "^1.5.0"`. No
  `"@lvqr/core"` dep (this component does not use the MoQ client).
  No `"@lvqr/player"` dep (no shared code; intentional).
* devDependencies: `"typescript": "^5.0.0"` (workspace inheritance
  via the root `bindings/js/package.json`).
* Initial version `0.3.2` lockstep with the rest of the SDK. No
  workspace-version bump triggered by this package's introduction.

The new package is added to the existing `bindings/js/package.json`
workspace via the `packages/*` glob (already present); no root
`package.json` change required beyond the implicit npm workspace
discovery.

### 8. Test scope: unit + Playwright e2e against real TestServer

Per CLAUDE.md, integration tests use real network connections, not
mocks. Test plan:

* **Unit (`packages/dvr-player/src/seekbar.test.ts`):** ~5 tests
  for the seek-bar arithmetic helpers -- time-to-x mapping, x-to-
  time mapping, percentile label generation (1-hour and 30-second
  ranges), live-edge threshold check (above / below / at). No DOM,
  pure functions extracted from the component class.
* **E2E (`bindings/js/tests/e2e/dvr-player.spec.ts`):** Playwright
  against a TestServer launched with `--archive-dir`,
  `--hls-dvr-window-secs=300`, `--rtmp-port`, etc. via
  `playwright.config.ts`'s existing `webServer` block (a session-
  153 modification of the `webServer` command-line, OR a new
  Playwright project pointing at a second `webServer` profile --
  preferred: a second project to avoid breaking the row-115 mesh
  test). Test body:
  1. Push a deterministic synthetic stream into the relay (the
     existing test scaffolding uses ffmpeg via `lvqr-test-utils`).
  2. Open a Playwright page that imports `@lvqr/dvr-player` and
     mounts `<lvqr-dvr-player src="http://127.0.0.1:18088/hls/live/test/master.m3u8" autoplay muted>`.
  3. Wait for the LIVE badge to appear in active state.
  4. Drag the seek bar 30 s back; assert `currentTime` advanced
     and `lvqr-dvr-seek` fired with `isLiveEdge: false`.
  5. Click Go Live; assert `currentTime` jumped back to live and
     `lvqr-dvr-live-edge-changed` fired with `isAtLiveEdge: true`.
  6. Hover the seek bar; assert the thumbnail strip rendered (a
     non-zero-byte canvas dump).

Mocks not acceptable for the e2e tier (CLAUDE.md). The unit tier is
pure-arithmetic helpers; no mocking of hls.js or the DOM.

## Anti-scope (explicit rejections)

* **No server-side thumbnail spritesheet.** Defer to v1.2 behind
  `thumbnails="server-sprite"`. No new ingest-crate work this
  session.
* **No fork of hls.js.** Pinned dep, upstream behavior; bug
  workarounds via the public hls.js config surface only.
* **No new HLS playlist tags.** The `/hls/*` playlist surface is
  unchanged; the component consumes the existing `#EXT-X-MEDIA-SEQUENCE`,
  `#EXT-X-TARGETDURATION`, `#EXT-X-ENDLIST`, and (session-152)
  `#EXT-X-DATERANGE` tags as-is.
* **No new `/playback/*` HLS playlist endpoint.** The `/playback/*`
  JSON surface is unchanged. A future thin VOD playlist route over
  redb-archived broadcasts is a candidate v1.1 / v1.2 follow-up.
* **No PWA / offline / service-worker layer.** Out of scope.
* **No analytics callbacks.** Operator integrators attach their
  own to the existing custom events.
* **No DASH engine.** A `engine="dash"` Shaka mode is v1.2.
* **No mobile-specific layout.** The seek bar is responsive but
  not touch-optimized beyond what HTML drag events provide; mobile
  polish is v1.2.
* **No version bump beyond the new package.** `@lvqr/player` and
  `@lvqr/core` stay at 0.3.2. Workspace stays at 0.4.1. No
  CHANGELOG entry beyond the SDK package list.
* **No npm publish.** The new package builds + tests in CI; release
  comes in a separate session.
* **No SCTE-35 marker visualization on the seek bar.** Session
  152's `#EXT-X-DATERANGE` markers are visible to integrators via a
  `hls.js` event subscription on the underlying instance (the
  component exposes `getHlsInstance()` for advanced consumers); a
  built-in marker-tick rendering is v1.1.

## Execution order

1. **Author this briefing.** Step 0; this file.

2. **Read-back confirmation.** One-paragraph summary plus answers
   to the 8 decisions above. Wait for explicit user OK before
   opening source.

3. **Pre-touch reading list (after confirmation):**
   * `tracking/SESSION_152_BRIEFING.md` -- the brief shape.
   * `bindings/js/packages/player/src/index.ts` -- the existing
     web-component pattern.
   * `bindings/js/packages/player/package.json` + `tsconfig.json`
     -- the publish + build conventions.
   * `bindings/js/package.json` + `bindings/js/playwright.config.ts`
     -- the workspace + e2e harness.
   * `crates/lvqr-hls/src/server.rs` -- the live HLS endpoint
     (`MULTI_HLS_PREFIX` at line 87) and its sliding-window /
     ENDLIST behavior.
   * `crates/lvqr-cli/src/lib.rs` -- where `--hls-dvr-window-secs`
     drives `max_segments` (around line 344).
   * `bindings/js/tests/e2e/mesh/` -- the existing Playwright
     fixture conventions.

4. **Land package skeleton.** New
   `bindings/js/packages/dvr-player/` with `package.json`,
   `tsconfig.json`, `src/index.ts`, `README.md` stub. Empty
   component class extending HTMLElement; verify `npm run build`
   in workspace root produces `packages/dvr-player/dist/index.js`
   + `index.d.ts`.

5. **Land the seek-bar arithmetic + unit tests.** Pure helpers in
   `src/seekbar.ts`; `src/seekbar.test.ts` covering time-to-x,
   x-to-time, label generation, live-edge threshold.

6. **Land the component shell.** Shadow DOM template, attribute
   wiring, hls.js bootstrap, video element + custom seek bar
   skeleton. No thumbnails yet, no Go Live.

7. **Land the LIVE badge + Go Live button.** Hook the
   live-edge-changed event off hls.js's `levelLoaded` +
   `videoEl.timeupdate`; debounce to 1 Hz; render badge state.

8. **Land the thumbnail strip.** Off-screen video + canvas
   `drawImage`; cache keyed by rounded-second; toggle via
   `thumbnails="..."` attribute.

9. **Land the Playwright e2e spec.** Either modify
   `playwright.config.ts` to add a second `projects` entry with a
   second `webServer` profile (preferred, keeps row-115 mesh test
   running) OR carve out a dedicated config file. The TestServer
   args: `--archive-dir <tmp>`, `--hls-dvr-window-secs 300`,
   `--rtmp-port <free>`, `--admin-port <free>`, `--no-auth-live-playback`
   (so the test does not need to mint signed URLs). Push synthetic
   ffmpeg stream; assert all six steps from section 8 above.

10. **Land docs.**
    * `bindings/js/packages/dvr-player/README.md` -- usage,
      attributes, events, programmatic API, bundle-size note,
      Safari note (native HLS still works without the component
      for ultra-minimal embeddings).
    * `docs/dvr-scrub.md` -- operator-side embedding recipe,
      relay-side gate semantics (signed-URL HMAC vs bearer
      token), the relationship between `--hls-dvr-window-secs`
      and the seekable range the component renders.

11. **Land HANDOFF + README.**
    * README "Recently shipped" gains a session-153 entry; ranked
      "Next up" item #3 (Dedicated DVR scrub web UI) flips to
      strikethrough; the remaining items shift up one rank.
    * HANDOFF session 153 close block (Project Status lead + a
      detail block per the session-152 shape).

12. **Push + verify CI green.** Workspace tests unchanged (no Rust
    surface touched); SDK tests run fresh against the new package;
    Playwright e2e runs as a new project. Wait for explicit user
    OK before pushing.

## Risks + mitigations

* **The kickoff prompt cited `/playback/{broadcast}/master.m3u8`
  as the source URL; that endpoint does not exist.** Mitigation:
  decision 1 above locks the source URL on the live HLS endpoint
  (`/hls/{broadcast}/master.m3u8`) which IS the surface that
  "works today". This is the single largest scope question; if the
  user expected a new `/playback/{broadcast}.m3u8` server route,
  scope expands by ~2 hours of Rust + tests in `lvqr-cli` and the
  v1 component then fronts that route instead. **Read-back must
  surface this question explicitly.**

* **hls.js v1.5 API drift.** hls.js is on a fast cadence; major
  version bumps could break the component. Mitigation: pin to
  `^1.5.x` (caret minor); upgrade in a dedicated session with a
  small smoke matrix.

* **Off-screen video for thumbnails doubles decode cost.** On weak
  clients (mobile, low-end laptops) running an off-screen video
  alongside the main one can stutter. Mitigation: `thumbnails="disabled"`
  attribute; document the trade-off; consider a single-decoder
  pause-then-seek-then-resume mode in v1.1 if integrators report
  pain.

* **Safari MSE quirks.** Safari's MediaSource implementation has
  historically lagged Chrome / Firefox, particularly for HLS in
  MSE mode. Mitigation: hls.js handles the worst of these; the
  Playwright matrix is Chromium-only for v1 (matching the row-115
  mesh test); cross-browser is a v1.1 polish item.

* **Bundle-size surprise.** ~150 KB hls.js dependency is large
  relative to `@lvqr/player`'s pure-MSE footprint. Mitigation:
  document the size in the README + `docs/dvr-scrub.md`; suggest
  the `peerDependencies` workaround for integrators who already
  ship hls.js elsewhere.

* **Token forwarding to hls.js segment requests.** hls.js's
  `xhrSetup` hook lets you set headers per request, but URL-level
  query-token fallback requires modifying every URL hls.js
  resolves. Mitigation: use the `xhrSetup` Authorization header
  path by default; document that operators using `?token=...`
  query-string auth need to either switch to bearer-header tokens
  or rely on the existing playlist-URL `?token=...` value being
  preserved by hls.js's URL resolution (it is, for the playlist
  but not for segment URLs without extra wiring -- prefer header
  tokens).

* **Live-edge threshold heuristic.** `3 * #EXT-X-TARGETDURATION`
  is a defensible default but operators with non-standard segment
  cadences may need tuning. Mitigation: `live-edge-threshold-secs`
  attribute exposes the knob.

* **Playwright `webServer` collision with the row-115 mesh test.**
  The existing config launches one `lvqr serve --mesh-enabled ...`
  with fixed ports. Mitigation: add a second Playwright project
  with its own `webServer` block + non-overlapping ports
  (e.g. admin 18089, hls 0, rtmp 11936); both projects opt into
  `fullyParallel: false` so they do not race.

* **Thumbnail cache bloat.** Caching one canvas bitmap per seek-
  hover position over a 1-hour DVR window is bounded but not
  trivial. Mitigation: LRU cap at 60 entries (one per percentile
  point on a 60-segment-per-minute window is overkill); document.

## Ground truth (session 153 brief-write)

* **Head**: `ba14809` on `main` (post-152). v0.4.1 unchanged. SDK
  packages 0.3.2 unchanged.
* **bindings/js shape**: workspace at `bindings/js/`,
  `packages/core/` + `packages/player/`. Tests at
  `bindings/js/tests/e2e/mesh/` (Playwright) +
  `bindings/js/tests/sdk/` (Vitest). `playwright.config.ts` runs
  one `lvqr serve --mesh-enabled` profile with fixed admin port
  18088.
* **`@lvqr/player` shape**: vanilla HTMLElement, shadow DOM,
  `customElements.define`, ESM-only via tsc, `dist/index.js +
  index.d.ts`, no bundler. License `MIT OR Apache-2.0`. Direct
  dep on `@lvqr/core@0.3.2`.
* **Relay HLS surface**: `/hls/{broadcast}/master.m3u8` served by
  `MultiHlsServer` (`crates/lvqr-hls/src/server.rs:87`); variant
  media playlists carry `#EXT-X-MEDIA-SEQUENCE` walking back
  through `--hls-dvr-window-secs / target_duration` segments
  (`crates/lvqr-cli/src/lib.rs:344`); `#EXT-X-ENDLIST` appears
  on broadcast finalize (`crates/lvqr-cli/src/lib.rs:798`,
  `crates/lvqr-hls/src/server.rs:319`).
* **Relay /playback/* surface**: JSON segment list via
  `playback_handler` (`crates/lvqr-cli/src/archive.rs:537`); raw
  segment bytes via `file_handler`; signed-URL HMAC gate active
  per session 124 / 148. **No HLS playlist render under
  `/playback/*`.**
* **CI**: 8 GitHub Actions workflows GREEN on session 152's
  substantive head (`ba14809`).
* **Tests**: Net additions expected from this session: roughly +5
  TypeScript Vitest unit (seek-bar arithmetic), +1 Playwright
  e2e project (the dvr-player spec). Workspace Rust test count
  unchanged. SDK packages unchanged at 0.3.2 except for the new
  `@lvqr/dvr-player@0.3.2`.

## Step 0 deliverable -- this briefing

Author at `tracking/SESSION_153_BRIEFING.md`. Read sections 1
through 8 in order; the actual implementation order is in section
"Execution order". The author of session 153 should re-read
`bindings/js/packages/player/src/index.ts` first (the established
web-component pattern this session mirrors), then
`bindings/js/playwright.config.ts` (the e2e harness this session
extends), then `crates/lvqr-hls/src/server.rs` near `MULTI_HLS_PREFIX`
+ `crates/lvqr-cli/src/lib.rs` near the `max_segments` calculation
(the live-HLS DVR window surface this session consumes).
Decision 1 above -- the wire-shape correction from
`/playback/{broadcast}/master.m3u8` to `/hls/{broadcast}/master.m3u8`
-- is the gating decision that determines whether v1 ships
component-only (the locked path) or component + new server route
(the rejected expansion).

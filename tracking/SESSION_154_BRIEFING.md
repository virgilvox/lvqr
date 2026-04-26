# Session 154 Briefing -- SCTE-35 ad-break markers on the DVR seek bar

**Date kick-off**: 2026-04-25 (locked at the close of session 153;
this brief picks up immediately, same calendar day).
**Predecessor**: Session 153 (`@lvqr/dvr-player` v0.3.2 -- drop-in
HLS DVR scrub web component; structured-vanilla `HTMLElement`
wrapping `hls.js@^1.5.0`; custom seek bar; LIVE pill; Go Live button;
client-side hover thumbnails; 32 Vitest unit + 15 Playwright e2e).
Default-gate tests at **1111** lib / 0 / 0 (workspace lib slice),
admin surface at **12 route trees**, origin/main head `81107f0`. SDK
packages 0.3.2; `@lvqr/dvr-player` is also at 0.3.2. Workspace
0.4.1 unchanged. The post-153 README "Next up" #1, #2, #3 are all
strikethrough (hot config reload v3 / SCTE-35 v1 / dedicated DVR
scrub web component).

## Goal

Session 152 shipped end-to-end SCTE-35 passthrough: a publisher's
splice events flow through the relay to LL-HLS as
`#EXT-X-DATERANGE` lines (per HLS 4.4.5.1) and to DASH as
`<EventStream>`/`<Event>` children of the single Period (per
SCTE 214-1, scheme `urn:scte:scte35:2014:xml+bin`). Session 153
shipped the dvr-player web component with a custom seek bar that
draws played / buffered fills, percentile time labels, a LIVE
badge, a Go Live button, and a hover thumbnail strip. The two
features touch the same wire today (the served HLS playlist
carries DATERANGE entries) but are not joined: the dvr-player
ignores DATERANGE; integrators who want ad-break visualization
have to subscribe to `Hls.Events.LEVEL_LOADED` via
`getHlsInstance()` and roll their own overlay.

After this session, `@lvqr/dvr-player` v0.3.3 ships a built-in
SCTE-35 marker layer on the seek bar:

* For each `#EXT-X-DATERANGE` entry inside the seekable range,
  the component draws a marker on the seek bar -- a vertical tick
  for time-signal singletons (`SCTE35-CMD`), and a coloured
  break-range span for paired `SCTE35-OUT` / `SCTE35-IN` entries
  (joined by their shared `ID`).
* Hover on a marker pops a small tooltip showing the daterange's
  metadata: kind (out / in / cmd), event id, splice duration (when
  the OUT carries a duration), and start time inside the
  seekable range.
* The component emits `lvqr-dvr-markers-changed` whenever the
  parsed marker set changes (new ones added, old ones aged out)
  and `lvqr-dvr-marker-crossed` when `currentTime` crosses a
  marker (ascending or descending).
* A new `markers="visible"` (default) | `"hidden"` attribute lets
  integrators turn off the rendering without losing the events.
* A new `getMarkers()` programmatic method returns the current
  marker list for operators who want to render their own overlay
  or feed downstream into an ad pipeline.

The rendered playlist is unchanged -- this session is a pure
*consumer* of session 152's `#EXT-X-DATERANGE` wire. No new
relay route, no new HLS tag, no Rust crate touched besides
test-helper additions for the e2e suite. The package versions
to **0.3.3** (additive feature, no breaking change). `@lvqr/player`
and `@lvqr/core` stay at 0.3.2; workspace stays at 0.4.1.

The session also closes session 153's secondary deferred item --
the live-stream-driven Playwright assertions, gated on a missing
ffmpeg-push helper. Marker rendering only meaningfully tests
end-to-end if a real RTMP publish injects an `onCuePoint`
`scte35-bin64` event into the relay during the test, and a real
hls.js parses the resulting playlist. The new helper lands as a
two-tool design (see decision 6 below).

## Decisions (locked at brief read-back, 2026-04-25)

The seven decisions below are drafted; the read-back fixes them.
The largest design lever is decision 2 -- hls.js v1.5+ already
does the PDT-to-currentTime conversion via `DateRange.startTime`,
so the component does NOT re-implement that mapping.

### 1. DATERANGE source: hls.js's `LevelDetails.dateRanges`

The hls.js v1.5+ `LevelLoadedData` payload carries
`data.details.dateRanges: Record<string, DateRange | undefined>`
keyed by the DATERANGE `ID` attribute. The `DateRange` class
exposes:

```ts
class DateRange {
  attr: AttrList;          // raw attribute key/values incl. SCTE35-OUT/IN/CMD
  get id(): string;
  get class(): string;
  get startTime(): number; // computed currentTime offset (PDT-anchored)
  get startDate(): Date;   // RFC 3339 wall-clock
  get endDate(): Date | null;
  get duration(): number | null;   // seconds when DATERANGE has DURATION
  get plannedDuration(): number | null;
  get cue(): DateRangeCue;
  get isInterstitial(): boolean;
  get isValid(): boolean;
}
```

The component listens to `Hls.Events.LEVEL_LOADED` (already
hooked in `onLevelLoaded` at `bindings/js/packages/dvr-player/src/index.ts:553`)
and reads `data.details.dateRanges`. No XHR intercept, no
playlist-text reparse, no separate `frag-loaded` plumbing. The
existing handler grows to populate the marker store and
re-render.

**Locked: consume `data.details.dateRanges` from
`Hls.Events.LEVEL_LOADED`.**

The marker store is keyed by ID, so successive playlist refreshes
that re-emit the same ID overwrite cleanly. Aged-out entries
(IDs no longer in the latest dateRanges record) are evicted from
the store as part of the same handler pass.

### 2. Time mapping: trust `hls.js`'s `DateRange.startTime`

`DateRange.startTime` is hls.js's pre-computed currentTime offset
derived from the `#EXT-X-PROGRAM-DATE-TIME` anchor on the
relevant segment. This is exactly the value the seek bar wants:
the component already maps a `currentTime` to a fraction along
the seek bar via `timeToFraction(time, range)` from
`bindings/js/packages/dvr-player/src/seekbar.ts:35`. Marker
rendering reuses that helper unchanged.

The pure-function helpers added in `src/markers.ts`:

* `markerToFraction(marker, range)` -- `timeToFraction` wrapper
  that returns `null` when the marker's `startTime` is outside
  `[range.start, range.end]` (so the renderer can suppress
  off-screen markers).
* `groupOutInPairs(markers)` -- groups markers by ID; an
  `{ outMarker, inMarker, kind: 'pair' }` falls out for any ID
  that has BOTH an `OUT` and an `IN`; an
  `{ outMarker: x, kind: 'open' }` falls out for an OUT without
  a matching IN (the live ad break is in flight); an
  `{ marker: x, kind: 'singleton' }` falls out for every
  CMD-only entry.
* `classifyMarker(dr)` -- inspects `dr.attr['SCTE35-OUT']` /
  `dr.attr['SCTE35-IN']` / `dr.attr['SCTE35-CMD']` and returns
  `'out' | 'in' | 'cmd' | 'unknown'`. The `'unknown'` branch
  exists for non-SCTE-35 DATERANGE entries (e.g. a `CLASS` of
  `com.apple.hls.interstitial`); those render as a neutral
  diamond on the bar but no SCTE-35-specific tooltip text.

Edge cases the brief locks now:

* **Missing PDT.** When the playlist's segments do not carry
  `#EXT-X-PROGRAM-DATE-TIME`, hls.js's `DateRange.startTime`
  returns NaN (per the hls.js source at `src/loader/date-range.ts`).
  The component's mapping helper detects NaN and excludes the
  marker from rendering. `getMarkers()` still returns the entry;
  the integrator can decide what to do.
* **Daterange outside seekable range.** `markerToFraction`
  returns `null`; the renderer skips the marker. The store
  retains it; if the seekable range later expands to include the
  marker, it shows up on the next render pass.
* **Daterange on the live-edge segment.** Marker fraction is
  clamped at 1.0; renders at the right edge of the seek bar.
  When LIVE-pill is in the live state the played fill covers it.
* **Daterange aged out of the sliding window.** hls.js drops
  the entry from `dateRanges` on the next playlist refresh; the
  component's diff pass evicts it from the store and emits
  `lvqr-dvr-markers-changed`.
* **Daterange ID collision across publishers.** Out of scope:
  one dvr-player instance plays one broadcast. ID uniqueness is
  per-playlist per HLS spec.

### 3. Marker shape: tick for singletons, coloured span for pairs

* **Time-signal / CMD singletons** render as a 2-px-wide vertical
  tick that overlays the seek bar (sits above the played fill,
  6 px taller than the bar so it's discoverable without
  obscuring drag affordances).
* **Out-only break-in-flight** renders as a tick (ditto) plus a
  faint translucent overlay running from the OUT's startTime to
  the live edge -- the operator's "we're in an ad" hint while
  the IN has not yet been received.
* **Out + In pairs** render as a coloured span between the two
  start times, plus tick marks at each endpoint. Span colour
  is the `--lvqr-marker-color` CSS custom property, default
  `rgba(255, 200, 80, 0.45)` (warm-amber translucent so the
  played-fill colour shows through on the played-side).
* **Hover tooltip** appears above the marker on `pointermove`
  (or focus on keyboard nav), positioned with the same logic as
  the existing thumbnail preview. Body lines:
  * Kind: "Out", "In", "Cue", "Out (in-flight)" depending on
    classification.
  * ID: the DATERANGE `ID` value (truncated to 24 chars + `...`
    if longer).
  * Time: `formatTime(startTime - range.start, span)` reusing
    the existing helper.
  * Duration: `formatDuration(seconds)` -- "1:30" / "12.000s"
    style, only shown when `dr.duration` is set.
  * Class: shown when `dr.class` is present and not the default
    SCTE-35 class.

The tooltip + tick layer renders inside the existing
`.seekbar-wrap` grid cell so the seek-bar footprint does not
change. CSS additions are scoped under `.seekbar .marker`,
`.seekbar .marker-span`, `.seekbar .marker-tooltip`.

Rejected: render markers BELOW the seek bar (loses the
precision-position-on-the-bar affordance). Rejected: render
markers AS native `<input type="range">` ticks (the `<datalist>`
tick mechanism is unstyled in Chromium and unreliable
cross-browser). Rejected: a single colour for OUT and IN
endpoints (operators conflate them; the brief uses a slightly
different tick height for OUT vs IN -- 8 px vs 6 px -- so a
quick visual scan distinguishes the two without needing a
tooltip).

### 4. Pair grouping: ID-keyed pair detection

HLS spec 4.4.5.1.4: SCTE35-OUT and the matching SCTE35-IN MUST
share the same `ID`. The relay's renderer at
`crates/lvqr-hls/src/manifest.rs:679` derives the ID from the
SCTE-35 `splice_event_id` (or PTS fallback), so an OUT and its
IN that share an event_id render with the same playlist `ID`.

The component groups markers in two passes:

1. **Classification pass** -- each daterange is classified by
   `classifyMarker(dr)` into `out` / `in` / `cmd` / `unknown`.
2. **Pair pass** -- a `Map<id, { out?, in? }>` is filled; any
   entry with both fields renders as a pair span; any
   `{ out only }` renders as an in-flight overlay; any
   `{ in only }` renders as a tick (the OUT may have aged out of
   the sliding window).

Pair rendering rules:

* **Span**: `out.startTime` to `in.startTime`. If `in.startTime`
  is less than `out.startTime` (clock skew, manually-edited
  playlist), swap.
* **In-flight overlay**: `out.startTime` to `seekable.end`.
  Re-renders on every `LEVEL_LOADED` so the right edge tracks
  the live edge. When the IN finally lands, the overlay is
  replaced by the closed span on the next render.

Programmatic API exposes both per-daterange entries AND grouped
pairs (decision 5 below) so consumers can pick whichever model
fits their pipeline.

### 5. API surface: attribute + events + getMarkers()

**Attribute** (one new):

* `markers="visible"` (default) | `"hidden"`. When `"hidden"`,
  the marker layer renders nothing; events still fire so an
  integrator's external overlay still works. When toggled at
  runtime, the layer re-renders or empties on the next
  `attributeChangedCallback` pass.

**Custom events** (two new; bubble + composed: false; prefix
matches existing convention):

* `lvqr-dvr-markers-changed` -- detail
  `{ markers: DvrMarker[], pairs: DvrMarkerPair[] }`. Fires on
  `LEVEL_LOADED` whenever the diff vs the prior store is
  non-empty (added, removed, or attribute changed). Coalesced to
  at-most-once per `LEVEL_LOADED` via a microtask flush.
* `lvqr-dvr-marker-crossed` -- detail
  `{ marker: DvrMarker, direction: 'forward' | 'backward',
  currentTime: number }`. Fires on `videoEl.timeupdate` when the
  player's currentTime crosses a marker's `startTime` in either
  direction (debounced 100 ms to avoid duplicate emits during a
  scrub). Useful for an ad-pipeline integrator that wants to
  log impressions at OUT / count completions at IN. The detail
  carries the marker, not just the ID, so the consumer can
  inspect kind / duration / id without a second lookup.

`DvrMarker` shape (exported as a TS type from `./markers`):

```ts
interface DvrMarker {
  id: string;
  kind: 'out' | 'in' | 'cmd' | 'unknown';
  startTime: number;            // currentTime offset
  startDate: Date;              // RFC 3339 wall-clock
  durationSecs: number | null;  // null when DATERANGE has no DURATION
  class: string | null;         // CLASS attribute or null
  scte35Hex: string | null;     // SCTE35-OUT/IN/CMD value or null
}

interface DvrMarkerPair {
  id: string;
  out: DvrMarker | null;
  in: DvrMarker | null;
  // 'pair' = both out + in present; 'open' = out only;
  // 'in-only' = in only; 'singleton' = cmd-only entry.
  kind: 'pair' | 'open' | 'in-only' | 'singleton';
}
```

**Programmatic API** (one new method on the element):

* `getMarkers(): { markers: DvrMarker[], pairs: DvrMarkerPair[] }`.
  Returns the current store contents, sorted by `startTime`.
  Returns empty arrays when no DATERANGE has been seen. Pure
  read; no side effects.

Existing API (`play`, `pause`, `seek`, `goLive`, `getHlsInstance`)
unchanged.

### 6. Test scope: unit + Playwright e2e + new RTMP push helpers

Per CLAUDE.md, integration tests use real network connections
not mocks. Test plan:

#### Unit (Vitest, pure)

* `bindings/js/packages/dvr-player/src/markers.test.ts` -- new.
  Roughly 12 tests:
  * `classifyMarker` returns `out` / `in` / `cmd` / `unknown`
    based on which `SCTE35-*` attribute is present.
  * `markerToFraction` clamps in-range, returns `null` for
    out-of-range or NaN.
  * `groupOutInPairs` pairs OUT+IN by ID, leaves CMD as
    singleton, leaves orphan OUT as `kind: 'open'`, leaves orphan
    IN as `kind: 'in-only'`, swaps reversed pairs.
  * `dvrMarkersFromHlsDateRanges` (the consumer-side adapter)
    converts a stub `Record<string, hlsDateRange>` into our
    `DvrMarker[]`, dropping NaN startTimes.
  * Sort stability: `getMarkers()` ordering is `startTime` asc
    then `id` asc.
  * `formatDuration` (a small new helper for the tooltip body):
    `< 60s` -> `"%0.3fs"`; `>= 60s` -> `"M:SS"`;
    `>= 3600s` -> `"H:MM:SS"`.
* No DOM, no hls.js dep, no mocking of either. Pure functions
  consumed by the component class.

#### E2E (Playwright)

* `bindings/js/tests/e2e/dvr-player/markers.spec.ts` -- new.
  Two tests:
  1. **Component-side rendering against a stub playlist.** Mount
     `<lvqr-dvr-player>` against an in-page-routed playlist
     that hardcodes a `#EXT-X-DATERANGE` block (re-using the
     `page.route` pattern from `mount.spec.ts`). Wait for
     `lvqr-dvr-markers-changed`; assert the shadow DOM contains
     the expected number of `.marker` ticks at the expected
     `left: ...%` values. Exercise the `markers="hidden"`
     toggle and assert the layer empties.
  2. **End-to-end against a real RTMP publish + onCuePoint
     injection.** Use the new `scte35RtmpPush` helper (see
     below) to publish ~5 seconds of synthetic video plus one
     mid-stream `onCuePoint scte35-bin64` event into the
     dvr-player webServer profile (port 11936). Wait for the
     served `/hls/{broadcast}/master.m3u8` variant playlist to
     contain `#EXT-X-DATERANGE`. Mount the dvr-player against
     that endpoint with `autoplay muted`. Wait for
     `lvqr-dvr-markers-changed` with a non-empty markers array.
     Assert the rendered shadow DOM contains a `.marker` tick
     at a fraction matching the OUT's `startTime / span`.

#### Test helpers (TWO new)

The brief introduces two distinct publisher helpers, neither of
which existed before. Both invoked from Playwright via
`child_process.spawn`. Both real network ingest, no mocks.

**Helper A -- `bindings/js/tests/helpers/rtmp-push.ts`**

Node-side wrapper that spawns ffmpeg as a child process to push
a synthetic RTMP stream into the relay. Closes session 153's
deferred "live-stream-driven Playwright assertions" item.
Reusable for any future dvr-player or player test that wants
the playlist to actually contain segments rather than relying on
DOM stubs.

```ts
interface RtmpPushOptions {
  rtmpUrl: string;        // e.g. rtmp://127.0.0.1:11936/live/dvr-test
  durationSecs: number;   // total runtime
  videoBitrateK?: number; // default 1500
}
function rtmpPush(opts: RtmpPushOptions): { stop(): Promise<void> };
```

ffmpeg invocation (lifted from existing handoff notes):

```
ffmpeg -re -f lavfi -i 'testsrc=size=320x180:rate=30' \
       -f lavfi -i 'sine=frequency=440' \
       -c:v libx264 -preset ultrafast -tune zerolatency -g 60 \
       -c:a aac -ar 44100 \
       -t 60 -f flv $rtmpUrl
```

The helper returns a control handle so the test body can stop
the ffmpeg child once the assertions have passed (or it self-
terminates after `durationSecs`).

CI parity check: GitHub Actions runners install ffmpeg via the
existing `apt-get install ffmpeg` step (already present for the
existing scte35 e2e Rust test); locally the developer's PATH
provides `/opt/homebrew/bin/ffmpeg` (verified at brief-write
time on this box, ffmpeg 8.1).

**Helper B -- `crates/lvqr-test-utils` `[[bin]]`
`scte35-rtmp-push`**

ffmpeg cannot natively emit AMF0 `onCuePoint` Data messages on
its FLV/RTMP output. The existing relay-side decoder lives in
the patched `rml_rtmp` v0.8 fork (see session 152 commit
`c436946` and the vendored crate at `vendor/rml_rtmp/`). The
publisher helper reuses the same `rml_rtmp` API surface (now
that the workspace has a clean dep on it via `[patch.crates-io]`)
to send BOTH a few seconds of synthetic video AND one or more
`onCuePoint scte35-bin64` AMF0 Data packets at chosen offsets.

The bin lives as `[[bin]] name = "scte35-rtmp-push"` in
`crates/lvqr-test-utils/Cargo.toml`. `lvqr-test-utils` is
`publish = false` per the workspace policy (CLAUDE.md), so the
bin only ever appears in dev / test builds. Compiles as part of
`cargo build --workspace --tests` -- which CI already runs.

CLI shape:

```
scte35-rtmp-push --rtmp-url rtmp://127.0.0.1:11936/live/dvr-test \
                 --duration-secs 8 \
                 --inject-at-secs 3 \
                 --scte35-hex 0xFC301100...
```

The Playwright spec spawns the bin via `child_process.spawn`
pointing at `target/debug/scte35-rtmp-push`. The test-runner
already builds `target/debug/lvqr` before invoking Playwright;
the same `cargo build --bin scte35-rtmp-push` runs in the same
build phase.

The bin is intentionally minimal -- it produces a single H.264
keyframe + a handful of P-frames (synthetic blocks of zeros
won't decode, but the relay's HLS bridge does NOT decode video,
it just packages NAL units; so a synthetic NAL pattern that
parses as H.264 is sufficient). A small `synthetic_h264_idr()`
helper writes a valid IDR + a few non-IDR slices so the HLS
ingest path closes a segment cleanly. The published broadcast
is then visible at `/hls/{broadcast}/master.m3u8` with one
`#EXT-X-DATERANGE` line inside the variant playlist.

Mocks not acceptable for the e2e tier (CLAUDE.md). The unit
tier is pure helpers; no mocking.

### 7. Anti-scope (locked)

Hard exclusions for this session, distinct from rejected
alternatives:

* **No new server-side wire shape.** The component reads
  session 152's `#EXT-X-DATERANGE` as-is. No new HLS attribute,
  no new admin endpoint, no test-only inject route on the relay.
* **No semantic interpretation.** The component renders markers
  + emits events. It does not decide what to do with the OUT
  (replace content, blackout, freeze frame). That is the
  integrator's downstream pipeline -- the events exist so the
  integrator can hook it.
* **No DASH-side EventStream rendering.** The dvr-player is
  HLS-only per session 153. DASH consumers (Shaka, dash.js) are
  separate; rendering the DASH `<EventStream>` on a different
  player engine is candidate v1.2 work along with the deferred
  `engine="dash"` mode.
* **No IDR-aligned splice handling.** The relay passes
  splice_info_section verbatim and does not enforce IDR
  alignment with the cue-out PTS; this remains the publisher's
  responsibility per session 152.
* **No CHANGELOG entry beyond the SDK version table.** The
  workspace `CHANGELOG.md` adds one line for the
  `@lvqr/dvr-player 0.3.3` bump; no separate "session 154"
  prose section.
* **No version bump beyond `@lvqr/dvr-player`.** `@lvqr/player`
  and `@lvqr/core` stay at 0.3.2. Workspace `Cargo.toml` stays
  at 0.4.1. No Rust crate touched besides the new `[[bin]]` on
  `lvqr-test-utils`.
* **No npm publish.** Release happens in a separate session.
* **No analytics callbacks beyond the two new events.**
* **No ad-pipeline integration recipes.** The brief documents
  the events; how an integrator wires them into VAST / VMAP /
  SSAI is out of scope.

## Execution order

1. **Author this briefing.** Step 0; this file.

2. **Read-back confirmation.** One-paragraph summary plus
   answers to the seven decisions above. Wait for explicit
   user OK before opening source.

3. **Pre-touch reading list (after confirmation):**
   * `tracking/SESSION_153_BRIEFING.md` -- the brief shape this
     mirrors.
   * `bindings/js/packages/dvr-player/src/index.ts` -- the
     existing component this session extends; specifically
     `onLevelLoaded` at line 553 (where the new daterange-
     extraction path attaches) and the seek-bar render block
     under `.seekbar-wrap` in `getTemplateHTML` (where marker
     ticks land).
   * `bindings/js/packages/dvr-player/src/seekbar.ts` -- the
     existing pure-function helpers; the new daterange-to-fraction
     helper lives alongside.
   * `crates/lvqr-codec/src/scte35.rs` -- parser surface; ground
     truth on what timing fields the wire carries.
   * `crates/lvqr-hls/src/manifest.rs` -- where `#EXT-X-DATERANGE`
     is rendered into the playlist; informs the wire shape the
     component reads.
   * `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs` -- the
     existing end-to-end test that establishes the publish-and-
     render shape; the new `scte35-rtmp-push` bin mirrors its
     splice_info_section construction.
   * `docs/scte35.md` -- publisher quickstart; the test push
     reuses the AMF0 onCuePoint payload shape.
   * `bindings/js/playwright.config.ts` -- the existing
     `dvr-player` profile webServer block; no changes expected
     (the new test slots into the same project).

4. **Land `markers.ts` + unit tests.** Pure helpers in
   `bindings/js/packages/dvr-player/src/markers.ts`:
   `classifyMarker`, `markerToFraction`, `groupOutInPairs`,
   `dvrMarkersFromHlsDateRanges`, `formatDuration`. Vitest
   coverage in `markers.test.ts`. Verify `npm run build` +
   `npm run test:unit` from `bindings/js/`.

5. **Land the marker store + LEVEL_LOADED hook.** Extend
   `LvqrDvrPlayerElement` to hold `private markerStore: Map<id, DvrMarker>`,
   maintain it on `LEVEL_LOADED`, diff against the prior pass,
   emit `lvqr-dvr-markers-changed`. No rendering yet.

6. **Land marker rendering.** New shadow-DOM elements under
   `.seekbar-wrap`: `.marker-layer` div with one
   `.marker[data-id, data-kind]` child per visible marker, plus
   `.marker-span` children for paired ranges. CSS additions
   under the existing `<style>`. Style hooks:
   `--lvqr-marker-color`, `--lvqr-marker-tick-color`,
   `--lvqr-marker-tooltip-bg`. Tooltip on `pointermove` /
   `focusin` reusing the preview-positioning logic.

7. **Land `markers="hidden"` toggle.** `attributeChangedCallback`
   case; no event suppression (events still fire when hidden).

8. **Land `getMarkers()` + `lvqr-dvr-marker-crossed`.**
   Programmatic API method returning store contents. `timeupdate`
   handler with a 100 ms debounce that scans for marker
   crossings and emits per crossing.

9. **Land `rtmp-push.ts` Node helper.** Spawns ffmpeg with the
   documented invocation; returns a control handle.

10. **Land `scte35-rtmp-push` Rust bin.** New `[[bin]]` in
    `crates/lvqr-test-utils/Cargo.toml`. Reuses `rml_rtmp`
    publisher session API; reuses the existing
    `build_splice_insert_section` helper from
    `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs` (extracted
    into a small public helper module on `lvqr-test-utils` so
    the test bin and the existing e2e test share it).
    `synthetic_h264_idr` helper writes a minimal valid NAL
    sequence. Verify the bin builds via `cargo build --bin
    scte35-rtmp-push`.

11. **Land `markers.spec.ts` Playwright e2e.** Two tests as
    sketched in decision 6. The component-side rendering test
    reuses the existing `mount.spec.ts` `page.route` pattern;
    the end-to-end test invokes both helpers and waits for the
    real playlist to carry the DATERANGE.

12. **Land docs.**
    * `bindings/js/packages/dvr-player/README.md` -- new
      "SCTE-35 ad-break markers" section between "Programmatic
      API" and "Bundle size". The anti-scope's "No SCTE-35
      marker visualization" entry flips to "Shipped in v0.3.3
      -- see [`docs/dvr-scrub.md`](../../../docs/dvr-scrub.md)".
      The events table gains the two new events; the
      attributes table gains `markers`.
    * `docs/dvr-scrub.md` -- new "SCTE-35 ad-break markers"
      section. Anti-scope's existing "No SCTE-35 marker
      rendering" entry flips to "Shipped in v0.3.3".
    * `docs/scte35.md` -- new "Client-side rendering with
      `@lvqr/dvr-player`" section pointing at `docs/dvr-scrub.md`
      and showing a minimal embedding recipe.
    * `docs/sdk/javascript.md` -- the dvr-player attribute and
      event tables (around line 62 onwards) gain the new entries.
    * `bindings/js/packages/dvr-player/package.json` --
      `"version": "0.3.3"`.
    * Workspace `CHANGELOG.md` -- bump line under SDK packages
      for `@lvqr/dvr-player 0.3.3`.

13. **Land HANDOFF + README.**
    * README "Recently shipped" gains a session 154 entry
      above the session 153 entry.
    * `tracking/HANDOFF.md` session 154 close block (Project
      Status lead per existing shape).

14. **Push + verify CI green.** Workspace tests run unchanged
    (the new bin + the `lvqr-test-utils` helper module compile
    and pass `cargo clippy --workspace`); SDK tests gain the
    new Vitest suite; Playwright runs the new e2e spec under
    the existing dvr-player project. Wait for explicit user OK
    before pushing.

## Risks + mitigations

* **hls.js DATERANGE coverage gap.** hls.js v1.5+ exposes
  `LevelDetails.dateRanges`, but older v1.4 lacked the property.
  The pinned `^1.5.0` already excludes that branch, but a
  defensive `data?.details?.dateRanges ?? {}` lookup avoids any
  TypeError on a future hls.js release that renames the field.
  Mitigation: the unit-tested `dvrMarkersFromHlsDateRanges`
  adapter accepts an arbitrary record shape and falls through
  cleanly when fields are missing.

* **`DateRange.startTime` is NaN when PDT is absent.** The relay
  emits `#EXT-X-PROGRAM-DATE-TIME` on every segment via the
  existing manifest renderer (session 124), but a hand-crafted
  test playlist without PDT exercises the NaN branch. Mitigation:
  `markerToFraction` returns `null` for non-finite startTime;
  `dvrMarkersFromHlsDateRanges` drops the entry from the
  rendered list but retains the underlying `dateRange` reachable
  via `getMarkers()` for an integrator that wants raw access.

* **ffmpeg unavailable in CI.** GitHub Actions runners install
  ffmpeg via a pre-existing `apt-get install ffmpeg -y` step
  for the captions / scte35 e2e Rust tests. Mitigation: the
  Playwright spec calls `which ffmpeg` in `test.beforeAll` and
  emits `test.skip()` when absent (matches the
  `mount.spec.ts:63` skip-when-hls.js-bundle-missing pattern),
  so the e2e test does not break local runs on a stripped-down
  dev box.

* **`scte35-rtmp-push` bin not built before Playwright runs.**
  The test-runner's CI build step is
  `cargo build -p lvqr-cli` today (per session 153
  `playwright.config.ts:65` comment). Mitigation: extend the CI
  build step to `cargo build -p lvqr-cli -p lvqr-test-utils
  --bins`, AND have the Playwright spec `test.beforeAll` skip
  when `target/debug/scte35-rtmp-push` is missing -- so a
  developer running Playwright locally without the bin gets a
  clear skip rather than a confusing spawn failure.

* **Pair detection across a sliding-window boundary.** A SCTE35-
  IN may arrive on a playlist refresh AFTER the matching OUT
  has aged out of the window. Mitigation: the marker store is
  keyed by daterange ID; an entry without an OUT renders with
  `kind: 'in-only'`, and the store correctly rebuilds pairs as
  IDs come and go. The unit tests cover this branch.

* **Marker tick layout interferes with the existing thumbnail
  preview hover.** Both surfaces respond to `pointermove` on
  the seekbar. Mitigation: the marker layer sits below the
  preview overlay in the z-order (`z-index: 1` vs preview
  `z-index: 2`); marker tooltip and thumbnail preview are
  mutually exclusive (the marker tooltip suppresses the
  thumbnail render when hover is over a marker, with a 4-px
  proximity gate).

* **`rml_rtmp` publisher API surface.** The vendored fork
  exposes a server session API; using the publisher (client)
  side requires care. Mitigation: the bin uses
  `rml_rtmp::sessions::ClientSessionInner` directly and writes
  AMF0 / chunk framing manually -- the same pattern the
  upstream `rml_rtmp` examples use for a publisher. Test
  coverage: the `markers.spec.ts` end-to-end test IS the
  coverage; if the bin can publish a stream that produces a
  `#EXT-X-DATERANGE` line on the served playlist, the bin
  works.

* **Aged-out marker re-emit.** If a marker ages out of the
  window then a *later* refresh re-emits the same ID (e.g.
  re-broadcast of the same content, or operator error), the
  store treats it as a fresh entry. This is the correct
  behaviour but a `lvqr-dvr-markers-changed` event fires twice
  for a given ID over the broadcast lifetime. Documented in
  the docs, not a bug.

* **Cross-broadcast bleed.** Switching the `src` attribute to
  a different broadcast must clear the marker store; otherwise
  stale markers from broadcast A render against broadcast B's
  seek bar. Mitigation: `startPlayback` (which already runs on
  `src` change at `index.ts:373`) calls `this.markerStore.clear()`
  + emits a clear `lvqr-dvr-markers-changed` with empty arrays.

## Ground truth (session 154 brief-write)

* **Head**: `81107f0` on `main` (post-153). v0.4.1 unchanged.
  SDK packages `@lvqr/core 0.3.2`, `@lvqr/player 0.3.2`,
  `@lvqr/dvr-player 0.3.2`. After session 154:
  `@lvqr/dvr-player` -> `0.3.3`; the others unchanged.
* **bindings/js shape**: workspace at `bindings/js/`,
  `packages/{core,player,dvr-player}/`. Tests at
  `bindings/js/tests/e2e/{mesh,dvr-player}/` (Playwright) +
  `bindings/js/tests/sdk/` (Vitest) + per-package
  `src/*.test.ts` (Vitest). `playwright.config.ts` runs two
  webServer profiles on ports 18088 (mesh) and 18089
  (dvr-player); the dvr-player profile already passes
  `--no-auth-live-playback --hls-dvr-window 300 --archive-dir
  ...` so the test does not need to mint signed URLs.
* **`@lvqr/dvr-player` shape**: vanilla `HTMLElement`, shadow
  DOM, `customElements.define`, ESM-only via tsc, dependent on
  `hls.js@^1.5.0`. Public seek-bar arithmetic in
  `src/seekbar.ts`. Existing internal helpers under
  `src/internals/{attrs,dispatch}.ts`.
* **hls.js DATERANGE surface**:
  `LevelDetails.dateRanges: Record<string, DateRange | undefined>`
  per `bindings/js/node_modules/hls.js/dist/hls.d.ts:2697`. Each
  entry exposes `id`, `class`, `startTime` (currentTime offset),
  `startDate` (Date), `duration`, `attr.SCTE35-OUT/IN/CMD`. No
  separate XHR intercept needed.
* **Relay HLS DATERANGE surface**: `#EXT-X-DATERANGE` rendered
  by `crates/lvqr-hls/src/manifest.rs:290` after `#EXT-X-MAP`
  and before the first segment. Pruned in lock-step with
  segment eviction at `manifest.rs:661`. ID derives from
  splice_event_id; CLASS is `urn:scte:scte35:2014:bin`;
  DURATION is rendered when the splice_insert sets the duration
  flag.
* **Relay SCTE-35 codec**: `lvqr_codec::scte35::SpliceInfo` and
  `parse_splice_info_section` at `crates/lvqr-codec/src/scte35.rs`.
  Existing test fixtures: the
  `build_splice_insert_section(event_id, pts_90k, duration_90k)`
  helper at `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs:61`
  is the ground-truth synthetic SCTE-35 generator the new
  `scte35-rtmp-push` bin reuses.
* **CI**: 8 GitHub Actions workflows GREEN on session 153 head
  (`81107f0`).
* **Tests**: net additions expected from this session:
  roughly +12 TypeScript Vitest unit (markers helpers),
  +2 Playwright e2e (markers.spec.ts: a stub-routed render
  test + a real-publish end-to-end test), +1 Rust workspace
  bin (`scte35-rtmp-push` on `lvqr-test-utils`), +1 small
  shared helper module on `lvqr-test-utils` (the
  `splice_insert_section_bytes` extraction). Workspace Rust
  test count unchanged (the new bin is build-only). SDK
  packages: `@lvqr/dvr-player` bumps to 0.3.3; others
  unchanged at 0.3.2.

## Step 0 deliverable -- this briefing

Author at `tracking/SESSION_154_BRIEFING.md`. Read decisions 1
through 7 in order; the actual implementation order is in
"Execution order". The author of session 154 should re-read
`bindings/js/packages/dvr-player/src/index.ts` first (the
component this session extends; the `onLevelLoaded` hook at
line 553 is the entry point for the new daterange-consumption
path), then `bindings/js/packages/dvr-player/src/seekbar.ts`
(the pure-helper pattern the new `markers.ts` mirrors), then
`crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs` (the
splice_insert_section construction that the new
`scte35-rtmp-push` bin lifts).

Decision 2 -- `DateRange.startTime` already does the PDT-
to-currentTime mapping for us -- is the design lever that
keeps this session bounded. Without it, the component would
need to anchor on `#EXT-X-PROGRAM-DATE-TIME` itself and own a
small wall-clock-to-currentTime helper; with it, the consumer
just reads a number and passes it through `timeToFraction`.
The risk register flags the NaN branch (no PDT in the
playlist) but the relay always emits PDT, so the NaN branch
is a correctness guard rather than a primary path.

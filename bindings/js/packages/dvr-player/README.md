# @lvqr/dvr-player

Drop-in DVR scrub web component for the LVQR live video relay. Wraps
hls.js against the relay's live HLS endpoint (with a sliding-window
DVR depth driven by `--hls-dvr-window-secs`), replaces the native
controls with a custom seek bar carrying time-axis labels, surfaces
a LIVE indicator that tracks the live-edge delta, exposes a Go Live
button for the paused-vs-live transition, renders client-side hover
thumbnails via canvas `drawImage`, and (since v0.3.3) paints SCTE-35
ad-break markers on the seek bar from session 152's
`#EXT-X-DATERANGE` passthrough.

Sister package to [`@lvqr/player`](../player), which targets the
MoQ-Lite over WebTransport / WebSocket live path. Use `@lvqr/player`
when you want sub-second latency on the LVQR-native protocol; use
`@lvqr/dvr-player` when you want HLS DVR scrub semantics.

## Installation

```bash
npm install @lvqr/dvr-player
```

## Usage

```html
<script type="module">
  import '@lvqr/dvr-player';
</script>
<lvqr-dvr-player
  src="https://relay.example.com:8080/hls/live/cam1/master.m3u8"
  token="<bearer-token-or-omit>"
  autoplay
  muted
></lvqr-dvr-player>
```

The `src` attribute points at the relay's live HLS endpoint; the
DVR depth is whatever the relay was configured with via
`--hls-dvr-window-secs` on `lvqr serve`. There is no separate
"DVR endpoint" -- the same playlist surface carries both the live
edge and the configured back-window, and the component reads the
seekable range from the loaded playlist.

## Attributes

| Attribute | Default | Notes |
|---|---|---|
| `src` | (required) | Master playlist URL. |
| `autoplay` | absent | Start playback when the manifest is parsed. |
| `muted` | absent | Start muted. Required by browser autoplay policies. |
| `token` | absent | Bearer token forwarded as `Authorization: Bearer <token>` on every hls.js segment / playlist request via xhrSetup. Falls back to a `?token=...` query-param attached to the master URL when native HLS is in use (Safari without hls.js). |
| `live-edge-threshold-secs` | `max(6, 3 * #EXT-X-TARGETDURATION)` | Delta in seconds below which `currentTime` registers as "at live edge". |
| `thumbnails` | `enabled` | `enabled` renders client-side canvas thumbnails on hover (lazy-init second hls.js instance on first hover); `disabled` skips the off-screen video and saves the double-decode CPU cost. |
| `controls` | `custom` | `custom` renders the LVQR custom seek bar + LIVE pill. `native` hides the custom UI and falls back to the browser's `<video controls>`. |
| `markers` | `visible` | `visible` paints SCTE-35 ad-break markers on the seek bar (ticks for time-signal singletons, coloured spans for paired OUT/IN ranges, in-flight overlays for an OUT whose IN has not yet landed). `hidden` empties the layer; events still fire. |

## Custom events

All events bubble (composed: false). Detail shapes:

* `lvqr-dvr-seek` -- `{ fromTime: number, toTime: number, isLiveEdge: boolean, source: 'user' | 'programmatic' }`
* `lvqr-dvr-live-edge-changed` -- `{ isAtLiveEdge: boolean, deltaSecs: number, thresholdSecs: number }`
* `lvqr-dvr-error` -- `{ code: string, message: string, fatal: boolean, source: 'hls.js' | 'component' }`
* `lvqr-dvr-markers-changed` -- `{ markers: DvrMarker[], pairs: DvrMarkerPair[] }` (v0.3.3+). Fires on `LEVEL_LOADED` whenever the parsed `#EXT-X-DATERANGE` set diffs against the prior pass (added, removed, or attribute changed). Empty arrays on `src` change.
* `lvqr-dvr-marker-crossed` -- `{ marker: DvrMarker, direction: 'forward' | 'backward', currentTime: number }` (v0.3.3+). Fires when `currentTime` crosses a marker's `startTime` (debounced 100 ms per marker so a scrub does not double-fire).

The `lvqr-dvr-live-edge-changed` event fires only on threshold
crossings (debounced 250 ms) -- not every `timeupdate` tick.

## Programmatic API

The element exposes these instance methods:

```ts
class LvqrDvrPlayerElement extends HTMLElement {
  play(): Promise<void>;
  pause(): void;
  seek(time: number): void;        // emits lvqr-dvr-seek with source: 'programmatic'
  goLive(): void;                  // jumps to seekable.end + resumes
  getHlsInstance(): Hls | null;    // escape hatch for advanced consumers
  getMarkers(): { markers: DvrMarker[], pairs: DvrMarkerPair[] }; // v0.3.3+
}
```

`getHlsInstance()` exposes the underlying hls.js instance so callers
can subscribe to events the component does not re-emit (e.g.
`Hls.Events.AUDIO_TRACK_LOADED`, `Hls.Events.LEVEL_SWITCHED`).

`getMarkers()` returns the current SCTE-35 marker store keyed by the
playlist's DATERANGE `ID`, sorted by ascending `startTime` then
`id`. Useful for operators who want to render their own overlay
(e.g. a chapter list, an out-of-band ad-pipeline integration) or
who want to feed the parsed list downstream.

## SCTE-35 ad-break markers

When the served HLS playlist carries `#EXT-X-DATERANGE` lines (per
HLS spec section 4.4.5.1), the seek bar paints them as visual
markers:

* **Time-signal / CMD singletons** -- vertical tick at the
  marker's `startTime`.
* **Paired OUT + IN** (joined by their shared DATERANGE `ID`) --
  coloured break-range span between the two start times, plus
  ticks at each endpoint. Default fill is warm amber via
  `--lvqr-marker-color`.
* **OUT-only in-flight breaks** (an OUT whose matching IN has not
  yet arrived) -- faint translucent overlay running from the
  OUT's start time to the live edge, plus a tick at the OUT.
* **Hover tooltip** -- shows the marker kind, ID, time inside
  the seekable range, duration when set, and CLASS attribute
  when present and not the default `urn:scte:scte35:2014:bin`.

Marker rendering relies on the playlist carrying
`#EXT-X-PROGRAM-DATE-TIME`; LVQR's relay always emits PDT, so
this works out of the box. Toggle off via `markers="hidden"` if
you prefer to render your own overlay against the
`lvqr-dvr-markers-changed` event detail.

CSS hooks for theming the marker layer:

```css
lvqr-dvr-player {
  --lvqr-marker-color: rgba(255, 200, 80, 0.45);   /* paired OUT/IN span fill */
  --lvqr-marker-tick-color: #ffc850;               /* tick colour */
  --lvqr-marker-in-flight: rgba(255, 200, 80, 0.18); /* OUT-only overlay */
  --lvqr-marker-tooltip-bg: rgba(0, 0, 0, 0.85);   /* tooltip background */
}
```

See [`docs/dvr-scrub.md`](../../../docs/dvr-scrub.md#scte-35-ad-break-markers)
for an end-to-end recipe (publisher onCuePoint -> relay
DATERANGE -> dvr-player marker render).

## Bundle size

`@lvqr/dvr-player` itself is ~12 KB minified. The hls.js dependency
adds ~150 KB minified gzipped. Total drop-in cost ~165 KB gz.

If you already ship hls.js in another bundle and want
deduplication, install `hls.js` as a peer-dep alternative -- the
component's import (`import Hls from 'hls.js'`) resolves through
your bundler's module map.

## Browser compatibility

* **Chromium (Chrome, Edge, Brave, Opera)** -- hls.js does the work.
* **Firefox** -- hls.js does the work.
* **Safari** -- the component still uses hls.js when supported. On
  iOS Safari where MSE is not available, the component falls back
  to native HLS (`<video src="...">`); the custom seek bar still
  renders but the hover thumbnail strip is unavailable in that
  fallback (no programmatic seeks on a separate decoder).

## CDN drop-in

For a script-tag-only embed without a bundler, declare an importmap
that resolves `hls.js` against your CDN of choice:

```html
<script type="importmap">
  {
    "imports": {
      "hls.js": "https://cdn.jsdelivr.net/npm/hls.js@^1.5.0/dist/hls.mjs"
    }
  }
</script>
<script type="module" src="https://cdn.jsdelivr.net/npm/@lvqr/dvr-player@^0.3/dist/index.js"></script>
<lvqr-dvr-player src="..." muted autoplay></lvqr-dvr-player>
```

## Theming

Override the documented CSS custom properties to retheme the
component:

```css
lvqr-dvr-player {
  --lvqr-accent: #ff3b30;             /* LIVE pill + played track */
  --lvqr-control-bg: rgba(0,0,0,0.55);
  --lvqr-thumb-color: #fff;           /* seek-bar drag thumb */
  --lvqr-buffered-color: rgba(255,255,255,0.35);
  --lvqr-played-color: var(--lvqr-accent);
}
```

The shadow-DOM tree exposes `part="..."` attributes on the major
sub-elements (`video`, `seekbar`, `live-badge`, `go-live-button`,
`play-button`, `mute-button`, `time-display`, `labels`, `preview`,
`markers`, `marker-tooltip`, `controls`, `live-overlay`, `status`)
for `::part(...)` styling.

## Anti-scope

* **No DASH playback.** HLS only. The relay supports both, but the
  v1 component targets HLS via hls.js; an `engine="dash"` mode
  with Shaka Player is candidate v1.2 work.
* **No server-side thumbnail spritesheet.** Thumbnails are
  client-side via canvas; integrators wanting WEBVTT
  `#EXT-X-IMAGE-STREAM-INF` sprites should track v1.2.
* **SCTE-35 marker visualization shipped in v0.3.3.** See the
  "SCTE-35 ad-break markers" section above and
  [`docs/dvr-scrub.md`](../../../docs/dvr-scrub.md#scte-35-ad-break-markers).
* **No analytics callbacks.** Attach your own listeners to the
  documented custom events.

## License

MIT OR Apache-2.0 dual-licensed (matching `@lvqr/player`). The LVQR
relay itself is AGPL-3.0-or-later or commercial; the SDK packages
deliberately use a permissive dual-license so integrators can ship
the player into proprietary frontends without triggering the AGPL
distribution clause.

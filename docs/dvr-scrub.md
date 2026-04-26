# DVR scrub web UI

Operators publishing live video through LVQR can give their viewers
a DVR scrub experience -- pause, rewind, scrub through a sliding
window of the broadcast, jump back to the live edge -- by embedding
the [`@lvqr/dvr-player`](../bindings/js/packages/dvr-player) web
component on their watch page. This document covers the relay-side
configuration the component depends on and the integrator-side
embedding recipe.

## What ships in v1

* `<lvqr-dvr-player>` web component, vanilla custom element in the
  same package shape as `@lvqr/player` (ESM-only, tsc build, MIT OR
  Apache-2.0).
* Custom seek bar with HH:MM:SS percentile labels.
* LIVE pill that toggles based on the live-edge delta.
* "Go Live" button for the paused-vs-live transition.
* Client-side hover thumbnails (canvas `drawImage` against an
  off-screen hls.js instance, lazy-initialized on first hover).
* Typed custom events: `lvqr-dvr-seek`,
  `lvqr-dvr-live-edge-changed`, `lvqr-dvr-error`.

The component consumes the relay's existing live HLS endpoint
(`/hls/{broadcast}/master.m3u8`). No new server-side route was added
in v1; the DVR depth is whatever `--hls-dvr-window-secs` was set to
when `lvqr serve` was launched.

## Relay configuration

Three flags on `lvqr serve` are relevant:

* `--archive-dir <PATH>` -- enables the redb segment index that
  backs the `/playback/*` JSON surface. Strictly speaking the v1
  `<lvqr-dvr-player>` does not need this -- it consumes
  `/hls/{broadcast}/master.m3u8` only -- but production deployments
  generally want both surfaces (the JSON `/playback/*` surface
  feeds operator tooling; the HLS surface feeds end-viewer
  scrub).

* `--hls-dvr-window-secs <SECS>` -- the size of the sliding window
  the live HLS playlist exposes. 300 (five minutes) is a reasonable
  default for a typical live stream; 3600 (one hour) is normal for
  game / talk shows; 86400 (24 hours) is feasible for 24/7
  channels. Higher values mean a deeper scrub window for viewers
  but more `#EXTINF` entries in the rendered playlist (one per
  segment over the window). The component reads
  `videoEl.seekable.start(0)` and `seekable.end(...)` to determine
  the seekable range, so any walked-back-from-live window the
  playlist exposes is the window the seek bar shows.

* `--hmac-playback-secret <HEX>` (optional) -- if set, every
  `/playback/*` and `/hls/*` request is gated by an HMAC-signed URL
  pair (`?exp=<ts>&sig=<base64url>`). Operators mint these
  per-viewer via `lvqr_cli::sign_playback_url(...)` (see
  `docs/auth.md`). The `<lvqr-dvr-player>` component preserves any
  query-string parameters on the master URL through hls.js's URL
  resolution, so a signed master URL flows correctly. For
  Authorization-header-style bearer tokens, set the `token`
  attribute and the component sets `Authorization: Bearer <token>`
  on every request via hls.js's `xhrSetup` hook.

## Integrator-side embed recipe

The minimal embed:

```html
<script type="module" src="https://cdn.jsdelivr.net/npm/@lvqr/dvr-player@^0.3"></script>
<lvqr-dvr-player
  src="https://relay.example.com:8080/hls/live/cam1/master.m3u8"
  muted
  autoplay
></lvqr-dvr-player>
```

For npm-bundled deployments:

```ts
import '@lvqr/dvr-player';

const player = document.querySelector('lvqr-dvr-player');
player?.addEventListener('lvqr-dvr-seek', (e) => {
  const { fromTime, toTime, isLiveEdge, source } = (e as CustomEvent).detail;
  console.log(`seek ${fromTime} -> ${toTime}, live=${isLiveEdge}, by=${source}`);
});
player?.addEventListener('lvqr-dvr-live-edge-changed', (e) => {
  const { isAtLiveEdge, deltaSecs } = (e as CustomEvent).detail;
  console.log(`live edge: ${isAtLiveEdge ? 'at' : 'behind'} (${deltaSecs.toFixed(1)}s)`);
});
```

For signed-URL deployments (the relay was launched with
`--hmac-playback-secret`):

```ts
// On your server, when issuing the watch page:
import { sign } from '@lvqr/server-helpers'; // or roll your own HMAC
const exp = Math.floor(Date.now() / 1000) + 3600;
const sig = sign(secret, `/hls/live/cam1?exp=${exp}`);
const src = `https://relay.example.com:8080/hls/live/cam1/master.m3u8?exp=${exp}&sig=${sig}`;
```

Pass that `src` straight into the component; hls.js preserves the
query string when fetching the variant playlists and segments.

For bearer-token deployments:

```html
<lvqr-dvr-player
  src="https://relay/hls/live/cam1/master.m3u8"
  token="eyJhbGciOi..."
  muted
  autoplay
></lvqr-dvr-player>
```

The component sets `Authorization: Bearer <token>` on every hls.js
XHR via the library's `xhrSetup` hook. Bearer tokens are preferred
over `?token=...` query-string tokens for two reasons: (i) tokens
in URLs leak into access logs and shared-link copies; (ii) hls.js's
URL resolution does not always preserve arbitrary query params on
segment requests, so a query-string token can fall off mid-segment
fetch.

## Theming

Override the CSS custom properties documented in the package
README. Common overrides:

```css
lvqr-dvr-player {
  --lvqr-accent: #4f9eff;          /* corporate blue instead of red */
  --lvqr-played-color: var(--lvqr-accent);
}
lvqr-dvr-player::part(seekbar) {
  border-radius: 0;
}
```

For a complete reskin, the public `part` attributes (`video`,
`seekbar`, `live-badge`, `go-live-button`, `play-button`,
`mute-button`, `time-display`, `labels`, `preview`, `controls`,
`live-overlay`, `status`) are reachable via `::part(...)`.

## Implementation notes

The component is a vanilla `class extends HTMLElement` with shadow
DOM. Reactivity is attribute-driven via `attributeChangedCallback`
and a small set of `_updateXxx()` methods. Pure-function
arithmetic (time-to-x mapping, label generation, threshold
checks) lives in `src/seekbar.ts` and is unit-tested in Vitest
(`bindings/js/tests/sdk/dvr-player-seekbar.spec.ts`); the live
behavior runs against a real `lvqr serve` in the Playwright
project at `bindings/js/tests/e2e/dvr-player/`.

The component does not depend on any web-component-framework
runtime (no Lit, no Stencil). The only runtime dep is hls.js. This
matches the `@lvqr/player` posture and keeps the SDK monorepo
framework-free.

## Anti-scope

* **No archived-broadcast scrub.** Once a broadcast finalizes and
  the live HLS playlist gains `#EXT-X-ENDLIST`, the same component
  can scrub the retained window (the playlist becomes a VOD
  surface). After the window expires the playlist is no longer
  served; archived-broadcast scrub for older recordings is a
  candidate follow-up that would render an HLS playlist from the
  redb `/playback/*` index. Tracking item; not in v1.

* **No DASH.** The component is HLS-only; the relay's DASH egress
  serves dash.js / Shaka clients directly without LVQR-side UI.

* **No SCTE-35 marker rendering.** Session 152 shipped SCTE-35
  passthrough; rendering ad-break markers as ticks on the seek bar
  is a v1.1 candidate. Advanced consumers can subscribe via
  `getHlsInstance().on(Hls.Events.LEVEL_LOADED, ...)` to the
  underlying `#EXT-X-DATERANGE` events.

* **No server-side thumbnail spritesheets.** Hover thumbnails are
  client-side via canvas. Server-side WEBVTT image-stream sprites
  are v1.2 (would need new thumbnailing in `lvqr-record` /
  `lvqr-archive`).

* **No analytics, PWA, or service-worker layer.** Operator
  integrators attach their own listeners to the documented custom
  events.

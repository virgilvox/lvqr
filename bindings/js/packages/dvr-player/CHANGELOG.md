# `@lvqr/dvr-player` Changelog

User-visible changes to the LVQR HLS DVR scrub web component
published on npm. The head of `main` is always the source of
truth; this file summarises shipped + unreleased work between
npm releases. For session-by-session engineering notes see
[`tracking/HANDOFF.md`](../../../../tracking/HANDOFF.md).

## [0.3.3] - 2026-04-28

First publish to npm. The package was scaffolded at version
`0.3.2` in session 153 lockstep with the rest of the SDK and
bumped to `0.3.3` in session 154 with the SCTE-35 marker
delta, but neither version shipped to npm; this is the first
artefact registry consumers can install. Sister package to
[`@lvqr/player`](../player), targeting HLS DVR semantics where
`@lvqr/player` targets MoQ-Lite live playback.

### Added

* **`<lvqr-dvr-player>` web component** (session 153). Vanilla
  `class extends HTMLElement` wrapping `hls.js ^1.5.0` against
  the relay's live HLS endpoint with the sliding-window DVR
  depth driven by `--hls-dvr-window-secs`. Custom seek bar with
  HH:MM:SS percentile labels at 0/25/50/75/100% of the seekable
  range, LIVE pill that toggles when `seekable.end -
  currentTime` crosses `max(6, 3 * #EXT-X-TARGETDURATION)`
  (configurable via `live-edge-threshold-secs`), explicit Go
  Live button that only renders when behind the live edge,
  client-side hover thumbnails via canvas `drawImage` against
  a lazy second hls.js instance (LRU-capped at 60 entries;
  opt-out via `thumbnails="disabled"`), bearer-token auth via
  `xhrSetup` (`Authorization: Bearer`) plus query-string
  fallback for native HLS in Safari MSE-less mode. Public
  events `lvqr-dvr-seek` / `lvqr-dvr-live-edge-changed` /
  `lvqr-dvr-error`; programmatic API `play()` / `pause()` /
  `seek(time)` / `goLive()` / `getHlsInstance()`. Themable via
  CSS custom properties (`--lvqr-accent`, `--lvqr-control-bg`,
  etc.) and `::part(...)` access on the shadow-DOM tree.

* **SCTE-35 ad-break markers on the seek bar** (session 154).
  Reads `#EXT-X-DATERANGE` entries from `hls.js`'s
  `LevelDetails.dateRanges` (v1.5+) on `Hls.Events.LEVEL_LOADED`.
  Vertical ticks for CMD / time-signal singletons, coloured
  break-range spans for paired SCTE35-OUT + SCTE35-IN entries
  joined by their shared DATERANGE `ID`, faint in-flight
  overlays for an OUT whose IN has not yet landed. Hover
  tooltip shows kind / id / time / duration. New
  `markers="visible|hidden"` attribute toggles the visual layer;
  events still fire when hidden. Two new public events:
  `lvqr-dvr-markers-changed` (fires on diff vs prior LEVEL_LOADED)
  and `lvqr-dvr-marker-crossed` (debounced 100 ms per id). New
  programmatic `getMarkers()` returns
  `{ markers, pairs }`. CSS hooks: `--lvqr-marker-color`,
  `--lvqr-marker-tick-color`, `--lvqr-marker-in-flight`,
  `--lvqr-marker-tooltip-bg`. New shadow parts: `markers`,
  `marker-tooltip`.

* **Client-side glass-to-glass SLO sampler** (session 156
  follow-up). Three opt-in attributes
  (`slo-sampling="enabled"`, `slo-endpoint="<URL>"`,
  `slo-sample-interval-secs`) drive a sampler timer that reads
  the playlist's PDT anchor via the standard
  `HTMLMediaElement.getStartDate()` HLS extension, computes
  `latency_ms = Date.now() - (startDate + currentTime * 1000)`,
  and POSTs `{broadcast, transport: "hls", ingest_ts_ms,
  render_ts_ms}` to the new `POST /api/v1/slo/client-sample`
  admin endpoint with the existing `token` attribute as bearer
  (rides the dual-auth path -- admin or per-broadcast subscribe
  token). Best-effort: any failure is silently dropped so SLO
  push cannot disrupt playback. Pure helpers in
  `src/slo-sampler.ts` (`computeLatencyMs`,
  `broadcastFromHlsSrc`, `pushSample`) covered by 16 Vitest
  unit tests.

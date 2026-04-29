# `@lvqr/admin-ui` Changelog

User-visible changes to the LVQR operator admin console published on
npm. The head of `main` is always the source of truth; this file
summarises shipped + unreleased work between npm releases. For
session-by-session engineering notes see
[`tracking/HANDOFF.md`](../../../../tracking/HANDOFF.md).

## [1.0.0] - 2026-04-28

First release. Ships the full operator console alongside the v1.0.0
release wave (Rust workspace + `@lvqr/core` + `@lvqr/dvr-player` +
`@lvqr/player` + Python `lvqr` all at 1.0.0). Built from
`mockups/admin_ui.html` + `mockups/tallyboard-storybook.html` design
contracts; wired against the v0.4.2 `/api/v1/*` server surface via
`@lvqr/core 1.0.0`'s `LvqrAdminClient`.

### Added

* **Vue 3 + Vite + TypeScript + Pinia + Vue Router stack.** Static
  SPA; deployable behind any static host (nginx, Caddy, Digital
  Ocean App Platform, GitHub Pages). No SSR, no backend.

* **Tallyboard design system** (`src/styles/tokens.css`). Every CSS
  custom property from `mockups/tallyboard-storybook.html` lifted
  verbatim. Operator-side theming = swap a single tokens file. Color
  ramps: bone / paper / chalk surfaces, ink scale, tally amber
  primary, wire cyan secondary, plus on-air / ready / warn / idle
  status colors. Typography: Instrument Serif (display), Figtree
  (body), JetBrains Mono (mono). 8-step spacing scale.

* **App shell** -- topbar + left rail + main + status bar grid
  layout (CSS grid, drawer rail at `<lg` breakpoints), search
  shortcut, on-air pill, cluster status indicator, version pill.

* **19 routes** mapped to LVQR's actual `/api/v1/*` surface:
  Dashboard, Streams, StreamDetail, Recordings, DVR, Ingest,
  Filters, FilterDetail, Transcode, Agents, Egress, Cluster, Mesh,
  Federation, Auth, Provenance, Observability, Logs, Settings. Pages
  backed by real routes wire fully; pages naming features the
  server does not expose (recordings list, transcode-edit,
  agent-edit, log tail, full config GET/PUT, WASM chain edit) render
  a placeholder with a v1.x backlog HTML comment + a "configure via
  `lvqr serve` flag X" hint. Adapt to the LVQR surface; never invent
  server routes.

* **Multi-relay connection profiles.** Connection picker in the
  topbar lets operators register N profiles (name + base URL +
  optional bearer token). Active profile drives every API call.
  Profiles persist in localStorage; a "this is per-browser local
  storage" warning surfaces when adding the first token.

* **Polling cadence.** Stats 5 s, streams 10 s, SLO 30 s, mesh 10 s,
  cluster 15 s, streamkeys 30 s, configReload on demand. Every
  store auto-refreshes when its view is mounted; pauses when the
  view unmounts.

* **Plugin plumbing.** `window.__LVQR_ADMIN_PLUGINS__` array read at
  bootstrap; each entry registers a rail item + a route +
  optionally a Pinia store. Theme overrides via CSS custom
  properties. v1.0 ships the contract + docs; example plugins +
  marketplace deferred to future minor releases.

* **WASM chain UI: ordered-list view.** Renders the chain's
  `slots[]` from `GET /api/v1/wasm-filter` in insertion order with
  per-slot + per-(broadcast,track) seen / kept / dropped counters.
  Chain editing is configured via `lvqr serve --wasm-filter`
  (process-startup); the UI surfaces the configured shape but does
  not mutate it. Node-graph editor deferred to a future session.

* **Embedded `<lvqr-dvr-player>`** for the DVR view. Direct dep on
  `@lvqr/dvr-player 1.0.0` so the operator gets a working scrub
  surface in the same console.

### Deployment recipes shipped in `README.md`

* Local dev (`npm run dev` against `lvqr serve` on localhost)
* Static-hosted production (drop `dist/` behind any static host)
* Digital Ocean App Platform (App Spec example)
* Multi-relay (point one console at staging + production
  simultaneously via connection profiles)

### Known v1.0 limitations

* Live log tail: no LVQR admin route yet; placeholder + `RUST_LOG` /
  `journalctl` recipe.
* Server-side ingest CRUD: ingest endpoints are configured via CLI
  flags; the UI shows the recipe + per-publisher rows from
  `/api/v1/streams`.
* Transcode + AI agent edits: process-startup config; UI shows a
  read-only summary + flag-recipe.
* WASM chain edits: process-startup config; UI shows a read-only
  ordered-list view of the configured chain + per-slot counters.

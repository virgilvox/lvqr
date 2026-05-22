# `@lvqr/admin-ui`

Operator admin console for [LVQR](https://github.com/virgilvox/lvqr) live
streaming relays. Vue 3 SPA. Static deploy. Multi-relay. Themable.

## Quick start

```bash
# 1. Run a relay (no auth, with a DVR archive so the Recordings view has data)
lvqr serve --no-auth --archive-dir ./archive

# 2. Run the console against it
cd bindings/js/packages/admin-ui
npm install
npm run dev            # open http://localhost:5173

# Production: build a static bundle and serve dist/ anywhere
npm run build && npm run preview
```

On first load the console seeds a connection profile pointing at
`http://localhost:8080` (override with `VITE_LVQR_RELAY_URL`, or an
`app-config.json` at the served root). If your relay has auth enabled, paste
an admin token into the profile via the connection drawer in the topbar.
Add more relays there too -- the active profile drives every API call.

## What it does

`@lvqr/admin-ui` is a static single-page app that talks to one or many LVQR
relays over the typed admin client (`@lvqr/core`). It is **not** bundled
into the `lvqr` Rust binary -- you deploy it independently behind any
static host (nginx, Caddy, Digital Ocean App Platform, GitHub Pages,
Cloudflare Pages, Vercel) and point it at your relay or relays.

Every view in the console maps directly to an `/api/v1/*` route the relay
already exposes:

| View | Backed by | Notes |
|---|---|---|
| Dashboard | `/api/v1/{stats, streams, slo}` | KPIs + top streams + top SLO rows |
| Streams | `/api/v1/streams` | Filterable list |
| Stream detail | `/api/v1/streams/{name}` + `/api/v1/{slo, mesh}` | Per-broadcast tracks + SLO + mesh |
| Recordings | `/api/v1/archive` | Recorded broadcasts; deep-links into DVR scrub |
| DVR | embedded `<lvqr-dvr-player>` | Live HLS DVR scrub (`?broadcast=` preselect) |
| Ingest | `/api/v1/{server-info, streams}` + recipes | Bound-listener inventory + publisher recipes + live publishers |
| Filters | `/api/v1/wasm-filter` | Read-only ordered slot list + per-slot counters |
| Filter detail | `/api/v1/wasm-filter` | Per-slot drilldown |
| Transcode | `/api/v1/transcode/ladders` | Ladder + live counters; runtime add/remove rendition |
| Agents | `/api/v1/agents` | Configured agents + attachments; runtime start/stop |
| Egress | `/api/v1/slo` | Per-transport latency breakdown |
| Cluster | `/api/v1/cluster/{nodes, broadcasts, config}` | Read-only |
| Mesh | `/api/v1/mesh` | Tree viz + per-peer detail |
| Federation | `/api/v1/cluster/federation` | Per-link status |
| Auth | `/api/v1/streamkeys/*` + `/api/v1/config-reload` | Stream key CRUD; provider status (read-only) |
| Provenance | `/playback/verify/<broadcast>` | C2PA verify form |
| Observability | `/metrics` + `/api/v1/{stats, slo}` | KPIs + Prometheus scrape recipe |
| Logs | `/api/v1/logs` (SSE) | Live tail with level filter (query-token auth) |
| Settings | `/api/v1/config-reload` | Hot reload trigger; connection profiles |

Every view now maps to a live `/api/v1/*` route. Where a subsystem is
configured at process startup the view surfaces its live state; the
transcode and agent views additionally support runtime add/remove against
the relay. The console adapts to the LVQR surface; it never invents server
routes.

## Deployment recipes

### Local development against a localhost relay

```bash
# in one terminal
lvqr serve --no-auth --archive-dir ./archive

# in another
cd bindings/js/packages/admin-ui
npm install
npm run dev
# open http://localhost:5173/
```

The first-run flow seeds a connection profile from the resolved default
relay URL: `app-config.json`'s `defaultRelayUrl` if present, else
`VITE_LVQR_RELAY_URL`, else `http://localhost:8080`. Add more relays via
the connection drawer in the topbar.

### Static-hosted production (any host)

```bash
npm run build
# upload dist/ to your static host
```

`dist/index.html` works behind any host that serves SPAs. Use hash-based
routing by default (no rewrite rules needed). For runtime configuration
without rebuilding, drop an `app-config.json` at the served root:

```json
{
  "defaultRelayUrl": "https://relay.example.com",
  "grafanaUrl": "https://grafana.example.com/d/lvqr/lvqr"
}
```

### Digital Ocean App Platform

App Spec snippet (the same shape works for the App Platform CLI):

```yaml
name: lvqr-admin
static_sites:
  - name: console
    source_dir: bindings/js/packages/admin-ui
    build_command: npm run build
    output_dir: dist
    catchall_document: index.html
    envs:
      - key: VITE_LVQR_RELAY_URL
        value: https://relay.example.com
```

### nginx behind your existing reverse proxy

```nginx
server {
    listen 443 ssl http2;
    server_name admin.lvqr.example;

    root /var/www/lvqr-admin/dist;
    index index.html;

    location / {
        try_files $uri /index.html;
    }
}
```

### Multi-relay (one console, many relays)

Each connection profile stores `{ id, label, baseUrl, bearerToken? }` in
`localStorage`. Operators register relays via the topbar drawer; the
active profile drives every API call. Profiles never round-trip to a
backend.

A profile may also carry per-protocol port overrides
(`rtmpPort` / `whipPort` / `whepPort` / `hlsPort` / `dashPort` / `srtPort` /
`rtspPort` / `moqPort`) for relays that bind non-default ports; the publish
/ subscribe URL recipes honor them, falling back to the documented defaults
otherwise.

## Theming

Every visual token is a CSS custom property in
`src/styles/tokens.css`. Override the file at build time, or ship a
sibling stylesheet that re-declares `:root { ... }` after the package's
own CSS loads. Common ramps:

* `--bone` / `--paper` / `--paper-hi` / `--chalk*` -- surfaces
* `--ink` / `--ink-light` / `--ink-muted` / `--ink-faint` / `--ink-ghost`
* `--tally*` -- amber primary
* `--wire*` -- cyan secondary
* `--on-air` / `--ready` / `--warn` / `--idle` -- status

## Plugins

Set `window.__LVQR_ADMIN_PLUGINS__` before the bundle loads:

```html
<script>
window.__LVQR_ADMIN_PLUGINS__ = [
  {
    id: 'cost-explorer',
    label: 'Cost',
    path: '/plugins/cost',
    rail: 'system',
    icon: 'chart',
    component: window.MyCostComponent,
  },
];
</script>
<!-- ...then the app's own entry: the hashed assets/index-*.js that the
     built dist/index.html references -->
```

Set `window.__LVQR_ADMIN_PLUGINS__` before the admin-ui bundle's entry
script runs. Each entry registers a Vue Router route (`path`, keyed by `id`)
plus a rail entry (`label`, `rail` defaulting to `"system"`, `icon`
defaulting to `"plugin"`). The `component` must be a Vue 3 component
(`defineComponent` or a compiled `.vue`); host pages typically pre-build
their plugins as IIFE bundles attaching to `window`. Duplicate ids and ids
that collide with a built-in route are skipped with a console warning. v1.0
ships the plumbing; example plugins ship in a future release.

## Multi-relay auth model

Tokens (admin, JWT) are stored per-profile in this browser's
`localStorage`. The console issues no backend session; revoke a token by
removing the profile or rotating the token at the relay. Never share a
device's profile list as-is -- the bearer tokens are recoverable from
DevTools.

## Development scripts

```bash
npm run dev          # vite dev server with HMR
npm run build        # type-check + vite production build into dist/
npm run preview      # serve dist/ locally (for testing the production bundle)
npm run test:unit    # vitest unit tests
```

## Runtime mutation surface

* Live log tail: `GET /api/v1/logs` Server-Sent Events stream with a level
  filter, surfaced in the Logs view. Because `EventSource` cannot set an
  `Authorization` header, the admin token rides as a `?token=` query param;
  prefer a short-lived token since URLs can land in proxy logs.
* Transcode ladders: add / remove renditions at runtime from the Transcode
  view (`POST` / `DELETE /api/v1/transcode/ladders`). Requires a transcode
  ladder configured at startup (the runner only installs then); runtime-added
  renditions are not advertised in the HLS master playlist (composed at
  startup) though their output broadcasts are directly accessible.
* AI agents: start / stop the Whisper captions agent at runtime from the
  Agents view (`POST` / `DELETE /api/v1/agents`). The relay must be built
  with `--features whisper`; the model path is read on the relay host.

## Known v1.0 limitations

* Server-side ingest CRUD: ingest listeners are bound at startup via CLI
  flags + TOML config; runtime listener start/stop is on the v1.x backlog.
* Broadcast stop / kick subscriber: not exposed -- egress subscribers are
  anonymous stream readers and ingest sessions have no per-connection cancel
  handle; v1.x backlog.
* WASM chain edits: process-startup config; the UI surfaces the chain's
  read-only ordered-list view + per-slot counters. Node-graph editor is
  on the v1.x backlog.
* Full GET / PUT of the `--config` file: not exposed by the relay; v1.x
  backlog.

## License

MIT OR Apache-2.0

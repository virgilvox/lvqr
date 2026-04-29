# `@lvqr/admin-ui`

Operator admin console for [LVQR](https://github.com/virgilvox/lvqr) live
streaming relays. Vue 3 SPA. Static deploy. Multi-relay. Themable.

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
| Stream detail | `/api/v1/{streams, slo, mesh}` | Per-broadcast view |
| Recordings | -- (placeholder) | LVQR has no archive list API yet; v1.x backlog |
| DVR | embedded `<lvqr-dvr-player>` | Live HLS DVR scrub |
| Ingest | `/api/v1/{stats, streams}` + recipes | Publisher quickstart per protocol |
| Filters | `/api/v1/wasm-filter` | Read-only ordered slot list + per-slot counters |
| Filter detail | `/api/v1/wasm-filter` | Per-slot drilldown |
| Transcode | -- (placeholder) | Process-startup config; v1.x backlog |
| Agents | -- (placeholder) | Process-startup config; v1.x backlog |
| Egress | `/api/v1/slo` | Per-transport latency breakdown |
| Cluster | `/api/v1/cluster/{nodes, broadcasts, config}` | Read-only |
| Mesh | `/api/v1/mesh` | Tree viz + per-peer detail |
| Federation | `/api/v1/cluster/federation` | Per-link status |
| Auth | `/api/v1/streamkeys/*` + `/api/v1/config-reload` | Stream key CRUD; provider status (read-only) |
| Provenance | `/playback/verify/<broadcast>` | C2PA verify form |
| Observability | `/metrics` + `/api/v1/{stats, slo}` | KPIs + Prometheus scrape recipe |
| Logs | -- (placeholder) | No live tail route; recipe shown |
| Settings | `/api/v1/config-reload` | Hot reload trigger; connection profiles |

Where LVQR doesn't expose a route the mockup describes, the view renders a
clear placeholder + a v1.x backlog comment + a "configure via `lvqr serve`
flag X" hint. The console adapts to the LVQR surface; it never invents
server routes.

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

The first-run flow seeds a connection profile from
`VITE_LVQR_RELAY_URL` (default `http://localhost:8080`). Add more relays
via the connection drawer in the topbar (cluster icon).

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

Each connection profile stores `{ label, baseUrl, bearerToken? }` in
`localStorage`. Operators register relays via the topbar drawer; the
active profile drives every API call. Profiles never round-trip to a
backend.

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
<script type="module" src="/admin-ui/assets/main.js"></script>
```

Each entry registers a Vue Router route + a rail entry. The `component`
must be a Vue 3 component (`defineComponent` or compiled `.vue`); host
pages typically pre-build their plugins as IIFE bundles attaching to
`window`. v1.0 ships the plumbing; example plugins ship in a future
release.

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

## Known v1.0 limitations

* Live log tail: no LVQR admin route yet. Use `journalctl -u lvqr.service
  -f` or `kubectl logs -f` against the host.
* Server-side ingest CRUD: configured via CLI flags + TOML config file.
* Transcode + AI agent edits: process-startup config.
* WASM chain edits: process-startup config; the UI surfaces the chain's
  read-only ordered-list view + per-slot counters. Node-graph editor is
  on the v1.x backlog.
* Full GET / PUT of the `--config` file: not exposed by the relay; v1.x
  backlog.

## License

MIT OR Apache-2.0

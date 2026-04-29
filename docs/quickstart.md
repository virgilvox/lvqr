# Quickstart

Zero to streaming in five minutes.

## Install

```bash
# From crates.io
cargo install lvqr-cli

# From source
git clone https://github.com/virgilvox/lvqr.git
cd lvqr
cargo build --release
# Binary at target/release/lvqr
```

## Start the server

```bash
lvqr serve
```

This binds the zero-config defaults:

| Surface | Port | Protocol | Enabled |
|---|---|---|---|
| MoQ / QUIC / WebTransport | 4443/udp | MoQ over moq-lite | always |
| RTMP ingest | 1935/tcp | RTMP | always |
| LL-HLS | 8888/tcp | HTTP/1.1 | always |
| Admin HTTP + WS | 8080/tcp | HTTP + WebSocket fMP4 | always |

A self-signed TLS cert is auto-generated for the QUIC listener.
That is fine for local development; for production, supply
`--tls-cert` / `--tls-key` (PEM). See
[deployment](deployment.md).

### Turn on the other protocols

Every protocol beyond the four always-on surfaces is gated on a
non-zero port:

```bash
lvqr serve \
  --dash-port 8889 \
  --whep-port 8443 \
  --whip-port 8443 \
  --rtsp-port 8554 \
  --srt-port 8890
```

Note that WHIP and WHEP can share the same HTTPS port when
supplied with the same `--tls-*` pair; the routers are disjoint
paths (`/whip/*` vs `/whep/*`).

## Publish a test stream

### From OBS

Settings → Stream → Custom:
- Server: `rtmp://localhost:1935/live`
- Stream Key: `demo`

For WHIP (OBS 30+):
- Service: WHIP
- URL: `https://localhost:8443/whip/live/demo`
- Bearer token: leave blank unless `--publish-key` or
  `--jwt-secret` is set.

### From ffmpeg

```bash
# RTMP
ffmpeg -re -f lavfi -i testsrc=size=640x360:rate=30 \
  -f lavfi -i sine=frequency=440:sample_rate=44100 \
  -c:v libx264 -preset ultrafast -tune zerolatency \
  -c:a aac -b:a 128k \
  -f flv rtmp://localhost:1935/live/demo

# RTSP (requires --rtsp-port 8554)
ffmpeg -re -i source.mp4 -c copy -f rtsp rtsp://localhost:8554/live/demo

# SRT (requires --srt-port 8890)
ffmpeg -re -i source.mp4 -c copy -f mpegts \
  srt://localhost:8890?streamid=live/demo
```

## Play back

Every ingest protocol feeds every egress protocol through the
same unified fragment pipeline, so a broadcast published over
RTMP is immediately watchable via HLS, DASH, WHEP, MoQ, and
WebSocket fMP4.

- **LL-HLS** (hls.js, Safari, ffplay):
  `http://localhost:8888/hls/live/demo/playlist.m3u8`
- **MPEG-DASH** (dash.js):
  `http://localhost:8889/dash/live/demo/manifest.mpd`
- **WHEP** (WebRTC player):
  `https://localhost:8443/whep/live/demo`
- **MoQ** (Chrome / Edge 107+ with WebTransport, using
  `@lvqr/player`):
  `https://localhost:4443/live/demo`
- **WebSocket fMP4 fallback** (MSE in any browser):
  `ws://localhost:8080/ws/live/demo`

### Reference clients

- LL-HLS: open the playlist URL in the
  [hls.js demo](https://hls-js.netlify.app/demo/).
- DASH: open the MPD URL in the
  [dash.js reference player](https://reference.dashif.org/dash.js/).
- WHEP: use the [Broadcast Box](https://github.com/Glimesh/broadcast-box)
  client or the `simple-whep-client` reference.
- Bundled test app: `cd test-app && ./serve.sh` exposes the
  `@lvqr/player` web component on `http://localhost:3000`.

### DVR scrub

Add `--archive-dir /var/lib/lvqr/archive` and every ingested
fragment is written to disk with a `redb` index entry. The
admin HTTP API grows three routes (JSON segment-list surface,
useful for operator tooling and post-finalize archive
indexing):

```bash
# Every archived video segment for a broadcast, oldest first
curl 'http://localhost:8080/playback/live/demo'

# Decode-time window scrub (track timescale, not wallclock)
curl 'http://localhost:8080/playback/live/demo?track=0.mp4&from=0&to=1800000'

# Most-recent segment anchor (for "jump to live minus N seconds")
curl 'http://localhost:8080/playback/latest/live/demo'
```

**For end-viewer DVR scrub in the browser**, install
`@lvqr/dvr-player` and embed the component against the relay's
live HLS endpoint. The DVR window depth is whatever
`--hls-dvr-window` was configured with on `lvqr serve` (default
120 s; bump to 3600 for a one-hour scrub window):

```html
<script type="module">
  import '@lvqr/dvr-player';
</script>
<lvqr-dvr-player
  src="http://localhost:8080/hls/live/demo/master.m3u8"
  muted
  autoplay
></lvqr-dvr-player>
```

The component renders a custom seek bar with HH:MM:SS labels,
a LIVE pill, a Go Live button, and client-side hover
thumbnails. See [`docs/dvr-scrub.md`](dvr-scrub.md) for the
full operator embedding recipe and theming surface.

Auth and CORS defaults: if `--subscribe-token` or
`--jwt-secret` is set, playback routes inherit the same
credential as live subscribe. Unauthenticated servers serve
playback with `CorsLayer::permissive()`; tighten before
exposing publicly.

## Hardware encoders (optional)

Build with one of the per-backend Cargo features and select via
`--transcode-encoder`:

```bash
# macOS VideoToolbox
cargo build --release --features hw-videotoolbox -p lvqr-cli
lvqr serve --transcode-rendition 720p,480p,240p --transcode-encoder videotoolbox

# Linux + Nvidia (NVENC)
cargo build --release --features hw-nvenc -p lvqr-cli
lvqr serve --transcode-rendition 720p,480p,240p --transcode-encoder nvenc

# Linux + Intel iGPU / AMD (VA-API)
cargo build --release --features hw-vaapi -p lvqr-cli
lvqr serve --transcode-rendition 720p,480p,240p --transcode-encoder vaapi

# Linux + Intel Quick Sync
cargo build --release --features hw-qsv -p lvqr-cli
lvqr serve --transcode-rendition 720p,480p,240p --transcode-encoder qsv
```

Each backend probes its required GStreamer encoder element at
factory construction; if the runtime hardware is missing
`build()` opts out cleanly with a warn log rather than silently
falling back to software (a deliberate "operator-pickable
hardware tier" design choice). System-package prerequisites per
host are documented in
[`docs/deployment.md#hardware-encoder-prerequisites`](deployment.md#hardware-encoder-prerequisites).

## Monitor

```bash
curl http://localhost:8080/healthz       # liveness (always 200)
curl http://localhost:8080/readyz        # readiness (subsystems up)
curl http://localhost:8080/api/v1/stats  # connection + publisher counts
curl http://localhost:8080/api/v1/streams  # active broadcasts
curl http://localhost:8080/metrics       # Prometheus scrape
```

Point a Prometheus scrape at `/metrics`. For OTLP gRPC (spans
+ metrics) to Jaeger / Tempo / Grafana:

```bash
LVQR_OTLP_ENDPOINT=http://collector.local:4317 \
LVQR_SERVICE_NAME=lvqr-edge-01 \
LVQR_OTLP_RESOURCE="deploy.env=prod,region=us-east-1" \
  lvqr serve --dash-port 8889
```

Both Prometheus and OTLP paths can run simultaneously; the CLI
composes the `OtelMetricsRecorder` with the Prometheus recorder
via `metrics_util::FanoutBuilder`. Full recipe:
[observability](observability.md).

## Running as a cluster

```bash
# Node A
lvqr serve \
  --cluster-listen 10.0.0.1:10007 \
  --cluster-advertise-hls http://10.0.0.1:8888 \
  --dash-port 8889 --cluster-advertise-dash http://10.0.0.1:8889

# Node B
lvqr serve \
  --cluster-listen 10.0.0.2:10007 \
  --cluster-seeds 10.0.0.1:10007 \
  --cluster-advertise-hls http://10.0.0.2:8888 \
  --dash-port 8889 --cluster-advertise-dash http://10.0.0.2:8889
```

Publishing on either node auto-claims broadcast ownership.
Subscribers hitting the non-owner receive a 302 to the owner's
advertised URL for HLS, DASH, and RTSP. Admin routes expose
cluster state:

```bash
curl http://10.0.0.1:8080/api/v1/cluster/nodes
curl http://10.0.0.1:8080/api/v1/cluster/broadcasts
curl http://10.0.0.1:8080/api/v1/cluster/config
```

Full recipe: [cluster](cluster.md).

## Auth

```bash
# Static tokens (env vars also accepted: LVQR_*)
lvqr serve \
  --admin-token "$(openssl rand -hex 32)" \
  --publish-key "$(openssl rand -hex 16)" \
  --subscribe-token "$(openssl rand -hex 16)"

# HS256 JWT (replaces static tokens)
lvqr serve \
  --jwt-secret "$(openssl rand -hex 32)" \
  --jwt-issuer "https://auth.example.com" \
  --jwt-audience "lvqr-edge"
```

Both `Authorization: Bearer <tok>` and `?token=<tok>` query
parameter are honoured on every auth surface. Every auth
failure increments `lvqr_auth_failures_total{entry="..."}` for
alerting.

### Runtime stream-key CRUD

Mint, revoke, and rotate ingest stream keys at runtime without
bouncing the relay. Tokens are
`lvqr_sk_<43-char base64url-no-pad>`. Default-on; opt out with
`--no-streamkeys`.

```bash
# Mint a per-broadcast key (admin token required when --admin-token is set)
curl -X POST -H "Authorization: Bearer $ADMIN_TOKEN" \
  http://localhost:8080/api/v1/streamkeys

# List active keys
curl -H "Authorization: Bearer $ADMIN_TOKEN" \
  http://localhost:8080/api/v1/streamkeys

# Revoke
curl -X DELETE -H "Authorization: Bearer $ADMIN_TOKEN" \
  http://localhost:8080/api/v1/streamkeys/$KEY_ID

# Rotate (mints a new token under the same key id; old token denied)
curl -X POST -H "Authorization: Bearer $ADMIN_TOKEN" \
  http://localhost:8080/api/v1/streamkeys/$KEY_ID/rotate
```

The store is in-memory (restart loses keys; use
`LVQR_PUBLISH_KEY` for the durable single-key path). Full
recipe: [`auth.md#stream-key-crud-admin-api`](auth.md).

## Hot config reload

`lvqr serve --config <path.toml>` plus SIGHUP / `POST
/api/v1/config-reload` rotate auth providers, mesh ICE servers,
the HMAC playback secret, JWKS endpoint URLs, and webhook auth
URLs without bouncing the relay. The chain wraps in
`HotReloadAuthProvider` (always-on `arc_swap::ArcSwap` swap;
single-digit-ns reads).

```toml
# /etc/lvqr.toml -- every field optional
hmac_playback_secret = "rotate-me-monthly"

[auth]
publish_key = "operator-secret-v1"
admin_token = "ops-team"
# OR (asymmetric JWT, --features jwks)
# jwks_url = "https://idp.example.com/.well-known/jwks.json"
# OR (decision webhook, --features webhook)
# webhook_auth_url = "https://decisioner.example.com/check"

[[mesh_ice_servers]]
urls = ["turn:turn.example:3478"]
username = "u"
credential = "p"
```

```bash
# Boot against the file
lvqr serve --config /etc/lvqr.toml --admin-token "$ADMIN_TOKEN"

# Rotate the publish key
sed -i 's/operator-secret-v1/operator-secret-v2/' /etc/lvqr.toml
kill -HUP $(pgrep lvqr)
# OR
curl -X POST -H "Authorization: Bearer $ADMIN_TOKEN" \
  http://localhost:8080/api/v1/config-reload
```

The response shows which categories the reload effectively
touched:

```json
{
  "config_path": "/etc/lvqr.toml",
  "last_reload_at_ms": 1735170000000,
  "last_reload_kind": "admin_post",
  "applied_keys": ["auth", "mesh_ice"],
  "warnings": []
}
```

Failed reloads (malformed TOML, JWT init reject, JWKS initial
fetch failure) leave the prior chain live and return 500 with
the parse error in the body. Full recipe + hot-reloadable key
matrix: [`config-reload.md`](config-reload.md).

## Next steps

- [`architecture.md`](architecture.md) -- how the 29 crates fit
  together and the ten load-bearing architectural decisions
- [`deployment.md`](deployment.md) -- systemd, TLS, reverse
  proxy, firewall, Prometheus + OTLP collectors
- [`cluster.md`](cluster.md) -- chitchat cluster plane, lease
  tuning, redirect-to-owner semantics
- [`observability.md`](observability.md) -- OTLP endpoint
  setup, resource attribution, sampling, Grafana dashboards
- [`mesh.md`](mesh.md) -- peer mesh topology planner +
  browser-side WebRTC DataChannel data plane (data plane
  shipped session 144; topology + signaling on the Rust side,
  per-peer relay on the browser side via @lvqr/core)

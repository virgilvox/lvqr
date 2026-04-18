# Deployment

Production deployment guide for LVQR. Covers binary install,
systemd, TLS, reverse proxy, firewall, Prometheus scrape,
OTLP collector, and cluster bootstrap.

## Install

```bash
# From source (recommended until binary releases are on GitHub)
git clone https://github.com/virgilvox/lvqr.git
cd lvqr
cargo build --release
install -m 755 target/release/lvqr /usr/local/bin/lvqr

# From crates.io
cargo install lvqr-cli
```

## Port matrix

Open these ports in your firewall based on which protocols the
node exposes. Only the first four are required for a default
single-node deploy.

| Port | Protocol | Flag | Direction | Notes |
|---|---|---|---|---|
| 4443/udp | QUIC / MoQ | `--port` | inbound | WebTransport needs UDP; cannot proxy |
| 1935/tcp | RTMP | `--rtmp-port` | inbound | Publishers |
| 8888/tcp | LL-HLS | `--hls-port` | inbound | Subscribers + hls.js |
| 8080/tcp | Admin + WS | `--admin-port` | inbound | Restrict `/api/v1/*` |
| 8889/tcp | DASH | `--dash-port` | inbound | Optional |
| 8443/tcp | WHEP / WHIP | `--whep-port` / `--whip-port` | inbound | HTTPS required |
| 8554/tcp | RTSP | `--rtsp-port` | inbound | Optional |
| 8890/udp | SRT | `--srt-port` | inbound | UDP; cannot proxy |
| 10007/udp | chitchat gossip | `--cluster-listen` | intra-cluster | Between nodes only |
| 4317/tcp | OTLP gRPC | `LVQR_OTLP_ENDPOINT` | outbound | To collector |

```bash
# ufw example for a single-node deploy with HLS + DASH + WHIP
ufw allow 4443/udp
ufw allow 1935/tcp
ufw allow 8888/tcp
ufw allow 8889/tcp
ufw allow 8443/tcp
ufw allow from 10.0.0.0/8 to any port 8080  # admin internal only

# Cluster nodes: allow chitchat between LVQR nodes only
ufw allow from 10.0.0.0/24 to any port 10007 proto udp
```

## TLS

LVQR auto-generates a self-signed cert at boot if `--tls-cert`
/ `--tls-key` are unset. That is fine for local development
and for the QUIC listener behind a trusted network, but not
for public subscribers.

For production, supply real certs. Let's Encrypt via
`certbot` is the standard path:

```bash
certbot certonly --standalone -d relay.example.com

lvqr serve \
  --tls-cert /etc/letsencrypt/live/relay.example.com/fullchain.pem \
  --tls-key  /etc/letsencrypt/live/relay.example.com/privkey.pem
```

### WebTransport + reverse proxy

WebTransport (QUIC) **cannot be proxied by a TCP reverse
proxy**. Expose port 4443/udp directly to clients with its own
cert. The HTTP surfaces (HLS, DASH, admin, WHIP/WHEP) can sit
behind Caddy or Nginx:

```Caddyfile
relay.example.com {
    reverse_proxy /hls/*       localhost:8888
    reverse_proxy /dash/*      localhost:8889
    reverse_proxy /api/v1/*    localhost:8080
    reverse_proxy /healthz     localhost:8080
    reverse_proxy /readyz      localhost:8080
    reverse_proxy /metrics     localhost:8080
    reverse_proxy /whep/*      localhost:8443
    reverse_proxy /whip/*      localhost:8443
    # /ws/* terminates at admin port; Caddy proxies WS automatically.
    reverse_proxy /ws/*        localhost:8080
}
```

`relay.example.com:4443/udp` must bypass the reverse proxy
entirely. Cloudflare, Fastly, and other CDNs do not currently
proxy WebTransport; most deploys give the MoQ listener its own
subdomain (`moq.relay.example.com`).

## systemd

```ini
# /etc/systemd/system/lvqr.service
[Unit]
Description=LVQR live video server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=lvqr
Group=lvqr
EnvironmentFile=/etc/lvqr/lvqr.env
ExecStart=/usr/local/bin/lvqr serve
Restart=always
RestartSec=5
LimitNOFILE=65536

# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ReadWritePaths=/var/lib/lvqr
AmbientCapabilities=
CapabilityBoundingSet=

[Install]
WantedBy=multi-user.target
```

```sh
# /etc/lvqr/lvqr.env
RUST_LOG=lvqr=info,warn

# Core ports (defaults shown; override only if needed)
# LVQR_PORT=4443
# LVQR_RTMP_PORT=1935
# LVQR_ADMIN_PORT=8080
# LVQR_HLS_PORT=8888

# Optional protocols
LVQR_DASH_PORT=8889
LVQR_WHIP_PORT=8443
LVQR_WHEP_PORT=8443
LVQR_RTSP_PORT=8554
LVQR_SRT_PORT=8890

# TLS
LVQR_TLS_CERT=/etc/letsencrypt/live/relay.example.com/fullchain.pem
LVQR_TLS_KEY=/etc/letsencrypt/live/relay.example.com/privkey.pem

# Storage
LVQR_RECORD_DIR=/var/lib/lvqr/record
LVQR_ARCHIVE_DIR=/var/lib/lvqr/archive

# Auth (rotate on deploy)
LVQR_JWT_SECRET=...
LVQR_JWT_ISSUER=https://auth.example.com
LVQR_JWT_AUDIENCE=lvqr-edge
LVQR_ADMIN_TOKEN=...

# Observability
LVQR_OTLP_ENDPOINT=http://otel-collector.internal:4317
LVQR_SERVICE_NAME=lvqr-edge-01
LVQR_OTLP_RESOURCE=deploy.env=prod,region=us-east-1
LVQR_TRACE_SAMPLE_RATIO=0.1

# Cluster (omit for single-node)
LVQR_CLUSTER_LISTEN=10.0.0.1:10007
LVQR_CLUSTER_SEEDS=10.0.0.2:10007,10.0.0.3:10007
LVQR_CLUSTER_ADVERTISE_HLS=http://10.0.0.1:8888
LVQR_CLUSTER_ADVERTISE_DASH=http://10.0.0.1:8889
LVQR_CLUSTER_ADVERTISE_RTSP=rtsp://10.0.0.1:8554
```

```bash
sudo mkdir -p /var/lib/lvqr/{record,archive}
sudo chown -R lvqr:lvqr /var/lib/lvqr
sudo systemctl enable --now lvqr
sudo journalctl -u lvqr -f
```

## Prometheus scrape

Add a scrape job pointing at the admin port:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: lvqr
    metrics_path: /metrics
    static_configs:
      - targets:
          - 10.0.0.1:8080
          - 10.0.0.2:8080
          - 10.0.0.3:8080
        labels:
          cluster: prod-us-east-1
```

Key counters:
- `lvqr_frames_published_total{type="video|audio"}`
- `lvqr_bytes_ingested_total{type="video|audio"}`
- `lvqr_frames_relayed_total{transport="ws|moq"}`
- `lvqr_bytes_relayed_total{transport="ws|moq"}`
- `lvqr_auth_failures_total{entry="rtmp|ws|moq|admin|ws_ingest|playback"}`
- `lvqr_ws_connections_total{direction="subscribe|publish"}`
- `lvqr_moq_connections_total`
- `lvqr_rtmp_connections_total`
- `lvqr_active_moq_sessions` (gauge)
- `lvqr_active_streams` (gauge)
- `lvqr_mesh_peers` (gauge)

## OTLP sidecar

Running both Prometheus scrape AND OTLP export simultaneously
is supported; the `lvqr-cli` composition root fanouts the
`metrics::counter!` call sites to both backends via
`metrics_util::FanoutBuilder`. See
[observability](observability.md) for the resource-attribute
layout, sampling recipes, and Jaeger / Tempo / Grafana wiring.

## Cluster bootstrap

Minimum viable two-node cluster:

```bash
# Node A (seed)
lvqr serve \
  --cluster-listen 10.0.0.1:10007 \
  --cluster-advertise-hls  http://10.0.0.1:8888 \
  --cluster-advertise-dash http://10.0.0.1:8889 \
  --cluster-advertise-rtsp rtsp://10.0.0.1:8554

# Node B (joins A)
lvqr serve \
  --cluster-listen 10.0.0.2:10007 \
  --cluster-seeds  10.0.0.1:10007 \
  --cluster-advertise-hls  http://10.0.0.2:8888 \
  --cluster-advertise-dash http://10.0.0.2:8889 \
  --cluster-advertise-rtsp rtsp://10.0.0.2:8554
```

Verify:

```bash
curl http://10.0.0.1:8080/api/v1/cluster/nodes
curl http://10.0.0.1:8080/api/v1/cluster/broadcasts
```

First publisher for a broadcast name on either node auto-claims
ownership. Subscribers hitting the non-owner receive a 302 to
the owner's advertised URL. Full reference:
[cluster](cluster.md).

## Upgrade strategy

1. Roll one node at a time. The cluster is eventually
   consistent; taking one node down for ~30 s is covered by
   the 10 s ownership lease expiry (broadcast ownership
   reclaimed on the next publisher reconnect).
2. Drain subscribers by 302ing to a peer: stop the node's
   ingest first, let publishers reconnect elsewhere, then
   stop egress. Chitchat gossip removes the node from the
   membership list within one gossip round (~1 s).
3. Health probes: wire load balancers to `/readyz`. It
   returns 503 during shutdown so traffic drains before the
   process exits.

## Firewall hardening checklist

- [ ] `--admin-port` (8080) limited to internal subnets.
- [ ] `--admin-token` or `--jwt-secret` set; leaving admin
      unauthenticated is a known footgun.
- [ ] `--subscribe-token` or JWT set if subscriber access is
      restricted. Note: `CorsLayer` defaults to permissive;
      `docs/observability.md` tracks the tightening work.
- [ ] chitchat port (10007/udp) only open to sibling nodes.
- [ ] `/metrics` either on an internal interface or behind a
      Prometheus-only firewall rule.
- [ ] OTLP collector endpoint reachable from the node's
      egress but not from the public internet.

## Observability and cluster details

- [observability](observability.md) -- OTLP collector recipes,
  resource attributes, sampling ratios, Jaeger / Tempo /
  Grafana dashboards.
- [cluster](cluster.md) -- chitchat membership, lease tuning,
  redirect-to-owner semantics, upgrade patterns, failure
  modes.

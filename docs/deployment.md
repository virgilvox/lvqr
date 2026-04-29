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

## Hardware encoder prerequisites

LVQR ships four hardware encoder backends, each behind a Cargo
feature on `lvqr-cli` + `lvqr-transcode`. The build pulls the
`gstreamer-rs` bindings; the runtime needs the matching
GStreamer plugin + vendor driver installed on the host.

```bash
# macOS VideoToolbox (Apple Silicon + Intel)
brew install gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-libav
# vtenc_h264_hw ships with applemedia in gst-plugins-bad

# Ubuntu / Debian + Nvidia NVENC
sudo apt-get install -y \
  gstreamer1.0-tools gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad \
  gstreamer1.0-libav \
  nvidia-cuda-toolkit
# Plus a working Nvidia driver (proprietary or open). Verify:
gst-inspect-1.0 nvh264enc | head -8

# Ubuntu / Debian + Intel VA-API (iGPU + Arc)
sudo apt-get install -y \
  gstreamer1.0-tools gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-bad gstreamer1.0-libav \
  intel-media-va-driver-non-free
# Or for AMD: replace the va-driver line with `mesa-va-drivers`.
# Verify libva sees a device + the encoder element loads:
vainfo
gst-inspect-1.0 vah264enc | head -8

# Ubuntu / Debian + Intel Quick Sync (oneVPL / legacy MFX)
sudo apt-get install -y \
  gstreamer1.0-plugins-bad libmfx1
# qsvh264enc selects oneVPL on Tiger Lake+ and MFX on older
# Skylake-Coffee Lake automatically. Verify:
gst-inspect-1.0 qsvh264enc | head -8

# Rocky / RHEL 8+ + Nvidia
sudo dnf install -y \
  gstreamer1-plugins-bad-free gstreamer1-libav \
  cuda-toolkit
```

Each backend's `is_available()` probes for its required GStreamer
elements at factory construction and emits a `warn!` log naming
the missing element if the host can't drive the requested
encoder. `build()` then opts out of every stream cleanly --
**no silent fallback to software**. This is deliberate: an
operator who builds with `--features hw-nvenc` and selects
`--transcode-encoder nvenc` should hard-fail (visibly) on a host
without an Nvidia driver rather than burn CPU silently.

Build-time selection (one feature per binary):

```bash
cargo build --release --features hw-videotoolbox -p lvqr-cli  # macOS
cargo build --release --features hw-nvenc        -p lvqr-cli  # Linux + Nvidia
cargo build --release --features hw-vaapi        -p lvqr-cli  # Linux + Intel iGPU / AMD
cargo build --release --features hw-qsv          -p lvqr-cli  # Linux + Intel QSV
```

Multiple features may compile together, but a binary's runtime
selection is via the `--transcode-encoder
software|videotoolbox|nvenc|vaapi|qsv` flag.

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

## Archive: `io_uring` write backend (Linux-only)

The DVR archive writer routes CMAF segments to disk via
`lvqr-archive`'s `write_segment`. By default it uses
`std::fs::create_dir_all` + `std::fs::write`; on Linux hosts the
crate can be rebuilt with the `io-uring` feature to route the
file-create + payload write + `fsync` phase through a
`tokio-uring::fs::File` instead. This is a build-time decision --
the feature is off by default and entirely absent from the macOS +
Windows dep graph.

### When to enable

Turn it on if all of these are true:

- Target deployment is Linux with kernel >= 5.6 (the `io_uring_*`
  syscall family landed in 5.1 but the `IORING_OP_WRITE` /
  `IORING_OP_CLOSE` shape relied on by tokio-uring 0.5 is stable
  from 5.6 onward).
- You are archiving high-bitrate streams or many concurrent
  broadcasts. The win scales with segment size + segment rate --
  large keyframes at frequent intervals benefit most.
- Your container runtime does NOT drop `io_uring_*` from its
  default seccomp profile. Docker 24+ and containerd 1.7+ allow
  io_uring by default; older runtimes and some managed
  Kubernetes distributions still block it (gVisor, for example,
  blocks io_uring unconditionally).

Leave it off if any of these are true:

- You are targeting macOS, Windows, or a BSD variant. The feature
  is a no-op on those targets because the `tokio-uring` dep is
  gated on `cfg(target_os = "linux")`.
- Your archive workload is bursty-small (AAC-only, or a handful
  of sub-4-KiB fragments per second). The per-call
  `tokio_uring::start` setup cost is a fixed overhead that small
  writes do not amortise. Run the bench (below) to confirm on
  your hardware + kernel; the crossover point varies.
- You operate under a hardened seccomp profile that does not
  include io_uring. The crate's runtime fallback will carry on
  correctly, but you gain no benefit and pay one cold-start
  `tracing::warn` per process.

### Enable

Rebuild `lvqr-cli` with the feature forwarded through to
`lvqr-archive`:

```bash
cargo build --release -p lvqr-cli --features lvqr-archive/io-uring
```

Or, if you consume `lvqr-archive` directly in a downstream crate:

```toml
[dependencies]
lvqr-archive = { version = "0.4", features = ["io-uring"] }
```

No runtime flag, no config file change. The path switch is
compile-time only; callers do not change.

### Measure on your host

`lvqr-archive` ships a criterion bench that parameterises segment
size across `[4 KiB, 64 KiB, 256 KiB, 1 MiB]`. The recommended
workflow is criterion's saved baselines: run the std path first,
save it as a named baseline, then run the io-uring path against
it.

```bash
# 1. Capture the std::fs baseline.
cargo bench -p lvqr-archive --bench io_uring_vs_std -- \
    --save-baseline std

# 2. Re-run with the feature on; criterion diffs vs. the saved baseline.
cargo bench -p lvqr-archive --features io-uring \
    --bench io_uring_vs_std -- --baseline std
```

Run both on the same host. Different CPUs, kernels, and block
devices produce materially different results; numbers captured on
one machine are not portable to another. Archive writes are
disk-IO-bound, so do not run the bench against `tmpfs`
(`/dev/shm`) -- tmpfs bypasses the block-device IO scheduler and
hides the effect the feature is designed to measure. Use a real
disk mount, and if `/tmp` is small, `TMPDIR=/var/tmp` (or a
dedicated scratch partition) before running.

Interpret the output like any criterion run: look at the
throughput delta + the p99 latency column. A positive throughput
delta on 256 KiB + 1 MiB variants with a no-worse result on the 4
KiB variant is the signal to enable the feature. If the 4 KiB
variant regresses measurably, leave it off or revisit when
session-90-scope bench work promotes the writer to a persistent
current-thread runtime (see
`tracking/TIER_4_PLAN.md` section 4.1, option (b)).

### Runtime fallback

The feature cannot silently fail on a misconfigured host. At
first call, `write_segment` attempts `tokio_uring::start`; if the
kernel rejects the setup (too old, seccomp drop, missing CAP),
the crate catches the panic, emits exactly one warning, and
pins `std::fs::write` for the rest of the process lifetime.
Subsequent writes skip the probe and go straight to the std
path, so the cost of the fallback is one log line.

The warning looks like this:

```text
WARN lvqr_archive::writer: tokio_uring::start failed (kernel < 5.6
    or sandbox without io_uring syscalls); falling back to std::fs
    for archive writes for the rest of this process
    path=/var/lib/lvqr/archive/live/dvr/0.mp4/00000001.m4s
```

If you see this in production while running on a kernel you
believe supports io_uring, check:

1. Your container seccomp profile includes the `io_uring_setup`,
   `io_uring_enter`, and `io_uring_register` syscalls.
2. No `LimitMEMLOCK` or `LimitNOFILE` in your systemd unit is
   capping ringbuf allocation below tokio-uring's defaults.
3. You are not inside a gVisor or Kata sandbox that blocks
   io_uring by policy (those sandboxes will never allow it --
   run without the feature).

On-path `io::Error`s (disk full, permission denied, ENOENT on a
since-deleted archive dir) after the runtime is up surface as
`ArchiveError::Io` and are logged by the caller, exactly as the
std::fs path would. Those errors do NOT trip the fallback latch;
the next segment retries the io_uring path cleanly.

### Caveats

- `create_dir_all` stays on `std::fs` even with the feature on.
  tokio-uring 0.5 exposes no mkdir primitive; the archive tree
  (`<root>/<broadcast>/<track>/`) is created once per
  `(broadcast, track)` pair and then amortised across thousands
  of segment writes, so the extra syscall is noise compared to
  the payload write.
- The feature only affects archive *writes*. Playback reads
  still use `tokio::fs::read` in the `lvqr-cli::archive`
  file-handler. Adding an io_uring reader path doubles the
  validation surface and the gains on the read side are smaller
  (reads are typically page-cache hits); deferred to a later
  session if a latency SLO forces it.
- Archive segment ordering is enforced by the caller's existing
  `BroadcasterArchiveIndexer::drain` task -- one drain task per
  `(broadcast, track)`, which serialises writes per stream. The
  io-uring feature does not change this contract.

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

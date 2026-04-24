# Observability

LVQR exposes three telemetry paths:

- **Structured logs** via `tracing` to stdout. Always on.
- **OTLP gRPC** export of spans and metrics when
  `LVQR_OTLP_ENDPOINT` is set. Optional.
- **Prometheus scrape** on the admin port. Always on.

OTLP and Prometheus can run simultaneously. The
`lvqr-cli::start` composition root combines the OTel metrics
recorder with the Prometheus scrape recorder via
`metrics_util::layers::FanoutBuilder`, so every existing
`metrics::counter!` / `gauge!` / `histogram!` call site flows
to both backends without any call-site changes.

## Environment variables

| Env | Default | Purpose |
|---|---|---|
| `LVQR_OTLP_ENDPOINT` | unset | OTLP gRPC target (`http://collector:4317`). Unset = stdout fmt only; no OTel providers constructed. |
| `LVQR_SERVICE_NAME` | `lvqr` | `service.name` resource applied to every span + metric. |
| `LVQR_OTLP_RESOURCE` | `` | Extra resource attributes as comma-separated `k=v` pairs: `deploy.env=prod,region=us-east-1,node_id=edge-01`. |
| `LVQR_TRACE_SAMPLE_RATIO` | `1.0` | Head-based sampling ratio in `[0.0, 1.0]`. `0.1` records 10 % of traces. Clamped to the range on parse. |
| `LVQR_LOG_JSON` | `false` | Switch stdout fmt layer from pretty to JSON. (Wired in Tier 3 session J; session H + I land the OTLP surfaces first.) |
| `RUST_LOG` | `lvqr=info` | Standard `tracing_subscriber::EnvFilter`. Applies to both the stdout fmt layer and the OTLP span exporter. |

## Spans (Tier 3 session H)

When `LVQR_OTLP_ENDPOINT` is set, every `tracing` span emitted
by the server flows out via an OTLP gRPC span exporter behind
a `BatchSpanProcessor`. Spans render to stdout AND to the
configured collector; operators never lose visibility by
enabling OTLP.

```bash
LVQR_OTLP_ENDPOINT=http://otel-collector.internal:4317 \
LVQR_SERVICE_NAME=lvqr-edge-01 \
LVQR_OTLP_RESOURCE="deploy.env=prod,region=us-east-1" \
LVQR_TRACE_SAMPLE_RATIO=0.1 \
  lvqr serve --dash-port 8889 --whip-port 8443
```

Spans the server emits today:
- `rtsp.describe` / `rtsp.setup` / `rtsp.play` (per-request)
- `hls.playlist.render` (per hit, at debug)
- `moq.session.lifecycle` (per QUIC session)
- `rtmp.session.lifecycle` (per RTMP connection)
- `bridge.auto_claim` (per cluster-mode broadcast start)
- `observability.initialized` (one per process)

Sampling is head-based only (LBD from `tracking/TIER_3_PLAN.md`).
`Sampler::TraceIdRatioBased(LVQR_TRACE_SAMPLE_RATIO)` applied
to the `TracerProvider`. Tail sampling is explicitly out of
scope.

### Jaeger / Tempo recipe

Point LVQR at your OTel collector (or directly at a Jaeger
OTLP port):

```yaml
# otel-collector-config.yaml
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317
exporters:
  otlp/tempo:
    endpoint: tempo.internal:4317
    tls:
      insecure: true
service:
  pipelines:
    traces:
      receivers: [otlp]
      exporters: [otlp/tempo]
```

Jaeger UI queries:
- `service.name="lvqr-edge-01"` -- single-node trace browsing.
- `service.name=~"lvqr-edge-.+"` -- fleet-wide.

## Metrics (Tier 3 session I)

Two recorder paths; the fanout is transparent to call sites.

```
  metrics::counter!(...)
       │
       ▼
  metrics-util::FanoutBuilder
     ├─► PrometheusRecorder ────► GET /metrics scrape
     └─► OtelMetricsRecorder ───► SdkMeterProvider
                                   │
                                   ▼
                                  PeriodicReader (60 s default)
                                   │
                                   ▼
                                  OTLP gRPC MetricExporter
```

The OtelMetricsRecorder implements `metrics::Recorder` and
interns instruments by metric name in `DashMap`s. Call-site
labels flow as per-record OTel attributes. Counter + histogram
round-trip cleanly; gauges use an `UpDownCounter<f64>` for
`.increment` / `.decrement`, and `.set(v)` emits `(v − last)`
via an `AtomicU64` last-value cache so the running total
converges on the set value.

Known limitation: concurrent `.set` on the same metric name
can drop a write under contention. Only
`lvqr_mesh_offload_percentage` uses `.set` today, and mesh
topology is single-threaded, so no call site is affected.
Session J may upgrade this to a per-`(name, labels)`
`ObservableGauge` path if needed.

### Counters (selected)

| Metric | Type | Labels | Emitted by |
|---|---|---|---|
| `lvqr_frames_published_total` | counter | `type=video\|audio` | lvqr-ingest bridge |
| `lvqr_bytes_ingested_total` | counter | `type=video\|audio` | lvqr-ingest bridge |
| `lvqr_frames_relayed_total` | counter | `transport=ws\|moq` | lvqr-cli + lvqr-relay |
| `lvqr_bytes_relayed_total` | counter | `transport=ws\|moq` | lvqr-cli + lvqr-relay |
| `lvqr_rtmp_connections_total` | counter | -- | lvqr-ingest |
| `lvqr_moq_connections_total` | counter | -- | lvqr-relay |
| `lvqr_ws_connections_total` | counter | `direction=publish\|subscribe` | lvqr-cli |
| `lvqr_auth_failures_total` | counter | `entry=rtmp\|ws\|moq\|admin\|ws_ingest\|playback` | every auth surface |
| `lvqr_active_moq_sessions` | gauge | -- | lvqr-relay |
| `lvqr_active_streams` | gauge | -- | lvqr-ingest |
| `lvqr_mesh_peers` | gauge | -- | lvqr-mesh |
| `lvqr_mesh_offload_percentage` | gauge (.set) | -- | lvqr-mesh |

Resource attributes applied to every metric:
- `service.name` (from `LVQR_SERVICE_NAME`)
- every `LVQR_OTLP_RESOURCE` `k=v` pair
- `instrumentation.scope.name = "lvqr-observability"` (for
  the OTel path)

### Prometheus scrape

Always on; exposed at `http://<admin-host>:8080/metrics`.
No TLS; put it behind an internal-only firewall rule or a
reverse proxy.

```yaml
# prometheus.yml
scrape_configs:
  - job_name: lvqr
    scrape_interval: 15s
    metrics_path: /metrics
    static_configs:
      - targets: ["10.0.0.1:8080", "10.0.0.2:8080", "10.0.0.3:8080"]
        labels:
          cluster: prod-us-east-1
```

### OTLP metric export

Uses a `PeriodicReader` over `runtime::Tokio` on top of the
OTLP gRPC `MetricExporter`. Default collect interval is 60 s;
this is the OTel SDK default and is reasonable for metric
aggregation. To tune, build a custom `PeriodicReader` in an
embedder crate (the `build_meter_provider` helper is public).

## Fanout: running both at once

The most common production configuration: Prometheus scrape
for ops dashboards + OTLP for traces + OTLP for metrics
federation. LVQR does this with a single env-var flip:

```bash
LVQR_OTLP_ENDPOINT=http://collector:4317 lvqr serve \
  --dash-port 8889
```

Under the hood, `lvqr-cli::start` pattern-matches on
`(install_prometheus, otel_metrics_recorder)`:

- **Both set** → `metrics_util::FanoutBuilder` composes the
  OTel recorder and the Prometheus recorder. The
  `PrometheusRecorder::handle()` is captured before the
  fanout so `/metrics` still works.
- **Prom only** → Prometheus recorder installed standalone.
- **OTel only** → OtelMetricsRecorder installed standalone.
- **Neither** (tests) → no recorder installed; call sites
  no-op.

## Logs (pending Tier 3 session J)

Today, logs are pretty-printed to stdout via a default
`tracing_subscriber::fmt::layer()`. Tier 3 session J will:

- When `LVQR_LOG_JSON=true`, switch the fmt layer to JSON
  one-event-per-line.
- Emit `trace_id` and `span_id` fields on every log event
  while a span is active, so Loki / Promtail can
  cross-reference with OTLP traces.

This is pending as of session 82 close; see
`tracking/HANDOFF.md` session-83 entry point.

## Resource attribution recipes

Pick one convention and stick to it across the fleet. The
goal is every span and every metric carries enough resource
attributes to answer "which node, which cluster, which
environment, which version".

```bash
# Single-node dev
LVQR_SERVICE_NAME=lvqr-dev \
LVQR_OTLP_RESOURCE="deploy.env=dev" \
  lvqr serve

# Prod edge node in a cluster
LVQR_SERVICE_NAME=lvqr-edge \
LVQR_OTLP_RESOURCE="deploy.env=prod,region=us-east-1,node_id=edge-01,version=0.4.0" \
LVQR_TRACE_SAMPLE_RATIO=0.05 \
  lvqr serve --cluster-listen 10.0.0.1:10007 --cluster-seeds 10.0.0.2:10007

# Staging with full sampling for debugging
LVQR_SERVICE_NAME=lvqr-staging \
LVQR_OTLP_RESOURCE="deploy.env=staging,region=us-east-1" \
LVQR_TRACE_SAMPLE_RATIO=1.0 \
  lvqr serve
```

`service.name` is the single field every Jaeger / Tempo /
Honeycomb UI keys off. Using distinct values per tier and
region keeps filters simple; pushing the same
`service.name=lvqr` from every node and distinguishing only
via resource attributes is also fine but requires UI
filters.

## Sampling guidance

- **Dev / staging:** `LVQR_TRACE_SAMPLE_RATIO=1.0`
  (everything). No load; maximum visibility.
- **Prod edge:** `LVQR_TRACE_SAMPLE_RATIO=0.05`-`0.1`.
  Spans are cheap at the emit site but expensive to store.
  5 % is enough to debug rare bugs and keep the collector
  bill reasonable.
- **Incident response:** bump to `1.0`, restart one node,
  collect traces for the duration of the incident, revert.
  The env-var-driven approach means no redeploy.

## Grafana dashboards

A starter `grafana/dashboards/lvqr.json` is tracked in
`deploy/` and imports the metrics above. Panels:

- Ingest throughput (`rate(lvqr_frames_published_total[5m])`
  by `type`).
- Relay throughput (`rate(lvqr_bytes_relayed_total[5m])` by
  `transport`).
- Active stream count (`lvqr_active_streams`).
- Active subscriber count
  (`lvqr_active_moq_sessions + lvqr_ws_connections_total`).
- Auth failure rate
  (`rate(lvqr_auth_failures_total[5m])` by `entry`).
- Cluster node count (derived from `/api/v1/cluster/nodes`
  via a blackbox exporter or a custom scrape).

## Latency SLO (Tier 4 item 4.7)

`lvqr_subscriber_glass_to_glass_ms` is a Prometheus histogram
recorded by every instrumented egress surface on each
subscriber-side fragment delivery. Labels: `broadcast` +
`transport`. Fired from `lvqr_admin::slo::LatencyTracker`'s
`record()` method and simultaneously written into a
per-`(broadcast, transport)` ring buffer that powers the
read-only admin route:

```bash
curl -H "Authorization: Bearer $LVQR_ADMIN_TOKEN" \
  http://localhost:8080/api/v1/slo
```

The route returns
`{ broadcasts: [{ broadcast, transport, p50_ms, p95_ms, p99_ms,
max_ms, sample_count, total_observed } ] }` sorted by
`(broadcast, transport)`.

The rule pack
[`deploy/grafana/alerts/lvqr-slo.rules.yaml`](../deploy/grafana/alerts/lvqr-slo.rules.yaml)
ships five Prometheus alerts (critical p99 > 4s, warning p99 > 2s,
warning p95 > 1.5s, info p50 > 500ms, warning no-recent-samples)
tuned for the default LL-HLS shape. The companion dashboard
[`deploy/grafana/dashboards/lvqr-slo.json`](../deploy/grafana/dashboards/lvqr-slo.json)
panels the same percentiles plus the raw sample rate.

Full runbook, per-transport threshold table, and troubleshooting
checklist: [`docs/slo.md`](slo.md).

## WASM filter chain (PLAN Phase D session 137)

`lvqr_wasm_fragments_total{outcome=keep|drop}` is a Prometheus
counter fired by the filter bridge on every fragment that flows
through the configured `--wasm-filter` chain. In parallel, the
bridge maintains per-`(broadcast, track)` atomic counters that
the admin API surfaces:

```bash
curl -H "Authorization: Bearer $LVQR_ADMIN_TOKEN" \
  http://localhost:8080/api/v1/wasm-filter
```

Returns:

```json
{
  "enabled": true,
  "chain_length": 2,
  "broadcasts": [
    { "broadcast": "live/cam1", "track": "0.mp4",
      "seen": 1240, "kept": 1238, "dropped": 2 }
  ]
}
```

`chain_length` is the number of filters composed via repeated
`--wasm-filter` (or comma-separated `LVQR_WASM_FILTER`). The
`broadcasts` array gives every `(broadcast, track)` pair the
filter tap has seen since startup; counters are atomic but may
drift by one or two between reads for the same key. When
`--wasm-filter` is unset the route replies 200 OK with
`{ enabled: false, chain_length: 0, broadcasts: [] }` so
dashboards pre-baking the shape do not need a 404 handler.

Use this route to verify a deployed filter chain is (a) the
length you configured and (b) actually observing traffic. If
`chain_length` is wrong, the CLI flag did not parse the way you
expected. If `seen == 0` for every entry, the broadcast never
reached the bridge. If `dropped > 0` surprises you, one of the
chained filters is denying.

## Load-bearing invariant (LBD #4)

Lifecycle events (publisher up / down, viewer join / leave)
go through `tokio::sync::broadcast` (the `EventBus`).
Per-fragment / per-byte counters go through the `metrics`
crate directly. The observability plane reads from both; it
does not re-home either onto the bus.

This is why the OTLP landing in sessions H / I did **not**
require any call-site changes in `lvqr-ingest`,
`lvqr-relay`, `lvqr-admin`, or `lvqr-mesh`. Protect this
split; it is what keeps the data plane cheap.

## Further reading

- [architecture](architecture.md) -- observability plane in
  context of the full workspace.
- [deployment](deployment.md) -- Prometheus + OTLP collector
  integration.
- [`tracking/TIER_3_PLAN.md`](../tracking/TIER_3_PLAN.md) --
  the per-session observability plane decomposition
  (sessions G through J).
- [OpenTelemetry Rust](https://github.com/open-telemetry/opentelemetry-rust)
  -- upstream SDK.

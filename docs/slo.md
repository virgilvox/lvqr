# Latency SLO

Tier 4 item 4.7 covers the **latency SLO**: a server-side
glass-to-glass latency histogram per
`(broadcast, transport)` pair plus a read-only admin route
(`/api/v1/slo`) and a Prometheus alert pack.

This document is the operator runbook. If you are reading a
`runbook_url` link on a firing alert, scroll to the alert's named
section below.

## What the metric measures

`lvqr_subscriber_glass_to_glass_ms` is a Prometheus histogram
recorded by every instrumented egress surface. Each sample is the
delta between two wall-clock timestamps:

1. **Ingest stamp**: `Fragment::ingest_time_ms`, set by
   `lvqr_ingest::dispatch::publish_fragment` (or a federation relay
   pre-stamp) at the moment the ingest protocol handler decoded the
   fragment. Every ingest protocol (RTMP, SRT, RTSP, WHIP, WS) feeds
   through the same dispatch helper, so the field is always present
   for live publishes.
2. **Egress emit**: the moment the egress surface delivered the
   fragment to subscribers. For LL-HLS, this is the `push_chunk_bytes`
   call inside `BroadcasterHlsBridge::drain` (107 A). For MPEG-DASH,
   this is the `push_video_segment` / `push_audio_segment` call inside
   `BroadcasterDashBridge::drain` (109 A); samples are per-Fragment,
   not per-finalized-DASH-segment, so operators reading the
   "sample rate" panel see one tick per incoming fragment even though
   DASH `$Number$` URIs address full segments. WS / MoQ / WHEP
   instrumentation is a small additive follow-up, blocked on MoQ
   frame-carried ingest-time propagation.

The histogram labels are:

| Label | Source |
|---|---|
| `broadcast` | Broadcast name (e.g. `live/demo`). One label value per live broadcast. |
| `transport` | Egress surface string (`"hls"`, `"ws"`, `"dash"`, `"moq"`, `"whep"`). Kept as a string (not an enum) so new protocols slot in without a type change. |
| `le` | Standard Prometheus histogram bucket label. |

### What it does NOT measure

* **Client render latency**. Browser playback frame-display time is
  a client SDK signal; pair these metrics with
  `@lvqr/player` browser telemetry (Tier 5) for a full
  glass-to-glass picture.
* **Ingest network RTT**. The publisher's RTT to the server is not
  subtracted from the ingest stamp; a very slow uplink inflates
  every downstream sample.
* **Subscriber-side buffering**. HLS players typically hold a
  multi-segment buffer; the SLO measures the server's contribution
  to end-to-end latency, not the client's play-out buffering.

## Querying live latency

Two read paths are available.

### `/api/v1/slo` admin route

Returns a point-in-time snapshot drawn from the
`lvqr-admin::slo::LatencyTracker` ring buffer (1024 samples per
`(broadcast, transport)` key).

```bash
curl -H "Authorization: Bearer $LVQR_ADMIN_TOKEN" \
  http://localhost:8080/api/v1/slo
```

Response shape:

```json
{
  "broadcasts": [
    {
      "broadcast": "live/demo",
      "transport": "hls",
      "p50_ms": 180,
      "p95_ms": 420,
      "p99_ms": 890,
      "max_ms": 1200,
      "sample_count": 1024,
      "total_observed": 4810
    }
  ]
}
```

Fields:

* `sample_count` -- samples currently retained in the ring buffer.
  Capped at 1024; a busy broadcast overwrites oldest-first.
* `total_observed` -- lifetime sample count since the tracker was
  constructed (unbounded). Useful for sanity-checking that samples
  are flowing.

### Prometheus histogram

Standard `histogram_quantile()` over the bucket counts:

```promql
histogram_quantile(
  0.99,
  sum by (broadcast, transport, le) (
    rate(lvqr_subscriber_glass_to_glass_ms_bucket[5m])
  )
)
```

The Grafana dashboard under
[`deploy/grafana/dashboards/lvqr-slo.json`](../deploy/grafana/dashboards/lvqr-slo.json)
panels p50 / p95 / p99 + the raw sample rate.

## Alert rule pack

The Prometheus rule pack is
[`deploy/grafana/alerts/lvqr-slo.rules.yaml`](../deploy/grafana/alerts/lvqr-slo.rules.yaml).
Five rules, targeting the default LL-HLS latency shape:

| Alert | Expression (abbreviated) | Threshold | Fire delay | Severity |
|---|---|---|---|---|
| `LvqrSloLatencyP99VeryHigh` | `histogram_quantile(0.99, ...)` | `> 4000 ms` | 2 min | critical |
| `LvqrSloLatencyP99High` | `histogram_quantile(0.99, ...)` | `> 2000 ms` | 5 min | warning |
| `LvqrSloLatencyP95High` | `histogram_quantile(0.95, ...)` | `> 1500 ms` | 5 min | warning |
| `LvqrSloLatencyP50High` | `histogram_quantile(0.50, ...)` | `> 500 ms` | 10 min | info |
| `LvqrSloNoRecentSamples` | `rate(..._count[5m]) == 0 AND rate(..._count[30m]) > 0` | -- | 5 min | warning |

### Critical p99 above 4s

Trigger: `LvqrSloLatencyP99VeryHigh`.

1. Pull the `/api/v1/slo` snapshot for the flagged
   `(broadcast, transport)`. If `sample_count < 100`, percentiles
   are noisy; wait for more samples before acting.
2. Check `lvqr_fragment_broadcast_lagged_skips_total` for the same
   broadcast. A non-zero rate means a subscriber is lagging the
   in-memory ring buffer -- typically a slow HLS player on a
   congested last-mile connection. Action: lower
   `LVQR_HLS_DVR_WINDOW` if the server is under memory pressure, or
   accept that specific subscribers will be dropped.
3. Check host CPU on the egress machine. x264enc or the HLS drain
   task may be starved if CPU is pinned by a ladder transcode.
4. Check the ingest side via `/api/v1/streams`. If the publisher's
   fragment rate has slowed (check via the bridge counters), the
   latency is publisher-driven and the server is catching up.

### Warning p99 above 2s

Trigger: `LvqrSloLatencyP99High`.

Standard LL-HLS SLO burn. On LL-HLS this is roughly 1 segment + 1
round-trip above nominal. Not urgent but worth a look during next
maintenance window:

1. Confirm the broadcast in question is LL-HLS. If it is a WebRTC
   (`transport="whep"`) or MoQ (`transport="moq"`) broadcast, 2s
   is already a hard failure; clone the rule pack with a tighter
   threshold (see [Threshold tuning by transport](#threshold-tuning-by-transport)
   below).
2. Correlate with `lvqr_transcode_dropped_fragments_total` if the
   ABR ladder is enabled. A full worker channel means the x264enc
   stage is back-pressuring; lower the ladder or upgrade host CPU.

### Warning p95 above 1.5s

Trigger: `LvqrSloLatencyP95High`.

p95 catches the middle-majority of subscribers. If p95 is up but
p99 is calm, the slowdown is broad rather than tail-only: check
ingest-side delays (publisher uplink, RTMP bridge fragment rate,
transcode ladder saturation).

### Info p50 above 500ms

Trigger: `LvqrSloLatencyP50High`.

Median latency elevation over 10 minutes. On LL-HLS this is rarely
urgent; on WebRTC / MoQ it is a hard SLO violation and should be
tightened via per-transport thresholds.

### No recent samples

Trigger: `LvqrSloNoRecentSamples`.

Broadcast had samples within the last 30 min but none in the last
5 min. Common causes:

1. **Publisher disconnected**. Check `/api/v1/streams` -- if the
   broadcast is no longer listed, the publisher left. The
   broadcast's HLS playlist will finalize on its own shortly.
2. **Egress drain task crashed**. Grep server logs for
   `BroadcasterHlsBridge: drain terminated (producers closed)`
   messages. If the drain terminated while a publisher is still
   active, the drain panicked or errored; look for the preceding
   error.
3. **Subscriber back-pressure stalled the broadcaster**.
   `lvqr_fragment_broadcast_lagged_skips_total` rate climbing
   alongside zero sample rate is the tell.

## Threshold tuning by transport

The defaults are tuned for LL-HLS. Other transports have tighter
SLOs. Clone the rule pack and scope each rule to a specific
transport label. The recommended thresholds:

| Transport | p50 info | p95 warning | p99 warning | p99 critical | Fire delay tier |
|---|---|---|---|---|---|
| HLS (LL) | 500 ms | 1500 ms | 2000 ms | 4000 ms | default |
| HLS (legacy, 6s seg) | 2000 ms | 6000 ms | 8000 ms | 12000 ms | one tier slower |
| DASH | 1000 ms | 3000 ms | 4000 ms | 8000 ms | default |
| WHEP (WebRTC) | 100 ms | 250 ms | 500 ms | 1000 ms | default |
| MoQ | 80 ms | 200 ms | 400 ms | 800 ms | one tier faster |
| WS (fMP4) | 300 ms | 800 ms | 1200 ms | 2500 ms | default |

To scope to a transport in the rule expression, add a matcher on
the `_bucket` selector:

```promql
histogram_quantile(
  0.99,
  sum by (broadcast, transport, le) (
    rate(lvqr_subscriber_glass_to_glass_ms_bucket{transport="whep"}[5m])
  )
)
```

## v1 limitations

* **LL-HLS (107 A) + MPEG-DASH (109 A) egress instrumentation is
  live**. WS / MoQ / WHEP still need a one-line
  `tracker.record(broadcast, transport, delta_ms)` at their
  subscriber-delivery point; those drains consume `moq_lite` frames
  rather than `Fragment` values, so the `ingest_time_ms` stamp is
  not on the wire today and wiring them up is a small design session
  (`carry ingest_time_ms on the MoQ frame header` is the canonical
  approach). The alert pack + dashboard already label-match
  generically on `transport`, so they light up automatically the
  moment a new transport starts recording samples.
* **No time-windowed retention on the admin snapshot**. The ring
  buffer is size-bounded (1024 samples per key); a quiet broadcast
  keeps stale samples until new traffic arrives. Operators who need
  time-aligned views read Prometheus directly.
* **Server-side measurement only**. True glass-to-glass requires
  browser SDK telemetry (Tier 5).
* **No admission control**. Operators react to alerts; the server
  does not refuse subscribers preemptively. "Refuse subscribers that
  would blow the budget" is research scope, not v1.

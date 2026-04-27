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
   fragment to subscribers. Four drain points are live:
   * LL-HLS -- `push_chunk_bytes` inside
     `BroadcasterHlsBridge::drain` (107 A). One sample per
     partial / segment delivery.
   * MPEG-DASH -- `push_video_segment` / `push_audio_segment`
     inside `BroadcasterDashBridge::drain` (109 A). Samples are
     per-Fragment, not per-finalized-DASH-segment, so operators
     reading the "sample rate" panel see one tick per incoming
     fragment even though DASH `$Number$` URIs address full
     segments.
   * WebSocket fMP4 relay -- an auxiliary
     `FragmentBroadcasterRegistry` subscription spawned inside
     `ws_relay_session` (110 A). The MoQ-side drain that feeds
     the wire is unchanged; the aux subscription samples
     `Fragment::ingest_time_ms` per-session so a disconnected
     subscriber does not record ghost samples.
   * WHEP RTP packetizer -- the `Str0mSessionHandle::on_raw_sample`
     -> `SessionMsg::Video` -> `write_sample` path in
     `lvqr-whep::str0m_backend` (110 B). Samples are recorded
     after `writer.write` returns `Ok(true)`; pre-Connected,
     codec-mismatch, and AAC-to-Opus drops are excluded so the
     histogram only sees real RTP packets.
   Pure MoQ subscribers drinking directly from `OriginProducer`
   stay out of scope for **server-side** measurement: there is no
   server-side drain task to hook. Client-recorded samples are
   accepted via [`POST /api/v1/slo/client-sample`](#client-pushed-samples-post-apiv1sloclient-sample)
   below; the same `LatencyTracker` rings both server-stamped and
   client-pushed entries on a shared
   `lvqr_subscriber_glass_to_glass_ms` histogram, keyed by
   `transport`. Coverage as of session 157:
   * **HLS subscribers** push by default via the
     `@lvqr/dvr-player` web component's built-in PDT-anchored
     sampler.
   * **Pure-MoQ subscribers** still cannot push: the session 157
     audit confirmed the MoQ wire has no per-frame wall-clock
     anchor a subscriber could lift. A sidecar
     `<broadcast>/0.timing` MoQ track is the v1.2 close-out plan;
     see [`tracking/SESSION_157_BRIEFING.md`](../tracking/SESSION_157_BRIEFING.md).

The histogram labels are:

| Label | Source |
|---|---|
| `broadcast` | Broadcast name (e.g. `live/demo`). One label value per live broadcast. |
| `transport` | Egress surface string (`"hls"`, `"ws"`, `"dash"`, `"moq"`, `"whep"`). Kept as a string (not an enum) so new protocols slot in without a type change. |
| `le` | Standard Prometheus histogram bucket label. |

### What it does NOT measure

* **Client render latency** (server-stamped samples only). The
  server-stamped half of the histogram measures
  `egress_emit_ms - ingest_time_ms` -- network from publisher to
  egress + server-side processing, but not the subscriber's
  network + decode + render contribution. The session 156
  follow-up shipped a complementary signal: client-pushed samples
  via [`POST /api/v1/slo/client-sample`](#client-pushed-samples-post-apiv1sloclient-sample).
  When a client (e.g. `@lvqr/dvr-player`) records and pushes
  `render_ts_ms - ingest_ts_ms`, the same histogram receives
  end-to-end glass-to-glass values keyed on the same
  `(broadcast, transport)` label pair. Operators reading the
  Grafana dashboard see both halves merged into the same
  percentile panels.
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

### Client-pushed samples (`POST /api/v1/slo/client-sample`)

Client SDKs that compute their own glass-to-glass latency on
received frames push samples to this route; the server records
each into the same `LatencyTracker` ring buffer that powers
[`/api/v1/slo`](#apiv1slo-admin-route) and the
`lvqr_subscriber_glass_to_glass_ms` histogram. Shipped in
the session 156 follow-up; closes the documented path forward
for transports the server cannot measure directly (LL-HLS / DASH
/ WS / WHEP egress is server-stamped natively, so this route is
the path for HLS subscriber-side render latency, MoQ once the
v1.2 sidecar-track lands, or any other future transport).

Request body (JSON):

```json
{
  "broadcast": "live/cam1",
  "transport": "hls",
  "ingest_ts_ms": 1714066800000,
  "render_ts_ms": 1714066800120
}
```

* `broadcast` -- non-empty, <= 256 chars. Matches the broadcast
  key used elsewhere in the admin surface.
* `transport` -- non-empty, <= 32 chars. Free-form string label;
  the recorder does not whitelist values, so new transports slot
  in without a server-side change. Use `"hls"`, `"dash"`,
  `"moq"`, `"whep"`, `"ws"` to merge with server-stamped samples
  on the same Grafana panels.
* `ingest_ts_ms` -- wall-clock UNIX-ms timestamp anchored on the
  publisher's frame. Recovery is transport-specific. HLS
  subscribers lift it from `#EXT-X-PROGRAM-DATE-TIME` via
  `HTMLMediaElement.getStartDate() + currentTime` (see the
  reference client at
  [`bindings/js/packages/dvr-player/src/slo-sampler.ts`](../bindings/js/packages/dvr-player/src/slo-sampler.ts)).
  MoQ subscribers cannot lift this from the wire today; v1.2
  sidecar-track plan in
  [`tracking/SESSION_157_BRIEFING.md`](../tracking/SESSION_157_BRIEFING.md).
* `render_ts_ms` -- wall-clock UNIX-ms timestamp the client
  recorded at frame render (typically `Date.now()` on the next
  animation frame after the video element advanced past the
  frame).

Validation (server-side):

* `render_ts_ms >= ingest_ts_ms` (negative latency is rejected
  as client clock skew).
* `render_ts_ms - ingest_ts_ms <= 300_000 ms` (5 minute cap;
  beyond is almost certainly clock skew between publisher and
  subscriber, not real latency, and would corrupt the
  percentile histogram).
* Both `broadcast` and `transport` non-empty within their length
  caps.

Response codes:

| Code | Meaning |
|---|---|
| 204 No Content | Recorded into the tracker. |
| 400 Bad Request | Validation failed (see body for which rule). |
| 401 Unauthorized | Bearer token rejected by both auth paths (admin and subscribe). |
| 503 Service Unavailable | `LatencyTracker` not configured on this server (operator built `AdminState` without `with_slo`). |

#### Authentication: dual

The route is mounted off the admin-only middleware so subscribers
can push samples for the broadcasts they're already authorized to
read, without needing to hold an admin token. The bearer token
on the `Authorization: Bearer <token>` header is checked against
two scopes in order:

1. `AuthContext::Admin { token }` -- operator scope. If the
   token is a valid admin token, the sample is accepted.
2. `AuthContext::Subscribe { token, broadcast: <body.broadcast> }`
   -- subscriber scope. If the token is a valid subscribe token
   for the broadcast in the request body, the sample is accepted.

The configured `AuthProvider`'s existing per-broadcast subscribe
logic naturally enforces "subscribers can only push samples for
broadcasts they're allowed to subscribe to," which prevents
token-laundering / sample-pollution from a subscriber pushing
samples against a broadcast it has no permission to read.

#### Reference client: `@lvqr/dvr-player`

The dvr-player web component ships with a built-in opt-in SLO
sampler. Three attributes:

```html
<lvqr-dvr-player
  src="https://relay.example.com:8080/hls/live/cam1/master.m3u8"
  token="<subscribe_token>"
  slo-sampling="enabled"
  slo-endpoint="https://relay.example.com:8080/api/v1/slo/client-sample"
  slo-sample-interval-secs="5">
</lvqr-dvr-player>
```

The sampler computes `latency_ms = Date.now() -
(videoEl.getStartDate() + videoEl.currentTime * 1000)` every
`slo-sample-interval-secs` (default 5 s) and POSTs to the
configured endpoint. Failures are silently dropped so SLO
sampling never disrupts playback. The component's existing
`token` attribute rides the dual-auth subscribe-token path.

#### Sample-rate counter

Each accepted sample increments
`lvqr_slo_client_samples_total{transport}` -- a Prometheus
counter scoped by transport label. Use it to confirm that a
deployed dvr-player fleet is actually pushing:

```promql
sum by (transport) (
  rate(lvqr_slo_client_samples_total[5m])
)
```

A non-zero rate on `transport="hls"` confirms the dvr-player
sampler fleet is healthy; zero (with a known-good fleet) points
at a network or auth misconfiguration on the relay's
client-sample route.

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

* **LL-HLS (107 A) + MPEG-DASH (109 A) + WS (110 A) + WHEP (110 B)
  egress instrumentation is live**. The fifth egress surface --
  pure MoQ subscribers drinking directly from `OriginProducer` --
  has no server-side drain task to hook. The 110 / v1.1-B scoping
  decision explicitly rejected the alternative of prefixing every
  MoQ frame payload with an 8-byte `ingest_time_ms` header,
  because `moq_lite` 0.15 frames are bare `Bytes` with no
  extension slot and a payload prefix would break every foreign
  MoQ client that decodes frames as raw CMAF bytes. The
  documented path forward shipped in the session 156 follow-up
  ([`POST /api/v1/slo/client-sample`](#client-pushed-samples-post-apiv1sloclient-sample)
  above): client-recorded samples merge into the same histogram
  via a generic transport label. HLS subscribers can already push
  by default through the `@lvqr/dvr-player` web component.
  **Pure-MoQ subscribers cannot push yet**: the session 157 audit
  confirmed the wire carries no per-frame wall-clock anchor a
  pure-MoQ client could lift -- `lvqr_fragment::MoqTrackSink::push`
  writes only the fMP4 payload bytes; the inverse `MoqGroupStream`
  emits zero `ingest_time_ms` by documented contract. The v1.2
  close-out plan is a sibling `<broadcast>/0.timing` MoQ track
  emitting `(group_id, ingest_time_ms)` anchors per keyframe;
  additive (foreign clients ignore the unknown track name), so
  the 110 / v1.1-B in-band rejection stays untouched. Sketch in
  [`tracking/SESSION_157_BRIEFING.md`](../tracking/SESSION_157_BRIEFING.md).
* **No time-windowed retention on the admin snapshot**. The ring
  buffer is size-bounded (1024 samples per key); a quiet broadcast
  keeps stale samples until new traffic arrives. Operators who need
  time-aligned views read Prometheus directly.
* **Server-side measurement is half the picture; client-pushed is
  the other half**. The histogram now ingests both halves on
  the same percentile panels: server-stamped samples
  (`ingest_time_ms -> egress_emit_ms`) and client-pushed samples
  (`ingest_ts_ms -> render_ts_ms`) merge under a shared
  `(broadcast, transport)` label pair. The HLS half is live as
  of session 156 follow-up via the `@lvqr/dvr-player` built-in
  sampler; pure-MoQ remains open as v1.2 per the bullet above.
* **No admission control**. Operators react to alerts; the server
  does not refuse subscribers preemptively. "Refuse subscribers that
  would blow the budget" is research scope, not v1.

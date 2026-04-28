# Architecture

LVQR is a 29-crate Rust workspace built around one central
claim: every track in the system is a sequence of `Fragment`
values, and every protocol the server speaks is a projection
over that shared fragment stream. The architecture is
optimised to keep that claim true as new protocols land, so
adding RTSP or WHIP does not turn into a data-plane rewrite.

This document maps the system at the level that matters for
new contributors: the unified data model, the three planes
(data / cluster / observability), the 29 crates that
implement them, and the ten load-bearing architectural
decisions you must preserve before touching cross-crate
boundaries.

## The unified Fragment model

```rust
// crates/lvqr-fragment/src/lib.rs
pub struct Fragment {
    pub track_id: TrackId,
    pub group_id: u64,
    pub object_id: u64,
    pub priority: u8,
    pub dts: i64,
    pub pts: i64,
    pub duration: u32,
    pub flags: FragmentFlags,  // keyframe / independent / discardable
    pub payload: Bytes,
}
```

Every ingest crate produces fragments; every egress crate
consumes fragments through a `FragmentObserver` /
`RawSampleObserver` tap installed on the shared
`FragmentBroadcasterRegistry`. MoQ subgroups, LL-HLS partials,
CMAF chunks, DASH segments, MoQ DVR fetch, WHEP RTP
packetization, and the redb archive index are all the same
thing addressed differently.

This is load-bearing decision #1 from the roadmap. The payoff
is visible in `lvqr-cli::start`: adding a new egress is a
~50-line bridge crate that installs an observer; adding a new
ingest is a bridge that produces fragments. No egress code
changes when a new ingest lands, and vice versa.

## Data plane (ingest → segmenter → egress)

```
  RTMP (1935)  ─┐
  WHIP  (HTTPS) ├┐
  SRT   (UDP)   ├┼─► FragmentBroadcaster ─► FragmentObserver taps
  RTSP  (TCP)   ├┘   per (broadcast, track)  ├─► MoQ relay
  WS fMP4       ┘                            ├─► LL-HLS playlist + segments
                                             ├─► DASH MPD + segments
                                             ├─► WHEP RTP packetizer
                                             ├─► WebSocket fMP4 forwarder
                                             ├─► lvqr-record (disk)
                                             └─► lvqr-archive (redb index)
```

The registry's `(broadcast, track)` keying generalises beyond the
default `"0.mp4"` (video) and `"1.mp4"` (audio) tracks. Three
sibling tracks ship today:

* **`"captions"`** -- WebVTT cues from
  `lvqr-agent-whisper::WhisperCaptionsAgent` (Tier 4 item 4.5);
  feeds the LL-HLS subtitles rendition under
  `/hls/{broadcast}/captions/playlist.m3u8`.
* **`"scte35"`** -- SCTE-35 ad markers (session 152). Producers:
  the SRT MPEG-TS demuxer (PMT stream_type 0x86 with private-
  section reassembly) and the patched RTMP onCuePoint
  scte35-bin64 path. Consumers: `BroadcasterScte35Bridge` in
  `lvqr-cli` projects each parsed event into the per-broadcast
  HLS DateRange window (`#EXT-X-DATERANGE` per HLS spec section
  4.4.5.1) and the per-broadcast DASH Period EventStream
  (`<EventStream schemeIdUri="urn:scte:scte35:2014:xml+bin">`
  per ISO/IEC 23009-1 G.7 + SCTE 214-1). Splice_info_section
  bytes flow through verbatim with CRC verification but no
  semantic interpretation. The reserved track name is exported
  as `lvqr_fragment::SCTE35_TRACK`. See [`scte35.md`](scte35.md).
* Per-broadcast / per-track dynamic surface for future agents.

Every ingest goes through `lvqr-cmaf::TrackCoalescer` for AVC
Annex-B ↔ AVCC conversion and sample boundary detection, then
through `build_moof_mdat` to produce CMAF-compliant fMP4 boxes.
One segmenter, one box writer, one set of codec parsers
(`lvqr-codec`) shared by every protocol.

LBD #6 pins this in place: the CMAF segmenter is the data-plane
root, and recording + DVR archive are sinks on the segmenter,
not separate consumers of the ingest-side bridges. That is what
kills the "recording watcher only polls the RTMP bridge and
misses WS-ingested streams" bug class the Tier 0 audit
surfaced.

## Cluster plane (chitchat)

```
  Node A                                 Node B
  ┌───────────────┐   UDP/10007 gossip   ┌───────────────┐
  │ lvqr-cluster  │ ◄─────────────────►  │ lvqr-cluster  │
  │ • members     │                      │ • members     │
  │ • broadcasts/ │                      │ • broadcasts/ │
  │   {name→node} │                      │   {name→node} │
  │ • endpoints/  │                      │ • endpoints/  │
  │   {node→urls} │                      │   {node→urls} │
  │ • capacity    │                      │ • capacity    │
  │ • config LWW  │                      │ • config LWW  │
  └───────┬───────┘                      └───────┬───────┘
          │                                      │
          ▼ HLS/DASH/RTSP request                ▼
    not owner? → 302 to endpoint-URL owned for this broadcast
```

Every node registers in chitchat on boot; membership converges
to every node seeing every other node. The first publisher for
broadcast `X` arriving on node A writes `(X → A, lease)` to
chitchat KV and renews on every fragment emitted; leases expire
if the publisher disconnects. Subscribers hitting node B for
broadcast `X` get a 302 to A's advertised URL for HLS / DASH /
RTSP. MoQ subscribers on B pull tracks directly from A.

LBD #5 is the discipline that makes chitchat safe at scale:
gossip carries *only* membership, ownership pointers, node
capacity, config, and feature flags. It does NOT carry per-
frame counters, per-subscriber bitrates, or fast-changing
state. Hot state stays node-local.

The cluster plane intentionally has no consensus layer. Two
publishers for the same broadcast name on two nodes produces
two ownership claims; LWW reconciles. Linearizability is not a
design goal this tier.

Full reference: [`docs/cluster.md`](cluster.md).

## Observability plane (OTLP + Prometheus)

```
  tracing::info_span!  ──┐
                         ├─► tracing_subscriber::registry
  metrics::counter!  ────┘  ├─ fmt layer (stdout)
                            └─ tracing_opentelemetry layer  ─► OTLP gRPC spans
                                                                    │
                            OtelMetricsRecorder  ─► SdkMeterProvider│─► OTLP gRPC metrics
                                        │
                                        ▼
                            FanoutBuilder  ◄── PrometheusRecorder  ─► /metrics scrape
```

`lvqr-observability::init` inspects `LVQR_OTLP_ENDPOINT`. When
unset, only the stdout fmt layer runs and no metrics recorder
is installed -- single-node deployments pay nothing.

When set, `init` builds both the tracer provider (spans) and
the meter provider (metrics) with a shared `Resource`
containing `service.name`, every `LVQR_OTLP_RESOURCE` `k=v`
pair, and `Sampler::TraceIdRatioBased(trace_sample_ratio)` for
head sampling. The pre-built `OtelMetricsRecorder` is stashed
on the handle; `lvqr-cli::start` pulls it off, composes it
with the Prometheus scrape recorder through
`metrics_util::FanoutBuilder`, and installs the fanout.

LBD #4 pins the split: lifecycle events go through
`tokio::sync::broadcast` (the `EventBus`); per-frame / per-byte
counters go through the `metrics` crate directly. The
observability plane reads from both; it does not re-home
either onto the bus. That is why `metrics::counter!` call
sites in `lvqr-ingest` / `lvqr-relay` / `lvqr-admin` /
`lvqr-mesh` did not change when OTLP export landed.

Full reference: [`docs/observability.md`](observability.md).

## The 29 crates

```
Data model and transport facade
  lvqr-core          -- StreamId, TrackName, Frame, EventBus, RelayStats
  lvqr-fragment      -- Fragment model, FragmentMeta, MoqTrackSink, FragmentBroadcasterRegistry
  lvqr-moq           -- facade over moq-lite (version churn isolation)

Codecs and segmenter
  lvqr-codec         -- AVC / HEVC / AAC / Opus / SCTE-35 parsers + proptest + fuzz
  lvqr-cmaf          -- RawSample coalescer, CmafPolicy, build_moof_mdat

Ingest
  lvqr-ingest        -- RTMP + FLV + RtmpMoqBridge + observer taps + AMF0 onCuePoint scte35-bin64
  lvqr-whip          -- WebRTC ingest (H.264 + HEVC + Opus via str0m)
  lvqr-srt           -- SRT-over-UDP + MPEG-TS demuxer + PMT 0x86 SCTE-35 reassembly
  lvqr-rtsp          -- RTSP/1.0 server + interleaved RTP depacketizer

Egress
  lvqr-relay         -- MoQ/QUIC relay over moq-lite, zero-copy fanout
  lvqr-hls           -- LL-HLS + MultiHlsServer + master playlist + sliding DVR window + DATERANGE
  lvqr-dash          -- MPEG-DASH + MultiDashServer + MPD lifecycle + Period EventStream
  lvqr-whep          -- WebRTC egress via str0m, RTP packetization, AAC->Opus transcoder
  lvqr-mesh          -- peer mesh topology planner (browser data plane shipped session 144; see docs/mesh.md)

Auth, storage, admin
  lvqr-auth          -- noop / static / HS256 JWT / JWKS / webhook providers + hot-reload + stream-key store
  lvqr-record        -- fMP4 disk recorder subscribed to EventBus
  lvqr-archive       -- redb segment index for DVR scrub + /playback/* + io_uring writer + C2PA finalize
  lvqr-signal        -- WebRTC signaling for mesh peer assignments + ICE server push
  lvqr-admin         -- /api/v1/*, /metrics, /healthz, /readyz, auth mw, SLO tracker

Cluster and observability
  lvqr-cluster       -- chitchat membership, ownership, capacity, config, federation
  lvqr-observability -- tracing/OTLP, metrics-crate -> OTLP bridge, Fanout to Prometheus

Programmable data plane (Tier 4)
  lvqr-wasm          -- wasmtime per-fragment filter host + chain composition + notify hot-reload
  lvqr-agent         -- in-process AI agents framework (Agent trait + AgentRunner + lifecycle)
  lvqr-agent-whisper -- WhisperCaptionsAgent (AAC -> PCM -> WebVTT cues, Tier 4 item 4.5)
  lvqr-transcode     -- GStreamer ABR ladder + SoftwareTranscoder + VideoToolbox HW + AacToOpusEncoder

Composition and test infrastructure
  lvqr-cli           -- single-binary composition root (lvqr serve)
  lvqr-conformance   -- reference fixtures + external validator wrappers (publish = false)
  lvqr-test-utils    -- TestServer harness + scte35-rtmp-push test bin (publish = false)
  lvqr-soak          -- long-run soak driver (publish = false)
```

Every crate that owns a wire format or parser ships the
5-artifact test contract (proptest + fuzz + integration +
E2E + conformance). See [`tests/CONTRACT.md`](../tests/CONTRACT.md)
for the per-crate scorecard.

## Dependency boundaries

```
                 lvqr-core ──────────────────────────────────┐
                     │                                        │
        ┌────────────┼────────────┬───────────┬───────────┐   │
        ▼            ▼            ▼           ▼           ▼   ▼
  lvqr-moq   lvqr-fragment   lvqr-codec   lvqr-auth   lvqr-signal
        │            │            │
        └────┐       ▼            ▼
             │   lvqr-cmaf ◄──────┘
             │       │
        ┌────┼───────┼───────────┬──────────┬──────────┬──────────┐
        ▼    ▼       ▼           ▼          ▼          ▼          ▼
  lvqr-relay lvqr-ingest   lvqr-hls    lvqr-dash  lvqr-whep  lvqr-whip
                │                         ...
             lvqr-srt  lvqr-rtsp
                                   lvqr-record   lvqr-archive
                                   lvqr-mesh     lvqr-admin
                                   lvqr-cluster  lvqr-observability
                                                          │
                                                          ▼
                                                      lvqr-cli
```

Key invariants the dependency graph enforces:

- **No cycles.** `lvqr-core` has zero internal deps; every
  other crate depends either on `lvqr-core` or on a protocol-
  root crate (`lvqr-fragment`, `lvqr-cmaf`, `lvqr-moq`).
- **The facade pattern for volatile deps.** `lvqr-moq` is the
  only crate that imports `moq-lite`; every other crate uses
  our newtype wrappers. Same story for codecs: call sites
  import `lvqr-codec::H264Sps`, not `h264_reader::...`.
- **Cluster and observability do not depend on each other.**
  Either can be disabled independently. Single-node,
  no-OTLP deployments pull neither into the hot path.
- **`lvqr-cli` is the only crate that ties everything
  together.** All other crates are library targets usable in
  isolation (and are, in tests).

## Control vs hot path (LBD #3)

Control-plane traits use `async-trait` -- per-connection
allocation is fine. Data-plane traits use concrete types or
enum dispatch -- no per-fragment `dyn` dispatch anywhere.

Where this matters concretely:

- `IngestProtocol` + `RelayProtocol`: control plane,
  `async-trait`, allocates once per connection.
- `FragmentObserver` + `RawSampleObserver`: hot path,
  concrete types, called per fragment. Zero heap allocation
  on the emit path.
- `AuthProvider`: control plane, `async-trait`. Auth check
  runs once at session establishment.
- MoQ track dispatch: concrete `moq_lite::TrackProducer`
  through `lvqr-moq::Track`. No trait object per track.

## Composition root (lvqr-cli)

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let mut observability = lvqr_observability::init(
        lvqr_observability::ObservabilityConfig::from_env()
    )?;
    let otel_metrics_recorder = observability.take_metrics_recorder();

    let cli = Cli::parse();
    let result = match cli {
        Cli::Serve(args) => serve_from_args(args, otel_metrics_recorder).await,
    };

    drop(observability);  // force_flush + shutdown tracer + meter
    result
}
```

`lvqr-cli::start(config)` is the single entry point that
assembles every subsystem. Listeners bind before `start`
returns; callers that pass `port: 0` read the real bound
address back off `ServerHandle`. `lvqr-test-utils::TestServer`
uses this path to spin up a full-stack instance on ephemeral
ports inside integration tests -- every E2E test in the repo
exercises the same composition root the CLI ships.

## Further reading

- [`tracking/ROADMAP.md`](../tracking/ROADMAP.md) -- the full
  18-24 month plan and the ten load-bearing architectural
  decisions in context
- [`tracking/HANDOFF.md`](../tracking/HANDOFF.md) -- rolling
  session log; the current session block is always the
  freshest state-of-the-world
- [`tests/CONTRACT.md`](../tests/CONTRACT.md) -- the 5-artifact
  test contract every wire-format crate ships with
- [`docs/cluster.md`](cluster.md) -- cluster plane reference
- [`docs/observability.md`](observability.md) -- observability
  plane reference

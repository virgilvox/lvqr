# Tier 3 Plan -- Cluster + Observability

**Status**: planning. Approved for execution in sessions 71+.
**Source of truth**: `tracking/ROADMAP.md` is the project-level
plan; this document refines Tier 3 only.

## Mission

Tier 3 converts LVQR from a single-node server into a cluster-
capable product and makes running it at scale debuggable.

Two orthogonal planes land here:

* **Cluster plane** (chitchat): membership, broadcast ownership,
  node capacity, cluster-wide config. Lets N LVQR nodes present
  as one logical service so a subscriber can hit any node and be
  routed to the owner of its broadcast.
* **Observability plane** (OTLP + structured logs): spans, metrics,
  log-trace correlation. Existing `tracing` spans + `metrics-rs`
  counters already exist throughout the codebase -- this tier
  exposes them through an OTLP endpoint so Jaeger / Tempo /
  Honeycomb / Prometheus / Grafana can ingest them.

The two planes land as separate crates (`lvqr-cluster` and
`lvqr-observability`) wired behind opt-in flags. Single-node
operation stays the default and is not allowed to regress.

## Load-bearing decisions this tier must preserve

Pulled verbatim from ROADMAP.md section "The 10 Load-Bearing
Architectural Decisions":

* **LBD #3** (control vs hot path): control plane traits use
  `async-trait`. Data plane uses concrete types or enum dispatch.
  Cluster routing RPCs are control plane; per-fragment dispatch
  to an owning node stays concrete.
* **LBD #4** (EventBus split): lifecycle events via
  `tokio::sync::broadcast`; per-frame / per-byte counters via
  `metrics-rs` directly. Observability export reads from both
  sources but does not re-home them onto the bus.
* **LBD #5** (chitchat scope discipline): gossip carries
  membership, node capacity, broadcast → owner pointers, config,
  feature flags. It does NOT carry per-frame counters, per-
  subscriber bitrates, fast-changing state. Hot state stays node-
  local; cross-node lookups use direct node-to-node RPC keyed off
  chitchat pointers.

Any change proposed below that violates one of these is a red flag
and must be re-scoped before implementation starts.

Four session-64-pinned RTSP invariants (broadcaster drain tasks
hold only `BroadcasterStream` receivers, never strong `Arc`s) are
also untouched by this tier.

## Cluster plane

### Scope (what lands)

1. **Node membership**. Each LVQR process registers itself in a
   chitchat cluster on startup. Membership gossip converges to
   every node seeing every other node.
2. **Broadcast ownership**. The first publisher for broadcast X
   arriving on node A writes `(X → A, lease=<deadline>)` to
   chitchat KV. Nodes B..N look up the owner when a subscriber
   asks for X but A has no publisher. Leases renew on each
   emitted fragment; they expire if the publisher disconnects.
3. **Capacity advertisement**. Each node gossips its CPU %,
   memory RSS, and outbound bandwidth utilization every 5 s so a
   future load-aware router can spread subscribers without
   pinging every node.
4. **Cluster-wide config**. A narrow set of feature flags (e.g.
   `hls.low-latency.enabled`, `rtsp.tcp.interleaved.enabled`)
   gossip through chitchat KV. Node-local config stays in TOML.
5. **Admin HTTP surface**. `/admin/cluster/nodes`,
   `/admin/cluster/broadcasts`, `/admin/cluster/config` read-only
   endpoints so the operator can inspect cluster state without
   SSH.

### Anti-scope (explicit rejections)

* **Per-frame counters**. Use metrics-rs directly.
* **Subscriber-level state**. Node-local, not gossipped. A
  subscriber on node A that wants broadcast X owned by B either
  (a) gets a redirect response, or (b) gets its fragments pulled
  from B by A via a direct RPC. Neither involves gossip.
* **Consensus**. chitchat is anti-entropy gossip. It is eventually
  consistent. We will NOT add Raft, a leader election, or
  distributed locks. Broadcast ownership is a lease, not a lock;
  concurrent publishers on two nodes produce two broadcasts and a
  downstream reconciliation picks one.
* **Byzantine tolerance**. Nodes trust each other; cluster auth
  is TLS mutual auth at the transport, not a crypto-signed gossip
  message.
* **Dynamic capacity routing logic**. Capacity advertisement
  lands this tier; actually using capacity to place subscribers
  is Tier 4+. The data is available; the policy is deferred.

### API sketch

```rust
// crates/lvqr-cluster/src/lib.rs
pub struct Cluster {
    node_id: NodeId,
    chitchat: ChitChat,
}

impl Cluster {
    pub async fn bootstrap(config: ClusterConfig) -> Result<Self>;

    pub fn node_id(&self) -> NodeId;

    pub fn members(&self) -> impl Iterator<Item = ClusterNode>;

    pub async fn claim_broadcast(&self, name: &str, lease: Duration) -> Result<Claim>;
    pub async fn find_broadcast_owner(&self, name: &str) -> Option<NodeId>;

    pub fn capacity_gauge(&self) -> &CapacityGauge;

    pub fn config_get(&self, key: &str) -> Option<String>;
    pub fn config_watch(&self, key: &str) -> impl Stream<Item = Option<String>>;
}

pub struct ClusterConfig {
    pub listen: SocketAddr,                 // gossip port
    pub seeds: Vec<SocketAddr>,             // bootstrap peers
    pub node_id: Option<NodeId>,            // None = random ULID
    pub gossip_interval: Duration,          // default 1s
    pub failure_detector_max_hops: usize,   // default 3
}

pub struct Claim {
    pub broadcast: String,
    pub owner: NodeId,
    pub expires_at: SystemTime,
    pub _renewer: ClaimRenewer,
}

impl Drop for Claim { /* releases the lease */ }
```

Integration surfaces:

* `lvqr-cli::serve` grows an optional `ClusterConfig`. When
  supplied, the cli constructs a `Cluster` and passes it to every
  protocol crate that needs it.
* `lvqr-ingest` crates call `cluster.claim_broadcast(...)` when
  their first fragment lands and renew on each emit.
* `lvqr-hls` / `lvqr-dash` / `lvqr-rtsp` handlers fall back to
  `cluster.find_broadcast_owner()` when the local registry has no
  broadcaster for the requested name, and emit a 302 / RTSP
  redirect to the owner node.
* `lvqr-admin` wires the read-only HTTP endpoints.

### Decomposition into sessions

Each session is scoped so the workspace stays green at the end.

| # | Session | Deliverable | Verification | Status |
|---|---|---|---|---|
| A | 71 | `crates/lvqr-cluster/` scaffold; chitchat dep pinning; `Cluster::bootstrap` on a single node. | `cargo test -p lvqr-cluster --lib` | DONE |
| B | 72 | Two-node integration test: both see each other in `members()`. | Integration test with two `Cluster` instances on ephemeral ports. | DONE |
| C | 73 | Broadcast ownership KV. `claim_broadcast` + `find_broadcast_owner`; lease expiry. | Integration test: node A claims, node B sees it, A drops claim, B no longer sees it after expiry. | DONE |
| D | 74 | Capacity advertisement. CPU / RSS / bandwidth counters published to chitchat every 5 s. | Integration test reads `members()` and asserts capacity fields populate. | DONE |
| E | 75 | Cluster-wide config channel. Read-only HTTP endpoints under `/admin/cluster`. | Integration test + admin GET. | DONE |
| F1 | 76 | Endpoints KV + `OwnerResolver` mechanism on `lvqr-hls`. Library plumbing. | Unit tests prove resolver→302 via axum router. | DONE |
| F2a | 77 | Wire `Cluster` through `lvqr-cli::serve` + HLS redirect-to-owner. | Two in-process `start()` servers over real UDP loopback; HLS 302. | DONE |
| F2b | 78 | DASH + RTSP redirect-to-owner; DASH + RTSP e2e. | In-process two-node 302 for DASH + RTSP. | DONE |
| F2c | 79 | Ingest auto-claim on first broadcast. Publishers no longer need manual `claim_broadcast`. | Two-cluster test: registry `get_or_create` fires auto-claim; peer converges. Dedup across tracks. Release on broadcaster close. | DONE |

Nine sessions total for the cluster plane (the original session F row expanded
into F1+F2a+F2b+F2c as the surface area emerged across sessions 76-79).

### Cluster plane is complete

As of session 79 (F2c) the cluster plane carries every deliverable in the
[Scope](#scope-what-lands) list:

* Node membership ✓ (A + B)
* Broadcast ownership ✓ (C) + auto-claim on publish ✓ (F2c)
* Capacity advertisement ✓ (D)
* Cluster-wide config ✓ (E)
* Admin HTTP surface ✓ (E)
* `lvqr-cli` integration + HLS/DASH/RTSP redirect-to-owner ✓ (F2a + F2b)

The ffmpeg-subprocess full-stack e2e is deferred. The in-process two-node
redirect tests for each egress protocol + the auto-claim integration test
cover the same wire path end-to-end without the CI-flakiness of an
external binary dependence. A future session can add a subprocess test
if marketing demands a shareable demo script.

### Risks + mitigations

* **chitchat API churn**. Pin the git SHA. Upstream Quickwit has
  historically kept it stable but a hidden break would ripple.
  Mitigation: write a thin `lvqr-cluster::chitchat_adapter` that
  re-exports the handful of types we actually consume.
* **Gossip flood at high node counts**. Tier 3 targets 3-10 nodes.
  Beyond that, chitchat's anti-entropy footprint may need tuning.
  Explicit non-goal this tier: 100-node clusters.
* **Brain-split on broadcast ownership**. Eventual consistency
  means two publishers for the same broadcast may both land on
  different nodes for ~1 gossip round. Document this as an
  acceptable race; downstream reconciliation is a Tier 4 item if
  anyone actually hits it.

## Observability plane

### Scope (what lands)

1. **OTLP span export**. Existing `tracing::info!` / `debug!` /
   `tracing::instrument` spans already pepper every crate. Wire
   them through `tracing-opentelemetry` so when
   `LVQR_OTLP_ENDPOINT` is set the spans flow to an OTLP gRPC or
   HTTP endpoint.
2. **OTLP metric export**. The `metrics-rs` macros scattered
   across the codebase (e.g. `metrics::counter!("fragments_emitted")`)
   already feed a `metrics-exporter-prometheus` endpoint. Add a
   second exporter so the same counters flow out via OTLP.
3. **Structured JSON logs with trace_id correlation**. When a
   span is active, emitted log lines carry its `trace_id` /
   `span_id` so Loki / Promtail can cross-reference with
   distributed traces.
4. **OpenMetrics scrape endpoint exposure check**. The Prometheus
   exporter is already wired; this tier adds a minimal test
   asserting it produces parseable output (regression guard).

### Anti-scope

* **New instrumentation**. The existing spans + counters are the
  emit surface. If Tier 3 finds a missing span at a critical
  boundary we add it, but broad instrumentation work is Tier 4.
* **Custom UI**. Jaeger / Tempo / Grafana are the consumers. LVQR
  does not render traces or metrics.
* **Sampling logic**. Default head-based sampling (100 % or
  configurable rate). Tail sampling is deferred.
* **Wire protocols other than OTLP**. No Zipkin, no OpenCensus,
  no raw Jaeger. OTLP is the canonical protocol as of
  OpenTelemetry 1.0 and everything else supports it.

### API sketch

```rust
// crates/lvqr-observability/src/lib.rs
pub struct ObservabilityConfig {
    pub otlp_endpoint: Option<String>,     // "http://localhost:4317" etc.
    pub service_name: String,              // default "lvqr"
    pub resource_attributes: Vec<(String, String)>,
    pub json_logs: bool,                   // default true in prod, false in dev
    pub trace_sample_ratio: f64,           // default 1.0
}

impl ObservabilityConfig {
    pub fn from_env() -> Self;             // honour LVQR_OTLP_ENDPOINT etc.
}

pub struct ObservabilityHandle {
    _guard: Option<tracing_appender::WorkerGuard>,
    _tracer_provider: opentelemetry_sdk::trace::TracerProvider,
    _meter_provider: opentelemetry_sdk::metrics::SdkMeterProvider,
}

pub fn init(config: ObservabilityConfig) -> Result<ObservabilityHandle>;
```

Integration surface:

* `lvqr-cli::start` calls `lvqr_observability::init(
  ObservabilityConfig::from_env())` at the top of `main`, before
  any other init runs. The returned handle is held for the
  lifetime of the process so the exporter's background flusher
  does not leak.
* `lvqr-test-utils::init_test_tracing` is left untouched; tests
  keep the stdout subscriber and skip OTLP entirely.

### Decomposition into sessions

| # | Session | Deliverable | Verification | Status |
|---|---|---|---|---|
| G | 80 | `crates/lvqr-observability/` scaffold. `ObservabilityConfig::from_env` + stdout fmt layer. Wire into `lvqr-cli`. | Regression: existing tests unchanged; start logs still render. | DONE |
| H | 81 | OTLP span exporter. When `LVQR_OTLP_ENDPOINT` is set, spans flow out. | Integration test: in-memory `SpanExporter` captures a synthetic `tracing::info_span!` through the `tracing_opentelemetry` layer; `TraceIdRatioBased(0.0)` regression guard. | DONE |
| I | 82 | OTLP metric exporter + `metrics` crate bridge. `metrics::counter!` / `gauge!` / `histogram!` call sites flow out OTLP, fanouted with the existing Prometheus exporter via `metrics_util::FanoutBuilder` when both are enabled. | Integration test: in-memory `PushMetricExporter` captures two counter increments that sum to the expected total; label attributes propagate; `set` on gauges converges via delta updates. | DONE |
| J | 83 | JSON log + trace_id correlation. Custom `tracing_subscriber::fmt::format::FormatEvent` impl (`CorrelatedFormat`) that toggles pretty/JSON via an internal `Mode` enum and injects `trace_id` + `span_id` fields on every event inside a tracing-opentelemetry-bound span (reading `OtelData` off the span ref because the re-entrance guard nulls `Span::current()` inside `format_event`). | Integration test captures a JSON event through a `MakeWriter` shim, parses it, and asserts `trace_id` + `span_id` match the parent `SpanContext`; regression guard for no-span case; `#[tracing::Instrument]` async across-await case. | DONE |

Four sessions for the observability plane (G-J landed across sessions 80-83). **Tier 3 is now COMPLETE.**

## Total Tier 3 ETA

* Cluster plane: 6 sessions (A-F).
* Observability plane: 4 sessions (G-J).
* Buffer for integration + carry-over: 2 sessions.
* **Total: 12 sessions.** At the observed 10-15 sessions / calendar
  week velocity this is ~1 calendar week of focused work.

## Verification plan

Per tier:

1. Two-node cluster integration test under `crates/lvqr-cluster/tests/`.
2. Three-node `lvqr-soak` variant exercising the cross-node
   subscriber path. `lvqr-soak` gains a `--cluster-peers N`
   flag that spawns an in-process mini cluster.
3. End-to-end OTLP span capture test under
   `crates/lvqr-observability/tests/`. Uses the
   `opentelemetry-stdout` exporter + captured stdout for
   assertions; avoids network dependency.
4. Full `cargo test --workspace` green at every session close.

## Exit criteria (when is Tier 3 done)

* A 3-node LVQR cluster shares broadcasts: publishing on one node
  produces playback on all three via owner redirects.
* `LVQR_OTLP_ENDPOINT=http://localhost:4317 lvqr serve` emits
  spans visible in a local Jaeger instance.
* `/admin/cluster/nodes` and `/admin/cluster/broadcasts` return
  JSON describing the cluster state.
* Prometheus scrape endpoint produces parseable output unchanged
  from Tier 2.
* `cargo test --workspace` remains green.
* `HANDOFF.md` documents the cluster + observability surface as
  feature-complete.

## What Tier 4 unlocks once Tier 3 ships

Tier 4 (differentiators) depends on Tier 3 surfaces:

* **WASM per-fragment filters** need the fragment broadcaster
  path already in place (done in Tier 2.1) and an observability
  hook to report filter-execution latency (Tier 3, session I).
* **In-process AI agents** consume cluster-wide config
  (Tier 3, session E) so an agent can be enabled / disabled
  across the cluster.
* **Kubernetes operator** orchestrates cluster membership by
  flipping chitchat seeds (Tier 3, sessions A + F).

None of Tier 4 is blocked on a design decision; everything hinges
on the cluster + observability plumbing described above.

## Dependencies to pin

Target versions (to verify before session 71 starts):

| Crate | Version | Notes |
|---|---|---|
| `chitchat` | latest 0.x | Quickwit-maintained; pin to a SHA once the API shape is verified. |
| `opentelemetry` | 0.27 | Current major as of session 70 planning. |
| `opentelemetry_sdk` | 0.27 | Matches `opentelemetry`. |
| `opentelemetry-otlp` | 0.27 | gRPC exporter uses `tonic`. |
| `tracing-opentelemetry` | 0.28 | One-version ahead of core per their release cadence. |

Pin exact versions in `Cargo.toml` workspace deps. Any upgrade
gets its own session.

## Non-goals (explicit)

* **Multi-region deploys**. Tier 3 targets a single region / LAN.
  Cross-region gossip tuning is Tier 4.
* **Erasure-coded fragment replication**. One broadcast = one
  owner. Re-publishing on a backup node happens at the publisher
  layer, not the cluster layer.
* **Auto-scaling**. Capacity data is advertised; acting on it is
  a future tier.
* **Admin write APIs**. `/admin/cluster/*` endpoints are read-only
  this tier.

## Resolved questions (session 71, before code)

1. **Gossip port default = UDP/10007** (match the chitchat
   examples). LVQR's existing listeners cover: RTMP 1935, RTSP
   554, HLS 80/443, DASH 80/443, WHEP 80/443, WHIP 80/443,
   QUIC/MoQ 443, SRT 8890, admin 8080. None collide with 10007.
   The TCP interleaved / QUIC datagrams are all streams; 10007
   is UDP-only so no collision by protocol either.
2. **Broadcast-ownership lease = 10 s with 2.5 s renew interval**
   (4× renew-to-expire ratio). 10 s bounds the brain-split
   window on owner crash to ~10 s, which is tolerable for a
   cluster that explicitly chose eventual consistency. 2.5 s
   renewals survive one gossip round miss (default gossip
   interval = 1 s) and still leave 7.5 s of slack before
   expiry. Rationale: matches the "lease > 3× renew > gossip"
   rule of thumb from the Cassandra / Riak playbooks.
3. **OTLP default = gRPC on :4317** per the OpenTelemetry
   specification's recommendation. HTTP protobuf on :4318 is
   enabled via an explicit `LVQR_OTLP_PROTOCOL=http/protobuf`
   override; we do NOT support `http/json` (per spec it is
   optional and no major collector mandates it).
4. **`lvqr-mesh` vs `lvqr-cluster` orthogonality**: `lvqr-mesh`
   is browser-facing WebRTC peer-mesh signaling (clients
   federating to offload video from the server). `lvqr-cluster`
   is server-facing node coordination (nodes federating so a
   single logical service can span processes). Zero API
   overlap; neither crate depends on the other. A deployed
   LVQR node can run both, one, or neither. The `lvqr-cluster`
   crate docs call this out at the top to prevent future
   confusion.

# Cluster plane

LVQR's cluster plane turns N nodes into one logical live video
service. A subscriber hits any node for any broadcast; if that
node does not own the broadcast, it replies with a 302 to the
owner's advertised URL for HLS, DASH, or RTSP. MoQ subscribers
pull tracks directly from the owner over QUIC.

The cluster plane is optional and feature-gated. Single-node
deployments do not pay for it.

## Architecture

```
    Node A                                        Node B
  ┌───────────────┐    UDP/10007 gossip         ┌───────────────┐
  │ lvqr-cluster  │◄──────────────────────────► │ lvqr-cluster  │
  │               │        chitchat             │               │
  │ KV state:     │    anti-entropy             │ KV state:     │
  │  • members    │                             │  • members    │
  │  • broadcasts │                             │  • broadcasts │
  │  • endpoints  │                             │  • endpoints  │
  │  • capacity   │                             │  • capacity   │
  │  • config LWW │                             │  • config LWW │
  └───────┬───────┘                             └───────┬───────┘
          │                                             │
          ▼                                             ▼
   publisher P → A                              subscriber S → B
   auto-claim X→A                               lookup broadcast "X"
   renew lease every 2.5 s                      → owned by A
                                                → 302 to A's
                                                  advertise-hls URL
```

Built on [chitchat](https://github.com/quickwit-oss/chitchat),
the Quickwit-maintained gossip library. LVQR pins it via the
workspace dep and re-exports only the handful of types used,
so upstream API churn touches one crate.

## Why gossip, not consensus

The cluster plane is eventually consistent by design.
Broadcast ownership is a *lease*, not a lock; two publishers
for the same broadcast name on two nodes produce two
ownership claims for about one gossip round, then LWW picks
one. This is acceptable for a live video server: the race
window (~1 s) is dominated by the publisher's reconnect
jitter.

LVQR will **not** add Raft, a leader election, or a
distributed lock service. Linearizability is not a design
goal. If a future feature genuinely needs it (it won't; every
use case I've seen walks back to "a lease is fine"), that
would be a separate plane, not a chitchat extension.

Full context: load-bearing decision #5 in
[`tracking/ROADMAP.md`](../tracking/ROADMAP.md).

## What gossip carries

Gossip is narrow on purpose (LBD #5). The full KV schema:

| Key | Value | Consistency | Size |
|---|---|---|---|
| `nodes/<id>` | `{ cluster_id, version, uptime }` | membership | ~100 B |
| `broadcasts/<name>` | `{ owner, lease_expires_at }` | LWW, lease-based | ~80 B |
| `endpoints/<node_id>` | `{ hls, dash, rtsp, moq }` | LWW | ~200 B |
| `capacity/<node_id>` | `{ cpu_pct, mem_rss, bw_out_pct }` | 5 s gossip | ~50 B |
| `config/<key>` | `String` | LWW | bounded by operator |

Anti-pattern, explicitly rejected: per-frame counters,
per-subscriber bitrates, anything that changes per second.
Those stay node-local and are scraped via Prometheus /
OTLP. See [observability](observability.md).

## Configuration

```bash
lvqr serve \
  --cluster-listen 10.0.0.1:10007 \
  --cluster-seeds 10.0.0.2:10007,10.0.0.3:10007 \
  --cluster-node-id lvqr-edge-01 \
  --cluster-id prod-us-east-1 \
  --cluster-advertise-hls http://10.0.0.1:8888 \
  --cluster-advertise-dash http://10.0.0.1:8889 \
  --cluster-advertise-rtsp rtsp://10.0.0.1:8554
```

| Flag | Env | Default | Notes |
|---|---|---|---|
| `--cluster-listen` | `LVQR_CLUSTER_LISTEN` | unset (single-node) | UDP bind for chitchat |
| `--cluster-seeds` | `LVQR_CLUSTER_SEEDS` | `[]` | comma-separated ip:port |
| `--cluster-node-id` | `LVQR_CLUSTER_NODE_ID` | random `lvqr-<16 alnum>` | stable id across restarts |
| `--cluster-id` | `LVQR_CLUSTER_ID` | `"lvqr"` | isolates parallel clusters |
| `--cluster-advertise-hls` | `LVQR_CLUSTER_ADVERTISE_HLS` | unset | 302 target for HLS peers |
| `--cluster-advertise-dash` | `LVQR_CLUSTER_ADVERTISE_DASH` | unset | 302 target for DASH peers |
| `--cluster-advertise-rtsp` | `LVQR_CLUSTER_ADVERTISE_RTSP` | unset | 302 target for RTSP peers |

Sizing tips:
- **Cluster id.** Two LVQR deployments on the same subnet can
  stay isolated by running with different `--cluster-id`
  values. Gossip drops SYNs with a mismatched tag.
- **Seed list.** Any subset of peers works; chitchat only
  needs one reachable seed at bootstrap. List 2-3 for
  resilience during rolling upgrades.
- **Advertise URLs.** Must be reachable by peer subscribers,
  not just by peer nodes. Behind a reverse proxy, advertise
  the externally-visible URL.

## Broadcast ownership lease

```
     publisher connects to node A
         │
         ▼
     registry.get_or_create("live/demo")
         │  (lvqr-fragment FragmentBroadcasterRegistry hook)
         ▼
     install_cluster_claim_bridge fires on_entry_created
         │
         ▼
     Cluster::claim_broadcast("live/demo", lease=10s)
         │
         ▼
     chitchat write: broadcasts/live/demo → { owner=A, expires=T+10s }
         │
         ▼
     ClaimRenewer: every 2.5 s, if broadcast still active, renew
         │
         └─ drop → tombstone → peers see "not owned" next round
```

Defaults:
- `DEFAULT_CLAIM_LEASE = 10s`
- renewal interval = 2.5 s (4× renewal rate)
- gossip interval = 1 s

This matches the "lease > 3× renew > gossip" rule-of-thumb
from the Cassandra / Riak playbooks. Under clean operation,
the owner renews 4 times before the lease would expire.
Under a node crash, the lease stales after 10 s; any peer
can claim next.

Dedup across tracks: video and audio tracks for the same
broadcast produce one claim, not two. Auto-claim deduplicates
by broadcast name at the registry-hook layer.

Release: dropping the `Claim` (triggered when the
`BroadcasterStream` closes -- session-64 invariant) fires the
renewer's oneshot stop channel which tombstones the KV entry
so peers see the broadcast freed within one gossip round.

## Redirect-to-owner

When `subscriber S` hits node B for `/hls/live/demo/...`:

1. B's HLS handler inspects `FragmentBroadcasterRegistry`
   for a local broadcaster. Found → serve locally.
2. Not found → `cluster.find_broadcast_owner("live/demo")`.
3. Owner `A` resolved → `cluster.node_endpoints(A)`.
4. B replies 302 with `Location:
   http://a.advertised:8888/hls/live/demo/playlist.m3u8`.
5. Not found → 404.

Same shape for DASH (via
`lvqr_dash::MultiDashServer::with_owner_resolver`) and RTSP
(via `RtspServer::with_owner_resolver`). MoQ subscribers do
not 302; they open a direct MoQ session to the owner node.

## Admin routes

```bash
# Every known node + capacity
curl http://10.0.0.1:8080/api/v1/cluster/nodes

# Every known broadcast + owner
curl http://10.0.0.1:8080/api/v1/cluster/broadcasts

# Cluster-wide config keys
curl http://10.0.0.1:8080/api/v1/cluster/config
```

These are read-only. Write APIs are deferred to a future
tier; operators push config by restarting a node with a
different flag today.

## Upgrade / node replacement

1. **Drain.** Stop publishing to the node: redirect
   publishers to a peer (at the load balancer or at the
   publisher application). Node keeps serving existing
   subscribers while leases drain.
2. **Wait.** Broadcast leases expire inside 10 s after the
   last publisher disconnects. Chitchat membership marks the
   node as suspect when it misses gossip rounds.
3. **Terminate.** `SIGINT` → `ServerHandle::shutdown` awaits
   subsystem drain, tombstones cluster KV entries, exits
   cleanly.
4. **Replace.** New node bootstraps with the same
   `--cluster-id` and seeds; chitchat converges inside one
   gossip round.

Rolling upgrades: do one node at a time. Chitchat is stable
across LVQR versions as long as the workspace pin does not
change. Bumping the chitchat dep is always a separate
session with a two-node sanity test.

## Failure modes

- **Seed unreachable at bootstrap.** Node starts standalone;
  periodic reseed attempts reconnect if the seed appears
  later. Operator-visible: `/api/v1/cluster/nodes` shows
  only the local node.
- **Partition.** chitchat survives partition; both halves
  continue serving. On heal, ownership converges via LWW;
  the race window is ~1 s.
- **Clock skew.** Leases are derived from each node's local
  clock. Skew between nodes > the gossip interval can cause
  spurious lease staling. Run NTP.
- **Split-brain ownership.** Two publishers on two nodes
  for the same broadcast name land two claims for ~1 s.
  LWW picks one; the other's claim is overwritten. Consumer
  impact: subscribers on the losing node's subscribers
  briefly see no data before the redirect converges. Acceptable
  for live video; not acceptable if you need linearizable
  writes.

## What's deferred

- **ffmpeg-subprocess full-stack e2e.** In-process two-node
  integration tests exercise every wire path for every
  protocol. A subprocess test covers operational glue
  (binary path, external ffmpeg availability) that is mostly
  environment noise. Revisit when a shareable demo script
  is needed.
- **Cross-cluster federation.** Tier 4 per the roadmap.
  Unidirectional MoQ track forwarding between two clusters
  over an authenticated QUIC link. Three-week MVP-capped.
- **Capacity-aware subscriber placement.** Capacity data is
  gossiped today; acting on it to route subscribers is
  Tier 4+. The data is available; the policy is deferred.

## Internals

- `lvqr-cluster::Cluster::bootstrap(config)` -- constructs
  the chitchat node and starts the gossip task.
- `Cluster::claim_broadcast(name, lease)` -- writes the
  ownership KV and spawns a `ClaimRenewer`.
- `Cluster::find_broadcast_owner(name)` -- reads the KV.
- `Cluster::node_endpoints(id)` -- reads `endpoints/<id>`.
- `lvqr-cli::cluster_claim::install_cluster_claim_bridge`
  -- the bridge that crosses `lvqr-fragment` (registry) and
  `lvqr-cluster` (gossip) without either crate depending
  on the other.

Cross-crate glue lives in `lvqr-cli` by design. `lvqr-fragment`
and `lvqr-cluster` have zero deps on each other.

## Further reading

- [architecture](architecture.md) -- cluster plane in context
  of the full 25-crate workspace and the ten LBDs.
- [deployment](deployment.md) -- firewall, systemd, and
  rolling-upgrade recipes.
- [observability](observability.md) -- per-node metrics and
  OTLP export; cluster-wide rollup via a Prometheus / Tempo
  federation.
- [`tracking/TIER_3_PLAN.md`](../tracking/TIER_3_PLAN.md) --
  per-session cluster plane decomposition (A through F2c)
  and load-bearing decisions this tier preserves.

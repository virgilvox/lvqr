# Peer Mesh

The peer mesh is LVQR's planned bandwidth multiplier. Viewers
are meant to relay media to other viewers via WebRTC
DataChannels, offloading the bulk of server bandwidth for
high-fan-out broadcasts.

> **Status as of main (post v0.4.0): IMPLEMENTED.** Topology
> planner + signaling + subscribe-auth + server-side subscriber
> registration + client-side WebRTC relay + two-peer
> DataChannel E2E + actual-vs-intended offload reporting +
> three-peer Playwright matrix + TURN deployment recipe with
> server-driven ICE config + per-peer capacity advertisement
> all ship. The Rust
> side (`lvqr-mesh`, `lvqr-signal`) assigns peer positions in a
> relay tree, detects dead peers, reassigns orphans, pushes
> `AssignParent` messages over `/signal` at Register time, and
> gates `/signal` behind the shared `SubscribeAuth` provider via
> both `Sec-WebSocket-Protocol: lvqr.bearer.<token>` and
> `?token=<token>`. Every `ws_relay_session` registers its
> subscriber with `MeshCoordinator::add_peer` on connect and
> sends a leading `peer_assignment` JSON text frame on the WS
> so the client learns its server-generated peer_id +
> role + parent_id + depth. The JavaScript side (`@lvqr/core`
> `MeshPeer` class at
> `bindings/js/packages/core/src/mesh.ts`) connects to
> `/signal`, handles `AssignParent`, opens an
> `RTCPeerConnection` to the assigned parent, sets up a
> DataChannel, and forwards received media frames to children.
> A root peer exposes `pushFrame(data)` so the integrator can
> seed the mesh with bytes it drained from the server (MoQ,
> WebTransport, WS relay, or any other egress). A two-browser
> Playwright E2E
> ([`bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts`](../bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts))
> drives the full shape: `lvqr serve --mesh-enabled
> --mesh-root-peer-count 1` is booted by Playwright's
> `webServer`, two browser contexts register as `peer-one`
> (Root) and `peer-two` (Relay with parent=peer-one), the SDP
> offer/answer and ICE candidates flow through `/signal`, the
> DataChannel opens, and `peer-one.pushFrame(knownBytes)` is
> observed in `peer-two`'s `onFrame` callback. Completes in
> under a second on loopback.
>
> **Per-peer capacity advertisement** shipped in session 144.
> Browser peers can self-report a static `capacity` field on the
> `Register` signal message naming the maximum children they are
> willing to relay to. The server clamps the claim to the
> operator-configured `--max-peers` ceiling and threads it into
> the topology planner so a low-bandwidth peer (e.g. a known-
> mobile profile) can advertise `capacity: 0` or `capacity: 1`
> and the planner descends past it instead of over-loading it.
> `GET /api/v1/mesh` surfaces the per-peer `capacity` alongside
> the existing `intended_children` and `forwarded_frames` columns
> for dashboard visibility. See the "Per-peer capacity (session
> 144)" block below.
>
> **Actual-vs-intended offload reporting** shipped in session 141.
> Browser peers report their cumulative forwarded-frame count to
> the server every second via a new `ForwardReport` signal
> message; `GET /api/v1/mesh` surfaces the counts alongside the
> topology planner's intended `children` count in a new `peers`
> array. Operators can now compare "how many children did the
> planner assign to peer X" against "how many frames has peer X
> actually forwarded". See the "Per-peer offload snapshot" block
> below.
>
> **TURN deployment recipe + server-driven ICE config** shipped
> in session 143. New `--mesh-ice-servers <JSON>` CLI flag (env
> `LVQR_MESH_ICE_SERVERS`) accepts an array of `RTCIceServer`
> objects; the list flows down to every browser peer via a new
> `ice_servers` field on the existing `AssignParent` message,
> and `MeshPeer` rebuilds its `RTCPeerConnection({ iceServers })`
> from the snapshot when non-empty. Operators configure once on
> the server; clients pick up STUN/TURN entries automatically.
> Empty (default) preserves the constructor-provided fallback,
> so existing integrators see no behavior change. The runbook +
> sample `coturn.conf` ship in [`deploy/turn/`](../deploy/turn/).
>
> **Three-peer Playwright matrix** shipped in session 142
> (`bindings/js/tests/e2e/mesh/three-peer-chain.spec.ts`). Three
> Chromium browser contexts form a depth-2 chain
> (peer-1 -> peer-2 -> peer-3) and the test asserts both byte-for-
> byte frame delivery at the leaf AND the per-peer offload-report
> shape across the chain. The middle peer's `forwarded_frames`
> counter is the load-bearing signal: a single-hop test cannot
> distinguish "received-then-forwarded" from "received-only", so
> the depth-2 case is what proves session 141's reporting works
> on real multi-hop topologies. Browser matrix beyond Chromium +
> the `--features` cell sweep across WebRTC-heavy engines remain
> v1.2 candidates.
>
> A deployment that sets `--mesh-enabled` and pushes media into
> root peers via `MeshPeer.pushFrame` realizes the planner's
> `offload_percentage` once those root peers complete their
> WebRTC handshakes with their assigned children. The
> `offload_percentage` field on `/api/v1/mesh` is the planner's
> intended offload based on tree shape; per-peer
> `forwarded_frames` is the actual measured signal.
>
> Tracking in
> [`tracking/PLAN_V1.1.md`](../tracking/PLAN_V1.1.md) under
> "Peer mesh data plane". Separate from WASM per-fragment
> filters and the AI-agent work.

## How the topology planner works

1. The first N viewers connect directly to the server (root peers, default N=30)
2. Subsequent viewers are assigned a parent peer in the relay tree
3. Each peer relays to up to 3 children (configurable)
4. The tree self-balances: shallowest depth first, then fewest children

```
Server
  |-- Root Peer A
  |     |-- Peer D
  |     |-- Peer E
  |     |     |-- Peer H
  |     |     |-- Peer I
  |     |-- Peer F
  |
  |-- Root Peer B
  |     |-- Peer G
  ...
```

## Enable

```bash
lvqr serve --mesh-enabled --max-peers 3
```

This starts:
- The mesh coordinator (manages tree topology)
- The signaling server (WebSocket on `/signal` endpoint)

## Configuration

| Flag | Default | Description |
|------|---------|-------------|
| `--mesh-enabled` | false | Enable peer mesh relay |
| `--max-peers` | 3 | Max children per peer |
| `--mesh-root-peer-count` | 30 | Cap on direct-from-origin peers |
| `--mesh-ice-servers` | none | JSON array of `RTCIceServer` entries pushed to every browser peer via `AssignParent` (session 143) |

Internal defaults (hardcoded, will be configurable):
- Max tree depth: 6
- Heartbeat timeout: 10s

## TURN / STUN configuration (session 143)

Two ways to give browser peers their ICE-server list:

1. **Constructor-provided** (legacy / per-integrator). Pass
   `iceServers` to `new MeshPeer({...})`. Always works; no
   server config required. Each integrator threads the list
   through their own code.

2. **Server-driven** (preferred for operator deployments). Boot
   `lvqr serve` with `--mesh-ice-servers '[...]'`; every peer
   that registers on `/signal` receives the list via a new
   `ice_servers` field on the `AssignParent` server-push
   message and rebuilds its `RTCPeerConnection({ iceServers })`
   from the snapshot. Operators configure once on the server;
   clients pick up STUN/TURN entries automatically.

```sh
lvqr serve --mesh-enabled --mesh-ice-servers '[
  {"urls":["stun:stun.l.google.com:19302"]},
  {"urls":["turn:turn.example.com:3478"],"username":"u","credential":"p"}
]'
```

The shape is WebRTC's `RTCIceServer` JSON verbatim. Each entry
must carry `urls` (a string array), and TURN entries also carry
`username` + `credential`. `LVQR_MESH_ICE_SERVERS` is the env-
variable equivalent for systemd / docker units.

Empty (default) means "no opinion": `MeshPeer` uses its
constructor-provided list (or its hardcoded Google STUN
fallback). When `--mesh-ice-servers` is set, the server's list
is **authoritative** -- it replaces whatever the constructor
provided.

### When you need TURN

Most home and office deployments work with STUN-only ICE.
Symmetric NAT (carrier-grade NAT, some corporate firewalls)
allocates a different external port per destination, defeating
server-reflexive candidates. Add a TURN server in those cases.

The operator runbook + a minimal `coturn.conf` ship in
[`deploy/turn/`](../deploy/turn/) and walk through install,
config, the `--mesh-ice-servers` wiring, sanity-check via
`turnutils_uclient`, and the cost shape (TURN traffic flows
through the relay's NIC; plan capacity).

## Bandwidth offload (intended, not measured)

Once the Tier 4 data-plane work lands, the tree shape above
should approximate the following offload characteristics on
a 1 Gbps server at 4 Mbps per stream:

| Viewers | Server bandwidth | Mesh bandwidth | Server offload |
|---|---|---|---|
| 30 | 120 Mbps | 0 | 0% |
| 100 | 120 Mbps | 280 Mbps | 70% |
| 500 | 120 Mbps | 1,880 Mbps | 94% |
| 2000 | 120 Mbps | 7,880 Mbps | 98.5% |

The server would only directly serve ~30 root peers. The rest
would be served by the mesh. Today this is not yet achieved:
the planner builds the tree but media still flows through
server-backed egress.

## Reliability

- Each peer maintains connections to 2-3 parents
- If a parent disconnects, orphaned children are reassigned within seconds
- Dead peers detected via heartbeat timeout
- Tree depth limited to prevent latency accumulation

## Signaling Protocol

Peers connect to the `/signal` WebSocket endpoint and exchange:

```json
{"type": "Register", "peer_id": "abc123", "track": "live/stream"}
{"type": "Offer", "from": "abc123", "to": "def456", "sdp": "..."}
{"type": "Answer", "from": "def456", "to": "abc123", "sdp": "..."}
{"type": "IceCandidate", "from": "abc123", "to": "def456", "candidate": "..."}
{"type": "AssignParent", "parent_id": "def456", "depth": 1}
{"type": "PeerLeft", "peer_id": "abc123"}
```

### Authentication

`/signal` participates in the shared subscribe-auth pipeline as
of session 111-B1. Clients pass the bearer via either of two
channels (in preference order):

1. `Sec-WebSocket-Protocol: lvqr.bearer.<subscribe-token>`
   header. Matches the bearer transport used by `/ws/*`. The
   `lvqr-signal` handler echoes the offered subprotocol back
   on the upgrade response (session 111-B3) so RFC 6455-strict
   clients accept the handshake.
2. `?token=<subscribe-token>` query parameter. Fallback for
   clients that cannot set request headers on the initial
   WebSocket upgrade (older WebViews, some Python clients).

The token is checked against the configured `SubscribeAuth`
provider before the WebSocket upgrade completes. Noop-provider
deployments see no behavior change. Configured deployments
(static subscribe token or JWT) short-circuit with a 401 on
any upgrade without a valid bearer.

The `--no-auth-signal` CLI flag (and the
`TestServerConfig::without_signal_auth()` builder) disables the
gate for deployments that want open signaling with auth scoped
elsewhere.

## MoQ-over-DataChannel wire format (v1)

Once the data plane lands (session 111-B2 and later), media
frames forwarded between peers over WebRTC DataChannels use
the following framing:

```
[ 8-byte big-endian object_id ][ raw MoQ frame bytes ]
```

Each DataChannel message carries exactly one MoQ frame. The
8-byte big-endian `object_id` prefix is the MoQ track-scoped
object identifier the sender's producer emitted; receivers
strip the prefix, use the `object_id` for gap detection and
skip/reconnect reconciliation, and forward the raw bytes to
their MSE pipeline.

Design rationale:

- **Server-authoritative ordering.** The `object_id` comes
  from the server's MoQ producer, not a peer-side counter, so
  reconnecting children can align with the parent without a
  sync handshake.
- **MoQ wire stays pure on the QUIC side.** The prefix only
  exists on the DataChannel leg; the MoQ-over-QUIC leg
  continues to ship bare frames, matching the 110 scoping
  decision (no MoQ wire changes to preserve foreign-client
  compatibility).
- **Single message per frame.** One DataChannel send = one
  MoQ frame. DataChannel already fragments large messages
  across SCTP chunks; no application-layer fragmentation is
  needed. Future iterations can pack multiple small frames
  per message if bandwidth measurements warrant.

## Admin route

`GET /api/v1/mesh` returns the current tree-shape snapshot
(also reachable in-process via
`ServerHandle::mesh_coordinator()` in integration tests as of
session 111-B1):

```json
{
  "enabled": true,
  "peer_count": 42,
  "offload_percentage": 73.5,
  "peers": [
    {
      "peer_id": "peer-one",
      "role": "Root",
      "parent": null,
      "depth": 0,
      "intended_children": 3,
      "forwarded_frames": 1200
    },
    {
      "peer_id": "peer-seven",
      "role": "Relay",
      "parent": "peer-one",
      "depth": 1,
      "intended_children": 1,
      "forwarded_frames": 400
    }
  ]
}
```

### Per-peer offload snapshot (session 141)

The `peers` array carries per-peer offload stats. Operators can
compare the topology planner's `intended_children` assignment
against the peer's self-reported `forwarded_frames` count to
answer questions like "is peer X actually relaying to its tree
children?" and "which peer is the drop from zero-actual
forwards?"

| Field | Meaning |
|---|---|
| `peer_id` | Unique id registered via `/signal` |
| `role` | `Root`, `Relay`, or `Leaf` |
| `parent` | Parent peer id, or `null` for roots |
| `depth` | Distance from origin (0 = Root) |
| `intended_children` | Count of tree children the planner assigned to this peer |
| `forwarded_frames` | Cumulative frames the peer has reported forwarding via DataChannel |
| `capacity` | Per-peer self-reported relay capacity (clamped to global `--max-peers`); `null` when the client did not advertise one (session 144) |

#### ForwardReport wire message

The per-peer `forwarded_frames` value is reported by the browser
over the existing `/signal` WebSocket as a new message variant:

```json
{ "type": "ForwardReport", "forwarded_frames": 1200 }
```

The server resolves the sender from the WS session state (no
`peer_id` on the wire, so a peer can only report for itself).
`MeshPeer` emits this every second automatically, with
skip-on-unchanged: idle peers and leaf peers that never forward
stay silent on the signaling channel.

The cumulative value is **replaced** server-side rather than
accumulated, so a browser reconnect (client counter resets to
zero) simply drops the displayed value back down to the new
running total. Nothing drifts upward forever.

Added in session 141 with `#[serde(default)]` on the new `peers`
field so pre-141 clients and servers remain interoperable;
callers upgrading the SDK do not need a coordinated server bump.

`offload_percentage` is intended offload based on tree shape,
not measured traffic, until the data plane ships.

### Per-peer capacity (session 144)

The `capacity` column on `peers` is the per-peer self-reported
maximum-children value the client advertised on its `Register`
signal message. The server clamps the claim to the operator's
configured global `--max-peers` ceiling so a misbehaving client
cannot exceed the operator's limit. The topology planner consults
the per-peer cap in `find_best_parent`: a peer with `capacity: 1`
is treated as full after one child even when the global ceiling
is higher, which forces subsequent peers to descend past it.

#### Wire shape

`SignalMessage::Register` grows an optional `capacity: u32` field
behind `#[serde(default)]`:

```json
{ "type": "Register", "peer_id": "abc", "track": "live/test", "capacity": 3 }
```

Pre-144 clients omit the field entirely. The server treats `null`
or missing as "use the operator's global cap for this peer".

#### Browser SDK (`@lvqr/core`)

`MeshConfig.capacity?: number` is the integrator-side knob.
JSON.stringify drops undefined fields, so an unset config
produces a Register without the field and the server falls back
to its global ceiling.

```ts
const mesh = new MeshPeer({
  signalUrl: 'ws://localhost:8080/signal',
  peerId: crypto.randomUUID(),
  track: 'live/my-stream',
  capacity: 1,            // mobile profile: serve at most one child
});
```

#### Anti-scope (v1)

* No mid-session capacity revisions. The client advertises a
  static value at register time; tab-switch or network-change
  triggered updates would need either a separate `Capacity`
  signal variant or a re-register flow, both of which are v1.2
  candidates.
* No bandwidth-probing or CPU-headroom auto-detection. The
  browser platform does not expose an upload-bandwidth API, and
  CPU-headroom heuristics are unreliable across browsers; the
  integrator picks the value from their own profile knowledge.
* No `iceTransportPolicy: 'relay'` enforcement coupled to
  capacity=0. A peer that advertises capacity=0 is simply
  ineligible to host children; it still consumes media as a
  leaf.

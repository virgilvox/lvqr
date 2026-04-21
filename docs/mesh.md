# Peer Mesh

The peer mesh is LVQR's planned bandwidth multiplier. Viewers
are meant to relay media to other viewers via WebRTC
DataChannels, offloading the bulk of server bandwidth for
high-fan-out broadcasts.

> **Status as of v0.4.0: topology planner + signaling ship;
> client-side WebRTC relay ships in `@lvqr/core`; server-side
> data-plane wiring is pending.** The Rust side (`lvqr-mesh`,
> `lvqr-signal`) assigns peer positions in a relay tree,
> detects dead peers, reassigns orphans, and pushes
> `AssignParent` messages over `/signal` at Register time. The
> JavaScript side (`@lvqr/core` `MeshPeer` class at
> `bindings/js/packages/core/src/mesh.ts`) connects to
> `/signal`, handles `AssignParent`, opens an
> `RTCPeerConnection` to the assigned parent, sets up a
> DataChannel, and forwards received media frames to children.
> What is missing:
> - Server-side subscriber registration on the WebSocket relay
>   (`ws_relay_session` does not yet call `MeshCoordinator::add_peer`).
> - Server-side injection of MoQ frame bytes into a
>   DataChannel to seed root peers.
> - Subscribe-token admission on `/signal`.
> - An end-to-end test proving a second browser peer receives
>   frames through the mesh instead of directly from the server.
>
> Until all four land, a deployment that sets `--mesh-enabled`
> still serves every subscriber directly from the server; the
> offload percentage reported by `/api/v1/mesh` is intended
> offload based on tree shape, not measured traffic.
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

Internal defaults (hardcoded, will be configurable):
- Root peer count: 30
- Max tree depth: 6
- Heartbeat timeout: 10s

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

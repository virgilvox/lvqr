# Peer Mesh

The peer mesh is LVQR's planned bandwidth multiplier. Viewers
are meant to relay media to other viewers via WebRTC
DataChannels, offloading the bulk of server bandwidth for
high-fan-out broadcasts.

> **Status: topology planner only.** The mesh coordinator
> assigns peer positions in a relay tree, detects dead peers,
> and reassigns orphans. Actual media forwarding between peers
> over WebRTC DataChannels is **not implemented today**; media
> delivery uses the server-backed MoQ / HLS / DASH / WHEP
> egress described in [`architecture.md`](architecture.md). The
> bandwidth math in the "Bandwidth offload" section below is
> the *intended* behavior after the Tier 4 data-plane work
> lands; the offload percentage reported by `/api/v1/mesh` is
> intended offload based on tree shape, not measured traffic.
>
> Tracking in [`tracking/ROADMAP.md`](../tracking/ROADMAP.md)
> Tier 4. Separate from WASM per-fragment filters and the
> AI-agent work.

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

# Peer Mesh

The peer mesh is LVQR's bandwidth multiplier. Viewers relay media to other viewers via WebRTC DataChannels, offloading 75%+ of server bandwidth.

## How It Works

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

## Bandwidth Math

With mesh enabled on a 1 Gbps server at 4 Mbps per stream:

| Viewers | Server Bandwidth | Mesh Bandwidth | Server Offload |
|---------|-----------------|----------------|----------------|
| 30 | 120 Mbps | 0 | 0% |
| 100 | 120 Mbps | 280 Mbps | 70% |
| 500 | 120 Mbps | 1,880 Mbps | 94% |
| 2000 | 120 Mbps | 7,880 Mbps | 98.5% |

The server only directly serves ~30 root peers. Everyone else is served by the mesh.

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

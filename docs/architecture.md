# LVQR Architecture

## Overview

LVQR is a live video streaming relay built on the Media over QUIC (MoQ) protocol. It consists of several Rust crates organized in a workspace, plus JavaScript and Python client bindings.

## Data Flow

```
Publisher (OBS/ffmpeg)
    |
    | RTMP (port 1935)
    v
lvqr-ingest (RtmpMoqBridge)
    |
    | MoQ tracks via OriginProducer
    v
lvqr-relay (MoQ over QUIC/WebTransport, port 4443)
    |
    +---> Subscriber A (browser via WebTransport)
    +---> Subscriber B (gets Bytes::clone, zero copy)
    +---> Subscriber C (same ref-counted buffer)
    |
    v
lvqr-mesh (optional peer relay)
    |
    +---> Peer D relays to Peer E via WebRTC DataChannel
    +---> Peer E relays to Peers F, G, H
```

## Crate Dependency Graph

```
lvqr-core (Tier 0 - no internal deps)
    |
    +---> lvqr-signal (Tier 1)
    +---> lvqr-relay (Tier 2)
    +---> lvqr-ingest (Tier 2)
    |         |
    |         +---> (also depends on moq-lite for bridge)
    |
    +---> lvqr-mesh (Tier 2, depends on lvqr-signal)
    +---> lvqr-admin (Tier 3, depends on lvqr-core)
    |
    +---> lvqr-cli (Tier 4 - depends on all above)
    +---> lvqr-wasm (npm, not crates.io)
```

## Key Design Decisions

### moq-lite Origin Pattern

The relay does NOT manually forward tracks. It creates a shared `OriginProducer` and gives every connection access:

```rust
// Every MoQ connection gets the shared Origin
request.with_publish(origin.consume())  // send data TO subscriber
       .with_consume(origin)            // receive data FROM publisher
       .ok().await
```

moq-lite internally handles all ANNOUNCE/SUBSCRIBE/data routing through the shared Origin. The relay is a thin connection manager.

### RTMP-to-MoQ Bridge

The `RtmpMoqBridge` connects RTMP ingest callbacks to the MoQ Origin:

```rust
let bridge = RtmpMoqBridge::new(relay.origin().clone());
let rtmp_server = bridge.create_rtmp_server(rtmp_config);
```

When RTMP `publish` event fires: `origin.create_broadcast("app/key")` + `broadcast.create_track("video")`
When video data arrives: keyframes start new MoQ groups, delta frames append to current group.

### Peer Mesh Tree

The `MeshCoordinator` builds a relay tree:
- First N peers become root peers (directly served by server)
- Subsequent peers assigned as children of existing peers
- Assignment algorithm: shallowest depth first, then fewest children (balanced load)
- Max 3 children per peer, max 6 depth hops
- Dead peer detection via heartbeat timeout
- Orphaned children reassigned on parent disconnect

### Zero-Copy Fanout

```
QUIC Ingest --> Decrypt (userspace) --> Ring Buffer (Bytes ref) --> QUIC Send
                                             |
                                       Subscriber A: Bytes::clone() (refcount++)
                                       Subscriber B: Bytes::clone() (no data copy)
                                       Subscriber C: Bytes::clone()
```

## Protocol Stack

```
Application:  MoQ (Media over QUIC) via moq-lite
Transport:    QUIC (via quinn) / WebTransport
Ingest:       RTMP (via rml_rtmp)
Mesh:         WebRTC DataChannels (signaling via WebSocket)
Admin:        HTTP (via axum)
```

## CLI Architecture

`lvqr serve` starts three servers concurrently via `tokio::select!`:
1. MoQ relay (QUIC/WebTransport on port 4443)
2. RTMP ingest (TCP on port 1935)
3. Admin HTTP API (TCP on port 8080)

Graceful shutdown on SIGINT (Ctrl+C).

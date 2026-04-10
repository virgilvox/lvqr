# LVQR Architecture

## Overview

LVQR is a live video streaming relay built on the Media over QUIC (MoQ) protocol. It consists of several Rust crates organized in a workspace, plus JavaScript and Python client bindings.

## Data Flow

```
Publisher (OBS/ffmpeg)
    |
    | RTMP (port 1935)
    v
lvqr-ingest
    |
    | MoQ tracks (bytes::Bytes ref-counted)
    v
lvqr-core (Registry + Ring Buffer + GOP Cache)
    |
    | moq-lite OriginProducer/OriginConsumer
    v
lvqr-relay (MoQ over QUIC/WebTransport)
    |
    +---> Subscriber A (browser via WebTransport)
    +---> Subscriber B (gets Bytes::clone, zero copy)
    +---> Subscriber C (same ref-counted buffer)
    |
    v
lvqr-mesh (optional)
    |
    +---> Peer D relays to Peer E via WebRTC DataChannel
    +---> Peer E relays to Peers F, G, H
```

## Crate Dependency Graph

```
lvqr-core (no internal deps)
    |
    +---> lvqr-signal
    +---> lvqr-relay
    +---> lvqr-ingest
    +---> lvqr-admin
    |
    +---> lvqr-mesh (also depends on lvqr-signal)
    |
    +---> lvqr-cli (depends on all above)
    +---> lvqr-wasm (browser-only subset of core)
```

## Key Design Decisions

### Why moq-lite, not moq-relay?

The `moq-relay` crate is a complete, opinionated relay binary. LVQR uses `moq-lite` (the transport layer only) to control the relay logic -- specifically to integrate our ring buffer, GOP cache, peer mesh, and RTMP ingest.

### Zero-Copy Fanout

The critical performance property: when a publisher sends a frame, it lands in a `bytes::Bytes` buffer. Every subscriber receives a `Bytes::clone()`, which is a ref-count increment (no data copy). This is why LVQR can handle thousands of viewers with minimal CPU.

### io_uring (Linux only)

The `io-uring` feature flag enables `tokio-uring` for batched zero-copy sends on Linux. On macOS and other platforms, LVQR falls back to standard tokio networking. Both paths use the same relay logic.

### Peer Mesh

Viewers become relays via WebRTC DataChannels. The server seeds ~30 root peers; each peer can relay to up to 3 children. The mesh self-organizes, and the server's bandwidth multiplies exponentially.

## Protocol Stack

```
Application:  MoQ (Media over QUIC)
Transport:    QUIC (via quinn)
Delivery:     WebTransport (browsers), raw QUIC (native clients)
Fallbacks:    WebSocket + fMP4, LL-HLS
Mesh:         WebRTC DataChannels
```

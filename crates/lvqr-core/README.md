# lvqr-core

Core data structures for LVQR (Live Video QUIC Relay).

- **RingBuffer**: Fixed-capacity circular buffer with `bytes::Bytes` ref-counted sharing for zero-copy fanout
- **GopCache**: GOP (Group of Pictures) cache for late-join support with keyframe detection and LRU eviction
- **Registry**: Subscriber registry using `tokio::sync::broadcast` for lock-free fanout to multiple subscribers
- **Types**: StreamId, SubscriberId, TrackName, Frame, Gop, RelayStats

## Usage

```rust
use lvqr_core::{Registry, TrackName, Frame};
use bytes::Bytes;

let registry = Registry::new();
let track = TrackName::new("live/my-stream");

// Subscribe
let mut sub = registry.subscribe(&track);

// Publish (zero-copy fanout to all subscribers)
let frame = Frame::new(0, 0, true, Bytes::from_static(b"keyframe"));
registry.publish(&track, frame);

// Receive
let frame = sub.recv().await.unwrap();
```

## License

MIT OR Apache-2.0

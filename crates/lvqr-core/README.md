# lvqr-core

Shared cross-crate vocabulary for LVQR (Live Video QUIC Relay).

After the Tier 2.1 fragment-model landing, the in-memory fanout types
(`Registry`, `RingBuffer`, `GopCache`) that used to live here moved out
of the crate: MoQ routing and fanout is now handled by `lvqr-moq` via
`moq-lite::OriginProducer`, and cross-crate media exchange goes through
`lvqr-fragment`. What remains in `lvqr-core` is the vocabulary the
higher-tier crates share without pulling in a heavier dependency:

- **Types**: `StreamId`, `SubscriberId`, `TrackName`, `Frame`,
  `RelayStats` — small value types used as stable test and API
  vocabulary.
- **EventBus / RelayEvent**: lifecycle bus used by the RTMP bridge,
  the WebSocket ingest session, and the recorder to coordinate
  `BroadcastStarted` / `BroadcastStopped` events without polling.
- **CoreError**: shared error type for the above.

## Usage

```rust
use lvqr_core::{EventBus, RelayEvent, TrackName};

let bus = EventBus::new();
let mut rx = bus.subscribe();

bus.emit(RelayEvent::BroadcastStarted {
    name: "live/test".to_string(),
});

if let Ok(RelayEvent::BroadcastStarted { name }) = rx.recv().await {
    println!("broadcast started: {name}");
}

let track = TrackName::new("live/test");
assert_eq!(track.as_str(), "live/test");
```

## License

MIT OR Apache-2.0

# lvqr-relay

MoQ relay server for LVQR (Live Video QUIC Relay).

Accepts WebTransport/QUIC connections using [moq-native](https://crates.io/crates/moq-native) and routes media tracks via [moq-lite](https://crates.io/crates/moq-lite)'s Origin system for zero-copy fanout.

## Usage

```rust
use lvqr_relay::{RelayConfig, RelayServer};

let config = RelayConfig::new("0.0.0.0:4443".parse().unwrap());
let relay = RelayServer::new(config);
relay.run().await?;
```

## License

MIT OR Apache-2.0

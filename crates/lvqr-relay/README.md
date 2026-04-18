# lvqr-relay

MoQ/QUIC relay server for LVQR.

Accepts WebTransport and raw-QUIC connections using
[moq-native](https://crates.io/crates/moq-native) and routes
media tracks via [moq-lite](https://crates.io/crates/moq-lite)'s
`OriginProducer` for zero-copy fan-out. Subscribers receive
tracks as ref-counted `bytes::Bytes`; each additional
subscriber costs zero data copies.

`lvqr-relay` consumes fragments from `lvqr-fragment` through
the shared `FragmentBroadcasterRegistry`, so every ingest
protocol (RTMP, WHIP, SRT, RTSP, WebSocket fMP4) feeds MoQ
subscribers without per-protocol wiring.

## Usage

```rust
use lvqr_relay::{RelayConfig, RelayServer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = RelayConfig::new("0.0.0.0:4443".parse()?);
    let relay = RelayServer::new(config);
    relay.run().await?;
    Ok(())
}
```

`lvqr-cli::start` uses the `init_server()` variant to bind the
QUIC socket eagerly and return the bound address for tests
that pass `port: 0`.

## Auth and metrics

`RelayServer` accepts a `SharedAuth` provider via
`set_auth_provider`. Every subscribe / publish check fires
`AuthProvider::check` and increments
`lvqr_auth_failures_total{entry="moq"}` on failure. See
[`../lvqr-auth`](../lvqr-auth/README.md) for the provider
options (noop, static-token, HS256 JWT).

## License

MIT OR Apache-2.0

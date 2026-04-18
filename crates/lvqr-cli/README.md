# lvqr-cli

Single-binary composition root for LVQR.

`lvqr-cli` is the crate that ties the 24 other workspace crates
together. It exposes the `lvqr serve` binary (entry point of
`cargo install lvqr-cli`) and a re-usable library target
(`lvqr_cli::start`) that `lvqr-test-utils::TestServer` drives
to spin up a full-stack instance on ephemeral ports for
integration tests. Everything `lvqr serve` does in production
is exercised by the same composition root in tests.

## Binary usage

```bash
lvqr serve                            # zero-config: RTMP + MoQ + LL-HLS + admin
lvqr serve --dash-port 8889           # add DASH egress
lvqr serve --whip-port 8443           # add WHIP ingest
lvqr serve --srt-port 8890            # add SRT ingest
lvqr serve --rtsp-port 8554           # add RTSP ingest
lvqr serve --cluster-listen ...       # multi-node with chitchat gossip
```

Every protocol beyond the four always-on surfaces
(RTMP 1935, MoQ 4443/udp, LL-HLS 8888, admin 8080) is gated
on a non-zero port.

Full flag + env-var reference in the top-level
[`README.md`](../../README.md) and
[`docs/quickstart.md`](../../docs/quickstart.md).

## Library usage

```rust
use lvqr_cli::{ServeConfig, start};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = ServeConfig::loopback_ephemeral();  // for tests
    let handle = start(config).await?;
    // use handle.rtmp_addr() / hls_addr() / dash_addr() ...
    handle.shutdown().await?;
    Ok(())
}
```

Every listener binds before `start` returns, so callers that
pass `port: 0` can read the bound address back off
`ServerHandle`. Tests never have to poll or sleep.

## Features

- `default = ["rtmp", "quinn-transport", "cluster"]`
- `rtmp` -- RTMP ingest
- `quinn-transport` -- QUIC/MoQ relay via quinn
- `cluster` -- chitchat cluster plane and
  `--cluster-*` CLI flags

## Install

```bash
cargo install lvqr-cli
```

## License

MIT OR Apache-2.0

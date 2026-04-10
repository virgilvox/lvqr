# lvqr-cli

Command-line interface for LVQR (Live Video QUIC Relay).

Single binary that runs the relay server with RTMP ingest, MoQ relay, admin API, and optional peer mesh.

## Install

```bash
cargo install lvqr-cli
```

## Usage

```bash
lvqr serve --port 4443 --rtmp-port 1935 --admin-port 8080
```

## License

MIT OR Apache-2.0

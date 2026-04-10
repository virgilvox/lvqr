# Quickstart

## Install

### From source (recommended for development)

```bash
git clone https://github.com/virgilvox/lvqr.git
cd lvqr
cargo build --release
```

The binary is at `target/release/lvqr`.

### From crates.io

```bash
cargo install lvqr-cli
```

### Docker

```bash
docker pull ghcr.io/virgilvox/lvqr
docker run -p 4443:4443/udp -p 1935:1935 -p 8080:8080 ghcr.io/virgilvox/lvqr
```

## Start the Relay

```bash
lvqr serve
```

This starts:
- QUIC/MoQ relay on port 4443 (UDP)
- RTMP ingest on port 1935 (TCP)
- Admin HTTP API on port 8080 (TCP)

A self-signed TLS certificate is generated automatically for development.

## Stream from OBS

1. Open OBS Studio
2. Settings > Stream
3. Service: Custom
4. Server: `rtmp://your-server:1935/live`
5. Stream Key: `my-stream`
6. Start Streaming

## Watch

Open your browser to `https://your-server:8080/watch/my-stream`.

Or use the JavaScript player:

```html
<script src="https://cdn.jsdelivr.net/npm/@lvqr/core/dist/index.global.js"></script>
<lvqr-player src="https://your-server:4443/live/my-stream"></lvqr-player>
```

## Check Status

```bash
# Health check
curl http://localhost:8080/healthz

# List active streams
curl http://localhost:8080/api/v1/streams

# Server stats
curl http://localhost:8080/api/v1/stats
```

## Configuration

Create a `lvqr.toml` file:

```toml
[server]
port = 4443
rtmp_port = 1935
admin_port = 8080

[mesh]
enabled = true
max_peers = 3

[tls]
# Omit for auto-generated self-signed certs
# cert = "/path/to/cert.pem"
# key = "/path/to/key.pem"
```

Then run:

```bash
lvqr serve --config lvqr.toml
```

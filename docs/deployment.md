# Deployment Guide

## Docker

```bash
docker pull ghcr.io/virgilvox/lvqr
docker run -d \
  -p 4443:4443/udp \
  -p 1935:1935 \
  -p 8080:8080 \
  --name lvqr \
  ghcr.io/virgilvox/lvqr
```

## Binary

Download from [GitHub Releases](https://github.com/virgilvox/lvqr/releases) or build from source:

```bash
cargo install lvqr-cli
lvqr serve
```

## systemd

Create `/etc/systemd/system/lvqr.service`:

```ini
[Unit]
Description=LVQR Live Video QUIC Relay
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=lvqr
ExecStart=/usr/local/bin/lvqr serve --port 4443 --rtmp-port 1935 --admin-port 8080
Restart=always
RestartSec=5
Environment=RUST_LOG=lvqr=info

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now lvqr
```

## TLS

LVQR auto-generates a self-signed certificate for development. For production, provide real certs:

```bash
lvqr serve --tls-cert /etc/letsencrypt/live/relay.example.com/fullchain.pem \
           --tls-key /etc/letsencrypt/live/relay.example.com/privkey.pem
```

Or use Caddy as a reverse proxy for automatic HTTPS:

```
relay.example.com {
    reverse_proxy localhost:8080
}
```

QUIC traffic (port 4443) must be exposed directly since it uses UDP.

## Firewall

Required ports:
- **4443/udp**: QUIC/MoQ relay (WebTransport)
- **1935/tcp**: RTMP ingest (from OBS/ffmpeg)
- **8080/tcp**: Admin HTTP API (restrict to internal network)

```bash
# UFW example
ufw allow 4443/udp
ufw allow 1935/tcp
ufw allow from 10.0.0.0/8 to any port 8080
```

## Monitoring

The admin API provides health and stats endpoints:

```bash
# Health check (for load balancer probes)
curl http://localhost:8080/healthz

# Stats
curl http://localhost:8080/api/v1/stats

# Active streams
curl http://localhost:8080/api/v1/streams
```

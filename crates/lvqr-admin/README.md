# lvqr-admin

HTTP admin API for LVQR (Live Video QUIC Relay).

Provides health checks, stats, and stream management endpoints.

## Endpoints

- `GET /healthz` - Health check
- `GET /api/v1/stats` - Relay statistics (tracks, subscribers)
- `GET /api/v1/streams` - List active streams

## License

AGPL-3.0-or-later for open-source use; commercial license
available for proprietary / SaaS deployments. See the top-
level [`COMMERCIAL-LICENSE.md`](../../COMMERCIAL-LICENSE.md)
for the process.

# lvqr-admin

HTTP admin API for LVQR (Live Video QUIC Relay).

Provides health checks, stats, and stream management endpoints.

## Endpoints

- `GET /healthz` - Health check
- `GET /api/v1/stats` - Relay statistics (tracks, subscribers)
- `GET /api/v1/streams` - List active streams

## License

MIT OR Apache-2.0

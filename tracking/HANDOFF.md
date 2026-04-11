# LVQR Handoff Document

## Project Status: v0.1.0 Released

**Last Updated**: 2026-04-10
**Tests**: 51 Rust + 8 Python = 59 total
**Commits**: 10 on main

## All Packages Published

### crates.io (Rust)
| Crate | Version | Install |
|-------|---------|---------|
| lvqr-core | 0.1.0 | `cargo add lvqr-core` |
| lvqr-signal | 0.1.0 | `cargo add lvqr-signal` |
| lvqr-relay | 0.1.0 | `cargo add lvqr-relay` |
| lvqr-ingest | 0.1.0 | `cargo add lvqr-ingest` |
| lvqr-mesh | 0.1.0 | `cargo add lvqr-mesh` |
| lvqr-admin | 0.1.0 | `cargo add lvqr-admin` |
| lvqr-cli | 0.1.0 | `cargo install lvqr-cli` |

### npm (JavaScript)
| Package | Version | Install |
|---------|---------|---------|
| @lvqr/core | 0.1.0 | `npm install @lvqr/core` |
| @lvqr/player | 0.1.0 | `npm install @lvqr/player` |

### PyPI (Python)
| Package | Version | Install |
|---------|---------|---------|
| lvqr | 0.1.0 | `pip install lvqr` |

### WASM
- Builds via `wasm-pack build crates/lvqr-wasm --target web`
- Shipped as part of @lvqr/core npm package

## Architecture Summary

```
OBS/ffmpeg --RTMP--> lvqr-ingest --MoQ--> lvqr-relay --QUIC/WT--> Browser
                                              |
                                     lvqr-mesh (peer tree)
                                     lvqr-signal (WebRTC signaling)
                                     lvqr-admin (HTTP API)
```

## What's Implemented

- **7 Rust crates**: core, relay, ingest, mesh, signal, admin, cli
- **WASM client**: WebTransport + fingerprint auth for dev
- **TypeScript packages**: LvqrClient, LvqrAdminClient, LvqrPlayerElement
- **Python package**: Admin API client with httpx
- **CLI**: Runs relay + RTMP + admin + mesh concurrently
- **CI/CD**: GitHub Actions (ci.yml + release.yml)
- **Dockerfile**: Multi-stage Rust builder
- **Examples**: basic-relay, obs-to-browser, browser-player, python-admin
- **Docs**: architecture, quickstart, deployment, mesh, SDK (JS + Python)
- **Release script**: Tiered crate publishing

## Future Work

- Full MoQ protocol framing in WASM (currently transport-level only)
- MSE/WebCodecs video rendering pipeline in @lvqr/core
- io_uring send path on Linux (feature-gated, tokio fallback exists)
- WS-fMP4 and LL-HLS fallback output from relay
- Stream authentication (keys, tokens)
- Recording (S3/Spaces segment archival)
- Multi-server federation

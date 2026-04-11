# LVQR Handoff Document

## Project Status

**Last Updated**: 2026-04-10
**Tests**: 51 Rust + 8 Python = 59 total
**Published**: 6 crates on crates.io (lvqr-core, lvqr-signal, lvqr-relay, lvqr-ingest, lvqr-mesh, lvqr-admin)
**Pending Publish**: lvqr-cli (crates.io rate limit, retry after 2026-04-11 01:31 GMT)
**Commits**: 8 on main, all pushed to GitHub

## What's Complete

### Rust Backend (100%)
- **lvqr-core**: Ring buffer, GOP cache, subscriber registry (25 tests)
- **lvqr-relay**: MoQ relay via moq-native/moq-lite (3 integration tests)
- **lvqr-ingest**: RTMP server + MoQ bridge (2 tests)
- **lvqr-mesh**: Peer tree coordinator with balanced assignment (13 tests)
- **lvqr-signal**: WebSocket signaling for WebRTC peer connections (4 tests)
- **lvqr-admin**: HTTP API /healthz, /stats, /streams (4 tests)
- **lvqr-cli**: Runs relay + RTMP + admin + mesh/signal concurrently

### Python Bindings (100%)
- `lvqr` package: LvqrClient admin API client with httpx
- 8 unit tests passing

### Infrastructure (100%)
- GitHub Actions CI (fmt, clippy, test on ubuntu+macos, WASM, Docker)
- Release workflow (multi-platform binary builds, Docker push to ghcr.io)
- Release script (scripts/release.sh with tiered publishing)
- Dockerfile (multi-stage, exposes 4443/udp, 1935/tcp, 8080/tcp)

### Documentation (100%)
- README with badges
- docs/architecture.md, quickstart.md, deployment.md, mesh.md
- docs/sdk/python.md
- Per-crate READMEs for crates.io
- CONTRIBUTING.md, SECURITY.md, CODE_OF_CONDUCT.md

### Examples (100%)
- basic-relay: Minimal setup with config file
- obs-to-browser: Docker compose with OBS instructions
- python-admin: Admin client demo

## What Remains

### Phase B: Browser Stack (Not Yet Started)

1. **lvqr-wasm**: Implement WebTransport MoQ client in Rust/WASM
   - WebTransport connection via web-sys
   - MoQ protocol framing (minimal subset, no moq-lite since it depends on quinn)
   - Transport fallback (WebTransport -> WebSocket)
   - This is the largest remaining item

2. **@lvqr/core**: TypeScript wrapper around WASM
   - pnpm workspace in bindings/js/
   - Transport negotiation, MSE feeding
   - CDN-compatible global build

3. **@lvqr/player**: Web Component
   - `<lvqr-player src="...">` custom element
   - Shadow DOM, CSS parts

4. **Publish to npm**: @lvqr/core, @lvqr/player

### Other Pending
- Publish lvqr-cli to crates.io (after rate limit)
- Publish lvqr Python package to PyPI
- browser-player example (depends on @lvqr/player)

## Key Technical Notes

### DashMap Deadlock Prevention
In MeshCoordinator, never hold a get_mut() write lock while calling find_best_parent() (which iterates). Find parent first, then acquire write lock.

### Test Performance
Use `cargo test -p <crate> --lib` for fast iteration. Full workspace tests take 5+ minutes.

### WASM Architecture Decision
lvqr-wasm cannot use moq-lite (depends on quinn/tokio, not browser-compatible). Must implement a minimal MoQ client directly against web-sys WebTransport APIs.

# Contributing to LVQR

Thank you for your interest in contributing to LVQR. This document covers how to get started.

## Prerequisites

- **Rust 1.85+** (via [rustup](https://rustup.rs/))
- **Node.js 20+** (for browser bindings)
- **Docker** (for integration and e2e tests)
- **wasm-pack** (for WASM builds): `cargo install wasm-pack`

### Platform-specific

- **Linux**: Full feature support including `io_uring`
- **macOS**: All features except `io_uring` (uses tokio fallback)

## Getting Started

```bash
git clone https://github.com/virgilvox/lvqr.git
cd lvqr
cargo build --workspace
cargo test --workspace
```

## Development Workflow

1. Fork the repository and create a feature branch
2. Write your code with tests
3. Run the full check suite:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
4. Submit a pull request

## Project Structure

```
crates/
  lvqr-core/         Core data structures (ring buffer, GOP cache, registry)
  lvqr-relay/        MoQ relay and fan-out engine
  lvqr-mesh/         Peer mesh topology and gossip
  lvqr-ingest/       Protocol ingest (RTMP, WHIP, SRT)
  lvqr-signal/       WebRTC signaling server
  lvqr-admin/        HTTP admin API
  lvqr-wasm/         WebAssembly browser bindings
  lvqr-cli/          CLI binary
  lvqr-test-utils/   Shared test utilities
bindings/
  js/                TypeScript/JavaScript packages
  python/            Python client library
```

## Testing

### Unit tests
```bash
cargo test -p lvqr-core          # Single crate
cargo test --workspace --lib     # All unit tests
```

### Integration tests
```bash
cargo test --workspace --test '*'
```

### E2E tests (requires Docker)
```bash
docker compose -f docker/docker-compose.test.yml up --abort-on-container-exit
```

## Commit Messages

Use clear, descriptive commit messages:

```
Add GOP cache eviction policy

- Implement LRU eviction when max GOPs exceeded
- Add configurable max GOP count
- Update ring buffer to track GOP boundaries
```

## Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy` and address warnings
- Write doc comments (`///`) for all public API items
- Prefer `thiserror` for library error types, `anyhow` only in the CLI binary

## Adding a New Ingest Protocol

1. Create a module in `crates/lvqr-ingest/src/`
2. Add a feature flag in `crates/lvqr-ingest/Cargo.toml`
3. Implement the protocol parser and MoQ track publisher
4. Wire the feature flag through to `lvqr-cli`
5. Add integration tests
6. Document in `docs/`

## License

LVQR is dual-licensed: AGPL-3.0-or-later for open-source use
and commercial terms for proprietary / SaaS deployments. See
[`COMMERCIAL-LICENSE.md`](COMMERCIAL-LICENSE.md) at the repo
root for the commercial-license process.

By submitting a pull request you agree that your contribution
is licensed under AGPL-3.0-or-later AND you grant the project
maintainer (Moheeb Zara, hackbuildvideo@gmail.com) a
perpetual, irrevocable, worldwide license to relicense your
contribution under the commercial terms above. This is the
CLA-style mechanism that keeps the dual-license model honest;
every line in the repo is either owned by the maintainer or
explicitly relicenseable. If you cannot agree to those terms
for a specific contribution, please mention it in your pull
request and we will discuss alternatives.

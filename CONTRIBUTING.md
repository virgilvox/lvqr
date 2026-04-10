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

By contributing, you agree that your contributions will be licensed under the MIT OR Apache-2.0 license.

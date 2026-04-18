# LVQR Project Rules

## Commit Authorship
Never add Claude as an author, co-author, or contributor in git commits, files, or any other attribution. Do not use `Co-Authored-By` trailers or similar attribution mechanisms. Commits should appear as if written entirely by the human developer.

## Code Style
- No emojis in code, commit messages, or documentation
- No em-dashes or obvious AI language patterns in prose
- Keep comments concise and only where logic is non-obvious
- Follow standard Rust conventions: `cargo fmt`, `cargo clippy`
- Max line width: 120 characters

## Project Metadata
- Author: Moheeb Zara <hackbuildvideo@gmail.com>
- GitHub: virgilvox
- License: AGPL-3.0-or-later for open-source use; commercial license for
  proprietary / SaaS (see COMMERCIAL-LICENSE.md at repo root). Contributions
  are AGPL + commercial-relicense grant to the maintainer.
- npm scope: @lvqr

## Workspace Conventions
- All crates live under `crates/`
- All crates use workspace dependency inheritance
- Feature flags for platform-specific code (io_uring behind `io-uring` feature)
- `lvqr-test-utils` is `publish = false`
- Edition 2024, Rust 1.85+

## Testing
- Real integration tests with actual network connections, not mocks
- Each crate has unit tests in `#[cfg(test)]` modules
- Integration tests in `tests/` directories use `lvqr-test-utils`
- Docker for full e2e testing (ffmpeg RTMP push, browser playback)

## File Boundaries
- Only edit files within this repository
- Never modify files outside `/Users/obsidian/Projects/ossuary-projects/lvqr/`

## Publishing Order (crates.io)
```
Tier 0: lvqr-core
Tier 1: lvqr-signal
Tier 2: lvqr-relay, lvqr-ingest, lvqr-mesh
Tier 3: lvqr-admin
Tier 4: lvqr-wasm, lvqr-cli
```

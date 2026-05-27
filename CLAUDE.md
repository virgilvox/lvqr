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

Topologically sorted -- each tier depends only on prior tiers. 26
publishable crates; 3 are `publish = false` (internal-only).

```
Tier 0 (no internal deps):
  lvqr-archive, lvqr-auth, lvqr-codec, lvqr-core, lvqr-moq,
  lvqr-observability
Tier 1 (depends only on Tier 0):
  lvqr-fragment, lvqr-signal
Tier 2 (Tier 0 + Tier 1):
  lvqr-cmaf, lvqr-agent, lvqr-record, lvqr-transcode, lvqr-wasm,
  lvqr-relay, lvqr-cluster, lvqr-mesh
Tier 3 (Tier 0-2):
  lvqr-hls, lvqr-ingest, lvqr-agent-whisper, lvqr-admin
Tier 4 (Tier 0-3):
  lvqr-whep, lvqr-whip, lvqr-rtsp, lvqr-srt, lvqr-dash
Tier 5 (the rest of the workspace):
  lvqr-cli
```

Internal-only (`publish = false`, do not publish):
`lvqr-conformance`, `lvqr-soak`, `lvqr-test-utils`.

When bumping the workspace version, `cargo publish -p <crate>` each
crate in tier order; `cargo publish` waits for crates.io to index each
upload before the next dependent crate can resolve it.

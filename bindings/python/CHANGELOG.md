# `lvqr` Python SDK Changelog

User-visible changes to the LVQR Python SDK published on PyPI.
The head of `main` is always the source of truth; this file
summarises shipped + unreleased work between PyPI releases. For
session-by-session engineering notes see
[`tracking/HANDOFF.md`](../../tracking/HANDOFF.md).

## Unreleased (post-0.3.2)

### Added

* **`LvqrClient.config_reload_status()` /
  `trigger_config_reload()` + `ConfigReloadStatus` dataclass**
  (session 147). Defensive `.get(...)` parsers carry forward
  across sessions 148 / 149's `applied_keys` extension without
  a SDK change; the wire shape stays a simple
  `list[str]` / `list[str]` / etc.

* **`LvqrClient.streamkeys_*` + `StreamKey` / `StreamKeySpec`
  dataclasses** (session 146). Mint, list, revoke, and rotate
  ingest stream keys from Python. See `docs/sdk/python.md`.

## [0.3.2] - 2026-04-24

Tracker-only republish alongside `@lvqr/core` 0.3.2 + Rust
0.4.1. No SDK shape changes; sessions 141-144 source on `main`
is the published artifact.

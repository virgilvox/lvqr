# `lvqr` Python SDK Changelog

User-visible changes to the LVQR Python SDK published on PyPI.
The head of `main` is always the source of truth; this file
summarises shipped + unreleased work between PyPI releases. For
session-by-session engineering notes see
[`tracking/HANDOFF.md`](../../tracking/HANDOFF.md).

## [1.0.0] - 2026-04-28

Stability commitment for the Python admin client. Same source as
0.3.3; the version label moves with the rest of the SDK family
(`@lvqr/core`, `@lvqr/dvr-player`, `@lvqr/player`,
`@lvqr/admin-ui`) and the Rust workspace. No API change; every
method on `LvqrClient` keeps its existing signature.

## [0.3.3] - 2026-04-28

Cross-language SDK release wave alongside `@lvqr/core` 0.3.3
and the first npm publish of `@lvqr/dvr-player` 0.3.3. Brings
the Python admin client in line with the v0.4.2 server surface
(runtime stream-key CRUD + hot config reload).

### Added

* **`LvqrClient.config_reload_status()` /
  `trigger_config_reload()` + `ConfigReloadStatus` dataclass**
  (session 147). Defensive `.get(...)` parsers carry forward
  across sessions 148 / 149's `applied_keys` extension without
  a SDK change; the wire shape stays a simple
  `list[str]` / `list[str]` / etc.

* **`LvqrClient.list_streamkeys()` / `mint_streamkey()` /
  `revoke_streamkey()` / `rotate_streamkey()` + `StreamKey` /
  `StreamKeySpec` dataclasses** (session 146). Mint, list,
  revoke, and rotate ingest stream keys from Python. See
  `docs/sdk/python.md`.

## [0.3.2] - 2026-04-24

Tracker-only republish alongside `@lvqr/core` 0.3.2 + Rust
0.4.1. No SDK shape changes; sessions 141-144 source on `main`
is the published artifact.

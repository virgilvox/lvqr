# `@lvqr/core` Changelog

User-visible changes to the JavaScript / TypeScript SDK published
on npm. The head of `main` is always the source of truth; this
file summarises shipped + unreleased work between npm releases.
For session-by-session engineering notes see
[`tracking/HANDOFF.md`](../../../../tracking/HANDOFF.md).

## Unreleased (post-0.3.2)

### Added

* **`LvqrAdminClient.configReload()` / `triggerConfigReload()`**
  (session 147). New methods that GET / POST against
  `/api/v1/config-reload`, returning a `ConfigReloadStatus`
  carrying `config_path`, `last_reload_at_ms`,
  `last_reload_kind`, `applied_keys`, and `warnings`. The wire
  shape is unchanged across sessions 148 (mesh ICE + HMAC) and
  149 (JWKS + webhook); the SDK accepts the expanded
  `applied_keys` array generically without a code change.

* **`LvqrAdminClient.streamkeys.{list,mint,revoke,rotate}` +
  `StreamKey` / `StreamKeySpec` types** (session 146).
  Type-safe wrappers around the runtime stream-key CRUD admin
  API. See `docs/sdk/javascript.md`.

## [0.3.2] - 2026-04-24

Workspace + SDK version flip alongside the Rust 0.4.1
republish. Sessions 141-144 source (mesh data-plane completion,
TURN deployment recipe, per-peer capacity advertisement, three-
peer Playwright matrix) on `main` is the published artifact.
No SDK-shape changes vs. 0.3.1; tracker-only republish.

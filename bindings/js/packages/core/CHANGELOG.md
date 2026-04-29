# `@lvqr/core` Changelog

User-visible changes to the JavaScript / TypeScript SDK published
on npm. The head of `main` is always the source of truth; this
file summarises shipped + unreleased work between npm releases.
For session-by-session engineering notes see
[`tracking/HANDOFF.md`](../../../../tracking/HANDOFF.md).

## [1.0.0] - 2026-04-28

Stability commitment for the admin client surface. Same source as
0.3.3; the version label moves with the rest of the SDK family
(`@lvqr/dvr-player`, `@lvqr/player`, `@lvqr/admin-ui`) and the Rust
workspace + Python `lvqr`. No API change.

### Changed

* **Renamed 0.3.3 -> 1.0.0.** Every method on `LvqrAdminClient`
  keeps its existing signature; every TypeScript interface keeps
  its existing shape. Consumers upgrading from 0.3.3 do not need
  to touch their code beyond the version pin.

## [0.3.3] - 2026-04-28

Cross-language SDK release wave alongside Python `lvqr` 0.3.3
and the first npm publish of `@lvqr/dvr-player` 0.3.3. Brings
the JS admin client in line with the v0.4.2 server surface
(runtime stream-key CRUD + hot config reload). No MoQ subscriber
or transport-layer changes; the Tier 5 browser MoQ glass-to-
glass sampler that consumes the session 159 sidecar track is
deferred to a future minor release once the wire shape has
baked through one full release cycle.

### Added

* **`LvqrAdminClient.configReload()` / `triggerConfigReload()`**
  (session 147). New methods that GET / POST against
  `/api/v1/config-reload`, returning a `ConfigReloadStatus`
  carrying `config_path`, `last_reload_at_ms`,
  `last_reload_kind`, `applied_keys`, and `warnings`. The wire
  shape is unchanged across sessions 148 (mesh ICE + HMAC) and
  149 (JWKS + webhook); the SDK accepts the expanded
  `applied_keys` array generically without a code change.

* **`LvqrAdminClient.{listStreamKeys, mintStreamKey,
  revokeStreamKey, rotateStreamKey}` + `StreamKey` /
  `StreamKeySpec` / `StreamKeyList` types** (session 146).
  Type-safe wrappers around the runtime stream-key CRUD admin
  API. See `docs/sdk/javascript.md`.

### Removed

* **`./wasm` subpath export, `wasm` entry from `files`, and
  `build:wasm` script** (session 158 follow-up). The exported
  artefacts pointed at a pre-built browser-side `lvqr-wasm`
  bundle that was deleted in the 0.4-session-44 refactor; the
  `build:wasm` script targeted the surviving server-side
  wasmtime filter host crate which has no `wasm-bindgen`
  surface. The export path was dead in 0.3.2 and is now
  removed from the package surface entirely.

## [0.3.2] - 2026-04-24

Workspace + SDK version flip alongside the Rust 0.4.1
republish. Sessions 141-144 source (mesh data-plane completion,
TURN deployment recipe, per-peer capacity advertisement, three-
peer Playwright matrix) on `main` is the published artifact.
No SDK-shape changes vs. 0.3.1; tracker-only republish.

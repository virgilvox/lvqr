# Session 147 Briefing -- Hot config reload

**Date kick-off**: 2026-04-24 (locked at end of session 146; actual
implementation session 147 picks up from here).
**Predecessor**: Session 146 (runtime stream-key CRUD admin API).
The CRUD surface shipped clean; default-gate tests at 1070 / 0 / 0,
admin surface at 11 route trees, origin/main head `36044dc`. Next-up
#1 in README post-146 is hot config reload, which closes the
"every operator-facing setting needs a server bounce" gap that
sessions 146 (stream-key CRUD), 143 (TURN deploy), 128 (HMAC live
URLs), and 122 (admin client expansion) collectively narrowed but
did not close.

## Goal

Today every `lvqr serve` setting is fixed at process boot. Operators
rotating an auth secret, swapping a TURN server, refreshing the HMAC
playback secret, or revoking an external compromised key are all
forced into either a server bounce or an out-of-band orchestrator
rolling-restart. The 146 stream-key CRUD work cut the most common
operator action over to a runtime CRUD surface, but it did not
generalise: an operator who needs to swap their JWKS endpoint or
their HMAC secret is back to a bounce.

After this session, `lvqr serve --config <path>` reads a TOML file
that mirrors `ServeConfig`. SIGHUP triggers a re-read; live-eligible
keys (auth provider config, mesh ICE servers, HMAC playback secret)
apply atomically without a bounce. Structural keys (every `*_addr`,
every feature-gated path, `record_dir`, `archive_dir`, `mesh_enabled`,
`cluster_listen`) are diffed and surfaced as warnings on a new
`/api/v1/config-reload` admin route. A `POST /api/v1/config-reload`
admin route triggers the same reload pipeline so Windows operators
(no SIGHUP) and the JS / Python admin clients can drive reload
without shelling out.

## Decisions to confirm on read-back

### 1. Config file format: TOML

Matches Cargo conventions + the Rust ecosystem default. Supports
comments and per-section grouping. Mirror of `ServeConfig` field
names with `_` separators (no rename layer): the same `relay_addr`,
`hls_target_duration_secs`, `hmac_playback_secret`, `mesh_ice_servers`
that the CLI uses.

Alternative considered: YAML (broader operator familiarity) and JSON
(broadest tooling). YAML's whitespace-significance hurts diff
review; JSON does not allow comments. TOML wins on review-ability
+ ecosystem fit. **Confirm.**

### 2. Hot-reloadable key whitelist

* `auth.*` -- the provider construction config. Replacing the
  underlying `SharedAuth` via an `ArcSwap` swap means a publish or
  subscribe in flight finishes against whichever provider was current
  when its `check` was first invoked, and the next call sees the new
  provider.
* `mesh_ice_servers` -- the snapshot the signal callback embeds in
  every `AssignParent` message. Already a `Vec` cloned per-message,
  so an `ArcSwap<Vec<IceServer>>` replacement is wire-trivial.
* `hmac_playback_secret` -- the `Option<Arc<[u8]>>` that gates
  `?sig=...&exp=...` on `/playback/*` and live `/hls/*` + `/dash/*`.

Anti-scope inclusions: stream-key store contents (operators use the
146 CRUD API), wasm_filter chain-length (per-slot reload already
ships per session 137; chain shape stays fixed). **Confirm.**

### 3. Non-hot-reloadable keys (warn-on-diff)

Every `*_addr` and `*_port` (relay, rtmp, admin, hls, whep, whip,
dash, rtsp, srt, cluster_listen): bind sockets do not move without
recreating listeners, which would tear active connections.
Every feature-gated path (`c2pa`, `whisper_model`, `transcode_*`):
the dependency graph is fixed at compile time.
`record_dir`, `archive_dir`: live drains hold open file handles.
`mesh_enabled`: turning the mesh on or off mid-session would orphan
peers.
`streamkeys_enabled`: same, would orphan minted keys.
`tls_cert`, `tls_key`: TLS termination state is bound at handshake.

A diff that touches any of these surfaces a warning on the
`config-reload` admin route + a `tracing::warn!` log line. The route
returns 200 OK with a `warnings: [...]` field; reload of hot keys
still proceeds. **Confirm.**

### 4. Reload trigger: SIGHUP + admin POST

* `tokio::signal::unix::SignalKind::hangup()` listener on Unix.
  On Windows, the listener never fires (no SIGHUP equivalent); the
  admin POST is the only path.
* `POST /api/v1/config-reload` admin route triggers the same pipeline
  as SIGHUP. Returns 200 with the diff summary + warnings on success,
  4xx on parse failure (config file unreadable / malformed / does
  not pass validation).

Alternative considered: file watcher (`notify` crate). Rejected
because operators want explicit reload semantics -- a transient
half-write during deploy could trigger an unintended reload mid-
edit. **Confirm.**

### 5. Apply mechanism: `arc_swap::ArcSwap` for the three live keys

`crates/lvqr-cli/src/lib.rs::start()` builds three swappable handles:

```rust
let auth_swap: Arc<ArcSwap<dyn AuthProvider + Send + Sync + 'static>> = ...
let ice_servers_swap: Arc<ArcSwap<Vec<IceServer>>> = ...
let hmac_secret_swap: Arc<ArcSwap<Option<Arc<[u8]>>>> = ...
```

Every consumer that today receives `SharedAuth` by clone gets
either the swap handle (load-on-each-call) or a frozen snapshot
(load-once-at-startup) depending on whether the consumer is a
hot-path observer. RTMP / WHIP / SRT / RTSP auth callbacks load on
each call; the relay's connection-callback also loads on each call.

The SIGHUP / POST handler reads the new config, builds the new
`SharedAuth` (and ICE list, and secret), then atomically swaps each.
Failures during build are reported via the route response; existing
state stays intact (no partial reload).

`arc_swap` is already an indirect dep via several crates (chitchat,
parking_lot transitives). Promoting it to a direct workspace dep is
a one-line `Cargo.toml` change.

### 6. CLI: new `--config <path>` flag

* TOML file path. When unset, the SIGHUP listener still installs but
  does nothing on fire (no source of truth to re-read); the admin
  POST returns 503 "no --config path configured".
* When `--config` IS set, the file is the SOURCE OF TRUTH. CLI flags
  and env vars become defaults that the file can override; mismatches
  warn at parse time.
* Boot-time validation: a missing or malformed `--config` file
  refuses to start the server (no silent fall-through to defaults).

### 7. Admin route shape

```
GET  /api/v1/config-reload  -> ConfigReloadStatus
POST /api/v1/config-reload  -> 200 ConfigReloadStatus | 4xx Error
```

```rust
pub struct ConfigReloadStatus {
    pub config_path: Option<String>,
    #[serde(default)]
    pub last_reload_at_ms: Option<u64>,
    #[serde(default)]
    pub last_reload_kind: Option<String>, // "sighup" | "admin_post" | "none"
    #[serde(default)]
    pub applied_keys: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}
```

`#[serde(default)]` on every Optional field per the project
convention so older SDK clients never break on a server-side
addition.

## Anti-scope (explicit rejections for this session)

* **No file watcher.** Explicit SIGHUP / POST only.
* **No partial reload.** All hot keys reload atomically; build
  failure (e.g. JWKS unreachable) rolls back to prior state.
* **No federation reload.** `federation_links` stays in the
  warn-on-diff bucket. Restarting one outbound MoQ session under
  load is non-trivial; its own session.
* **No cluster topology reload.** Same.
* **No reloadable wasm_filter chain-length.** Per-slot module
  hot-reload already ships per session 137 (each path watches its
  own file). Adding / removing slots dynamically requires re-routing
  the bridge -- separate session.
* **No SDK shape change.** `LvqrAdminClient` and `LvqrClient` get a
  new `configReload()` / `config_reload()` method each but no new
  type machinery beyond the response struct.
* **No persistence backend.** Operators run `kill -HUP` or POST;
  there is no audit log of past reloads beyond the most recent.
* **No version bump or publish.** Workspace stays at 0.4.1.
* **No config-format expansion** beyond the existing `ServeConfig`
  fields. Adding a key means adding it to `ServeConfig` first.

## Execution order

1. **Author this briefing.** Done (post-146 close).

2. **Land the config-file parser.**
   * `crates/lvqr-cli/src/config_file.rs` (new): serde `ServeConfigFile`
     mirror + TOML round-trip + `merge_into(&mut ServeConfig)` helper.
   * Unit tests: round-trip parse, missing-fields-default,
     unknown-fields-warn (not error -- forwards-compat).

3. **Land the ArcSwap plumbing.**
   * Workspace `Cargo.toml`: add `arc_swap = "1"` to
     `[workspace.dependencies]`.
   * `crates/lvqr-auth/src/lib.rs`: type alias
     `pub type SwappableAuth = Arc<ArcSwap<SharedAuth>>` so consumers
     have a stable name.
   * `crates/lvqr-cli/src/lib.rs::start()`: build the three swap
     handles, thread through the bridges that take `SharedAuth`.
   * No behavior change yet (swap-once-at-boot is identical to
     today's clone-once-at-boot).

4. **Land the SIGHUP handler.**
   * `crates/lvqr-cli/src/config_reload.rs` (new): `ReloadHandle`
     owns the swap handles + the `Option<PathBuf>` config path +
     last-reload state. `reload(&self) -> Result<ConfigReloadStatus>`
     re-reads the file, builds new state, validates, atomically
     swaps each handle, records the result, returns the status.
   * `start()` spawns a tokio task that listens for SIGHUP and calls
     `handle.reload()` on each fire. Windows: the task install is
     a no-op (`#[cfg(unix)]`).

5. **Land the admin routes.**
   * `crates/lvqr-admin/src/config_reload_routes.rs` (new): GET +
     POST handlers.
   * `AdminState::with_config_reload(handle)` builder.

6. **Land the SDK methods.**
   * `bindings/js/packages/core/src/admin.ts`: `configReload()`
     (GET) and `triggerConfigReload()` (POST).
   * `bindings/python/python/lvqr/client.py`: `config_reload()`
     and `trigger_config_reload()`.

7. **Land the integration test.**
   * `crates/lvqr-cli/tests/config_reload_e2e.rs` (new): write a
     TOML file with `auth.publish_key = "first"`. Boot TestServer
     pointed at it. RTMP-publish with `"first"` succeeds. Rewrite
     the file with `auth.publish_key = "second"`. Send SIGHUP (or
     call admin POST). RTMP-publish with `"first"` denied;
     `"second"` succeeds.

8. **Docs.**
   * `docs/config-reload.md` (new): file format + reload semantics +
     hot-reloadable matrix + warn-on-diff matrix + operator
     examples.
   * `README.md`: Next-up #1 flips from `[ ]` to `[x]`.

9. **Session 147 close block in HANDOFF.**

## Risks + mitigations

* **`ArcSwap` overhead in the auth-check hot path.** `ArcSwap::load`
  is RCU-style: single-digit nanoseconds per load on x86, no lock,
  no atomic operation on the read fast-path. Already in use via
  several transitive deps; promoting to a direct workspace dep is
  the only change.

* **Config file format drift between releases.** A field added in
  session N+1 with no default means a pre-N+1 config file errors at
  parse. Mitigation: `#[serde(default)]` on every non-required
  field (matches the wire-shape convention this project already
  uses on every admin response), so older config files always
  parse forward.

* **Reload mid-publish race.** A SIGHUP firing during an in-flight
  RTMP publish handshake: the auth check has already loaded a
  `SharedAuth` snapshot for this session; the next handshake gets
  the new provider. No connection state is destroyed by reload.

* **Apply failure leaves the server in a partial-reload state.**
  Mitigation: build all new state objects FIRST (parse +
  validate the file, construct the new auth provider, parse the
  new ICE list, etc.); only then begin the swap sequence. A swap
  cannot fail at the swap step. Build failure returns `Err` with a
  reason; existing state stays intact.

* **`arc_swap` not stable on all targets.** Pure-Rust crate, builds
  on every Tier 1 + Tier 2 platform. No FFI, no platform-specific
  code path. Confirmed against the existing transitive use.

## Ground truth (session 147 brief-write)

* **Head**: `36044dc` on `main` (post-146 CHANGELOG follow-up).
  v0.4.1 on crates.io. Workspace builds clean.
* **lvqr-auth shape**: `AuthProvider` trait + 6 impls (Noop, Static,
  Jwt, Jwks, Webhook, MultiKey from session 146). `SharedAuth = Arc<dyn
  AuthProvider>` is the public type every ingest crate consumes by
  clone today.
* **lvqr-admin shape**: 11 route trees post-146. JSON body shape
  pattern matches `MeshPeerStats` from session 144 (each route's
  response struct has `Serialize + Deserialize` with
  `#[serde(default)]` on every Optional field).
* **CI**: All 8 GitHub Actions workflows GREEN on `b086fd2` (the
  session-146 substantive head). Docs commit `36044dc` running.
* **Tests**: Rust workspace 1070 / 0 / 0, pytest 35, Vitest 13.
  Net additions expected from this session: roughly +8 lvqr-cli
  config-file parsing unit + 4 reload-handle unit + 4 admin route
  unit + 1 RTMP integration test + 2 Vitest live + 2 pytest
  defensive-parse, putting the workspace at ~1085 / 0 / 0, pytest
  37, Vitest 15.

## Step 0 deliverable -- this briefing

Author at `tracking/SESSION_147_BRIEFING.md`. Read sections 1
through 7 in order; the actual implementation order is in section
"Execution order". The author of session 147 should re-read
`crates/lvqr-cli/src/config.rs` first (the canonical `ServeConfig`
shape), `crates/lvqr-cli/src/main.rs::serve_from_args` next (today's
build_auth + ServeConfig assembly), and
`crates/lvqr-auth/src/multi_key_provider.rs` after (the session 146
`ArcSwap`-shaped composition is the closest existing precedent for
the runtime swap mechanic).

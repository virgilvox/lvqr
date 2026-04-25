# Session 148 Briefing -- Hot config reload v2: mesh ICE + HMAC secret

**Date kick-off**: 2026-04-25 (locked at end of session 147; actual
implementation session 148 picks up from here).
**Predecessor**: Session 147 (hot config reload, auth-only v1).
The auth-section reload landed clean; default-gate tests at
**1099** / 0 / 0, admin surface at **12 route trees**, origin/main
head `7abadd0`. Session 147 explicitly deferred two hot-reload-
eligible keys (`mesh_ice_servers`, `hmac_playback_secret`); the
route surfaces warnings when an operator's file touches them.
Session 148 closes that gap.

## Goal

Today operators rotating a TURN server credential or refreshing
the HMAC playback secret are forced into a server bounce -- the
session 147 reload pipeline parses those sections and warns, but
does not apply. Both keys are operator-mutable (per-deployment
secrets, leaked-secret rotation) and would benefit from the same
SIGHUP / admin POST trigger the auth section already uses.

After this session, `lvqr serve --config <path>` extends to
hot-reload the mesh ICE server list and the HMAC playback secret
alongside the auth section. The admin route's `applied_keys`
field grows to include `"mesh_ice"` and `"hmac_secret"` when
those reload effectively. The deferred-warning lines drop. The
remaining deferred items (`jwks_url`, `webhook_auth_url`) stay
deferred per their async-builder complexity (separate session).

## Decisions to confirm on read-back

### 1. Apply mechanism: extend the existing `ArcSwap` pattern

Session 147 wrapped the resolved `SharedAuth` in
`HotReloadAuthProvider` (a sized newtype around an `ArcSwap`).
Session 148 adds two parallel handles:

```rust
pub type SwappableIceServers = Arc<ArcSwap<Vec<IceServer>>>;
pub type SwappableHmacSecret = Arc<ArcSwap<Option<Arc<[u8]>>>>;
```

Both live as fields on `ConfigReloadHandle` (alongside the
`hot_provider`). The reload pipeline rebuilds + swaps each
atomically. Readers in the signal callback / playback auth
middleware switch from "captured-by-clone" to "load-on-each-call"
and pay the same single-digit-ns ArcSwap::load cost the auth
path already pays.

### 2. Mesh ICE server reload: thread the swap through the signal callback

Today `start()`'s mesh enable branch does:

```rust
let ice_servers_for_signal = config.mesh_ice_servers.clone();
signal.set_peer_callback(Arc::new(move |event| {
    // ...
    Some(SignalMessage::AssignParent {
        // ...
        ice_servers: ice_servers_for_signal.clone(),
    })
}));
```

After session 148:

```rust
let ice_swap: SwappableIceServers = Arc::new(ArcSwap::from_pointee(
    config.mesh_ice_servers.clone(),
));
let ice_for_signal = ice_swap.clone();
signal.set_peer_callback(Arc::new(move |event| {
    let snapshot = ice_for_signal.load_full();
    // ... use (*snapshot).clone() in the AssignParent message
}));
// ConfigReloadHandle takes ice_swap.clone() as well.
```

On reload, the handle replaces the snapshot:
`ice_swap.store(Arc::new(file.mesh_ice_servers.clone()))`.
In-flight `AssignParent` emits finish against the prior snapshot;
the next `Register` fires the callback with the new snapshot.

### 3. HMAC playback secret reload: thread the swap through the playback + live HLS/DASH middleware

Today `start()`:

```rust
let hmac_playback_secret: Option<Arc<[u8]>> = config
    .hmac_playback_secret
    .as_ref()
    .map(|s| Arc::from(s.as_bytes()));
```

The captured value flows into `LivePlaybackAuthState { hmac_secret, .. }`
on the HLS + DASH live routes, and into `playback_router(.., hmac_playback_secret.clone())`
for the DVR routes.

After session 148:

```rust
let hmac_swap: SwappableHmacSecret = Arc::new(ArcSwap::from_pointee(
    config.hmac_playback_secret
        .as_ref()
        .map(|s| Arc::<[u8]>::from(s.as_bytes())),
));
```

The middleware closures capture `hmac_swap.clone()` and call
`hmac_swap.load_full()` per-request. On reload, the file's
top-level `hmac_playback_secret = "..."` is hashed into a new
`Arc<[u8]>` and the swap stores it. `None` (file omits the field)
clears the secret.

### 4. Reload pipeline extension

`ConfigReloadHandle::reload(kind)` adds two new build steps after
the existing auth-rebuild:

```rust
// Auth rebuild + swap (existing in 147).
self.hot_provider.swap(new_auth_chain);

// Mesh ICE reload (new).
self.ice_swap.store(Arc::new(file.mesh_ice_servers.clone()));

// HMAC secret reload (new).
let new_secret = file.hmac_playback_secret
    .as_ref()
    .map(|s| Arc::<[u8]>::from(s.as_bytes()));
self.hmac_swap.store(Arc::new(new_secret));
```

`applied_keys` gains entries:
* `"auth"` whenever the auth chain rebuilt (every reload today).
* `"mesh_ice"` whenever the file's mesh_ice_servers diff against the prior snapshot.
* `"hmac_secret"` whenever the file's hmac_playback_secret diff against the prior snapshot.

The diff check is on the `Arc<...>` pointer (cheap) and falls
back to deep `==` only when the pointer differs. Operators see
which categories their reload effectively touched.

### 5. Drop the deferred-warning lines

Session 147 added two warning emissions in `ConfigReloadHandle::reload`:

```rust
if !file.mesh_ice_servers.is_empty() {
    warnings.push("mesh_ice_servers in config file ignored: hot reload deferred to a future session");
}
if file.hmac_playback_secret.is_some() {
    warnings.push("hmac_playback_secret in config file ignored: hot reload deferred to a future session");
}
```

Session 148 deletes both. The route's `warnings` field stays in
the wire shape (forward-compat for future deferred categories).

### 6. Anti-scope (explicit rejections)

* **No `jwks_url` / `webhook_auth_url` reload.** Async builders +
  cached HTTP state. Their boot-time values stay; the warn-on-diff
  for those keys remains (so operators see they are not yet
  hot-reloaded). Their reload is a future session's work.
* **No structural-key reload** (port bindings, feature flags,
  record_dir, archive_dir, mesh_enabled, cluster_listen).
  Reload SURFACES warnings when the file's `[diff]`-eligible
  values change vs. boot, but never rebinds sockets or restarts
  subsystems. Operators who need to change those keys still
  bounce the relay.
* **No file watcher.** Explicit SIGHUP / POST only.
* **No per-key partial reload.** All three categories
  (auth, mesh_ice, hmac_secret) reload atomically, or the prior
  state stays live (build failure is rolled back).
* **No SDK shape change** beyond growing `applied_keys` entries.
  TS + Python clients already accept the array generically.
* **No version bump or publish.** Workspace stays at 0.4.1;
  SDK packages stay at 0.3.2.

## Execution order

1. **Author this briefing.** Done (post-147 close).

2. **Land the swap-handle plumbing.**
   * `crates/lvqr-cli/src/lib.rs`: build `SwappableIceServers`
     + `SwappableHmacSecret` in `start()` from
     `config.mesh_ice_servers` + `config.hmac_playback_secret`.
   * Refactor the signal callback to load the ICE list per call.
   * Refactor `LivePlaybackAuthState` + `playback_router` to take
     a swap handle instead of a captured `Option<Arc<[u8]>>`.

3. **Extend `ConfigReloadHandle`.**
   * Add `ice_swap` + `hmac_swap` fields.
   * Extend `reload(kind)` to swap both.
   * Drop the two deferred-warning emissions.

4. **Land integration tests.**
   * `crates/lvqr-cli/tests/config_reload_e2e.rs`: add two cases.
     (a) Mesh ICE swap: TestServer with mesh enabled, file with
     one ICE server. Open `/signal`, observe `AssignParent` with
     the boot list. Rewrite file with a different ICE server,
     POST reload, open another `/signal`, observe `AssignParent`
     with the new list. (b) HMAC secret swap: TestServer with
     `--hmac-playback-secret <boot>`. Sign a playback URL with
     the boot secret, GET `/playback/...`, expect 200. Rewrite
     file with a different secret, POST reload, the OLD-signed
     URL now 403; a new-signed URL 200.

5. **Land docs.**
   * `docs/config-reload.md`: drop the "deferred" callouts for
     mesh_ice_servers + hmac_playback_secret; move them to the
     "what hot-reloads" section.

6. **Land HANDOFF + README.**
   * README "Recently shipped" gains a session 148 entry.
   * HANDOFF session 148 close block.

7. **Push + verify CI green.**

## Risks + mitigations

* **`LivePlaybackAuthState` clones the secret per request.** The
  current code captures the `Option<Arc<[u8]>>` once and clones
  the Arc per request (Arc::clone is one atomic increment).
  Switching to `ArcSwap::load_full` returns an `Arc<Option<Arc<[u8]>>>`,
  one extra dereference; same single-digit-ns cost. Verify with
  the existing live_signed_url_e2e tests.

* **Mesh signal callback is on the hot path** (every Register
  fires it). `ArcSwap::load_full` returns an `Arc<Vec<IceServer>>`
  the closure clones into the wire message. Today's clone is
  identical -- the swap adds one ArcSwap::load on top.

* **Reload mid-`AssignParent`.** A SIGHUP firing while the signal
  callback is composing an `AssignParent`: the callback already
  loaded its snapshot before the swap. The message ships with the
  prior snapshot. The next callback invocation gets the new
  snapshot. No connection state is destroyed.

* **HMAC secret rotation invalidates outstanding URLs.** Operators
  rotating the secret expect outstanding signed URLs to stop
  working. The new secret is the only valid signer; the old one
  rejects on `/playback/*` and live HLS/DASH. This is the
  documented intent.

* **`Arc<[u8]>` clone semantics.** When the file omits
  `hmac_playback_secret`, the swap stores `Arc::new(None)`. The
  middleware then falls back to the standard subscribe-token gate.
  Tests cover both rotate-existing-secret and clear-secret cases.

## Ground truth (session 148 brief-write)

* **Head**: `7abadd0` on `main` (post-147). v0.4.1 unchanged.
  Workspace at **1099** tests / 0 / 0 (131 test binaries).
* **lvqr-auth shape**: `HotReloadAuthProvider` ships with the
  always-on wrap (session 147 step 1).
* **lvqr-cli shape**: `ConfigReloadHandle` owns the auth
  hot-swap; `start()` builds it once at boot.
* **lvqr-admin shape**: 12 route trees post-147. Session 148
  does NOT add a new route -- the admin POST returns the same
  `ConfigReloadStatus` shape, just with extended `applied_keys`.
* **CI**: All 8 GitHub Actions workflows GREEN on session 147's
  substantive heads (`e1465c5`, `7abadd0` in flight at brief-
  write).
* **Tests**: Net additions expected from this session: roughly
  +4 lvqr-cli unit (ice + hmac swap path, diff-detection
  helper) + 2 RTMP-style integration tests + 0 SDK changes,
  putting the workspace at ~1105 / 0 / 0, pytest 38 (unchanged),
  Vitest 13 (unchanged).

## Step 0 deliverable -- this briefing

Author at `tracking/SESSION_148_BRIEFING.md`. Read sections 1
through 5 in order; the actual implementation order is in section
"Execution order". The author of session 148 should re-read
`crates/lvqr-cli/src/config_reload.rs` first (the canonical 147
shape ConfigReloadHandle is being extended), then
`crates/lvqr-cli/src/lib.rs` around the signal callback +
`LivePlaybackAuthState` (the two refactor sites), and
`crates/lvqr-cli/tests/config_reload_e2e.rs` (the integration test
shape session 148 mirrors).

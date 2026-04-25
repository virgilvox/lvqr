# Session 149 Briefing -- Hot config reload v3: `jwks_url` + `webhook_auth_url`

**Date kick-off**: 2026-04-25 (locked at end of session 148; actual
implementation session 149 picks up from here).
**Predecessor**: Session 148 (hot config reload v2: mesh ICE + HMAC
secret). The `mesh_ice_servers` and `hmac_playback_secret` reload
landed clean alongside the auth section; default-gate tests at
**1107** / 0 / 0, admin surface at **12 route trees**, origin/main
head `807de76`. Session 148 explicitly deferred the two remaining
hot-reload-eligible auth keys (`jwks_url`, `webhook_auth_url`); the
file parser tolerates them but the reload pipeline does not rebuild
their providers. Session 149 closes that final gap.

## Goal

Today operators rotating the JWKS endpoint URL or the webhook auth
endpoint must bounce the relay -- session 148's reload pipeline
parses both keys but never instantiates a fresh `JwksAuthProvider`
or `WebhookAuthProvider`. Both keys are operator-mutable
(per-deployment IdP discovery, webhook URL rotation during incident
response) and would benefit from the same SIGHUP / admin POST
trigger the auth section / mesh ICE / HMAC secret already use.

After this session, `lvqr serve --config <path>` extends to
hot-reload the JWKS endpoint URL and the webhook auth URL alongside
the existing categories. The admin route's `applied_keys` field
grows to include `"jwks"` and `"webhook"` when those reload
effectively. With session 149 shipped, every hot-reloadable key the
file format defines is honored at runtime; the project's hot config
reload story is feature-complete.

## Decisions to confirm on read-back

### 1. Apply mechanism: async `reload`

Session 147 + 148's `ConfigReloadHandle::reload(&self, kind: &str)`
is synchronous. Session 149 must run async constructors
(`JwksAuthProvider::new(config).await`,
`WebhookAuthProvider::new(config).await`), so `reload` becomes:

```rust
pub async fn reload(&self, kind: &str) -> Result<ConfigReloadStatus> {
    // ... (existing sync work for static / JWT / mesh_ice / hmac) ...
    if jwks_url_changed_and_feature_enabled {
        let new_jwks = JwksAuthProvider::new(jwks_cfg).await?;
        // swap into chain
    }
    // ...
}
```

Call-site updates:
* `crates/lvqr-cli/src/lib.rs` -- the boot-time
  `handle.reload("boot")` call already runs inside `pub async fn
  start()`; just `.await` it. The SIGHUP listener task already
  spawns inside `tokio::spawn(async move { ... })`; same `.await`.
* `crates/lvqr-admin/src/config_reload_routes.rs` -- the POST
  handler is already `async fn`; the closure passed via
  `AdminState::with_config_reload` flips from sync `Box<dyn Fn(...)
  -> Result<...>>` to async `Box<dyn Fn(...) -> BoxFuture<'_,
  Result<...>>>`. The route response shape stays unchanged.

### 2. Provider lifetime: drop the old, swap in the new

Both `JwksAuthProvider` and `WebhookAuthProvider` already implement
`Drop` to abort their spawned background tasks (refresh / fetcher
loops). When `ConfigReloadHandle::reload` swaps a fresh provider
into the chain, the old provider's `Arc<...>` count drops to zero
the moment the swap completes (no in-flight `check()` retains it),
and `Drop::drop` aborts its background task.

The HotReloadAuthProvider chain rebuild already exists from session
147. Session 149 just extends `build_static_auth_from_effective`
into an async builder that, when the file's auth section names
`jwks_url`, instantiates `JwksAuthProvider` instead of the static /
JWT cascade; likewise for `webhook_auth_url`.

### 3. Diff trigger

`applied_keys` gains:
* `"jwks"` when `file.auth.jwks_url` differs from the current
  effective JWKS URL (operator just changed the discovery endpoint).
* `"webhook"` when `file.auth.webhook_auth_url` differs from the
  current effective webhook URL.

The diff is on the `Option<String>` URL field; ancillary tunables
(`jwks_refresh_interval`, `webhook_*_cache_ttl`) ride along with
the URL change but are not their own diff key (operators tuning
TTLs without changing the URL still get a rebuild because the URL
is the trigger; this is documented as the rebuild semantic).

### 4. Feature-gate posture

`JwksAuthProvider` is behind `lvqr-auth/jwks`; `WebhookAuthProvider`
is behind `lvqr-auth/webhook`. The reload pipeline must compile
without these features (the feature-gated CLI defaults from boot
already work that way). Session 149 wraps the rebuild branches in
`#[cfg(feature = "jwks")]` / `#[cfg(feature = "webhook")]`. When a
feature is OFF and the file names the corresponding URL, the route
emits a warning (`"jwks_url in config file ignored: lvqr-cli was
built without --features jwks"`); this is the SAME shape session
147 used for the deferred keys, repurposed for the
feature-disabled case.

### 5. Anti-scope (explicit rejections)

* **No JWKS cache preservation across URL change.** Rotating the
  URL drops the old key cache wholesale (the new provider starts
  with an empty cache and fetches on first check). Operators
  rotating keys at the SAME URL rely on the existing periodic
  refresh task -- no reload required for that case.
* **No webhook decision-cache preservation across URL change.**
  Same posture: new URL = new cache. The old provider's `Drop`
  abort-fetcher path already guarantees the old fetcher exits.
* **No structural-key reload** (port bindings, feature flags,
  record_dir, archive_dir, mesh_enabled, cluster_listen). Reload
  never rebinds sockets or restarts subsystems.
* **No file watcher.** Explicit SIGHUP / POST only.
* **No per-key partial reload.** All five categories
  (auth + mesh_ice + hmac_secret + jwks + webhook) reload
  atomically, or the prior state stays live (build failure is
  rolled back).
* **No SDK shape change** beyond growing `applied_keys` entries.
* **No version bump or publish.** Workspace stays at 0.4.1; SDK
  packages stay at 0.3.2.

## Execution order

1. **Author this briefing.** Done (post-148 close).

2. **Flip `ConfigReloadHandle::reload` to async.**
   * `crates/lvqr-cli/src/config_reload.rs`: add `async` to the
     `reload` signature; downstream `.await` updates in
     `crates/lvqr-cli/src/lib.rs` (boot + SIGHUP listener) and
     `crates/lvqr-admin/src/config_reload_routes.rs` (the closure
     wired via `AdminState::with_config_reload`).
   * Sync `build_static_auth_from_effective` is preserved (still
     called for the no-jwks-no-webhook cascade); a new
     `build_auth_from_effective` async function picks JWKS or
     webhook when their URL is set, falling through to the sync
     builder otherwise.

3. **Extend the reload pipeline.**
   * Capture the prior effective `jwks_url` + `webhook_auth_url`
     on the handle (alongside `boot_defaults`).
   * Diff against `file.auth.jwks_url` / `file.auth.webhook_auth_url`.
   * If changed AND feature enabled: rebuild the provider via its
     async constructor, then swap the auth chain via `hot_provider.
     swap(new_chain)`.
   * Push `"jwks"` / `"webhook"` into `applied_keys` only on
     effective rebuild.

4. **Land integration tests.**
   * `crates/lvqr-cli/tests/config_reload_e2e.rs`: feature-gated
     `#[cfg(feature = "jwks")]` test that boots a TestServer
     pointing at a `wiremock` JWKS server, mints a JWT signed
     under the boot key, GET succeeds. Rotates the file to point
     at a second JWKS server, POST reload, the boot-key JWT now
     denies; a fresh JWT signed under the second server's key
     allows.
   * Sister `#[cfg(feature = "webhook")]` test: mock webhook URL
     A returns allow for tokens prefixed `a-`; URL B returns allow
     for `b-`. Reload swaps the URL; old `a-` token denies, new
     `b-` token allows.

5. **Land docs.**
   * `docs/config-reload.md`: drop the deferred posture for
     `jwks_url` + `webhook_auth_url`; document the async-reload
     mechanic + the feature-disabled warning shape.

6. **Land HANDOFF + README.**
   * README "Recently shipped" gains a session 149 entry; the
     existing hot-config-reload sub-bullet updates to "v3 shipped --
     all five hot-reloadable keys honored".
   * HANDOFF session 149 close block.

7. **Push + verify CI green.**

## Risks + mitigations

* **Async closure churn in `AdminState::with_config_reload`.**
  Today the route closure is `Box<dyn Fn(&str) -> Result<...> + Send + Sync>`.
  Flipping to async means the closure returns
  `Pin<Box<dyn Future<Output = Result<...>> + Send + '_>>`.
  Mitigation: keep the route signature the same; the async-closure
  type alias change is internal to `lvqr-admin` and `lvqr-cli`. SDK
  shape unchanged (the wire response is the same).

* **JWKS rebuild can block on a slow IdP.** `JwksAuthProvider::new`
  performs an initial fetch with a `fetch_timeout` (default 5 s).
  If the IdP is slow or down, the reload route's POST hangs for up
  to that timeout. Mitigation: documented in the response body
  ("rebuild took N ms"; wider operator monitoring catches outliers
  via the existing `lvqr_config_reload_duration_seconds` metric).
  The prior provider stays live during the rebuild, so a failure
  doesn't break the running auth path.

* **Concurrent reloads racing the swap.** Two simultaneous
  `POST /api/v1/config-reload` could both build a fresh provider,
  with the second swap winning. Mitigation: serialize reloads via
  the existing `parking_lot::Mutex` already on
  `ConfigReloadHandle.state` (extend the lock scope to cover the
  build, not just the post-swap state mutate).

* **Feature-gated rebuild path adds two new compile permutations.**
  Mitigation: CI's existing feature matrix already builds with
  `--features jwks`, `--features webhook`, and `--features full`;
  session 149 just adds tests, not new feature combinations.

* **`AdminState::with_config_reload` callers in tests.** Tests that
  pre-build an admin state may need an `async` constructor.
  Mitigation: only the route closure type changes; tests that don't
  pass a closure (most of them) are unaffected.

## Ground truth (session 149 brief-write)

* **Head**: `807de76` on `main` (post-148). v0.4.1 unchanged.
  Workspace at **1107** tests / 0 / 0 (131 test binaries).
* **lvqr-auth shape**: `JwksAuthProvider` (jwks feature) and
  `WebhookAuthProvider` (webhook feature) ship with their own
  spawned refresh / fetcher tasks; `Drop` aborts them cleanly.
* **lvqr-cli shape**: `ConfigReloadHandle` owns the auth
  hot-swap, ICE swap, HMAC swap; `start()` builds it once at boot.
* **lvqr-admin shape**: 12 route trees post-148. Session 149 does
  NOT add a new route -- the admin POST returns the same
  `ConfigReloadStatus` shape, just with extended `applied_keys`.
* **CI**: 8 GitHub Actions workflows GREEN on session 148's
  substantive heads (`807de76` push at brief-write).
* **Tests**: Net additions expected from this session: roughly
  +5 lvqr-auth unit (jwks reload, webhook reload, drop-on-swap
  guard), +2 lvqr-cli unit (config_reload extends to jwks/webhook
  diff), +2 RTMP-shape integration tests behind feature gates,
  putting the workspace at ~1116 / 0 / 0 when compiled with
  `--features full`. Default-gate tests stay at 1107 plus the
  +2 lvqr-cli unit (the lvqr-auth + integration tests are
  feature-gated and don't run on the default-features cargo test
  pass).

## Step 0 deliverable -- this briefing

Author at `tracking/SESSION_149_BRIEFING.md`. Read sections 1
through 5 in order; the actual implementation order is in section
"Execution order". The author of session 149 should re-read
`crates/lvqr-cli/src/config_reload.rs` first (the canonical 147+148
shape `ConfigReloadHandle::reload` is being flipped to async), then
`crates/lvqr-auth/src/jwks_provider.rs` + `webhook_provider.rs`
(the async constructors + Drop semantics being thread through), and
`crates/lvqr-admin/src/config_reload_routes.rs` (the route closure
type being widened from sync to async). Tests target a +2 default-
gate net + ~+7 feature-gated.

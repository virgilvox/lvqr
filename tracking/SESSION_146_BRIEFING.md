# Session 146 Briefing -- Stream-key CRUD admin API

**Date kick-off**: 2026-04-24 (locked at end of session 145; actual
implementation session 146 picks up from here).
**Predecessor**: Session 145 (workspace 0.4.1 republish + audit
preflight). The release shipped clean; supply-chain audit is GREEN
on `origin/main`. Next-up list in README is now headed by the
shippable single-session items, with stream-key CRUD at #1.

## Goal

Today operators provision stream keys via static config: either
`LVQR_PUBLISH_KEY` (one shared key for every RTMP / SRT / WHIP
publish via `StaticAuthProvider`) or a JWT secret / JWKS endpoint
(every key is a JWT minted out-of-band). Both shapes force a
server bounce or an external mint pipeline for every operator-side
key change.

After this session, `lvqr-admin` exposes
`/api/v1/streamkeys` so an admin client can mint, list, revoke,
and rotate stream keys at runtime; a new `MultiKeyAuthProvider`
in `lvqr-auth` is a drop-in replacement for `StaticAuthProvider`
that authenticates publishes against the live key set instead of
a single fixed value. JS + Python admin clients grow matching
methods. Real integration test boots `lvqr serve`, mints a key
via the admin API, RTMP-publishes with that key, revokes it, and
asserts the next publish attempt is denied.

## Decisions locked

### 1. Wire shape: REST under `/api/v1/streamkeys`

```
GET    /api/v1/streamkeys                -> list { keys: [StreamKey] }
POST   /api/v1/streamkeys                -> mint { spec } -> StreamKey
DELETE /api/v1/streamkeys/:id            -> revoke
POST   /api/v1/streamkeys/:id/rotate     -> rotate (returns new token)
```

Where `StreamKey` is:

```rust
pub struct StreamKey {
    pub id: String,                  // server-assigned, UUID-ish
    pub token: String,               // the bearer string an ingest carries
    pub label: Option<String>,       // operator-friendly name
    pub broadcast: Option<String>,   // when set, key only authorises this broadcast
    pub created_at: u64,             // unix seconds
    pub expires_at: Option<u64>,     // unix seconds; None = no expiry
}
```

* Mint request body is `StreamKeySpec { label, broadcast, ttl_seconds }`;
  server fills in `id`, `token`, `created_at`, `expires_at`.
* Rotate generates a NEW `token` (and resets `created_at`); preserves
  `id`, `label`, `broadcast`, `expires_at` unless the rotate body
  overrides them. Old token is invalidated immediately.
* Revoke is hard-delete; no tombstone in v1.

`#[serde(default)]` on every Optional field so the wire shape can
grow new optional fields without breaking pre-146 clients.

### 2. Storage: `StreamKeyStore` trait + in-memory impl

```rust
pub trait StreamKeyStore: Send + Sync + 'static {
    fn list(&self) -> Vec<StreamKey>;
    fn get(&self, id: &str) -> Option<StreamKey>;
    fn get_by_token(&self, token: &str) -> Option<StreamKey>;
    fn mint(&self, spec: StreamKeySpec) -> StreamKey;
    fn revoke(&self, id: &str) -> bool;
    fn rotate(&self, id: &str, override_spec: Option<StreamKeySpec>) -> Option<StreamKey>;
}

pub type SharedStreamKeyStore = Arc<dyn StreamKeyStore>;
```

* `InMemoryStreamKeyStore` wraps a `DashMap<String, StreamKey>`
  keyed by `id`, plus a `DashMap<String, String>` reverse index
  `token -> id` for O(1) lookup at the auth-check hot path.
* No persistence backend in v1. Restart loses all minted keys.
  Operators pin known keys to startup config (existing
  `LVQR_PUBLISH_KEY` continues to work as today via
  `StaticAuthProvider`); the CRUD API is for dynamic ones.
* Sled / SQLite backing is its own session; the trait is shaped
  so the swap is purely additive.

### 3. Auth integration: `MultiKeyAuthProvider`

```rust
pub struct MultiKeyAuthProvider {
    store: SharedStreamKeyStore,
    fallback: Option<SharedAuth>,    // delegate when key not in store
}
```

* `check` for `AuthContext::Publish { key, broadcast, .. }`:
  1. `store.get_by_token(key)` -> if hit, check `expires_at` and
     `broadcast` scoping; allow or deny accordingly.
  2. If miss and `fallback.is_some()`, delegate to fallback.
  3. Otherwise deny.
* `check` for `Subscribe` and `Admin` always delegates to fallback
  (or denies if no fallback). The store is publish-only in v1.
* Composable: an operator can run JWT for admin + stream-keys for
  publish by setting `MultiKeyAuthProvider { store, fallback: Some(jwt_provider) }`.

### 4. CLI wiring

* `lvqr-cli` boot: when any of the existing publish-auth flags
  (`--auth-secret-jwt`, `--jwks-url`, `--webhook-auth-url`) are NOT
  set, install `MultiKeyAuthProvider { store: in_memory, fallback: None }`.
  Stream keys CRUD then becomes the default operator-facing path.
* When a publish-auth flag IS set, retain the existing provider but
  ALSO mount `MultiKeyAuthProvider { store: in_memory, fallback:
  Some(existing) }` so admin-minted keys take precedence and JWTs
  still work for operators with external mint pipelines.
* New `--no-streamkeys` flag opts out entirely (returns to today's
  shape: single `StaticAuthProvider` only).
* Admin route mounting: only when at least one of the publish-auth
  paths is configured; if everything is open (Noop), the routes are
  still mounted but mint/revoke have no enforcement effect (still
  useful for testing / observability).

### 5. Admin client surface

* `@lvqr/core/admin.ts`: new `streamKeys()` family with shape
  matching the Rust types byte-for-byte. Methods: `list()`,
  `mint(spec)`, `revoke(id)`, `rotate(id, override?)`.
* `bindings/python/python/lvqr/client.py`: same shape with
  defensive `.get(...)` parsers for cross-version compat.
* TypeScript + Python both grow `StreamKey` + `StreamKeySpec`
  type / dataclass with `#[serde(default)]`-equivalent semantics.

### 6. Integration test

`crates/lvqr-cli/tests/streamkeys_e2e.rs` (new):

1. Boot `TestServer`. No publish-auth flags so `MultiKeyAuthProvider`
   defaults install with empty store.
2. `POST /api/v1/streamkeys` with `{ label: "test", broadcast:
   "live/test" }`. Capture returned `token`.
3. RTMP publish with `stream_key = token` (use existing
   `lvqr-test-utils` ffmpeg push helper). Assert publish lands
   (broadcast appears in `/api/v1/streams`).
4. `DELETE /api/v1/streamkeys/:id`. Drop the publish.
5. RTMP publish again with the same `token`. Assert publish is
   rejected (auth deny surfaces as RTMP-side connection close
   plus a counter increment on `lvqr_auth_publish_denied_total`).

Real integration. No mocks. Matches the project's "real connections,
not mocks" rule per CLAUDE.md.

## Anti-scope (explicit rejections for this session)

* **No persistence backend.** In-memory only. Operator restart
  loses keys; static `LVQR_PUBLISH_KEY` continues to work for
  durable single-key shapes. Sled / SQLite store is its own
  session.
* **No per-key rate limits.** Counter machinery exists at the
  fragment / bytes layer; per-key splits would need a richer
  data model and are operator-demand-driven.
* **No daemon expiry sweep.** `expires_at` is checked on the
  auth path (lazy expiry); a future session can add a background
  task. Lazy expiry is correct semantically; only operator-facing
  cosmetics on the list endpoint show expired keys until next
  cleanup.
* **No subscribe-token CRUD.** Same surface could in principle
  expose viewer tokens, but `Subscribe` auth's existing
  HMAC-signed-URL path (sessions 124/128) already covers the
  common need. Adding a second one without operator demand is
  scope creep.
* **No JWT-mint endpoint.** This session adds STORE keys, not
  signed JWTs. Operators wanting signed JWTs continue to use
  their own minting pipeline.
* **No bulk operations.** Mint / revoke / rotate are per-key.
  Bulk import / export is its own session.
* **No webhook on key changes.** Operators wanting "key minted"
  callouts can poll the list endpoint or watch the
  `lvqr_streamkeys_changed_total` counter (added as a basic
  observability surface).
* **No auth-config-reload bundling.** Hot config reload is a
  separate next-up item (#2 in README post-cleanup); this
  session does NOT take on SIGHUP handling.

## Execution order

1. **Author this briefing.** Done (post-145 close, before any
   source touch).

2. **Land the lvqr-auth additions.**
   * `crates/lvqr-auth/src/stream_key_store.rs` (new): trait +
     `InMemoryStreamKeyStore` + tests.
   * `crates/lvqr-auth/src/multi_key_provider.rs` (new):
     `MultiKeyAuthProvider` + tests.
   * `crates/lvqr-auth/src/lib.rs`: re-exports.
   * `cargo test -p lvqr-auth --lib` clean.

3. **Land the admin routes.**
   * `crates/lvqr-admin/src/routes.rs`: 4 new handlers.
   * `crates/lvqr-admin/src/lib.rs`: re-exports + register.
   * `cargo test -p lvqr-admin --lib` clean.

4. **Land the lvqr-cli wiring.**
   * `crates/lvqr-cli/src/lib.rs::start()`: install
     `MultiKeyAuthProvider` per the rules in section 4 above.
   * `crates/lvqr-cli/src/main.rs`: new `--no-streamkeys` flag.

5. **Land the SDK client surfaces.**
   * `bindings/js/packages/core/src/admin.ts`: new types +
     methods.
   * `bindings/python/python/lvqr/{types,client,__init__}.py`:
     same shape.
   * Vitest + pytest cases extending the existing live-server
     suite (streamkeys list returns []; mint returns valid
     shape; revoke returns 204).

6. **Land the integration test.**
   * `crates/lvqr-cli/tests/streamkeys_e2e.rs` (new). Real
     RTMP publish + admin mint + revoke flow.

7. **Docs.**
   * `docs/auth.md` grows a "Stream-key CRUD" section.
   * `docs/sdk/javascript.md` + `docs/sdk/python.md` grow the
     types.
   * README's "Next up" item 1 flips from `[ ]` to `[x] shipped
     in session 146`.

8. **Session 146 close block in HANDOFF.**

9. **Push + verify CI green.**

## Risks + mitigations

* **`MultiKeyAuthProvider` ordering ambiguity if both store-hit
  and JWT-valid paths could allow a publish.** Section 3 locks
  store-first, fallback-second. Tests assert the order: a JWT
  that would normally allow a publish is rejected if a stream
  key with the SAME token is present and revoked.

* **Token collision in `InMemoryStreamKeyStore`.** Token
  generation uses 32 bytes of `OsRng` base64-encoded; collision
  probability is negligible. Reverse index `token -> id` panics
  on a collision (we trust the RNG).

* **Admin route lockout.** If an operator misconfigures the
  fallback path and revokes the only admin token, they would be
  locked out. v1 keeps Admin auth on the existing chain
  (`AuthContext::Admin` always delegates to fallback) so
  stream-key CRUD does not change admin auth semantics. Section
  3's "Admin always delegates to fallback" lock is the
  load-bearing safety property.

* **Wire-shape drift across SDK clients.** Vitest + pytest
  shape-test the response bodies against a live `lvqr serve`;
  any field added to the Rust types without a matching
  TypeScript / Python type update fails the SDK CI on the next
  push.

## Ground truth (session 146 brief-write)

* **Head**: `9a4d026` on `main` (post-145 cleanup landed). v0.4.1
  on crates.io. Workspace builds clean.
* **lvqr-auth shape**: `AuthProvider` trait + 5 impls (Noop,
  Static, Jwt, Jwks, Webhook). Single-key publish surface lives
  on `StaticAuthProvider`.
* **lvqr-admin shape**: 10 routes, JSON body shape pattern matches
  `MeshPeerStats` from session 144 (each route's response struct
  is `Serialize + Deserialize` with `#[serde(default)]` on every
  Optional field).
* **CI**: Supply-chain audit GREEN. All other jobs green or
  in-progress on the post-145-close head.
* **Tests**: Rust workspace 1043 / 0 / 3, pytest 30, Vitest 11.
  Net additions expected from this session: roughly +6 lvqr-auth
  unit tests + 4 lvqr-admin route tests + 1-2 RTMP integration
  test + 2 Vitest live + 2 pytest defensive-parse, putting the
  workspace at ~1055 / 0 / 3, pytest 32, Vitest 13.

## Step 0 deliverable -- this briefing

Author at `tracking/SESSION_146_BRIEFING.md`. Read sections 1
through 4 in order; the actual implementation order is in section
"Execution order". The author of session 146 should re-read
`crates/lvqr-auth/src/static_provider.rs` first (the canonical
single-key reference impl), then `crates/lvqr-admin/src/routes.rs`
(the canonical admin-route shape) before opening any new file.

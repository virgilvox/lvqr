# Hot config reload

`lvqr serve --config <path>` plus an admin route let operators
rotate auth providers, mesh ICE servers, the HMAC playback secret,
JWKS endpoint URLs, and webhook auth URLs without bouncing the
relay. SIGHUP (Unix) and `POST /api/v1/config-reload`
(cross-platform) feed into the same reload pipeline.

Session 147 shipped the auth-section reload (v1, auth-only).
Session 148 added `mesh_ice_servers` and `hmac_playback_secret`
(v2). Session 149 closed the final deferred-key gap by adding
`jwks_url` and `webhook_auth_url` (v3): the reload pipeline is now
async so it can run `JwksAuthProvider::new` /
`WebhookAuthProvider::new` constructors mid-process and atomically
swap the resulting provider into the auth chain. Every key the
file format defines is honored at runtime.

## Quick start

```bash
# 1. Write a config file.
cat > /etc/lvqr.toml <<'EOF'
hmac_playback_secret = "rotate-me-monthly"

[auth]
publish_key = "operator-secret-v1"
admin_token = "ops-team"

[[mesh_ice_servers]]
urls = ["turn:turn.example:3478"]
username = "u"
credential = "p"
EOF

# 2. Boot lvqr against it.
lvqr serve --config /etc/lvqr.toml --admin-port 8080

# 3. Rotate the publish key, the HMAC secret, or the TURN credential.
sed -i 's/operator-secret-v1/operator-secret-v2/' /etc/lvqr.toml

# 4. Trigger reload (Unix SIGHUP, cross-platform admin POST).
kill -HUP $(pgrep lvqr)
# or
curl -X POST -H "Authorization: Bearer ops-team" \
     http://localhost:8080/api/v1/config-reload
```

The response body confirms which categories the reload effectively
touched:

```json
{
  "config_path": "/etc/lvqr.toml",
  "last_reload_at_ms": 1735170000000,
  "last_reload_kind": "admin_post",
  "applied_keys": ["auth", "mesh_ice", "hmac_secret"],
  "warnings": []
}
```

`applied_keys` lists only the categories whose effective value
changed against the prior snapshot. A reload that only touches the
`[auth]` section returns `applied_keys: ["auth"]`; a TURN credential
rotation returns `["auth", "mesh_ice"]`; a no-op reload still rebuilds
auth (so `"auth"` is always present) but omits the other two when
their values match the prior snapshot.

## File format

TOML. All fields optional. Sections the operator omits fall back to
the corresponding CLI flag / env var at boot, and clear the
hot-reloadable category on subsequent reloads (see "Clear semantics"
below).

```toml
# Top-level HMAC playback secret. Hot-reloads (session 148).
hmac_playback_secret = "deadbeef..."

# Auth section.
[auth]
admin_token = "..."        # mirrors LVQR_ADMIN_TOKEN
publish_key = "..."        # mirrors LVQR_PUBLISH_KEY
subscribe_token = "..."    # mirrors LVQR_SUBSCRIBE_TOKEN
jwt_secret = "..."         # HS256 secret
jwt_issuer = "..."         # expected `iss` claim
jwt_audience = "..."       # expected `aud` claim
jwks_url = "..."           # JWKS endpoint URL; hot-reloads (session 149, requires --features jwks)
webhook_auth_url = "..."   # decision-webhook URL; hot-reloads (session 149, requires --features webhook)

# Mesh ICE servers. Hot-reloads (session 148).
[[mesh_ice_servers]]
urls = ["stun:stun.l.google.com:19302"]

[[mesh_ice_servers]]
urls = ["turn:turn.example.com:3478"]
username = "u"
credential = "p"
```

## What hot-reloads

* **`[auth]` section** -- the static-token (`admin_token` /
  `publish_key` / `subscribe_token`) and HS256 JWT (`jwt_secret`,
  `jwt_issuer`, `jwt_audience`) paths. Reload rebuilds the inner
  provider against the merged (CLI defaults + file overrides) shape
  and atomically swaps the live `SharedAuth`. In-flight `check()`
  calls finish against the prior snapshot; subsequent calls see the
  new provider.
* **`mesh_ice_servers`** (session 148) -- the operator-configured
  STUN / TURN list pushed to mesh peers via `AssignParent`. The
  `/signal` callback `load_full`s the swap once per Register, so the
  next peer to connect after a reload sees the new list. In-flight
  `AssignParent` emits finish against the prior snapshot.
* **`hmac_playback_secret`** (session 148) -- the HMAC-SHA256 key
  used by live HLS / DASH and DVR `/playback/*` middleware to verify
  `?sig=...&exp=...`. Reload swaps the secret atomically; the next
  request loads the new value. URLs signed under the prior secret
  stop verifying (the documented intent of a secret rotation).
* **`jwks_url`** (session 149, requires `--features jwks`) -- the
  JWKS discovery endpoint URL. Reload's async pipeline calls
  `JwksAuthProvider::new(...)` (which performs an initial HTTP
  fetch of the new URL's JWK set) and atomically swaps the
  resulting provider into the auth chain. The old provider's
  `Drop` aborts its periodic refresh task; its key cache is
  dropped wholesale (the new URL gets a fresh cache). Operators
  rotating keys at the SAME URL rely on the existing periodic
  refresh task; no reload required for that case.
* **`webhook_auth_url`** (session 149, requires `--features
  webhook`) -- the operator decision-webhook URL. Reload's async
  pipeline calls `WebhookAuthProvider::new(...)` (URL-syntax
  validation only; no probe of the endpoint) and swaps the
  resulting provider in. The old provider's `Drop` aborts its
  fetcher task; its decision cache is dropped wholesale.
  Outstanding cached `Allow` decisions for the prior URL stop
  applying.

The `jwks_url` and `webhook_auth_url` rebuilds happen
unconditionally on every reload when their URL is set
(meaning each reload triggers one HTTP fetch for JWKS users,
and one URL validation for webhook users). Operators who want
to skip the rebuild on no-op reloads should use SIGHUP or admin
POST sparingly; both providers' background tasks already keep
the cache fresh on the operator's chosen `refresh_interval` /
`allow_cache_ttl` cadence.

## Clear semantics

Omitting a hot-reloadable key from the file CLEARS the corresponding
runtime state on the next reload:

* **`mesh_ice_servers` absent (or empty array)** -> the swap stores
  an empty `Vec<IceServer>`. Subsequent `AssignParent` emits ship
  `ice_servers: []`; JS clients fall back to the
  `MeshPeer`-constructor default (typically a hardcoded Google STUN
  server).
* **`hmac_playback_secret` absent** -> the swap stores `None`. The
  live HLS / DASH and `/playback/*` middlewares stop honoring
  `?sig=...&exp=...` and fall through to the standard subscribe-token
  gate.
* **`[auth]` section absent** -> the merged effective shape falls
  back to whatever the CLI booted with (`AuthBootDefaults`); see the
  cascade in `crates/lvqr-cli/src/config_reload.rs::AuthBootDefaults`.

## What is preserved across reloads

* **The runtime stream-key store** (session 146). Operators manage
  it via the `/api/v1/streamkeys/*` CRUD API; reload never touches
  store contents. The fresh `MultiKeyAuthProvider` chain rebuilt on
  reload reuses the same store handle.

## What is NOT hot-reloaded

* **Feature-disabled URLs** -- when the file names `jwks_url` but
  `lvqr-cli` was built without `--features jwks`, the reload route
  surfaces a `warnings` entry naming the file value plus the
  feature flag the operator needs to rebuild with. Same shape for
  `webhook_auth_url` without `--features webhook`. The reload still
  succeeds; the auth chain falls through to static / JWT / Noop.
* **Structural keys** -- port bindings, feature flags, record /
  archive directories, `mesh_enabled`, cluster topology. Reload
  never rebinds sockets or restarts subsystems. Operators changing
  these keys must bounce the relay.

## Composition with other auth providers

`HotReloadAuthProvider` wraps the resolved chain in this order:

```
HotReloadAuthProvider
  -> MultiKeyAuthProvider (session 146; if --no-streamkeys is unset)
       -> JWKS (session 149; rebuilds on URL change, --features jwks)
          OR Webhook (session 149; rebuilds on URL change, --features webhook)
          OR Static / Jwt (rebuilds from file on reload)
          OR Noop (no auth configured)
```

Precedence within the rebuild path: JWKS > Webhook > JWT > Static
> Noop. The file cannot set both `jwks_url` and `webhook_auth_url`
in the same `[auth]` section; the reload route returns an error
naming both keys when this combination is detected.

The wrap is purely additive. When `--config` is unset the wrapper
is still in place but reload is a no-op (SIGHUP listener installs
but never fires; admin POST returns 503). The read fast path is
`ArcSwap::load` plus a delegate -- single-digit nanoseconds, no
lock. The same single-digit-ns cost applies to the mesh ICE and
HMAC swaps now that they share the `arc_swap::ArcSwap` pattern.

## Failure modes

* **File not found / unreadable at boot**: `lvqr serve` exits with a
  clear error before any listener binds. No silent fall-through to
  defaults.
* **Malformed TOML at reload**: the reload errors with the parser's
  source-position message; the prior auth provider, ICE list, and
  HMAC secret all stay live (no partial swap on a failed reload).
  The admin POST returns 500 with the parse error in the body.
* **Provider rebuild rejects**: e.g. JWT init fails because the
  secret rejected by `jsonwebtoken`. Same shape as malformed TOML --
  500, prior state intact.
* **JWKS initial fetch fails** (session 149): the reload pipeline
  awaits the new provider's HTTP fetch of the JWK set with the
  configured `fetch_timeout` (default 10 s). If the fetch errors
  (DNS resolution, TCP timeout, malformed response), the reload
  returns 500 and the prior auth chain stays live. Operators
  should monitor the `lvqr_config_reload_failures_total` metric +
  the route's response body for the specific failure reason.

## Observability

Every successful reload logs:

```
INFO config reload applied kind=sighup applied=2 warnings=0
INFO config reload applied kind=admin_post applied=3 warnings=0
```

`GET /api/v1/config-reload` returns the most recent reload's
metadata for dashboards + scripted polling. The `last_reload_kind`
field distinguishes SIGHUP-driven reloads from admin-API-driven
reloads in audit logs.

## Anti-scope (sessions 148 + 149)

* No file watcher (operator must explicitly SIGHUP or POST).
* No partial reload within a category -- each of the five hot-
  reloadable categories (`[auth]`, `mesh_ice_servers`,
  `hmac_playback_secret`, `jwks_url`, `webhook_auth_url`) reloads
  atomically. A failure during build leaves all prior state in
  place.
* No JWKS / webhook cache preservation across URL change. Rotating
  the URL drops the old key cache / decision cache wholesale; new
  cache builds from scratch.
* No federation / cluster topology reload.
* No version bump; workspace stays at 0.4.1, SDK packages stay at
  0.3.2.

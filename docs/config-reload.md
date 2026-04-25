# Hot config reload

`lvqr serve --config <path>` plus an admin route let operators
rotate auth providers, mesh ICE servers, and the HMAC playback
secret without bouncing the relay. SIGHUP (Unix) and `POST
/api/v1/config-reload` (cross-platform) feed into the same reload
pipeline.

Session 147 shipped the auth-section reload (v1, auth-only).
Session 148 closes the deferred-key gap by hot-reloading
`mesh_ice_servers` and `hmac_playback_secret` alongside auth.

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
jwks_url = "..."           # NOT hot-reloaded (boot-only)
webhook_auth_url = "..."   # NOT hot-reloaded (boot-only)

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

* **`jwks_url`, `webhook_auth_url`** -- their constructors are async
  and cache HTTP state; rebuilding mid-process needs additional
  plumbing that is its own session. Operators using these providers
  retain their boot-time values across reloads. The reload route
  does NOT emit a warning when these keys appear in the file (the
  boot-time apply already wired them); a future session will close
  this gap.
* **Structural keys** -- port bindings, feature flags, record /
  archive directories, `mesh_enabled`, cluster topology. Reload
  never rebinds sockets or restarts subsystems. Operators changing
  these keys must bounce the relay.

## Composition with other auth providers

`HotReloadAuthProvider` wraps the resolved chain in this order:

```
HotReloadAuthProvider
  -> MultiKeyAuthProvider (session 146; if --no-streamkeys is unset)
       -> Static / Jwt (rebuilds from file on reload)
       (or boot-time Jwks / Webhook -- not rebuilt)
```

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

## Anti-scope (session 148)

* No file watcher (operator must explicitly SIGHUP or POST).
* No partial reload within a category -- the `[auth]` section, the
  `mesh_ice_servers` list, and the `hmac_playback_secret` each
  reload atomically. A failure during build leaves the prior state
  in place.
* No `jwks_url` / `webhook_auth_url` reload (deferred to a future
  session because of async-builder + HTTP-cache complexity).
* No federation / cluster topology reload.
* No version bump; workspace stays at 0.4.1, SDK packages stay at
  0.3.2.

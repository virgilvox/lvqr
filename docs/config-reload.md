# Hot config reload

Session 147 added `lvqr serve --config <path>` and an admin route
that lets operators rotate auth providers without bouncing the
relay. SIGHUP (Unix) and `POST /api/v1/config-reload`
(cross-platform) feed into the same reload pipeline.

## Quick start

```bash
# 1. Write a config file.
cat > /etc/lvqr.toml <<'EOF'
[auth]
publish_key = "operator-secret-v1"
admin_token = "ops-team"
EOF

# 2. Boot lvqr against it.
lvqr serve --config /etc/lvqr.toml --admin-port 8080

# 3. Rotate the publish key.
sed -i 's/operator-secret-v1/operator-secret-v2/' /etc/lvqr.toml

# 4. Trigger reload (Unix SIGHUP, cross-platform admin POST).
kill -HUP $(pgrep lvqr)
# or
curl -X POST -H "Authorization: Bearer ops-team" \
     http://localhost:8080/api/v1/config-reload
```

The response body confirms the reload:

```json
{
  "config_path": "/etc/lvqr.toml",
  "last_reload_at_ms": 1735170000000,
  "last_reload_kind": "admin_post",
  "applied_keys": ["auth"],
  "warnings": []
}
```

## File format

TOML. All fields optional. Sections the operator omits fall back to
the corresponding CLI flag / env var.

```toml
# Top-level optional fields (deferred to a future increment for
# hot reload; currently surface as warnings on the reload route
# response when present).
hmac_playback_secret = "deadbeef..."

# Auth section -- the only section that hot-reloads in v1.
[auth]
admin_token = "..."        # mirrors LVQR_ADMIN_TOKEN
publish_key = "..."        # mirrors LVQR_PUBLISH_KEY
subscribe_token = "..."    # mirrors LVQR_SUBSCRIBE_TOKEN
jwt_secret = "..."         # HS256 secret
jwt_issuer = "..."         # expected `iss` claim
jwt_audience = "..."       # expected `aud` claim
jwks_url = "..."           # currently warn-on-diff (deferred)
webhook_auth_url = "..."   # currently warn-on-diff (deferred)

# Mesh ICE servers (deferred to a future increment; warn on diff).
[[mesh_ice_servers]]
urls = ["stun:stun.l.google.com:19302"]
```

## What hot-reloads (v1)

* **`[auth]` section** -- the static-token (`admin_token` /
  `publish_key` / `subscribe_token`) and HS256 JWT (`jwt_secret`,
  `jwt_issuer`, `jwt_audience`) paths. Reload rebuilds the inner
  provider against the merged (CLI defaults + file overrides) shape
  and atomically swaps the live `SharedAuth`. In-flight `check()`
  calls finish against the prior snapshot; subsequent calls see the
  new provider.

## What is preserved across reloads

* **The runtime stream-key store** (session 146). Operators manage
  it via the `/api/v1/streamkeys/*` CRUD API; reload never touches
  store contents. The fresh `MultiKeyAuthProvider` chain rebuilt on
  reload reuses the same store handle.

## What is deferred (warns on diff, no effect this session)

* **`jwks_url`, `webhook_auth_url`** -- their constructors are async
  and cache HTTP state; rebuilding mid-process needs additional
  plumbing. Operators using these providers retain their boot-time
  values across reloads.
* **`hmac_playback_secret`** -- the secret is captured by HLS / DASH
  / playback middleware closures at boot. Threading an `ArcSwap`
  through every middleware is its own session.
* **`mesh_ice_servers`** -- the signal callback closes over the
  list. Same `ArcSwap`-thread-through requirement.

When any deferred section is present in the file, the reload
response carries a `warnings` entry naming the section. The
auth-section reload still proceeds.

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
lock.

## Failure modes

* **File not found / unreadable at boot**: `lvqr serve` exits with a
  clear error before any listener binds. No silent fall-through to
  defaults.
* **Malformed TOML at reload**: the reload errors with the parser's
  source-position message; the prior provider stays live. The admin
  POST returns 500 with the parse error in the body.
* **Provider rebuild rejects**: e.g. JWT init fails because the
  secret rejected by `jsonwebtoken`. Same shape as malformed TOML --
  500 + prior provider intact.

## Observability

Every successful reload logs:

```
INFO config reload (sighup) succeeded warnings=0
INFO config reload (admin_post) succeeded warnings=2
```

`GET /api/v1/config-reload` returns the most recent reload's
metadata for dashboards + scripted polling. The
`last_reload_kind` field distinguishes SIGHUP-driven reloads from
admin-API-driven reloads in audit logs.

## Anti-scope (session 147)

* No file watcher (operator must explicitly SIGHUP or POST).
* No partial reload of the `[auth]` section -- all auth fields
  reload atomically, or the prior provider stays live.
* No mesh ICE servers / HMAC secret reload (deferred; warns on
  diff).
* No federation / cluster topology reload.
* No version bump; SDK packages stay at 0.3.2.

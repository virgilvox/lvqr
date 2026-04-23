# Authentication

LVQR ships one pluggable authentication layer (`lvqr-auth`) behind every
ingest and subscribe surface. A single JWT -- or a single static
publish key, or no gate at all -- admits or denies publishers and
viewers uniformly across every protocol the server speaks.

This document covers:

1. The claim shape and provider configuration.
2. How to pass the credential on each protocol (RTMP, WHIP, SRT, RTSP,
   WebSocket ingest, WebSocket subscribe).
3. One worked example per protocol.

## Providers

Three built-in providers are available:

| Provider | When to use |
|---|---|
| `NoopAuthProvider` | Open access. The default when no other provider is configured. |
| `StaticAuthProvider` | Short single-tenant deployments. Env-configured publish / subscribe / admin tokens. |
| `JwtAuthProvider` (feature `jwt`) | Multi-tenant or time-bound access. HS256 tokens with a shared secret. |

Custom providers implement the `AuthProvider` trait; the `check`
method receives an `AuthContext` and returns `AuthDecision::Allow` or
`AuthDecision::Deny { reason }`.

## JWT claim shape

`JwtAuthProvider` expects HS256 tokens with the following payload:

| Claim | Type | Required | Notes |
|---|---|---|---|
| `sub` | string | yes | Subject identifier. Logged. |
| `exp` | number | yes | Expiry, seconds since epoch. |
| `scope` | `"subscribe" \| "publish" \| "admin"` | yes | Scope hierarchy: admin implies publish implies subscribe. |
| `iss` | string | no | Expected issuer, when `JwtAuthConfig::issuer` is set. |
| `aud` | string | no | Expected audience, when `JwtAuthConfig::audience` is set. |
| `broadcast` | string | no | Binds the token to a specific `<app>/<name>`. Enforced on both publish and subscribe when the ingest surface knows the broadcast name at auth time (WHIP, SRT, RTSP, WS ingest, MoQ subscribe, WS subscribe). RTMP publish skips this binding because the stream key carries the JWT. |

Tokens are validated against `JwtAuthConfig::secret`. The provider is
synchronous; tokens are decoded inline on each request.

## Per-protocol token carriers

All five ingest surfaces funnel their bearer credential through the
same `AuthContext::Publish` shape before reaching the provider. The
extraction step lives in `lvqr_auth::extract` and exposes one helper
per protocol; call sites (`lvqr-whip`, `lvqr-srt`, `lvqr-rtsp`,
`lvqr-ingest`, `lvqr-cli` WS ingest) call that helper, not
`AuthContext::Publish` directly.

### RTMP

The RTMP stream key carries the credential verbatim. The URL is:

```
rtmp://HOST/APP/<jwt-or-static-key>
```

Example (ffmpeg):

```bash
ffmpeg -re -i input.mp4 \
  -c:v libx264 -preset veryfast -tune zerolatency \
  -c:a aac -ar 44100 -b:a 128k \
  -f flv rtmp://localhost:1935/live/eyJhbGciOiJIUzI1NiJ9...
```

### WHIP

WHIP uses a standard HTTP `Authorization: Bearer <jwt>` header on the
POST offer.

Example (curl):

```bash
curl -X POST http://localhost:7777/whip/live/cam1 \
  -H 'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9...' \
  -H 'Content-Type: application/sdp' \
  --data-binary @offer.sdp
```

A missing or invalid bearer returns HTTP 401. A broadcast-bound JWT
that names a different broadcast also returns 401.

### SRT

SRT has no bearer convention, so LVQR adopts a comma-separated
`streamid` KV payload:

```
m=publish,r=<broadcast>,t=<jwt>
```

Keys:

| Key | Meaning | Required |
|---|---|---|
| `m` | Mode. LVQR accepts any value; the parser is tolerant. | no |
| `r` | Resource / broadcast name. | yes, for broadcast-bound JWTs |
| `t` | Bearer token. | yes, when auth is configured |

Unknown keys are ignored. Key order does not matter. The legacy
`#!::` prefix used by some SRT access-control schemes is stripped
transparently.

Example (ffmpeg):

```bash
ffmpeg -re -i input.ts -c copy -f mpegts \
  'srt://localhost:7003?streamid=m=publish,r=live/cam1,t=eyJhbGciOiJIUzI1NiJ9...'
```

A denied connection receives SRT `ServerRejectReason::Unauthorized`
(code 2401) at handshake time; no task is spawned.

### RTSP

RTSP 2.0 supports `Authorization: Bearer`, and LVQR's `rtsp-types`-
based server passes the header through. The gate fires on ANNOUNCE
and RECORD (the RTSP publish entry points); DESCRIBE / PLAY
currently pass through unchecked because LVQR's RTSP surface is
publish-only.

Example (ffmpeg):

```bash
ffmpeg -re -i input.mp4 -c copy -f rtsp \
  -rtsp_transport tcp \
  -headers 'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9...' \
  rtsp://localhost:8554/live/cam1
```

A denied ANNOUNCE or RECORD returns RTSP `401 Unauthorized`.

### WebSocket ingest

WebSocket ingest accepts three carriers, in priority order:

1. A `Sec-WebSocket-Protocol` entry of the form `lvqr.bearer.<jwt>`.
   The server echoes the accepted subprotocol back in the handshake.
2. `Authorization: Bearer <jwt>` on the upgrade request.
3. A `?token=<jwt>` query parameter. Legacy fallback; logs a
   deprecation warning.

Example (wscat):

```bash
wscat -c ws://localhost:3000/ingest/live/cam1 \
  -s lvqr.bearer.eyJhbGciOiJIUzI1NiJ9...
```

### WebSocket / MoQ subscribe

Viewer tokens flow through the same resolver as WS ingest: subprotocol
first, header second, query third. The claim shape is the same; only
`scope: "subscribe"` is required.

## Example: one JWT, five protocols

A publish-scoped JWT bound to `live/cam1`:

```
{ "sub": "cam1", "scope": "publish", "broadcast": "live/cam1", "exp": 1800000000 }
```

Encoded with a shared secret, the same token drives every ingest
surface:

```bash
TOKEN=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJjYW0xIiwic2NvcGUiOiJwdWJsaXNoIiwiYnJvYWRjYXN0IjoibGl2ZS9jYW0xIiwiZXhwIjoxODAwMDAwMDAwfQ....

# RTMP
ffmpeg ... -f flv rtmp://host/live/$TOKEN

# WHIP
curl -H "Authorization: Bearer $TOKEN" -X POST http://host/whip/live/cam1 ...

# SRT
ffmpeg ... "srt://host:7003?streamid=m=publish,r=live/cam1,t=$TOKEN"

# RTSP
ffmpeg -headers "Authorization: Bearer $TOKEN" ... rtsp://host/live/cam1

# WS ingest
wscat -c ws://host/ingest/live/cam1 -s lvqr.bearer.$TOKEN
```

Swapping `live/cam1` for any other broadcast produces 401 / RTSP 401 /
SRT 2401 / WHIP 401, because the JWT's `broadcast` claim binds the
token to `live/cam1` specifically.

## Signed playback URLs

PLAN v1.1 row 121 ships a narrow secondary auth path on the
`/playback/*` routes: HMAC-signed, short-lived query-string
signatures for sharing a one-off link with a third party who
cannot authenticate against the normal subscribe-token gate.

### Enabling

```bash
lvqr serve --archive-dir /var/lvqr/archive --hmac-playback-secret $(openssl rand -hex 32)
# or via env:
LVQR_HMAC_PLAYBACK_SECRET=... lvqr serve --archive-dir ...
```

`--hmac-playback-secret` requires `--archive-dir`. The secret
should be 32+ random bytes; rotating it invalidates every
outstanding signed URL.

### URL shape

The signed-URL handler accepts two extra query parameters on
every `/playback/*` route:

| Param | Type | Meaning |
|---|---|---|
| `exp` | unsigned 64-bit Unix seconds | Signature expiry. The server rejects with `403 Forbidden` after `exp`. |
| `sig` | base64url-nopad HMAC-SHA256 | Signature over the canonical input string (see below). |

**Canonical input** is exactly the string
`"<request_path>?exp=<exp>"` with no leading scheme, host, or
port. Examples:

* `/playback/live/dvr?exp=1730000000`
* `/playback/latest/live/dvr?exp=1730000000`
* `/playback/file/live/dvr/0.mp4/00000001.m4s?exp=1730000000`

The signature is `HMAC-SHA256(secret, canonical_input)`
base64url-encoded without trailing `=` padding.

Other query params (`track`, `from`, `to`, `token`) are NOT
covered by the signature. The signature grants access to the
broadcast path; the other params shape what the scan returns
within that broadcast.

### Semantics

On every `/playback/*` request the server runs this precedence:

1. If `--hmac-playback-secret` is set AND both `sig` + `exp`
   are present on the request:
   - Verify `exp > now`. On fail: `403 Forbidden` with body
     `"signed URL expired"`.
   - Compute the expected HMAC and constant-time-compare
     against the provided `sig`. On fail: `403 Forbidden`
     with body `"signed URL signature invalid"`.
   - Both pass: grant access. The `Authorization: Bearer`
     header and `?token=` query param are ignored.
2. Otherwise: fall back to the normal `SubscribeAuth` subscribe
   gate. On fail: `401 Unauthorized`.

**Status code contract**: missing auth returns 401; wrong or
expired auth returns 403. Clients distinguish on status code
alone.

### Operator helper: `lvqr_cli::sign_playback_url`

For operators running their own admin service that mints signed
URLs server-side:

```rust
use lvqr_cli::sign_playback_url;

let secret = std::env::var("LVQR_HMAC_PLAYBACK_SECRET")?;
let share_path = "/playback/live/dvr";
let exp = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)?
    .as_secs()
    + 3600; // one-hour link

let suffix = sign_playback_url(secret.as_bytes(), share_path, exp);
let share_url = format!("https://relay.example.com:8080{share_path}?{suffix}");
// -> "https://relay.example.com:8080/playback/live/dvr?exp=...&sig=..."
```

The helper is pure and synchronous; operators can call it from
any context without a running LVQR server.

### Scope

* **Path-bound**: a signature for `/playback/live/a` cannot be
  reused on `/playback/live/b`. The broadcast is baked into the
  signed input via the path.
* **Expiry-bound**: a client cannot extend the expiry without
  re-signing against the secret.
* **No revocation list**: rotating `--hmac-playback-secret`
  invalidates every outstanding URL at once.
* **Signature covers path + exp only**, not `track` / `from` /
  `to` / other query params. Sharing a signed link gives the
  recipient free scrub within the broadcast. If you need
  tighter constraints (e.g. "only the 14:00-15:00 window"),
  scope the broadcast at the archive level or add additional
  auth in front of LVQR.
* **`/playback/*` only**. Live `/hls/*` and `/dash/*` routes
  use their own `SubscribeAuth` middleware and do not honor
  signed-URL params today. A follow-up could extend this to
  the live egress routes; for one-off share links the DVR
  path is the intended use case.

## Anti-scope

* No OAuth2 or JWKS key rotation. Static HS256 is the supported path.
* No revocation list. Token validity depends on `exp`.
* No per-protocol claim differences. The claim surface is flat.

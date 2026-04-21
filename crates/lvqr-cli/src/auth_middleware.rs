//! Tower middlewares that gate subscribe-side surfaces behind the
//! shared `SubscribeAuth` provider.
//!
//! Extracted out of `lib.rs` in the session-111-B1 follow-up refactor
//! so the composition root stays focused on wiring. Two middlewares
//! live here today:
//!
//! - [`live_playback_auth_middleware`] wraps the HLS and DASH live
//!   routers (session 112). Extracts the broadcast from
//!   `/hls/{broadcast}/<tail>` or `/dash/{broadcast}/<tail>` via the
//!   same `rfind('/')` rule the handlers use, honors
//!   `Authorization: Bearer <token>` first and `?token=<token>`
//!   second.
//! - [`signal_auth_middleware`] wraps the mesh `/signal` WebSocket
//!   (session 111-B1). `/signal` is not per-broadcast at the HTTP
//!   layer (clients send `Register { broadcast }` over the WS after
//!   the handshake), so the subscribe check uses an empty broadcast
//!   string; token extraction is query-only pending Sec-WebSocket-
//!   Protocol echo in `lvqr-signal` (session 111-B2+).
//!
//! Noop-provider deployments see no behavior change from either
//! middleware because the provider always returns `Allow`.
//! Configured deployments (static subscribe-token, JWT) get the
//! gate automatically.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use lvqr_auth::{AuthContext, AuthDecision, SharedAuth};

// =====================================================================
// Live HLS + DASH subscribe auth middleware (session 112)
// =====================================================================

/// State for the live-playback auth middleware. Clones are cheap
/// (one `Arc` and a `'static` label).
#[derive(Clone)]
pub(crate) struct LivePlaybackAuthState {
    pub(crate) auth: SharedAuth,
    /// Metric label for `lvqr_auth_failures_total{entry}` and the
    /// structured log line. Expected values: `"hls_live"`, `"dash_live"`.
    pub(crate) entry: &'static str,
}

/// Tower middleware that applies the subscribe-auth gate to live HLS
/// and DASH routes. The router it wraps serves `/hls/{broadcast}/...`
/// or `/dash/{broadcast}/...` catch-all paths; this middleware
/// extracts `broadcast` by the same `rfind('/')` rule the handlers
/// use, extracts a bearer token from the `Authorization` header or
/// the `?token=` query parameter, and calls
/// `SharedAuth::check(AuthContext::Subscribe { token, broadcast })`.
/// On `Allow` the request flows through; on `Deny` the middleware
/// short-circuits with a 401 carrying the provider's deny reason.
///
/// When the provider is `NoopAuthProvider` (no `--subscribe-token`,
/// no `--jwt-secret`), every check returns `Allow`, so this
/// middleware is a pure pass-through. Configured deployments get
/// the gate automatically.
///
/// Paths that do not match the `/{prefix}/{broadcast}/<tail>` shape
/// (eg. an accidental request to a path the inner router does not
/// know) fall through to the inner router, which will 404 normally.
/// The middleware never denies on a broadcast it cannot parse so
/// the 404 reason is preserved.
pub(crate) async fn live_playback_auth_middleware(
    State(state): State<LivePlaybackAuthState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or("").to_string();

    let Some(broadcast) = extract_live_playback_broadcast(&path) else {
        return next.run(request).await;
    };

    let token = extract_live_playback_token(request.headers(), &query);

    let decision = state.auth.check(&AuthContext::Subscribe {
        token,
        broadcast: broadcast.clone(),
    });
    match decision {
        AuthDecision::Allow => next.run(request).await,
        AuthDecision::Deny { reason } => {
            metrics::counter!("lvqr_auth_failures_total", "entry" => state.entry).increment(1);
            tracing::warn!(
                broadcast = %broadcast,
                entry = state.entry,
                reason = %reason,
                "live playback denied"
            );
            (StatusCode::UNAUTHORIZED, reason).into_response()
        }
    }
}

/// Extract the broadcast name from an `/hls/{broadcast}/<tail>` or
/// `/dash/{broadcast}/<tail>` path. Returns `None` when the path
/// does not carry the expected prefix or when the split would leave
/// an empty broadcast or tail (matches the handler's
/// `split_broadcast_path` logic in `lvqr-hls::server` /
/// `lvqr-dash::server`).
fn extract_live_playback_broadcast(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/hls/").or_else(|| path.strip_prefix("/dash/"))?;
    let idx = rest.rfind('/')?;
    if idx == 0 {
        return None;
    }
    let broadcast = &rest[..idx];
    let tail = &rest[idx + 1..];
    if broadcast.is_empty() || tail.is_empty() {
        return None;
    }
    Some(broadcast.to_string())
}

/// Extract a bearer token for the live-playback auth check. Prefers
/// the `Authorization: Bearer <token>` header; falls back to the
/// `?token=<token>` query parameter so native `<video>` elements (which
/// cannot set request headers) have a working path.
fn extract_live_playback_token(headers: &HeaderMap, query: &str) -> Option<String> {
    if let Some(hv) = headers.get(header::AUTHORIZATION)
        && let Ok(raw) = hv.to_str()
        && let Some(tok) = raw.strip_prefix("Bearer ")
        && !tok.is_empty()
    {
        return Some(tok.to_string());
    }
    for kv in query.split('&') {
        if let Some((k, v)) = kv.split_once('=')
            && k == "token"
            && !v.is_empty()
        {
            return Some(v.to_string());
        }
    }
    None
}

// =====================================================================
// Mesh /signal subscribe-auth middleware (session 111-B1)
// =====================================================================

/// State for the `/signal` auth middleware. Mirrors
/// [`LivePlaybackAuthState`] but without a transport label
/// since `/signal` is not per-broadcast at the HTTP layer
/// (clients send `Register { broadcast }` over the WS after
/// the handshake).
#[derive(Clone)]
pub(crate) struct SignalAuthState {
    pub(crate) auth: SharedAuth,
}

/// Tower middleware that applies the subscribe-auth gate to
/// the mesh `/signal` WebSocket. Accepts the bearer via the
/// `?token=<token>` query parameter. The WS subprotocol path
/// is intentionally unused for 111-B1: the underlying
/// `lvqr_signal::SignalServer` handler does not echo
/// `Sec-WebSocket-Protocol` back to the client, so relying on
/// subprotocol-carried bearer would break the RFC 6455
/// handshake for strict clients. Future work in 111-B2+ can
/// add subprotocol echo and re-enable the header path for
/// consistency with `/ws/*`.
///
/// `NoopAuthProvider` deployments see no behavior change
/// because the provider always returns `Allow`. Configured
/// deployments (static token, JWT) get an automatic 401 on
/// any `/signal` upgrade without a valid bearer.
pub(crate) async fn signal_auth_middleware(
    State(state): State<SignalAuthState>,
    request: Request,
    next: Next,
) -> Response {
    let token = extract_signal_token(request.uri().query().unwrap_or(""));
    let decision = state.auth.check(&AuthContext::Subscribe {
        token,
        broadcast: String::new(),
    });
    match decision {
        AuthDecision::Allow => next.run(request).await,
        AuthDecision::Deny { reason } => {
            metrics::counter!("lvqr_auth_failures_total", "entry" => "signal").increment(1);
            tracing::warn!(reason = %reason, "signal upgrade denied");
            (StatusCode::UNAUTHORIZED, reason).into_response()
        }
    }
}

/// Extract a bearer token from a `?token=<token>` query string
/// for the `/signal` auth gate. Kept separate from
/// [`extract_live_playback_token`] to make the design decision
/// (no subprotocol support for 111-B1) grep-able per-surface.
fn extract_signal_token(query: &str) -> Option<String> {
    for kv in query.split('&') {
        if let Some((k, v)) = kv.split_once('=')
            && k == "token"
            && !v.is_empty()
        {
            return Some(v.to_string());
        }
    }
    None
}

//! Shared HMAC-SHA256 signed-URL primitives for LVQR's playback surfaces.
//!
//! Two routes honor signed URLs today:
//!
//! * `/playback/*` (session 124, PLAN row 121) -- DVR scrub and VOD playback.
//!   Signature input is `"<path>?exp=<exp>"`; the operator helper is
//!   [`sign_playback_url`] which returns the query-string suffix a caller
//!   concatenates after a full request path.
//! * `/hls/<broadcast>/*` and `/dash/<broadcast>/*` (session 128) -- live
//!   playback. Signature input is `"hls:<broadcast>?exp=<exp>"` or
//!   `"dash:<broadcast>?exp=<exp>"`; a single sig grants access to every
//!   segment / chunk / manifest URL under that broadcast's live tree.
//!   Path-bound signatures are infeasible for live because LL-HLS playlists
//!   reference `part-*-<n>.m4s` URIs that roll over every 200 ms; minting a
//!   new URL per partial is impossible.
//!
//! Every variant shares one HMAC primitive so the verify + sign paths cannot
//! drift. Constant-time comparison on decoded bytes prevents a timing oracle
//! on signature verification. Errors are 403 Forbidden (not 401) so clients
//! can distinguish "auth presented but wrong" from "auth missing"; the body
//! string names which check failed.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

/// Outcome of the signed-URL verification step.
///
/// `NotAttempted` means the caller should fall through to the normal
/// subscribe-token gate: either the server has no secret configured, or
/// the client did not present both `sig` and `exp`. `Allow` short-circuits
/// the gate. `Deny` carries an already-built 403 response.
pub(crate) enum SignedUrlCheck {
    Allow,
    Deny(Response),
    NotAttempted,
}

/// Generic signed-URL verifier. Callers supply the signature input string
/// appropriate to their route (e.g. `"/playback/live/dvr?exp=1760000000"`
/// or `"hls:live/cam1?exp=1760000000"`) alongside the client-presented
/// `sig` + `exp` query params and a metric label.
///
/// Returns:
///
/// * [`SignedUrlCheck::NotAttempted`] when `hmac_secret` is `None` OR the
///   client did not present both `sig` and `exp`. The caller should fall
///   through to the normal subscribe-token gate.
/// * [`SignedUrlCheck::Deny`] with a 403 Forbidden when the signature or
///   expiry fails verification. The body names which check failed
///   (`"signed URL expired"`, `"signed URL malformed"`,
///   `"signed URL signature invalid"`).
/// * [`SignedUrlCheck::Allow`] when both checks pass. The caller skips
///   the subscribe-token gate.
pub(crate) fn verify_signed_url_generic(
    hmac_secret: Option<&[u8]>,
    signed_input: &str,
    sig: Option<&str>,
    exp: Option<u64>,
    metric_entry: &'static str,
) -> SignedUrlCheck {
    let Some(secret) = hmac_secret else {
        return SignedUrlCheck::NotAttempted;
    };
    let (sig, exp) = match (sig, exp) {
        (Some(s), Some(e)) => (s, e),
        _ => return SignedUrlCheck::NotAttempted,
    };

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if exp <= now_secs {
        metrics::counter!("lvqr_auth_failures_total", "entry" => metric_entry).increment(1);
        return SignedUrlCheck::Deny((StatusCode::FORBIDDEN, "signed URL expired").into_response());
    }

    let expected = compute_signature(secret, signed_input);
    let provided = match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig.as_bytes()) {
        Ok(bytes) => bytes,
        Err(_) => {
            metrics::counter!("lvqr_auth_failures_total", "entry" => metric_entry).increment(1);
            return SignedUrlCheck::Deny((StatusCode::FORBIDDEN, "signed URL malformed").into_response());
        }
    };
    if provided.len() != expected.len() || !bool::from(provided.ct_eq(&expected)) {
        metrics::counter!("lvqr_auth_failures_total", "entry" => metric_entry).increment(1);
        return SignedUrlCheck::Deny((StatusCode::FORBIDDEN, "signed URL signature invalid").into_response());
    }
    SignedUrlCheck::Allow
}

/// Compute HMAC-SHA256 of `signed_input` under `secret`. Used by every
/// sign / verify path in this module so the two halves cannot drift.
pub(crate) fn compute_signature(secret: &[u8], signed_input: &str) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(signed_input.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// Scheme tag for live-playback signed URLs. Renders lowercase on the
/// wire (`"hls"` / `"dash"`) so the signature input is stable across
/// operator tooling written in any language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveScheme {
    Hls,
    Dash,
}

impl LiveScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            LiveScheme::Hls => "hls",
            LiveScheme::Dash => "dash",
        }
    }
}

/// Build the signature input for a live HLS / DASH URL.
///
/// The signature grants broadcast-scoped access: one sig + exp pair
/// admits any URL under `/hls/<broadcast>/*` (or `/dash/<broadcast>/*`).
/// Path-bound signatures do not work for live because LL-HLS playlists
/// reference segment / partial URIs that change every 200 ms; an
/// operator minting one sig per URI cannot keep up.
fn live_signed_input(scheme: LiveScheme, broadcast: &str, exp: u64) -> String {
    format!("{}:{broadcast}?exp={exp}", scheme.as_str())
}

/// Generate a signed URL query suffix for a live HLS / DASH broadcast.
///
/// Returns `"exp=<exp>&sig=<b64url>"`; the caller concatenates after
/// `<url>?`. One suffix grants access to every URL under the broadcast's
/// live tree (master playlist, media playlist, init segment, every
/// numbered / partial segment) until `exp_unix` elapses.
///
/// # Example
///
/// ```ignore
/// use lvqr_cli::{sign_live_url, LiveScheme};
/// let exp = 1_760_000_000;
/// let suffix = sign_live_url(b"secret-key", LiveScheme::Hls, "live/cam1", exp);
/// // One URL grants everything below:
/// let playlist = format!("https://relay.example:8888/hls/live/cam1/playlist.m3u8?{suffix}");
/// let partial = format!("https://relay.example:8888/hls/live/cam1/part-video-42.m4s?{suffix}");
/// ```
pub fn sign_live_url(secret: &[u8], scheme: LiveScheme, broadcast: &str, exp_unix: u64) -> String {
    let signed_input = live_signed_input(scheme, broadcast, exp_unix);
    let sig_bytes = compute_signature(secret, &signed_input);
    let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&sig_bytes);
    format!("exp={exp_unix}&sig={sig_b64}")
}

/// Verify a live HLS / DASH URL's `sig` + `exp` query params against the
/// configured HMAC secret. The signature input is
/// `"<scheme>:<broadcast>?exp=<exp>"`; tampering with any of the three
/// (scheme, broadcast, exp) produces a different HMAC.
///
/// Metric label: `lvqr_auth_failures_total{entry="hls_signed_url"}` or
/// `entry="dash_signed_url"` so operators can distinguish live signed-URL
/// failures from the existing playback-route counter.
pub(crate) fn verify_live_signed_url(
    hmac_secret: Option<&[u8]>,
    scheme: LiveScheme,
    broadcast: &str,
    sig: Option<&str>,
    exp: Option<u64>,
) -> SignedUrlCheck {
    let (Some(sig_s), Some(exp_v)) = (sig, exp) else {
        // Short-circuit here so we do not allocate `signed_input` just to
        // throw it away. Mirrors the NotAttempted semantics of
        // verify_signed_url_generic.
        if hmac_secret.is_none() {
            return SignedUrlCheck::NotAttempted;
        }
        return SignedUrlCheck::NotAttempted;
    };
    let signed_input = live_signed_input(scheme, broadcast, exp_v);
    let entry = match scheme {
        LiveScheme::Hls => "hls_signed_url",
        LiveScheme::Dash => "dash_signed_url",
    };
    verify_signed_url_generic(hmac_secret, &signed_input, Some(sig_s), Some(exp_v), entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_scheme_bound() {
        let secret = b"s";
        let hls = compute_signature(secret, &live_signed_input(LiveScheme::Hls, "live/a", 100));
        let dash = compute_signature(secret, &live_signed_input(LiveScheme::Dash, "live/a", 100));
        assert_ne!(hls, dash, "hls vs dash scheme must produce different HMACs");
    }

    #[test]
    fn signature_is_broadcast_bound() {
        let secret = b"s";
        let a = compute_signature(secret, &live_signed_input(LiveScheme::Hls, "live/a", 100));
        let b = compute_signature(secret, &live_signed_input(LiveScheme::Hls, "live/b", 100));
        assert_ne!(a, b, "different broadcasts must produce different HMACs");
    }

    #[test]
    fn signature_is_expiry_bound() {
        let secret = b"s";
        let a = compute_signature(secret, &live_signed_input(LiveScheme::Hls, "live/a", 100));
        let b = compute_signature(secret, &live_signed_input(LiveScheme::Hls, "live/a", 200));
        assert_ne!(a, b, "different exp values must produce different HMACs");
    }

    #[test]
    fn sign_live_url_round_trips() {
        let secret = b"s";
        let exp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 60;
        let suffix = sign_live_url(secret, LiveScheme::Hls, "live/a", exp);
        // Parse out sig + exp from the suffix and feed to verify.
        let mut sig = None;
        let mut exp_parsed = None;
        for kv in suffix.split('&') {
            if let Some(v) = kv.strip_prefix("sig=") {
                sig = Some(v);
            }
            if let Some(v) = kv.strip_prefix("exp=") {
                exp_parsed = v.parse::<u64>().ok();
            }
        }
        assert_eq!(exp_parsed, Some(exp));
        let outcome = verify_live_signed_url(Some(secret), LiveScheme::Hls, "live/a", sig, exp_parsed);
        assert!(matches!(outcome, SignedUrlCheck::Allow));
    }

    #[test]
    fn cross_scheme_tamper_denied() {
        let secret = b"s";
        let exp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 60;
        let suffix = sign_live_url(secret, LiveScheme::Hls, "live/a", exp);
        let mut sig = None;
        for kv in suffix.split('&') {
            if let Some(v) = kv.strip_prefix("sig=") {
                sig = Some(v);
            }
        }
        // Feed the HLS-minted sig into the DASH verifier -- must deny.
        let outcome = verify_live_signed_url(Some(secret), LiveScheme::Dash, "live/a", sig, Some(exp));
        assert!(matches!(outcome, SignedUrlCheck::Deny(_)));
    }

    #[test]
    fn cross_broadcast_tamper_denied() {
        let secret = b"s";
        let exp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 60;
        let suffix = sign_live_url(secret, LiveScheme::Hls, "live/a", exp);
        let mut sig = None;
        for kv in suffix.split('&') {
            if let Some(v) = kv.strip_prefix("sig=") {
                sig = Some(v);
            }
        }
        let outcome = verify_live_signed_url(Some(secret), LiveScheme::Hls, "live/b", sig, Some(exp));
        assert!(matches!(outcome, SignedUrlCheck::Deny(_)));
    }

    #[test]
    fn expired_url_denied() {
        let secret = b"s";
        let exp_past = 1_000_000; // clearly in the past
        let suffix = sign_live_url(secret, LiveScheme::Hls, "live/a", exp_past);
        let mut sig = None;
        for kv in suffix.split('&') {
            if let Some(v) = kv.strip_prefix("sig=") {
                sig = Some(v);
            }
        }
        let outcome = verify_live_signed_url(Some(secret), LiveScheme::Hls, "live/a", sig, Some(exp_past));
        assert!(matches!(outcome, SignedUrlCheck::Deny(_)));
    }

    #[test]
    fn no_secret_returns_not_attempted() {
        let outcome = verify_live_signed_url(None, LiveScheme::Hls, "live/a", Some("x"), Some(100));
        assert!(matches!(outcome, SignedUrlCheck::NotAttempted));
    }

    #[test]
    fn no_sig_or_exp_returns_not_attempted() {
        let outcome = verify_live_signed_url(Some(b"s"), LiveScheme::Hls, "live/a", None, None);
        assert!(matches!(outcome, SignedUrlCheck::NotAttempted));
        let outcome = verify_live_signed_url(Some(b"s"), LiveScheme::Hls, "live/a", Some("x"), None);
        assert!(matches!(outcome, SignedUrlCheck::NotAttempted));
        let outcome = verify_live_signed_url(Some(b"s"), LiveScheme::Hls, "live/a", None, Some(100));
        assert!(matches!(outcome, SignedUrlCheck::NotAttempted));
    }
}

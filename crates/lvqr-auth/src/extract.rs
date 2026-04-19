//! Per-protocol token extractors.
//!
//! `AuthProvider::check` is the uniform decision surface: every provider
//! consumes an [`AuthContext`] and returns an [`AuthDecision`]. What varies
//! across LVQR's ingest protocols is how the caller's bearer credential
//! reaches the server in the first place:
//!
//! * RTMP carries it as the stream key (existing convention).
//! * WHIP / RTSP carry it as `Authorization: Bearer <jwt>`.
//! * SRT carries it as an `srt_tokio` `streamid` with comma-separated
//!   KV pairs (`m=publish,r=<broadcast>,t=<jwt>`).
//! * WebSocket ingest accepts a `lvqr.bearer.<jwt>` subprotocol, a
//!   legacy `?token=` query param, or `Authorization: Bearer`.
//!
//! This module exposes one `extract_<proto>` helper per surface that
//! turns the raw carrier bytes into an [`AuthContext::Publish`]. The
//! call sites (`lvqr-whip`, `lvqr-srt`, `lvqr-rtsp`, `lvqr-ingest`,
//! `lvqr-cli` WS ingest) then pass that context to
//! `AuthProvider::check`. One `JwtAuthProvider` + one token therefore
//! admits the publisher on every protocol.
//!
//! The extractors do not themselves reject; that is the provider's job.
//! If a token is missing or the carrier is malformed, the extractor
//! still returns an `AuthContext::Publish { key: "", .. }` so the
//! configured provider decides. `NoopAuthProvider` allows it (open
//! access stays open); JWT / static providers deny an empty key.
//!
//! Tier 4 item 4.8 session A.

use crate::provider::AuthContext;

/// Strip a `Bearer ` prefix off an HTTP `Authorization` header, trimming
/// surrounding whitespace off the resulting token.
///
/// Returns `None` when the header is absent, does not start with
/// `Bearer` (case-insensitive on the scheme per RFC 6750), or the token
/// substring is empty after trimming.
pub fn parse_bearer(header: Option<&str>) -> Option<String> {
    let raw = header?.trim();
    let rest = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .or_else(|| raw.strip_prefix("BEARER "))?;
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Parse an SRT `streamid` into comma-separated `key=value` pairs.
///
/// Accepts any key order and tolerates unknown keys for operator
/// tooling compatibility (ffmpeg, OBS, Larix). The LVQR-adopted shape
/// for publish is `m=publish,r=<broadcast>,t=<jwt>`, but this parser
/// does not enforce the `m` key -- the caller decides what is
/// required. Whitespace around keys and values is trimmed.
///
/// A legacy SRT-access-control shape, `#!::r=<resource>,m=request`,
/// prefixes the KV list with `#!::` -- the prefix is stripped if
/// present. Entries without `=` are ignored.
pub fn parse_srt_streamid(streamid: &str) -> Vec<(String, String)> {
    let trimmed = streamid.trim().strip_prefix("#!::").unwrap_or(streamid.trim());
    trimmed
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Look up a key in an SRT streamid KV list, returning the first match.
fn streamid_get<'a>(kv: &'a [(String, String)], key: &str) -> Option<&'a str> {
    kv.iter().find_map(|(k, v)| (k == key).then_some(v.as_str()))
}

/// Build an [`AuthContext::Publish`] for an RTMP publish.
///
/// The RTMP stream key carries the bearer credential under
/// `JwtAuthProvider`'s existing convention, so `key = stream_key`
/// verbatim. The broadcast field stays `None` because the broadcast
/// name on the wire IS `app/stream_key`, and adding it here would
/// double-count the JWT.
pub fn extract_rtmp(app: &str, stream_key: &str) -> AuthContext {
    AuthContext::Publish {
        app: app.to_string(),
        key: stream_key.to_string(),
        broadcast: None,
    }
}

/// Build an [`AuthContext::Publish`] for a WHIP POST /whip/{broadcast}.
///
/// The token comes off the request's `Authorization` header. The
/// broadcast is the path segment (including a slash, e.g.
/// `live/cam1`).
pub fn extract_whip(broadcast: &str, authorization: Option<&str>) -> AuthContext {
    AuthContext::Publish {
        app: "whip".into(),
        key: parse_bearer(authorization).unwrap_or_default(),
        broadcast: Some(broadcast.to_string()),
    }
}

/// Build an [`AuthContext::Publish`] for an SRT connection.
///
/// Parses the streamid KV payload in LVQR-adopted shape
/// `m=publish,r=<broadcast>,t=<jwt>`. If either `r` or `t` is absent
/// the field defaults to empty / None, and the configured provider
/// decides whether the connection is admitted.
pub fn extract_srt(streamid: &str) -> AuthContext {
    let kv = parse_srt_streamid(streamid);
    let key = streamid_get(&kv, "t").unwrap_or("").to_string();
    let broadcast = streamid_get(&kv, "r").map(|s| s.to_string());
    AuthContext::Publish {
        app: "srt".into(),
        key,
        broadcast,
    }
}

/// Build an [`AuthContext::Publish`] for an RTSP ANNOUNCE or RECORD.
///
/// `broadcast` is the resource path parsed out of the request URI by
/// the RTSP server (LVQR's convention: `rtsp://host/app/name` ->
/// `app/name`). The token comes off the `Authorization` header.
pub fn extract_rtsp(broadcast: &str, authorization: Option<&str>) -> AuthContext {
    AuthContext::Publish {
        app: "rtsp".into(),
        key: parse_bearer(authorization).unwrap_or_default(),
        broadcast: Some(broadcast.to_string()),
    }
}

/// Build an [`AuthContext::Publish`] for a WebSocket ingest upgrade.
///
/// The caller has already resolved the bearer token (typically via the
/// `lvqr.bearer.<token>` subprotocol, falling back to the legacy
/// `?token=` query param). This helper just wraps it with the
/// broadcast path so WS ingest lands on the same `AuthContext` shape
/// the other four protocols use.
pub fn extract_ws_ingest(resolved_token: Option<&str>, broadcast: &str) -> AuthContext {
    AuthContext::Publish {
        app: "ws".into(),
        key: resolved_token.unwrap_or("").to_string(),
        broadcast: Some(broadcast.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bearer_strips_scheme_case_insensitively() {
        assert_eq!(parse_bearer(Some("Bearer abc")), Some("abc".into()));
        assert_eq!(parse_bearer(Some("bearer abc")), Some("abc".into()));
        assert_eq!(parse_bearer(Some("BEARER abc")), Some("abc".into()));
        assert_eq!(parse_bearer(Some("  Bearer   abc  ")), Some("abc".into()));
    }

    #[test]
    fn parse_bearer_rejects_missing_or_malformed_headers() {
        assert_eq!(parse_bearer(None), None);
        assert_eq!(parse_bearer(Some("")), None);
        assert_eq!(parse_bearer(Some("Basic dXNlcg==")), None);
        assert_eq!(parse_bearer(Some("Bearer ")), None);
        assert_eq!(parse_bearer(Some("Bearer    ")), None);
    }

    #[test]
    fn parse_srt_streamid_handles_happy_path() {
        let kv = parse_srt_streamid("m=publish,r=live/cam1,t=jwt.payload.sig");
        assert_eq!(streamid_get(&kv, "m"), Some("publish"));
        assert_eq!(streamid_get(&kv, "r"), Some("live/cam1"));
        assert_eq!(streamid_get(&kv, "t"), Some("jwt.payload.sig"));
    }

    #[test]
    fn parse_srt_streamid_tolerates_any_order_and_unknown_keys() {
        let kv = parse_srt_streamid("t=TOK,extra=ignored,r=b/c,m=publish");
        assert_eq!(streamid_get(&kv, "t"), Some("TOK"));
        assert_eq!(streamid_get(&kv, "r"), Some("b/c"));
        assert_eq!(streamid_get(&kv, "extra"), Some("ignored"));
    }

    #[test]
    fn parse_srt_streamid_strips_legacy_access_control_prefix() {
        let kv = parse_srt_streamid("#!::r=live/cam1,m=publish,t=TOK");
        assert_eq!(streamid_get(&kv, "r"), Some("live/cam1"));
        assert_eq!(streamid_get(&kv, "t"), Some("TOK"));
    }

    #[test]
    fn parse_srt_streamid_empty_yields_empty_kv() {
        assert!(parse_srt_streamid("").is_empty());
        assert!(parse_srt_streamid("malformed_no_equals").is_empty());
    }

    #[test]
    fn extract_rtmp_preserves_stream_key_as_token() {
        let ctx = extract_rtmp("live", "some.jwt.token");
        let AuthContext::Publish { app, key, broadcast } = ctx else {
            panic!("expected Publish");
        };
        assert_eq!(app, "live");
        assert_eq!(key, "some.jwt.token");
        assert!(broadcast.is_none(), "RTMP skips broadcast binding");
    }

    #[test]
    fn extract_whip_pulls_bearer_and_broadcast() {
        let ctx = extract_whip("live/cam1", Some("Bearer myjwt"));
        let AuthContext::Publish { app, key, broadcast } = ctx else {
            panic!("expected Publish");
        };
        assert_eq!(app, "whip");
        assert_eq!(key, "myjwt");
        assert_eq!(broadcast.as_deref(), Some("live/cam1"));
    }

    #[test]
    fn extract_whip_missing_header_yields_empty_key() {
        let ctx = extract_whip("live/cam1", None);
        let AuthContext::Publish { key, broadcast, .. } = ctx else {
            panic!("expected Publish");
        };
        assert!(key.is_empty(), "provider decides; extractor does not reject");
        assert_eq!(broadcast.as_deref(), Some("live/cam1"));
    }

    #[test]
    fn extract_srt_happy_path() {
        let ctx = extract_srt("m=publish,r=live/cam1,t=JWT");
        let AuthContext::Publish { app, key, broadcast } = ctx else {
            panic!("expected Publish");
        };
        assert_eq!(app, "srt");
        assert_eq!(key, "JWT");
        assert_eq!(broadcast.as_deref(), Some("live/cam1"));
    }

    #[test]
    fn extract_srt_missing_token_yields_empty_key() {
        let ctx = extract_srt("m=publish,r=live/cam1");
        let AuthContext::Publish { key, broadcast, .. } = ctx else {
            panic!("expected Publish");
        };
        assert!(key.is_empty());
        assert_eq!(broadcast.as_deref(), Some("live/cam1"));
    }

    #[test]
    fn extract_srt_missing_broadcast_yields_none() {
        let ctx = extract_srt("m=publish,t=JWT");
        let AuthContext::Publish { key, broadcast, .. } = ctx else {
            panic!("expected Publish");
        };
        assert_eq!(key, "JWT");
        assert!(broadcast.is_none());
    }

    #[test]
    fn extract_rtsp_pulls_bearer_and_broadcast() {
        let ctx = extract_rtsp("live/cam1", Some("Bearer abc"));
        let AuthContext::Publish { app, key, broadcast } = ctx else {
            panic!("expected Publish");
        };
        assert_eq!(app, "rtsp");
        assert_eq!(key, "abc");
        assert_eq!(broadcast.as_deref(), Some("live/cam1"));
    }

    #[test]
    fn extract_rtsp_missing_header_yields_empty_key() {
        let ctx = extract_rtsp("live/cam1", None);
        let AuthContext::Publish { key, .. } = ctx else {
            panic!("expected Publish");
        };
        assert!(key.is_empty());
    }

    #[test]
    fn extract_ws_ingest_wraps_resolved_token() {
        let ctx = extract_ws_ingest(Some("TOK"), "live/cam1");
        let AuthContext::Publish { app, key, broadcast } = ctx else {
            panic!("expected Publish");
        };
        assert_eq!(app, "ws");
        assert_eq!(key, "TOK");
        assert_eq!(broadcast.as_deref(), Some("live/cam1"));
    }

    #[test]
    fn extract_ws_ingest_missing_token_yields_empty_key() {
        let ctx = extract_ws_ingest(None, "live/cam1");
        let AuthContext::Publish { key, broadcast, .. } = ctx else {
            panic!("expected Publish");
        };
        assert!(key.is_empty());
        assert_eq!(broadcast.as_deref(), Some("live/cam1"));
    }
}

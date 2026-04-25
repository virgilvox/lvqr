//! Runtime stream-key catalog backing the `/api/v1/streamkeys` admin
//! API and the [`crate::MultiKeyAuthProvider`] composition.
//!
//! Operators historically provisioned ingest stream keys via static
//! config (`LVQR_PUBLISH_KEY` for one shared key, or external JWT
//! minting). Both shapes force a server bounce or an out-of-band
//! pipeline for every key change. This module adds an in-memory
//! `StreamKeyStore` so an admin client can mint, list, revoke, and
//! rotate stream keys at runtime.
//!
//! # Wire shape
//!
//! [`StreamKey`] and [`StreamKeySpec`] are the byte-for-byte JSON
//! bodies the admin route serves. Every `Option<_>` field carries
//! `#[serde(default)]` so a server adding a new optional field does
//! not break older SDK clients (and vice versa).
//!
//! # Token format
//!
//! Tokens are `lvqr_sk_<43-char base64url-no-pad>`: 32 bytes of
//! `OsRng` output, base64url-encoded with no padding, prefixed with
//! `lvqr_sk_`. The typed prefix matches industry convention
//! (Stripe `sk_live_`, GitHub `ghp_`) and lets secret-scanners
//! recognise an LVQR key in a public commit.
//!
//! Ids are `<22-char base64url-no-pad>`: 16 random bytes, no prefix.
//! Operator-recognisable, URL-safe, and short enough to type into
//! a `DELETE /api/v1/streamkeys/<id>` curl invocation.
//!
//! # Persistence
//!
//! In-memory only in v1: restart loses every minted key. Operators
//! who need durable single-key publish auth keep using the existing
//! `StaticAuthProvider` (`LVQR_PUBLISH_KEY`); a sled / SQLite-backed
//! `StreamKeyStore` impl is its own session and the trait is shaped
//! so the swap is purely additive.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dashmap::DashMap;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Typed prefix on every minted token. See module-level docs.
pub const STREAM_KEY_TOKEN_PREFIX: &str = "lvqr_sk_";

/// One stream-key as the admin API serves it on the wire.
///
/// `id` is the stable handle the admin API uses for `DELETE` and
/// `rotate`; it never changes for the life of the key. `token` is
/// the actual bearer credential a publisher sends as the RTMP
/// stream key (or as the `Authorization: Bearer <token>` header on
/// WHIP / SRT). Rotate produces a new `token` while preserving
/// `id`; the old token is invalidated atomically the moment rotate
/// returns.
///
/// `created_at` is the unix-seconds wall clock at mint or rotate
/// time. `expires_at` is checked lazily on every auth-path lookup
/// (no daemon sweep); operators see expired keys on the list
/// endpoint until they call `revoke`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamKey {
    pub id: String,
    pub token: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub broadcast: Option<String>,
    pub created_at: u64,
    #[serde(default)]
    pub expires_at: Option<u64>,
}

/// Mint / rotate request body.
///
/// Every field is optional so the caller can opt into scoping
/// without breaking on a server that adds a sibling field later.
/// `ttl_seconds` is converted to `expires_at = now + ttl_seconds`
/// at mint time; passing `0` is the same as omitting the field
/// (server treats it as "no expiry").
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamKeySpec {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub broadcast: Option<String>,
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
}

/// Trait the [`crate::MultiKeyAuthProvider`] queries on every
/// `Publish` auth check, and the admin route mutates on every
/// CRUD call.
///
/// `Send + Sync + 'static` so a single [`SharedStreamKeyStore`]
/// can be shared across the auth-check path (RTMP / WHIP / SRT
/// callbacks), the admin axum handlers, and any future federation
/// or replication path. All methods take `&self` because the
/// in-memory impl is interior-mutable via `DashMap`; a future
/// sled / SQLite impl can do the same with its own internal
/// locking without changing the surface.
pub trait StreamKeyStore: Send + Sync + 'static {
    /// Snapshot of every key currently in the store, INCLUDING
    /// expired ones so operators can see what is stale and call
    /// `revoke`. The auth-path uses `get_by_token` which filters
    /// expired tokens on read.
    fn list(&self) -> Vec<StreamKey>;

    /// Look up by stable id. Returns the entry verbatim, including
    /// expired entries (the admin routes use this for rotate).
    fn get(&self, id: &str) -> Option<StreamKey>;

    /// Look up by bearer token. Returns `None` for unknown tokens
    /// AND for expired tokens (lazy expiry). The
    /// [`crate::MultiKeyAuthProvider`] treats `None` as "store miss"
    /// and falls through to the wrapped provider.
    fn get_by_token(&self, token: &str) -> Option<StreamKey>;

    /// Mint a new key with a fresh id + token. The token is
    /// guaranteed unique against existing entries (collision is
    /// statistically impossible at 2^256 entropy, and a panic
    /// guard on the reverse index makes any drift loud).
    fn mint(&self, spec: StreamKeySpec) -> StreamKey;

    /// Hard-delete by id. `true` if an entry was removed,
    /// `false` if the id was unknown.
    fn revoke(&self, id: &str) -> bool;

    /// Replace the token on an existing id. `override_spec.is_some()`
    /// also re-scopes `label` / `broadcast` / `expires_at` (the
    /// override is total: a `None` field on the override CLEARS the
    /// existing field). `override_spec.is_none()` preserves every
    /// non-token field. Returns `Some(new_key)` on success and
    /// `None` if the id was unknown.
    fn rotate(&self, id: &str, override_spec: Option<StreamKeySpec>) -> Option<StreamKey>;
}

/// Convenience alias used by the admin route + the multi-key auth
/// provider so neither has to spell the trait-object every time.
pub type SharedStreamKeyStore = Arc<dyn StreamKeyStore>;

/// In-memory `StreamKeyStore` backed by two `DashMap`s: a
/// primary `id -> StreamKey` map, and a reverse `token -> id`
/// index that lets the auth-path hot loop hit the entry in O(1).
///
/// The reverse index panics on a collision (two different ids
/// claiming the same token). 32 bytes of `OsRng` make a
/// collision statistically impossible; a panic guards the
/// invariant against any future code path that mints tokens
/// non-randomly.
#[derive(Debug, Default)]
pub struct InMemoryStreamKeyStore {
    by_id: DashMap<String, StreamKey>,
    by_token: DashMap<String, String>,
}

impl InMemoryStreamKeyStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StreamKeyStore for InMemoryStreamKeyStore {
    fn list(&self) -> Vec<StreamKey> {
        self.by_id.iter().map(|e| e.value().clone()).collect()
    }

    fn get(&self, id: &str) -> Option<StreamKey> {
        self.by_id.get(id).map(|e| e.value().clone())
    }

    fn get_by_token(&self, token: &str) -> Option<StreamKey> {
        let id = self.by_token.get(token).map(|e| e.value().clone())?;
        let key = self.by_id.get(&id).map(|e| e.value().clone())?;
        if is_expired(&key) { None } else { Some(key) }
    }

    fn mint(&self, spec: StreamKeySpec) -> StreamKey {
        let now = unix_now();
        let key = StreamKey {
            id: generate_id(),
            token: generate_token(),
            label: spec.label,
            broadcast: spec.broadcast,
            created_at: now,
            expires_at: ttl_to_expiry(spec.ttl_seconds, now),
        };
        if let Some(prior) = self.by_token.insert(key.token.clone(), key.id.clone()) {
            // OsRng-driven 32-byte tokens cannot collide in practice;
            // a hit here means the RNG was tampered with or a future
            // change started minting deterministic tokens. Panic loudly
            // rather than let the auth-path silently authenticate the
            // wrong principal.
            panic!(
                "stream-key token collision: token previously mapped to id {prior:?}; this should be statistically impossible"
            );
        }
        self.by_id.insert(key.id.clone(), key.clone());
        key
    }

    fn revoke(&self, id: &str) -> bool {
        let Some((_, key)) = self.by_id.remove(id) else {
            return false;
        };
        self.by_token.remove(&key.token);
        true
    }

    fn rotate(&self, id: &str, override_spec: Option<StreamKeySpec>) -> Option<StreamKey> {
        let mut entry = self.by_id.get_mut(id)?;
        let prior_token = entry.token.clone();
        let now = unix_now();
        let new_token = generate_token();
        match override_spec {
            Some(spec) => {
                entry.label = spec.label;
                entry.broadcast = spec.broadcast;
                entry.expires_at = ttl_to_expiry(spec.ttl_seconds, now);
            }
            None => { /* preserve label / broadcast / expires_at */ }
        }
        entry.token = new_token.clone();
        entry.created_at = now;
        let snapshot = entry.value().clone();
        drop(entry);
        // Reverse-index swap. The token is freshly generated against
        // OsRng so a collision is not realistic; we still panic on
        // the off chance because a silent overwrite would let the
        // old `prior_token` resolve to a still-live `id`.
        self.by_token.remove(&prior_token);
        if let Some(prev) = self.by_token.insert(new_token, id.to_string()) {
            panic!(
                "stream-key rotate: new token already mapped to id {prev:?}; this should be statistically impossible"
            );
        }
        Some(snapshot)
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn ttl_to_expiry(ttl_seconds: Option<u64>, now: u64) -> Option<u64> {
    match ttl_seconds {
        Some(0) | None => None,
        Some(ttl) => Some(now.saturating_add(ttl)),
    }
}

fn is_expired(key: &StreamKey) -> bool {
    match key.expires_at {
        Some(exp) => unix_now() >= exp,
        None => false,
    }
}

/// `lvqr_sk_<base64url(32 bytes OsRng)>`. See module docs.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let suffix = URL_SAFE_NO_PAD.encode(bytes);
    let mut out = String::with_capacity(STREAM_KEY_TOKEN_PREFIX.len() + suffix.len());
    out.push_str(STREAM_KEY_TOKEN_PREFIX);
    out.push_str(&suffix);
    out
}

/// `<base64url(16 bytes OsRng)>`. See module docs.
pub fn generate_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_returns_key_with_typed_prefix_and_id() {
        let store = InMemoryStreamKeyStore::new();
        let key = store.mint(StreamKeySpec {
            label: Some("camera-a".into()),
            broadcast: Some("live/cam-a".into()),
            ttl_seconds: None,
        });
        assert!(
            key.token.starts_with(STREAM_KEY_TOKEN_PREFIX),
            "token must carry the lvqr_sk_ prefix; got {:?}",
            key.token
        );
        // 32 bytes -> 43 base64url-no-pad chars; plus the 8-char prefix.
        assert_eq!(key.token.len(), STREAM_KEY_TOKEN_PREFIX.len() + 43);
        // 16 bytes -> 22 base64url-no-pad chars.
        assert_eq!(key.id.len(), 22);
        assert_eq!(key.label.as_deref(), Some("camera-a"));
        assert_eq!(key.broadcast.as_deref(), Some("live/cam-a"));
        assert_eq!(key.expires_at, None);
    }

    #[test]
    fn list_includes_minted_keys() {
        let store = InMemoryStreamKeyStore::new();
        let a = store.mint(StreamKeySpec::default());
        let b = store.mint(StreamKeySpec::default());
        let mut ids: Vec<String> = store.list().into_iter().map(|k| k.id).collect();
        ids.sort();
        let mut expected = vec![a.id, b.id];
        expected.sort();
        assert_eq!(ids, expected);
    }

    #[test]
    fn get_by_token_round_trips_and_returns_none_for_unknown() {
        let store = InMemoryStreamKeyStore::new();
        let key = store.mint(StreamKeySpec::default());
        let hit = store.get_by_token(&key.token).expect("minted token must hit");
        assert_eq!(hit.id, key.id);
        assert!(store.get_by_token("lvqr_sk_doesnotexist").is_none());
    }

    #[test]
    fn revoke_removes_both_indexes_and_reports_state() {
        let store = InMemoryStreamKeyStore::new();
        let key = store.mint(StreamKeySpec::default());
        assert!(store.revoke(&key.id), "first revoke should report removal");
        assert!(store.get(&key.id).is_none(), "primary index must drop the entry");
        assert!(
            store.get_by_token(&key.token).is_none(),
            "reverse index must drop the entry so the auth path falls through"
        );
        assert!(
            !store.revoke(&key.id),
            "second revoke on the same id should report no-op"
        );
    }

    #[test]
    fn rotate_no_override_preserves_scope_and_changes_token() {
        let store = InMemoryStreamKeyStore::new();
        let original = store.mint(StreamKeySpec {
            label: Some("preserve-me".into()),
            broadcast: Some("live/keep".into()),
            ttl_seconds: None,
        });
        let rotated = store.rotate(&original.id, None).expect("rotate must succeed");
        assert_eq!(rotated.id, original.id, "id is the stable handle");
        assert_ne!(rotated.token, original.token, "token must change");
        assert_eq!(rotated.label.as_deref(), Some("preserve-me"));
        assert_eq!(rotated.broadcast.as_deref(), Some("live/keep"));
        assert!(
            store.get_by_token(&original.token).is_none(),
            "old token must be invalidated immediately"
        );
        assert_eq!(
            store.get_by_token(&rotated.token).map(|k| k.id),
            Some(rotated.id.clone()),
            "new token must auth to the same id"
        );
    }

    #[test]
    fn rotate_with_override_replaces_scope() {
        let store = InMemoryStreamKeyStore::new();
        let original = store.mint(StreamKeySpec {
            label: Some("first-label".into()),
            broadcast: Some("live/first".into()),
            ttl_seconds: None,
        });
        let rotated = store
            .rotate(
                &original.id,
                Some(StreamKeySpec {
                    label: Some("second-label".into()),
                    broadcast: None,
                    ttl_seconds: Some(60),
                }),
            )
            .expect("rotate must succeed");
        assert_eq!(rotated.label.as_deref(), Some("second-label"));
        assert_eq!(
            rotated.broadcast, None,
            "override_spec.broadcast = None must CLEAR the prior scope"
        );
        let exp = rotated.expires_at.expect("ttl_seconds must populate expires_at");
        let now = unix_now();
        assert!(
            exp >= now && exp <= now + 60,
            "expires_at must be within the new TTL window"
        );
    }

    #[test]
    fn rotate_unknown_id_returns_none() {
        let store = InMemoryStreamKeyStore::new();
        assert!(store.rotate("doesnotexist", None).is_none());
    }

    #[test]
    fn lazy_expiry_filters_get_by_token_but_not_list() {
        let store = InMemoryStreamKeyStore::new();
        let key = store.mint(StreamKeySpec {
            label: None,
            broadcast: None,
            ttl_seconds: Some(0),
        });
        // ttl_seconds: 0 maps to "no expiry" by spec.
        assert!(key.expires_at.is_none(), "ttl_seconds=0 means no expiry");

        // Manually insert an already-expired key to exercise the lazy
        // filter: the auth path uses get_by_token, which must return
        // None for expired entries; the operator-facing list endpoint
        // must still surface them so revoke is discoverable.
        let mut already_expired = store.mint(StreamKeySpec::default());
        already_expired.expires_at = Some(1);
        store.by_id.insert(already_expired.id.clone(), already_expired.clone());

        assert!(
            store.get_by_token(&already_expired.token).is_none(),
            "expired entries must not satisfy auth-path lookups"
        );
        let list_ids: Vec<String> = store.list().into_iter().map(|k| k.id).collect();
        assert!(
            list_ids.contains(&already_expired.id),
            "list must surface expired entries so operators can revoke them"
        );
    }

    #[test]
    fn mint_yields_unique_tokens_across_calls() {
        let store = InMemoryStreamKeyStore::new();
        let a = store.mint(StreamKeySpec::default());
        let b = store.mint(StreamKeySpec::default());
        assert_ne!(a.id, b.id);
        assert_ne!(a.token, b.token);
    }

    #[test]
    fn streamkey_serde_round_trips_with_optional_fields_omitted() {
        // Pre-146 client perspective: a server adding `expires_at` on
        // the wire later must not break a client deserializer that
        // omitted it. The flip-side is also true; both directions
        // hinge on `#[serde(default)]` on every Optional.
        let body = r#"{"id":"abc","token":"lvqr_sk_xyz","created_at":1700000000}"#;
        let parsed: StreamKey = serde_json::from_str(body).expect("must parse minimal body");
        assert_eq!(parsed.id, "abc");
        assert_eq!(parsed.token, "lvqr_sk_xyz");
        assert_eq!(parsed.created_at, 1_700_000_000);
        assert!(parsed.label.is_none());
        assert!(parsed.broadcast.is_none());
        assert!(parsed.expires_at.is_none());
    }

    #[test]
    fn streamkeyspec_default_round_trip_accepts_empty_object() {
        let spec: StreamKeySpec = serde_json::from_str("{}").expect("empty body must parse");
        assert!(spec.label.is_none());
        assert!(spec.broadcast.is_none());
        assert!(spec.ttl_seconds.is_none());
    }
}

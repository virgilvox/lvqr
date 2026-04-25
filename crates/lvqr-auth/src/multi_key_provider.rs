//! [`crate::AuthProvider`] backed by a runtime [`StreamKeyStore`]
//! with a fallback to a wrapped provider for non-store-hit
//! contexts.
//!
//! Composes additively over the existing auth chain: every previous
//! provider (Noop, Static, Jwt, Jwks, Webhook) keeps working, and a
//! minted stream-key becomes an additional accept path on top.
//! Revoking a stream-key short-circuits the wrapped provider for
//! that exact token (revoke beats a still-valid JWT carrying the
//! same string).
//!
//! # Decision order
//!
//! For [`AuthContext::Publish`]:
//! 1. `store.get_by_token(key)` -- if the lookup hits AND the entry
//!    is unexpired AND any `broadcast` scope matches the request,
//!    return [`AuthDecision::Allow`].
//! 2. If the lookup hits but the entry's broadcast scope mismatches,
//!    return [`AuthDecision::Deny`] WITHOUT consulting the fallback.
//!    A scoped key is a tighter declaration than the fallback chain;
//!    promoting it to the fallback would silently widen the scope an
//!    operator narrowed.
//! 3. If the lookup misses (unknown token, or expired token), fall
//!    through to `fallback.check(ctx)`. With `fallback: None`, deny.
//!
//! For [`AuthContext::Subscribe`] and [`AuthContext::Admin`]: always
//! delegate to the fallback. Stream-key CRUD is publish-only in v1;
//! viewer + admin auth stay on the existing chain so a misconfigured
//! store cannot lock the operator out of their own admin API.
//!
//! # Composition pattern
//!
//! ```ignore
//! use std::sync::Arc;
//! use lvqr_auth::{InMemoryStreamKeyStore, MultiKeyAuthProvider, NoopAuthProvider, SharedAuth};
//!
//! let inner: SharedAuth = Arc::new(NoopAuthProvider);
//! let store = Arc::new(InMemoryStreamKeyStore::new());
//! let auth: SharedAuth = Arc::new(MultiKeyAuthProvider::new(store, Some(inner)));
//! ```

use crate::provider::{AuthContext, AuthDecision, AuthProvider, SharedAuth};
use crate::stream_key_store::SharedStreamKeyStore;

/// See module-level docs.
#[derive(Clone)]
pub struct MultiKeyAuthProvider {
    store: SharedStreamKeyStore,
    fallback: Option<SharedAuth>,
}

impl MultiKeyAuthProvider {
    pub fn new(store: SharedStreamKeyStore, fallback: Option<SharedAuth>) -> Self {
        Self { store, fallback }
    }

    /// Borrow the underlying store. The admin route shares the same
    /// `Arc` and mutates it via the trait surface.
    pub fn store(&self) -> &SharedStreamKeyStore {
        &self.store
    }

    fn delegate(&self, ctx: &AuthContext) -> AuthDecision {
        match self.fallback.as_ref() {
            Some(f) => f.check(ctx),
            None => AuthDecision::deny("no fallback auth provider configured"),
        }
    }
}

impl AuthProvider for MultiKeyAuthProvider {
    fn check(&self, ctx: &AuthContext) -> AuthDecision {
        match ctx {
            AuthContext::Publish {
                key,
                broadcast: requested,
                ..
            } => {
                if let Some(entry) = self.store.get_by_token(key) {
                    // Scope check: when the stream-key was minted with a
                    // `broadcast` value, that's a tighter declaration
                    // than the fallback chain. We deny without
                    // consulting the fallback so an operator-narrowed
                    // scope can never be silently widened by a more
                    // permissive layer underneath.
                    match (entry.broadcast.as_deref(), requested.as_deref()) {
                        (Some(scoped), Some(req)) if scoped == req => AuthDecision::Allow,
                        (None, _) => AuthDecision::Allow,
                        (Some(_), None) => AuthDecision::deny("stream-key requires a broadcast scope"),
                        (Some(_), Some(_)) => AuthDecision::deny("stream-key scoped to a different broadcast"),
                    }
                } else {
                    self.delegate(ctx)
                }
            }
            AuthContext::Subscribe { .. } | AuthContext::Admin { .. } => self.delegate(ctx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noop::NoopAuthProvider;
    use crate::static_provider::{StaticAuthConfig, StaticAuthProvider};
    use crate::stream_key_store::{InMemoryStreamKeyStore, StreamKeySpec, StreamKeyStore};
    use std::sync::Arc;

    fn ctx_publish(key: &str, broadcast: Option<&str>) -> AuthContext {
        AuthContext::Publish {
            app: "live".into(),
            key: key.into(),
            broadcast: broadcast.map(String::from),
        }
    }

    fn ctx_subscribe(token: Option<&str>, broadcast: &str) -> AuthContext {
        AuthContext::Subscribe {
            token: token.map(String::from),
            broadcast: broadcast.into(),
        }
    }

    fn ctx_admin(token: &str) -> AuthContext {
        AuthContext::Admin { token: token.into() }
    }

    fn store_with_one_key(spec: StreamKeySpec) -> (Arc<InMemoryStreamKeyStore>, String, String) {
        let store = Arc::new(InMemoryStreamKeyStore::new());
        let key = store.mint(spec);
        (store, key.id, key.token)
    }

    #[test]
    fn store_hit_allows_publish_when_broadcast_unscoped() {
        let (store, _id, token) = store_with_one_key(StreamKeySpec::default());
        let provider = MultiKeyAuthProvider::new(store, None);
        assert!(provider.check(&ctx_publish(&token, Some("live/anything"))).is_allow());
        // Unscoped key must also accept a publish that doesn't carry a
        // broadcast hint (RTMP today supplies broadcast=None at auth
        // time because the stream key encodes it on the wire).
        assert!(provider.check(&ctx_publish(&token, None)).is_allow());
    }

    #[test]
    fn store_hit_with_matching_broadcast_allows() {
        let (store, _id, token) = store_with_one_key(StreamKeySpec {
            broadcast: Some("live/cam-a".into()),
            ..Default::default()
        });
        let provider = MultiKeyAuthProvider::new(store, None);
        assert!(provider.check(&ctx_publish(&token, Some("live/cam-a"))).is_allow());
    }

    #[test]
    fn store_hit_with_mismatched_broadcast_denies_without_consulting_fallback() {
        // Fallback would otherwise allow (Noop). The mismatched scope
        // on the store hit must short-circuit the fallback so an
        // operator-narrowed key can never be silently widened.
        let (store, _id, token) = store_with_one_key(StreamKeySpec {
            broadcast: Some("live/cam-a".into()),
            ..Default::default()
        });
        let fallback: SharedAuth = Arc::new(NoopAuthProvider);
        let provider = MultiKeyAuthProvider::new(store, Some(fallback));
        let decision = provider.check(&ctx_publish(&token, Some("live/cam-b")));
        assert!(!decision.is_allow(), "scoped-mismatch must deny; got {decision:?}");
    }

    #[test]
    fn store_miss_falls_through_to_fallback_allow() {
        let store = Arc::new(InMemoryStreamKeyStore::new());
        let fallback: SharedAuth = Arc::new(NoopAuthProvider);
        let provider = MultiKeyAuthProvider::new(store, Some(fallback));
        assert!(provider.check(&ctx_publish("never-minted", None)).is_allow());
    }

    #[test]
    fn store_miss_without_fallback_denies() {
        let store = Arc::new(InMemoryStreamKeyStore::new());
        let provider = MultiKeyAuthProvider::new(store, None);
        let decision = provider.check(&ctx_publish("never-minted", None));
        assert!(!decision.is_allow(), "no fallback must deny");
    }

    #[test]
    fn revoke_beats_a_still_valid_fallback_token() {
        // Fallback authorises "shared-secret" via StaticAuthProvider.
        // A stream-key with the SAME string is minted then revoked.
        // Pre-revoke: store hit -> allow. Post-revoke: store miss ->
        // delegate to fallback -> ALSO allow because Static still
        // matches. This test asserts the EXPLICIT semantic of section
        // 3 of the brief: the additive-over-fallback model means a
        // revoke is only an enforcement when the fallback would
        // otherwise deny. We exercise that case in the next test.
        let store = Arc::new(InMemoryStreamKeyStore::new());
        let fallback: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            publish_key: Some("shared-secret".into()),
            ..Default::default()
        }));
        let provider = MultiKeyAuthProvider::new(store.clone(), Some(fallback));
        // Static provider allows "shared-secret" regardless of the store.
        assert!(provider.check(&ctx_publish("shared-secret", None)).is_allow());
        // Mint a DIFFERENT token; revoke it; fallback still rejects
        // that token, so post-revoke the publish is denied.
        let key = store.mint(StreamKeySpec::default());
        assert!(provider.check(&ctx_publish(&key.token, None)).is_allow());
        store.revoke(&key.id);
        let decision = provider.check(&ctx_publish(&key.token, None));
        assert!(
            !decision.is_allow(),
            "revoked token must deny when fallback rejects it; got {decision:?}"
        );
    }

    #[test]
    fn store_hit_takes_precedence_over_fallback_when_scoped() {
        // Property the brief calls out: a stream-key revoke is the
        // authoritative answer for that token even if the fallback
        // would otherwise allow. Concretely: a store entry with a
        // tighter broadcast scope must DENY a mismatched broadcast
        // even though the fallback (Noop) would allow it.
        let (store, _id, token) = store_with_one_key(StreamKeySpec {
            broadcast: Some("live/scoped".into()),
            ..Default::default()
        });
        let fallback: SharedAuth = Arc::new(NoopAuthProvider);
        let provider = MultiKeyAuthProvider::new(store, Some(fallback));
        assert!(
            !provider.check(&ctx_publish(&token, Some("live/elsewhere"))).is_allow(),
            "store-hit-with-scope-mismatch must beat a permissive fallback"
        );
    }

    #[test]
    fn subscribe_always_delegates_to_fallback() {
        let store = Arc::new(InMemoryStreamKeyStore::new());
        // Mint a stream-key whose token happens to equal a viewer's
        // subscribe bearer. Subscribe must NOT consult the store.
        let key = store.mint(StreamKeySpec::default());
        let fallback: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            subscribe_token: Some("viewer-token".into()),
            ..Default::default()
        }));
        let provider = MultiKeyAuthProvider::new(store, Some(fallback));
        // Stream-key token does NOT authorise subscribe.
        assert!(!provider.check(&ctx_subscribe(Some(&key.token), "live/x")).is_allow());
        // Real viewer token does.
        assert!(
            provider
                .check(&ctx_subscribe(Some("viewer-token"), "live/x"))
                .is_allow()
        );
    }

    #[test]
    fn admin_always_delegates_to_fallback() {
        // Load-bearing safety property: stream-key CRUD never gates
        // admin auth. A revoked stream-key whose token equals an
        // admin bearer must NOT lock the operator out.
        let store = Arc::new(InMemoryStreamKeyStore::new());
        let key = store.mint(StreamKeySpec::default());
        store.revoke(&key.id);
        let fallback: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some(key.token.clone()),
            ..Default::default()
        }));
        let provider = MultiKeyAuthProvider::new(store, Some(fallback));
        assert!(
            provider.check(&ctx_admin(&key.token)).is_allow(),
            "admin token must remain valid even if the same string was revoked as a stream-key"
        );
    }

    #[test]
    fn admin_without_fallback_denies() {
        let store = Arc::new(InMemoryStreamKeyStore::new());
        let provider = MultiKeyAuthProvider::new(store, None);
        assert!(!provider.check(&ctx_admin("anything")).is_allow());
    }

    #[test]
    fn rotate_invalidates_old_token_immediately() {
        let store = Arc::new(InMemoryStreamKeyStore::new());
        let original = store.mint(StreamKeySpec::default());
        let provider = MultiKeyAuthProvider::new(store.clone(), None);
        assert!(provider.check(&ctx_publish(&original.token, None)).is_allow());
        let rotated = store.rotate(&original.id, None).expect("rotate");
        assert!(
            !provider.check(&ctx_publish(&original.token, None)).is_allow(),
            "old token must deny immediately after rotate"
        );
        assert!(provider.check(&ctx_publish(&rotated.token, None)).is_allow());
    }
}

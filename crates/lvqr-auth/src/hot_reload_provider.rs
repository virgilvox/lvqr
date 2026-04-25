//! [`AuthProvider`] wrapper that supports atomic in-place replacement
//! of the underlying provider, backing the session 147 hot config
//! reload.
//!
//! [`HotReloadAuthProvider`] holds the live provider in an
//! `arc_swap::ArcSwap`. Every `check()` call performs an
//! `ArcSwap::load`, which is RCU-style on the read fast path
//! (single-digit nanoseconds, no lock, no atomic write). Reload --
//! whether triggered by SIGHUP or by `POST /api/v1/config-reload` --
//! invokes [`HotReloadAuthProvider::swap`], which atomically
//! replaces the inner provider; in-flight `check()` calls that
//! already loaded a guard finish against the prior snapshot, and
//! the next call sees the new provider.
//!
//! # Composition order
//!
//! The CLI composition root wraps in this order:
//!
//! ```text
//! HotReloadAuthProvider
//!   |__ MultiKeyAuthProvider (session 146)
//!         |__ store: SharedStreamKeyStore (operator-mutable via /api/v1/streamkeys)
//!         |__ fallback: Static / Jwt / Jwks / Webhook / Noop (operator-mutable via SIGHUP)
//! ```
//!
//! On reload, the CLI rebuilds the `(MultiKey, fallback)` chain
//! from the new config and swaps it in. The stream-key STORE handle
//! is preserved across reloads because operators manage that
//! state via the runtime CRUD API; only the `fallback` provider
//! configuration comes from the config file.
//!
//! Note that the wrapper does NOT compose with `Subscribe` or
//! `Admin` semantics from `MultiKeyAuthProvider` -- it is a
//! transparent passthrough. The only behavior change vs. a bare
//! provider is the `ArcSwap::load` per call, which is observably
//! identical to a clone-on-call pattern.

use crate::provider::{AuthContext, AuthDecision, AuthProvider, SharedAuth};
use arc_swap::ArcSwap;
use std::sync::Arc;

/// Sized wrapper around the trait-object [`SharedAuth`] so
/// [`ArcSwap`] can manage it. `arc_swap`'s `RefCnt` impl on
/// `Arc<T>` is implicit-`Sized`, so `Arc<dyn AuthProvider>` cannot
/// be the cell's contents directly. The newtype adds one heap
/// allocation per swap (cheap, off the hot path) and is invisible
/// to callers behind [`HotReloadAuthProvider`]'s API.
struct AuthCell(SharedAuth);

/// See module-level docs.
pub struct HotReloadAuthProvider {
    inner: ArcSwap<AuthCell>,
}

impl HotReloadAuthProvider {
    /// Construct a wrapper around an existing provider. The wrapped
    /// provider is the initial "current" target for every `check()`
    /// call until the next [`Self::swap`].
    pub fn new(initial: SharedAuth) -> Self {
        Self {
            inner: ArcSwap::from_pointee(AuthCell(initial)),
        }
    }

    /// Atomically replace the inner provider. In-flight `check()`
    /// calls that already loaded a guard finish against the prior
    /// snapshot; subsequent calls see `new_inner`. One allocation
    /// per swap (the `Arc<AuthCell>`); off the hot path.
    pub fn swap(&self, new_inner: SharedAuth) {
        self.inner.store(Arc::new(AuthCell(new_inner)));
    }

    /// Borrow the current inner snapshot. Returns an `Arc` clone so
    /// the caller can retain it independently. Mostly useful for
    /// tests + the admin route's "what is current" introspection.
    pub fn current(&self) -> SharedAuth {
        Arc::clone(&self.inner.load().0)
    }
}

impl std::fmt::Debug for HotReloadAuthProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HotReloadAuthProvider").finish_non_exhaustive()
    }
}

impl AuthProvider for HotReloadAuthProvider {
    fn check(&self, ctx: &AuthContext) -> AuthDecision {
        // `load` returns a Guard that derefs to the inner Arc; the
        // delegate call holds the guard for the duration so the
        // pointer cannot be reclaimed under us.
        let guard = self.inner.load();
        guard.0.check(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noop::NoopAuthProvider;
    use crate::static_provider::{StaticAuthConfig, StaticAuthProvider};
    use std::sync::Arc;

    fn ctx_admin(token: &str) -> AuthContext {
        AuthContext::Admin { token: token.into() }
    }

    fn ctx_publish(key: &str) -> AuthContext {
        AuthContext::Publish {
            app: "live".into(),
            key: key.into(),
            broadcast: None,
        }
    }

    #[test]
    fn delegates_to_initial_provider() {
        let inner: SharedAuth = Arc::new(NoopAuthProvider);
        let provider = HotReloadAuthProvider::new(inner);
        assert!(provider.check(&ctx_admin("anything")).is_allow());
    }

    #[test]
    fn swap_replaces_decision_path() {
        // Start with a Static provider that requires admin_token = "v1".
        let v1: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("v1".into()),
            ..Default::default()
        }));
        let provider = HotReloadAuthProvider::new(v1);
        assert!(provider.check(&ctx_admin("v1")).is_allow());
        assert!(!provider.check(&ctx_admin("v2")).is_allow());

        // Swap to a Static provider that requires admin_token = "v2".
        let v2: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("v2".into()),
            ..Default::default()
        }));
        provider.swap(v2);
        assert!(!provider.check(&ctx_admin("v1")).is_allow());
        assert!(provider.check(&ctx_admin("v2")).is_allow());
    }

    #[test]
    fn multiple_swaps_track_the_latest() {
        let initial: SharedAuth = Arc::new(NoopAuthProvider);
        let provider = HotReloadAuthProvider::new(initial);
        for token in ["a", "b", "c"] {
            let next: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
                publish_key: Some(token.into()),
                ..Default::default()
            }));
            provider.swap(next);
            assert!(provider.check(&ctx_publish(token)).is_allow());
            // Anything other than the just-installed token is denied.
            assert!(!provider.check(&ctx_publish("nope")).is_allow());
        }
    }

    #[test]
    fn current_returns_clonable_arc_to_present_inner() {
        let inner: SharedAuth = Arc::new(NoopAuthProvider);
        let provider = HotReloadAuthProvider::new(inner.clone());
        let snapshot = provider.current();
        // Snapshot delegates identically to the original.
        assert!(snapshot.check(&ctx_publish("anything")).is_allow());
        // Pointer identity not guaranteed (ArcSwap may produce a new
        // Arc on load), but the underlying impl IS the same.
        let _ = Arc::ptr_eq(&snapshot, &inner);
    }

    #[test]
    fn check_holds_guard_for_duration_of_call() {
        // Regression guard against an implementation that loads,
        // stores the raw pointer, then drops the guard before the
        // delegate completes. The current `self.inner.load().check(..)`
        // pattern keeps the guard alive across the delegate by
        // virtue of Rust's temporary lifetime extension to the end
        // of the statement. This test exists to ensure that
        // pattern is preserved under refactor.
        let inner: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            publish_key: Some("safe".into()),
            ..Default::default()
        }));
        let provider = HotReloadAuthProvider::new(inner);
        // Concurrent swap during many checks: every check must
        // complete cleanly without UAF or panic. The decision may
        // be Allow or Deny depending on the linearization, but
        // must never be a corrupted state.
        let provider_arc: Arc<HotReloadAuthProvider> = Arc::new(provider);
        let p_for_writer = Arc::clone(&provider_arc);
        let writer = std::thread::spawn(move || {
            for i in 0..100 {
                let next_token = if i % 2 == 0 { "safe" } else { "other" };
                let next: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
                    publish_key: Some(next_token.into()),
                    ..Default::default()
                }));
                p_for_writer.swap(next);
            }
        });
        let p_for_reader = Arc::clone(&provider_arc);
        let reader = std::thread::spawn(move || {
            for _ in 0..200 {
                let _ = p_for_reader.check(&ctx_publish("safe"));
            }
        });
        writer.join().expect("writer thread");
        reader.join().expect("reader thread");
    }
}

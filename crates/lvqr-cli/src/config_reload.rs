//! Hot config reload (session 147).
//!
//! Owns the runtime state needed to rebuild the live auth provider
//! from a fresh `--config` file read. Both SIGHUP (Unix-only) and
//! `POST /api/v1/config-reload` (cross-platform) feed into the same
//! [`ConfigReloadHandle::reload`] entry point.
//!
//! # Scope (v1)
//!
//! * **Auth provider** rebuilds atomically against the merged
//!   (CLI defaults + file overrides) shape. Static / JWT-HS256 paths
//!   only -- the JWKS and webhook providers retain their boot-time
//!   values because their constructors are async + cache HTTP state
//!   that does not round-trip cleanly through a synchronous swap.
//! * **Stream-key store** is preserved across reloads (operators
//!   manage it via the `/api/v1/streamkeys/*` runtime CRUD API
//!   shipped in session 146).
//!
//! # Deferred to a future increment
//!
//! * Mesh ICE servers + HMAC playback secret reload (the briefing
//!   names them as hot-reloadable; the wiring requires Arc-swap-
//!   threading through the signal callback + the playback auth
//!   middleware respectively, which is its own session).
//! * Warn-on-diff for non-hot keys (port bindings, feature flags,
//!   record/archive dirs).
//! * `jwks_url` and `webhook_auth_url` reload (async builder
//!   complexity -- they stay at their boot-time values).
//!
//! These deferrals are documented in `docs/config-reload.md` and on
//! the admin route response in the `warnings` field when an
//! operator's reload would have touched a deferred key.

use crate::config_file::{AuthSection, ServeConfigFile};
use anyhow::{Context, Result};
use lvqr_admin::ConfigReloadStatus;
use lvqr_auth::{
    HotReloadAuthProvider, MultiKeyAuthProvider, NoopAuthProvider, SharedAuth, SharedStreamKeyStore, StaticAuthConfig,
    StaticAuthProvider,
};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Auth-shaped CLI defaults captured at boot. The reload pipeline
/// merges these with the file's [`AuthSection`] overrides (file
/// wins on a non-`None` field, defaults fill the rest).
#[derive(Debug, Clone, Default)]
pub struct AuthBootDefaults {
    pub admin_token: Option<String>,
    pub publish_key: Option<String>,
    pub subscribe_token: Option<String>,
    pub jwt_secret: Option<String>,
    pub jwt_issuer: Option<String>,
    pub jwt_audience: Option<String>,
}

impl AuthBootDefaults {
    /// Apply file overrides on top of the boot defaults: every
    /// non-`None` field in `overrides` wins over the corresponding
    /// boot default. Returns the effective auth shape the reload
    /// pipeline will hand to [`build_static_auth_from_effective`].
    pub fn merge_with(&self, overrides: &AuthSection) -> AuthBootDefaults {
        AuthBootDefaults {
            admin_token: overrides.admin_token.clone().or_else(|| self.admin_token.clone()),
            publish_key: overrides.publish_key.clone().or_else(|| self.publish_key.clone()),
            subscribe_token: overrides
                .subscribe_token
                .clone()
                .or_else(|| self.subscribe_token.clone()),
            jwt_secret: overrides.jwt_secret.clone().or_else(|| self.jwt_secret.clone()),
            jwt_issuer: overrides.jwt_issuer.clone().or_else(|| self.jwt_issuer.clone()),
            jwt_audience: overrides.jwt_audience.clone().or_else(|| self.jwt_audience.clone()),
        }
    }
}

/// Build the inner (non-MultiKey-wrapped) auth provider from the
/// effective auth shape. Mirrors the boot-time
/// `build_static_or_jwt_auth` cascade: JWT-HS256 if `jwt_secret`,
/// else static-token if any of the three tokens is set, else
/// `NoopAuthProvider`.
pub fn build_static_auth_from_effective(eff: &AuthBootDefaults) -> Result<SharedAuth> {
    if let Some(secret) = &eff.jwt_secret {
        let provider = lvqr_auth::JwtAuthProvider::new(lvqr_auth::JwtAuthConfig {
            secret: secret.clone(),
            issuer: eff.jwt_issuer.clone(),
            audience: eff.jwt_audience.clone(),
        })
        .map_err(|e| anyhow::anyhow!("JWT provider rebuild failed: {e}"))?;
        return Ok(Arc::new(provider) as SharedAuth);
    }
    let cfg = StaticAuthConfig {
        admin_token: eff.admin_token.clone(),
        publish_key: eff.publish_key.clone(),
        subscribe_token: eff.subscribe_token.clone(),
    };
    if cfg.has_any() {
        Ok(Arc::new(StaticAuthProvider::new(cfg)) as SharedAuth)
    } else {
        Ok(Arc::new(NoopAuthProvider) as SharedAuth)
    }
}

/// Mutable state tracked across reloads.
#[derive(Default)]
struct ReloadState {
    last_reload_at_ms: Option<u64>,
    last_reload_kind: Option<String>,
    applied_keys: Vec<String>,
    warnings: Vec<String>,
}

/// Owns the live state needed to drive a reload. The CLI's
/// composition root constructs one (when `--config` is set) and
/// hands it off to the SIGHUP listener task + the admin router via
/// `AdminState::with_config_reload`.
pub struct ConfigReloadHandle {
    config_path: PathBuf,
    boot_defaults: AuthBootDefaults,
    streamkey_store: Option<SharedStreamKeyStore>,
    streamkeys_enabled: bool,
    hot_provider: Arc<HotReloadAuthProvider>,
    state: Mutex<ReloadState>,
}

impl ConfigReloadHandle {
    pub fn new(
        config_path: PathBuf,
        boot_defaults: AuthBootDefaults,
        streamkey_store: Option<SharedStreamKeyStore>,
        streamkeys_enabled: bool,
        hot_provider: Arc<HotReloadAuthProvider>,
    ) -> Self {
        Self {
            config_path,
            boot_defaults,
            streamkey_store,
            streamkeys_enabled,
            hot_provider,
            state: Mutex::new(ReloadState::default()),
        }
    }

    /// Path to the configured `--config` file. Used by the admin
    /// route to surface the path on the GET status response.
    pub fn config_path(&self) -> &std::path::Path {
        &self.config_path
    }

    /// Trigger a reload. Re-reads the file, rebuilds the inner auth
    /// (Static / JWT-HS256), wraps in MultiKey if streamkeys are
    /// enabled, then atomically swaps the wrapper. Failures during
    /// build (e.g. malformed file, JWT secret rejected by the
    /// provider) leave the prior provider in place and surface as
    /// `Err` on the route.
    ///
    /// `kind` is recorded on the status surface so operators can
    /// distinguish SIGHUP-driven reloads from admin-API-driven
    /// reloads in audit logs.
    pub fn reload(&self, kind: &str) -> Result<ConfigReloadStatus> {
        let file = ServeConfigFile::from_path(&self.config_path)
            .with_context(|| format!("read config file at {}", self.config_path.display()))?;

        let effective = self.boot_defaults.merge_with(&file.auth);
        let new_inner =
            build_static_auth_from_effective(&effective).context("rebuild auth provider from effective config")?;

        let new_chain: SharedAuth = if self.streamkeys_enabled {
            if let Some(store) = &self.streamkey_store {
                Arc::new(MultiKeyAuthProvider::new(store.clone(), Some(new_inner))) as SharedAuth
            } else {
                new_inner
            }
        } else {
            new_inner
        };

        let mut warnings = Vec::new();
        if !file.mesh_ice_servers.is_empty() {
            warnings.push("mesh_ice_servers in config file ignored: hot reload deferred to a future session".into());
        }
        if file.hmac_playback_secret.is_some() {
            warnings
                .push("hmac_playback_secret in config file ignored: hot reload deferred to a future session".into());
        }

        // Atomic swap. From this point on, every new auth-check
        // call lands on the new chain.
        self.hot_provider.swap(new_chain);

        let now_ms = unix_now_ms();
        let kind_string = kind.to_string();
        let mut state = self.state.lock();
        state.last_reload_at_ms = Some(now_ms);
        state.last_reload_kind = Some(kind_string.clone());
        state.applied_keys = vec!["auth".into()];
        state.warnings = warnings.clone();

        tracing::info!(
            kind = %kind_string,
            path = %self.config_path.display(),
            warnings = warnings.len(),
            "config reload applied"
        );

        Ok(ConfigReloadStatus {
            config_path: Some(self.config_path.display().to_string()),
            last_reload_at_ms: Some(now_ms),
            last_reload_kind: Some(kind_string),
            applied_keys: vec!["auth".into()],
            warnings,
        })
    }

    /// Read-only status for `GET /api/v1/config-reload`.
    pub fn status(&self) -> ConfigReloadStatus {
        let state = self.state.lock();
        ConfigReloadStatus {
            config_path: Some(self.config_path.display().to_string()),
            last_reload_at_ms: state.last_reload_at_ms,
            last_reload_kind: state.last_reload_kind.clone(),
            applied_keys: state.applied_keys.clone(),
            warnings: state.warnings.clone(),
        }
    }
}

/// `Arc<ConfigReloadHandle>` shared between SIGHUP listener + admin
/// router.
pub type SharedReloadHandle = Arc<ConfigReloadHandle>;

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lvqr_auth::{AuthContext, AuthProvider, InMemoryStreamKeyStore};

    fn ctx_publish(key: &str) -> AuthContext {
        AuthContext::Publish {
            app: "live".into(),
            key: key.into(),
            broadcast: None,
        }
    }

    fn write_config(dir: &std::path::Path, body: &str) -> PathBuf {
        let path = dir.join("lvqr.toml");
        std::fs::write(&path, body).expect("write config");
        path
    }

    fn make_handle(path: PathBuf, boot_publish: Option<&str>) -> SharedReloadHandle {
        let boot = AuthBootDefaults {
            publish_key: boot_publish.map(String::from),
            ..Default::default()
        };
        let inner = build_static_auth_from_effective(&boot).expect("boot build");
        let hot = Arc::new(HotReloadAuthProvider::new(inner));
        Arc::new(ConfigReloadHandle::new(path, boot, None, false, hot))
    }

    #[test]
    fn reload_replaces_publish_key_from_file() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(
            dir.path(),
            r#"[auth]
publish_key = "from-file-v1""#,
        );
        let handle = make_handle(path.clone(), None);

        // Before reload: no publish_key configured -> NoopAuthProvider
        // -> any key allowed.
        assert!(handle.hot_provider.check(&ctx_publish("anything")).is_allow());

        let status = handle.reload("test").expect("reload ok");
        assert_eq!(status.applied_keys, vec!["auth".to_string()]);
        assert!(status.warnings.is_empty());

        // After reload: publish_key=from-file-v1 wins.
        assert!(handle.hot_provider.check(&ctx_publish("from-file-v1")).is_allow());
        assert!(!handle.hot_provider.check(&ctx_publish("anything")).is_allow());
    }

    #[test]
    fn boot_defaults_fill_unset_file_fields() {
        let dir = tempfile::tempdir().expect("tmp");
        // File omits publish_key; CLI default kicks in.
        let path = write_config(
            dir.path(),
            r#"[auth]
admin_token = "from-file""#,
        );
        let handle = make_handle(path, Some("from-cli-default"));

        handle.reload("test").expect("reload ok");

        // CLI default for publish_key still in effect post-reload.
        assert!(handle.hot_provider.check(&ctx_publish("from-cli-default")).is_allow());
        assert!(!handle.hot_provider.check(&ctx_publish("nope")).is_allow());
    }

    #[test]
    fn reload_again_with_changed_file_swaps_to_v2() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(
            dir.path(),
            r#"[auth]
publish_key = "v1""#,
        );
        let handle = make_handle(path.clone(), None);
        handle.reload("first").expect("reload v1");
        assert!(handle.hot_provider.check(&ctx_publish("v1")).is_allow());

        std::fs::write(
            &path,
            r#"[auth]
publish_key = "v2""#,
        )
        .expect("rewrite");
        handle.reload("second").expect("reload v2");

        assert!(!handle.hot_provider.check(&ctx_publish("v1")).is_allow());
        assert!(handle.hot_provider.check(&ctx_publish("v2")).is_allow());
    }

    #[test]
    fn reload_failure_leaves_prior_state_intact() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(
            dir.path(),
            r#"[auth]
publish_key = "good""#,
        );
        let handle = make_handle(path.clone(), None);
        handle.reload("ok").expect("baseline ok");
        assert!(handle.hot_provider.check(&ctx_publish("good")).is_allow());

        // Corrupt the file. Reload errors.
        std::fs::write(&path, "this is = not = valid toml").expect("write garbage");
        let err = handle.reload("malformed").expect_err("must error");
        let chain = format!("{err:#}");
        assert!(
            chain.to_lowercase().contains("parse") || chain.to_lowercase().contains("toml"),
            "unexpected error chain: {chain}"
        );

        // Prior state still in effect: "good" still authenticates.
        assert!(handle.hot_provider.check(&ctx_publish("good")).is_allow());
    }

    #[test]
    fn warnings_surface_for_deferred_sections() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(
            dir.path(),
            r#"
hmac_playback_secret = "abc"

[[mesh_ice_servers]]
urls = ["stun:stun.l.google.com:19302"]"#,
        );
        let handle = make_handle(path, None);

        let status = handle.reload("test").expect("reload ok");
        assert_eq!(status.warnings.len(), 2);
        assert!(status.warnings.iter().any(|w| w.contains("hmac_playback_secret")));
        assert!(status.warnings.iter().any(|w| w.contains("mesh_ice_servers")));
    }

    #[test]
    fn status_returns_path_even_before_first_reload() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(dir.path(), "");
        let handle = make_handle(path.clone(), None);
        let status = handle.status();
        assert_eq!(status.config_path.as_deref(), Some(path.display().to_string().as_str()));
        assert!(status.last_reload_at_ms.is_none());
        assert!(status.last_reload_kind.is_none());
    }

    #[test]
    fn streamkey_store_preserved_across_reloads() {
        // When MultiKey is enabled (streamkeys_enabled=true), the
        // store handle is captured once in the ConfigReloadHandle
        // and the new chain reuses the SAME store on every reload.
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(
            dir.path(),
            r#"[auth]
publish_key = "fallback""#,
        );
        let store: SharedStreamKeyStore = Arc::new(InMemoryStreamKeyStore::new());
        let key = store.mint(lvqr_auth::StreamKeySpec::default());

        let boot = AuthBootDefaults {
            publish_key: Some("fallback".into()),
            ..Default::default()
        };
        let inner = build_static_auth_from_effective(&boot).expect("boot");
        let chain: SharedAuth = Arc::new(MultiKeyAuthProvider::new(store.clone(), Some(inner)));
        let hot = Arc::new(HotReloadAuthProvider::new(chain));
        let handle = Arc::new(ConfigReloadHandle::new(
            path,
            boot,
            Some(store.clone()),
            true,
            hot.clone(),
        ));

        // Pre-reload: minted key authenticates (store hit).
        assert!(hot.check(&ctx_publish(&key.token)).is_allow());

        handle.reload("test").expect("reload ok");

        // Post-reload: minted key STILL authenticates (same store
        // handle, fresh MultiKey wrap pointing at the same Arc).
        assert!(hot.check(&ctx_publish(&key.token)).is_allow());
    }
}

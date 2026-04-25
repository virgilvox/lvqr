//! Hot config reload (sessions 147 + 148).
//!
//! Owns the runtime state needed to rebuild the live auth provider,
//! mesh ICE-server list, and HMAC playback secret from a fresh
//! `--config` file read. Both SIGHUP (Unix-only) and `POST
//! /api/v1/config-reload` (cross-platform) feed into the same
//! [`ConfigReloadHandle::reload`] entry point.
//!
//! # Hot-reloadable keys (v2)
//!
//! * **Auth provider** rebuilds atomically against the merged
//!   (CLI defaults + file overrides) shape. Static / JWT-HS256 paths
//!   only -- the JWKS and webhook providers retain their boot-time
//!   values because their constructors are async + cache HTTP state
//!   that does not round-trip cleanly through a synchronous swap.
//! * **Mesh ICE servers** (session 148): the file's
//!   `mesh_ice_servers` array replaces the live snapshot the
//!   `/signal` callback hands to clients via `AssignParent`. Empty
//!   array clears the list (clients fall back to their constructor
//!   default).
//! * **HMAC playback secret** (session 148): the file's top-level
//!   `hmac_playback_secret` replaces the live secret used by the
//!   live HLS / DASH and DVR `/playback/*` middleware. Missing key
//!   clears the secret (subsequent signed URLs fail; the routes
//!   fall through to the subscribe-token gate).
//! * **Stream-key store** is preserved across reloads (operators
//!   manage it via the `/api/v1/streamkeys/*` runtime CRUD API
//!   shipped in session 146).
//!
//! # Still deferred
//!
//! * `jwks_url` and `webhook_auth_url` reload (async builder
//!   complexity -- they stay at their boot-time values; the route's
//!   warnings field stays in the wire shape for forward-compat).
//! * Structural-key reload (port bindings, feature flags,
//!   record/archive dirs, mesh_enabled, cluster_listen). Reload
//!   never rebinds sockets or restarts subsystems.

use crate::config_file::{AuthSection, ServeConfigFile};
use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use lvqr_admin::ConfigReloadStatus;
use lvqr_auth::{
    HotReloadAuthProvider, MultiKeyAuthProvider, NoopAuthProvider, SharedAuth, SharedStreamKeyStore, StaticAuthConfig,
    StaticAuthProvider,
};
use lvqr_signal::IceServer;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Atomically swappable handle to the operator's mesh ICE server
/// list. Built once at boot, cloned into the `/signal` peer
/// callback, and replaced on each reload by
/// [`ConfigReloadHandle::reload`]. The callback `load_full`s on
/// every emit; cost is one ArcSwap::load (single-digit ns) on top
/// of the existing per-emit clone.
pub type SwappableIceServers = Arc<ArcSwap<Vec<IceServer>>>;

/// Atomically swappable handle to the operator's HMAC-SHA256
/// playback secret. `Some(arc_bytes)` configures the live HLS /
/// DASH and `/playback/*` routes to honor `?sig=...&exp=...`;
/// `None` falls through to the subscribe-token gate. Replaced on
/// each reload by [`ConfigReloadHandle::reload`]; the live-playback
/// middleware `load_full`s per request.
pub type SwappableHmacSecret = Arc<ArcSwap<Option<Arc<[u8]>>>>;

/// Build a fresh [`SwappableIceServers`] from a starting list. The
/// composition root calls this once at boot before constructing
/// the [`ConfigReloadHandle`].
pub fn new_ice_swap(initial: Vec<IceServer>) -> SwappableIceServers {
    Arc::new(ArcSwap::from_pointee(initial))
}

/// Build a fresh [`SwappableHmacSecret`] from a starting secret
/// string. `None` (no `--hmac-playback-secret` and no file entry)
/// produces a swap whose inner is `None`.
pub fn new_hmac_swap(initial: Option<&str>) -> SwappableHmacSecret {
    let inner: Option<Arc<[u8]>> = initial.map(|s| Arc::<[u8]>::from(s.as_bytes()));
    Arc::new(ArcSwap::from_pointee(inner))
}

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
    ice_swap: SwappableIceServers,
    hmac_swap: SwappableHmacSecret,
    state: Mutex<ReloadState>,
}

impl ConfigReloadHandle {
    pub fn new(
        config_path: PathBuf,
        boot_defaults: AuthBootDefaults,
        streamkey_store: Option<SharedStreamKeyStore>,
        streamkeys_enabled: bool,
        hot_provider: Arc<HotReloadAuthProvider>,
        ice_swap: SwappableIceServers,
        hmac_swap: SwappableHmacSecret,
    ) -> Self {
        Self {
            config_path,
            boot_defaults,
            streamkey_store,
            streamkeys_enabled,
            hot_provider,
            ice_swap,
            hmac_swap,
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
    /// provider) leave the prior provider, ICE list, and HMAC
    /// secret in place and surface as `Err` on the route.
    ///
    /// Beyond the auth chain, the reload also swaps:
    ///
    /// * `mesh_ice_servers` -> the `/signal` callback's snapshot.
    ///   Empty array clears the list.
    /// * `hmac_playback_secret` -> the live HLS / DASH and
    ///   `/playback/*` middleware secret. `None` clears it.
    ///
    /// `applied_keys` grows entries (`"mesh_ice"` / `"hmac_secret"`)
    /// only when the new value differs from the prior snapshot, so
    /// operators see exactly which categories their reload
    /// effectively touched. `"auth"` is always present (the chain
    /// is rebuilt unconditionally).
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

        // Diff each hot-reloadable category against its prior
        // snapshot BEFORE swapping so applied_keys reflects what
        // actually changed (operators see "auth" alone vs.
        // "auth + hmac_secret", etc.).
        let prior_ice = self.ice_swap.load_full();
        let ice_changed = (*prior_ice) != file.mesh_ice_servers;

        let new_hmac: Option<Arc<[u8]>> = file
            .hmac_playback_secret
            .as_ref()
            .map(|s| Arc::<[u8]>::from(s.as_bytes()));
        let prior_hmac = self.hmac_swap.load_full();
        let hmac_changed = match (prior_hmac.as_ref(), new_hmac.as_ref()) {
            (Some(a), Some(b)) => a.as_ref() != b.as_ref(),
            (None, None) => false,
            _ => true,
        };

        // Atomic swap. From this point on, every new auth-check,
        // signal-callback, and playback-middleware call lands on
        // the new state.
        self.hot_provider.swap(new_chain);
        self.ice_swap.store(Arc::new(file.mesh_ice_servers.clone()));
        self.hmac_swap.store(Arc::new(new_hmac));

        let mut applied_keys: Vec<String> = vec!["auth".into()];
        if ice_changed {
            applied_keys.push("mesh_ice".into());
        }
        if hmac_changed {
            applied_keys.push("hmac_secret".into());
        }
        let warnings: Vec<String> = Vec::new();

        let now_ms = unix_now_ms();
        let kind_string = kind.to_string();
        let mut state = self.state.lock();
        state.last_reload_at_ms = Some(now_ms);
        state.last_reload_kind = Some(kind_string.clone());
        state.applied_keys = applied_keys.clone();
        state.warnings = warnings.clone();

        tracing::info!(
            kind = %kind_string,
            path = %self.config_path.display(),
            applied = applied_keys.len(),
            warnings = warnings.len(),
            "config reload applied"
        );

        Ok(ConfigReloadStatus {
            config_path: Some(self.config_path.display().to_string()),
            last_reload_at_ms: Some(now_ms),
            last_reload_kind: Some(kind_string),
            applied_keys,
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

    /// Build a [`ConfigReloadHandle`] for tests. Each invocation
    /// creates fresh swap handles seeded from
    /// `(boot_ice, boot_hmac)`; the returned tuple lets tests
    /// inspect the swap snapshots after a reload.
    #[allow(clippy::type_complexity)]
    fn make_handle(
        path: PathBuf,
        boot_publish: Option<&str>,
        boot_ice: Vec<IceServer>,
        boot_hmac: Option<&str>,
    ) -> (
        SharedReloadHandle,
        Arc<HotReloadAuthProvider>,
        SwappableIceServers,
        SwappableHmacSecret,
    ) {
        let boot = AuthBootDefaults {
            publish_key: boot_publish.map(String::from),
            ..Default::default()
        };
        let inner = build_static_auth_from_effective(&boot).expect("boot build");
        let hot = Arc::new(HotReloadAuthProvider::new(inner));
        let ice = new_ice_swap(boot_ice);
        let hmac = new_hmac_swap(boot_hmac);
        let handle = Arc::new(ConfigReloadHandle::new(
            path,
            boot,
            None,
            false,
            hot.clone(),
            ice.clone(),
            hmac.clone(),
        ));
        (handle, hot, ice, hmac)
    }

    #[test]
    fn reload_replaces_publish_key_from_file() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(
            dir.path(),
            r#"[auth]
publish_key = "from-file-v1""#,
        );
        let (handle, hot, _ice, _hmac) = make_handle(path.clone(), None, Vec::new(), None);

        // Before reload: no publish_key configured -> NoopAuthProvider
        // -> any key allowed.
        assert!(hot.check(&ctx_publish("anything")).is_allow());

        let status = handle.reload("test").expect("reload ok");
        assert_eq!(status.applied_keys, vec!["auth".to_string()]);
        assert!(status.warnings.is_empty());

        // After reload: publish_key=from-file-v1 wins.
        assert!(hot.check(&ctx_publish("from-file-v1")).is_allow());
        assert!(!hot.check(&ctx_publish("anything")).is_allow());
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
        let (handle, hot, _ice, _hmac) = make_handle(path, Some("from-cli-default"), Vec::new(), None);

        handle.reload("test").expect("reload ok");

        // CLI default for publish_key still in effect post-reload.
        assert!(hot.check(&ctx_publish("from-cli-default")).is_allow());
        assert!(!hot.check(&ctx_publish("nope")).is_allow());
    }

    #[test]
    fn reload_again_with_changed_file_swaps_to_v2() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(
            dir.path(),
            r#"[auth]
publish_key = "v1""#,
        );
        let (handle, hot, _ice, _hmac) = make_handle(path.clone(), None, Vec::new(), None);
        handle.reload("first").expect("reload v1");
        assert!(hot.check(&ctx_publish("v1")).is_allow());

        std::fs::write(
            &path,
            r#"[auth]
publish_key = "v2""#,
        )
        .expect("rewrite");
        handle.reload("second").expect("reload v2");

        assert!(!hot.check(&ctx_publish("v1")).is_allow());
        assert!(hot.check(&ctx_publish("v2")).is_allow());
    }

    #[test]
    fn reload_failure_leaves_prior_state_intact() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(
            dir.path(),
            r#"[auth]
publish_key = "good""#,
        );
        let (handle, hot, ice, hmac) = make_handle(
            path.clone(),
            None,
            vec![IceServer {
                urls: vec!["stun:prior.example:3478".into()],
                username: None,
                credential: None,
            }],
            Some("prior-secret"),
        );
        handle.reload("ok").expect("baseline ok");
        assert!(hot.check(&ctx_publish("good")).is_allow());
        let prior_ice = ice.load_full();
        let prior_hmac = hmac.load_full();

        // Corrupt the file. Reload errors.
        std::fs::write(&path, "this is = not = valid toml").expect("write garbage");
        let err = handle.reload("malformed").expect_err("must error");
        let chain = format!("{err:#}");
        assert!(
            chain.to_lowercase().contains("parse") || chain.to_lowercase().contains("toml"),
            "unexpected error chain: {chain}"
        );

        // Prior state still in effect: "good" still authenticates,
        // and the ICE + HMAC swaps still hold their pre-malformed
        // snapshots (no partial swap on a failed reload).
        assert!(hot.check(&ctx_publish("good")).is_allow());
        assert_eq!(*ice.load_full(), *prior_ice);
        assert_eq!(hmac_bytes(&hmac.load_full()), hmac_bytes(&prior_hmac));
    }

    /// Lift the inner Option's bytes out of an `Arc<Option<Arc<[u8]>>>`
    /// into an owned `Option<Vec<u8>>`. Used by tests that compare a
    /// pre-reload snapshot against the post-reload state without
    /// fighting Option's move-on-map semantics.
    fn hmac_bytes(snap: &Arc<Option<Arc<[u8]>>>) -> Option<Vec<u8>> {
        let inner: &Option<Arc<[u8]>> = snap.as_ref();
        inner.as_ref().map(|a| a.as_ref().to_vec())
    }

    #[test]
    fn applied_keys_includes_mesh_ice_when_diff() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(
            dir.path(),
            r#"
[[mesh_ice_servers]]
urls = ["stun:stun.l.google.com:19302"]"#,
        );
        let (handle, _hot, ice, _hmac) = make_handle(path.clone(), None, Vec::new(), None);

        let status = handle.reload("test").expect("reload ok");
        assert!(status.applied_keys.iter().any(|k| k == "mesh_ice"));
        assert!(status.warnings.is_empty());
        let snapshot = ice.load_full();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].urls, vec!["stun:stun.l.google.com:19302"]);
    }

    #[test]
    fn applied_keys_omits_mesh_ice_when_unchanged() {
        let dir = tempfile::tempdir().expect("tmp");
        let same = vec![IceServer {
            urls: vec!["stun:stun.l.google.com:19302".into()],
            username: None,
            credential: None,
        }];
        let path = write_config(
            dir.path(),
            r#"
[[mesh_ice_servers]]
urls = ["stun:stun.l.google.com:19302"]"#,
        );
        let (handle, _hot, _ice, _hmac) = make_handle(path, None, same, None);

        let status = handle.reload("test").expect("reload ok");
        assert!(
            !status.applied_keys.iter().any(|k| k == "mesh_ice"),
            "ICE list unchanged across boot+file; applied_keys must omit it: {:?}",
            status.applied_keys
        );
    }

    #[test]
    fn applied_keys_includes_hmac_secret_when_diff() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(dir.path(), r#"hmac_playback_secret = "from-file""#);
        let (handle, _hot, _ice, hmac) = make_handle(path, None, Vec::new(), None);

        let status = handle.reload("test").expect("reload ok");
        assert!(status.applied_keys.iter().any(|k| k == "hmac_secret"));
        let snapshot = hmac.load_full();
        let bytes = snapshot.as_ref().as_ref().expect("secret was stored");
        assert_eq!(bytes.as_ref(), b"from-file");
    }

    #[test]
    fn applied_keys_omits_hmac_secret_when_unchanged() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(dir.path(), r#"hmac_playback_secret = "same-secret""#);
        let (handle, _hot, _ice, _hmac) = make_handle(path, None, Vec::new(), Some("same-secret"));

        let status = handle.reload("test").expect("reload ok");
        assert!(
            !status.applied_keys.iter().any(|k| k == "hmac_secret"),
            "HMAC secret unchanged across boot+file; applied_keys must omit it: {:?}",
            status.applied_keys
        );
    }

    #[test]
    fn missing_hmac_in_file_clears_prior_secret() {
        let dir = tempfile::tempdir().expect("tmp");
        // File omits hmac_playback_secret entirely.
        let path = write_config(dir.path(), "");
        let (handle, _hot, _ice, hmac) = make_handle(path, None, Vec::new(), Some("boot-secret"));

        let status = handle.reload("test").expect("reload ok");
        // Diff: prior was Some("boot-secret"), new is None -> diffed.
        assert!(status.applied_keys.iter().any(|k| k == "hmac_secret"));
        let snapshot = hmac.load_full();
        assert!(snapshot.as_ref().is_none(), "missing key in file must clear the secret");
    }

    #[test]
    fn empty_mesh_ice_in_file_clears_prior_list() {
        let dir = tempfile::tempdir().expect("tmp");
        // File omits mesh_ice_servers entirely (empty array shape).
        let path = write_config(dir.path(), "");
        let (handle, _hot, ice, _hmac) = make_handle(
            path,
            None,
            vec![IceServer {
                urls: vec!["stun:boot.example:3478".into()],
                username: None,
                credential: None,
            }],
            None,
        );

        let status = handle.reload("test").expect("reload ok");
        assert!(status.applied_keys.iter().any(|k| k == "mesh_ice"));
        let snapshot = ice.load_full();
        assert!(snapshot.is_empty(), "missing key in file must clear the list");
    }

    #[test]
    fn reload_does_not_emit_deferred_warnings_for_session_148_keys() {
        // Session 147 emitted "deferred" warnings for
        // mesh_ice_servers + hmac_playback_secret. Session 148
        // honors both keys in-place; the warnings drop. This test
        // is the regression guard.
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(
            dir.path(),
            r#"
hmac_playback_secret = "abc"

[[mesh_ice_servers]]
urls = ["stun:stun.l.google.com:19302"]"#,
        );
        let (handle, _hot, _ice, _hmac) = make_handle(path, None, Vec::new(), None);

        let status = handle.reload("test").expect("reload ok");
        assert!(
            status.warnings.is_empty(),
            "session 148: deferred warnings must drop; got {:?}",
            status.warnings
        );
        assert!(status.applied_keys.iter().any(|k| k == "mesh_ice"));
        assert!(status.applied_keys.iter().any(|k| k == "hmac_secret"));
    }

    #[test]
    fn status_returns_path_even_before_first_reload() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = write_config(dir.path(), "");
        let (handle, _hot, _ice, _hmac) = make_handle(path.clone(), None, Vec::new(), None);
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
        let ice = new_ice_swap(Vec::new());
        let hmac = new_hmac_swap(None);
        let handle = Arc::new(ConfigReloadHandle::new(
            path,
            boot,
            Some(store.clone()),
            true,
            hot.clone(),
            ice,
            hmac,
        ));

        // Pre-reload: minted key authenticates (store hit).
        assert!(hot.check(&ctx_publish(&key.token)).is_allow());

        handle.reload("test").expect("reload ok");

        // Post-reload: minted key STILL authenticates (same store
        // handle, fresh MultiKey wrap pointing at the same Arc).
        assert!(hot.check(&ctx_publish(&key.token)).is_allow());
    }
}

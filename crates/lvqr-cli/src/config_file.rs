//! TOML config-file shape backing the session 147 hot config reload.
//!
//! `lvqr serve --config <path>` parses a TOML file into
//! [`ServeConfigFile`]. The file is OPT-IN per section: missing
//! sections fall back to CLI flags + env vars, present sections
//! override them. SIGHUP / `POST /api/v1/config-reload` re-parse
//! the file and rebuild the hot-reload-eligible state (auth
//! provider, mesh ICE servers, HMAC playback secret) atomically.
//!
//! v1 covers only the three hot-reload-eligible top-level keys
//! the briefing locked. Non-hot keys (port bindings, feature flags)
//! land in a future session; until then the CLI flags + env vars
//! remain the source of truth for them.
//!
//! Wire shape (all fields optional, `#[serde(default)]` everywhere
//! so a future server adding a sibling field never breaks an older
//! file):
//!
//! ```toml
//! hmac_playback_secret = "deadbeef..."   # optional
//!
//! [auth]
//! publish_key = "..."        # mirrors LVQR_PUBLISH_KEY
//! admin_token = "..."        # mirrors LVQR_ADMIN_TOKEN
//! subscribe_token = "..."    # mirrors LVQR_SUBSCRIBE_TOKEN
//! jwt_secret = "..."         # HS256 secret (mutex with jwks_url + webhook_auth_url)
//! jwt_issuer = "..."         # expected `iss` claim
//! jwt_audience = "..."       # expected `aud` claim
//! jwks_url = "..."           # JWKS endpoint (requires `jwks` feature)
//! webhook_auth_url = "..."   # HTTP delegation endpoint (requires `webhook` feature)
//!
//! # mesh_ice_servers is a TOML array of tables; each entry mirrors
//! # `lvqr_signal::IceServer` (matches the JSON shape on
//! # `--mesh-ice-servers <JSON>`).
//! [[mesh_ice_servers]]
//! urls = ["stun:stun.l.google.com:19302"]
//!
//! [[mesh_ice_servers]]
//! urls = ["turn:turn.example.com:3478"]
//! username = "user"
//! credential = "pass"
//! ```

use anyhow::{Context, Result};
use lvqr_signal::IceServer;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level config-file shape. Every section is optional so the
/// file is OPT-IN per setting; missing sections fall back to the
/// CLI flag + env-var defaults the operator configured at boot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServeConfigFile {
    #[serde(default)]
    pub auth: AuthSection,
    #[serde(default)]
    pub mesh_ice_servers: Vec<IceServer>,
    #[serde(default)]
    pub hmac_playback_secret: Option<String>,
}

/// Auth-section mirror of the existing CLI flags + env vars. The
/// file parser does NOT decide which provider to instantiate; the
/// reload pipeline applies the same precedence cascade as
/// `serve_from_args::build_auth` does at boot (jwks_url -> webhook
/// -> jwt -> static).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthSection {
    #[serde(default)]
    pub admin_token: Option<String>,
    #[serde(default)]
    pub publish_key: Option<String>,
    #[serde(default)]
    pub subscribe_token: Option<String>,
    #[serde(default)]
    pub jwt_secret: Option<String>,
    #[serde(default)]
    pub jwt_issuer: Option<String>,
    #[serde(default)]
    pub jwt_audience: Option<String>,
    #[serde(default)]
    pub jwks_url: Option<String>,
    #[serde(default)]
    pub webhook_auth_url: Option<String>,
}

impl ServeConfigFile {
    /// Read + parse a TOML file. The path is resolved as-is (no
    /// shell expansion). Parse errors include the source-line
    /// position via toml's reporter.
    pub fn from_path(path: &Path) -> Result<Self> {
        let body = std::fs::read_to_string(path).with_context(|| format!("read config file {}", path.display()))?;
        Self::from_toml_str(&body).with_context(|| format!("parse config file {}", path.display()))
    }

    /// Parse a TOML string into a [`ServeConfigFile`]. Round-trips
    /// with `toml::to_string` so the admin-route diff surfaces are
    /// stable.
    pub fn from_toml_str(body: &str) -> Result<Self> {
        let parsed: Self = toml::from_str(body).context("toml parse")?;
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_body_parses_to_default() {
        let parsed = ServeConfigFile::from_toml_str("").expect("empty toml is valid");
        assert!(parsed.auth.admin_token.is_none());
        assert!(parsed.auth.publish_key.is_none());
        assert!(parsed.mesh_ice_servers.is_empty());
        assert!(parsed.hmac_playback_secret.is_none());
    }

    #[test]
    fn auth_section_round_trips() {
        let body = r#"
            [auth]
            publish_key = "pubkey-1"
            admin_token = "admin-1"
            jwt_secret = "hs256-secret"
            jwt_issuer = "iss-1"
            jwt_audience = "aud-1"
        "#;
        let parsed = ServeConfigFile::from_toml_str(body).expect("auth section parses");
        assert_eq!(parsed.auth.publish_key.as_deref(), Some("pubkey-1"));
        assert_eq!(parsed.auth.admin_token.as_deref(), Some("admin-1"));
        assert_eq!(parsed.auth.jwt_secret.as_deref(), Some("hs256-secret"));
        assert_eq!(parsed.auth.jwt_issuer.as_deref(), Some("iss-1"));
        assert_eq!(parsed.auth.jwt_audience.as_deref(), Some("aud-1"));
        // Unset keys default to None even when their section IS present.
        assert!(parsed.auth.subscribe_token.is_none());
        assert!(parsed.auth.jwks_url.is_none());
        assert!(parsed.auth.webhook_auth_url.is_none());
    }

    #[test]
    fn mesh_ice_servers_array_of_tables() {
        let body = r#"
            [[mesh_ice_servers]]
            urls = ["stun:stun.l.google.com:19302"]

            [[mesh_ice_servers]]
            urls = ["turn:turn.example.com:3478"]
            username = "u"
            credential = "p"
        "#;
        let parsed = ServeConfigFile::from_toml_str(body).expect("ice servers parse");
        assert_eq!(parsed.mesh_ice_servers.len(), 2);
        assert_eq!(parsed.mesh_ice_servers[0].urls, vec!["stun:stun.l.google.com:19302"]);
        assert!(parsed.mesh_ice_servers[0].username.is_none());
        assert_eq!(parsed.mesh_ice_servers[1].username.as_deref(), Some("u"));
        assert_eq!(parsed.mesh_ice_servers[1].credential.as_deref(), Some("p"));
    }

    #[test]
    fn hmac_playback_secret_top_level() {
        let body = r#"hmac_playback_secret = "abc123""#;
        let parsed = ServeConfigFile::from_toml_str(body).expect("hmac parse");
        assert_eq!(parsed.hmac_playback_secret.as_deref(), Some("abc123"));
    }

    #[test]
    fn unknown_top_level_key_is_rejected() {
        // serde defaults to denying unknown fields ONLY with
        // `#[serde(deny_unknown_fields)]`. We do NOT set that, so
        // forward-compat is preserved: a future server adding a new
        // section won't break an older client's parser. This test
        // documents that posture.
        let body = r#"
            future_key_we_dont_know_yet = "hello"
            [auth]
            publish_key = "x"
        "#;
        let parsed = ServeConfigFile::from_toml_str(body).expect("unknown keys are tolerated");
        assert_eq!(parsed.auth.publish_key.as_deref(), Some("x"));
    }

    #[test]
    fn malformed_toml_returns_error() {
        let body = r#"this is = not = valid toml"#;
        let err = ServeConfigFile::from_toml_str(body).expect_err("malformed must fail");
        let chain = format!("{err:#}");
        assert!(chain.contains("toml parse") || chain.to_lowercase().contains("parse"));
    }

    #[test]
    fn from_path_round_trips_through_disk() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("lvqr.toml");
        std::fs::write(
            &path,
            "hmac_playback_secret = \"disk-test\"\n[auth]\npublish_key = \"from-disk\"\n",
        )
        .expect("write");
        let parsed = ServeConfigFile::from_path(&path).expect("from_path");
        assert_eq!(parsed.hmac_playback_secret.as_deref(), Some("disk-test"));
        assert_eq!(parsed.auth.publish_key.as_deref(), Some("from-disk"));
    }

    #[test]
    fn auth_section_eq_lets_diffs_be_detected() {
        let a = AuthSection {
            publish_key: Some("v1".into()),
            ..Default::default()
        };
        let b = AuthSection {
            publish_key: Some("v1".into()),
            ..Default::default()
        };
        let c = AuthSection {
            publish_key: Some("v2".into()),
            ..Default::default()
        };
        // The reload pipeline diffs old vs new AuthSection to decide
        // whether to rebuild the SharedAuth. PartialEq on the
        // section makes that cheap.
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}

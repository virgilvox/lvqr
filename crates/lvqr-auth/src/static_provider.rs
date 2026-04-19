use crate::provider::{AuthContext, AuthDecision, AuthProvider};

/// Configuration for the static-token authentication provider.
///
/// Each `Option<String>` enables a specific gate. When a field is `None`, that
/// gate is open (no token required). This makes auth gradual: configure
/// `admin_token` to lock down the admin API while leaving publish and subscribe
/// open, for example.
#[derive(Debug, Clone, Default)]
pub struct StaticAuthConfig {
    /// Required bearer token for admin API access. `None` = open.
    pub admin_token: Option<String>,
    /// Required RTMP/WS publish key. `None` = open.
    /// Compared against the RTMP `stream_key` or the WS `?token=` query param.
    pub publish_key: Option<String>,
    /// Required viewer token for WS relay and MoQ subscribe. `None` = open.
    pub subscribe_token: Option<String>,
}

impl StaticAuthConfig {
    /// Construct a config from environment variables.
    /// Empty strings are treated as unset.
    pub fn from_env() -> Self {
        let pick = |name: &str| std::env::var(name).ok().filter(|s| !s.is_empty());
        Self {
            admin_token: pick("LVQR_ADMIN_TOKEN"),
            publish_key: pick("LVQR_PUBLISH_KEY"),
            subscribe_token: pick("LVQR_SUBSCRIBE_TOKEN"),
        }
    }

    /// Returns true if any token is configured. When false, this provider is
    /// equivalent to `NoopAuthProvider`.
    pub fn has_any(&self) -> bool {
        self.admin_token.is_some() || self.publish_key.is_some() || self.subscribe_token.is_some()
    }
}

/// Auth provider backed by static tokens passed via configuration or env vars.
#[derive(Debug, Clone)]
pub struct StaticAuthProvider {
    config: StaticAuthConfig,
}

impl StaticAuthProvider {
    pub fn new(config: StaticAuthConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &StaticAuthConfig {
        &self.config
    }
}

/// Constant-time equality for short tokens. We expect tokens to be short and
/// of equal length when valid; this guards against trivial timing oracles.
fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

impl AuthProvider for StaticAuthProvider {
    fn check(&self, ctx: &AuthContext) -> AuthDecision {
        match ctx {
            AuthContext::Publish {
                app: _,
                key,
                broadcast: _,
            } => match &self.config.publish_key {
                None => AuthDecision::Allow,
                Some(expected) if ct_eq(expected, key) => AuthDecision::Allow,
                Some(_) => AuthDecision::deny("invalid publish key"),
            },
            AuthContext::Subscribe { token, broadcast: _ } => match (&self.config.subscribe_token, token) {
                (None, _) => AuthDecision::Allow,
                (Some(expected), Some(t)) if ct_eq(expected, t) => AuthDecision::Allow,
                (Some(_), _) => AuthDecision::deny("invalid or missing subscribe token"),
            },
            AuthContext::Admin { token } => match &self.config.admin_token {
                None => AuthDecision::Allow,
                Some(expected) if ct_eq(expected, token) => AuthDecision::Allow,
                Some(_) => AuthDecision::deny("invalid admin token"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_publish(key: &str) -> AuthContext {
        AuthContext::Publish {
            app: "live".into(),
            key: key.into(),
            broadcast: None,
        }
    }
    fn ctx_subscribe(token: Option<&str>) -> AuthContext {
        AuthContext::Subscribe {
            token: token.map(String::from),
            broadcast: "live/test".into(),
        }
    }
    fn ctx_admin(token: &str) -> AuthContext {
        AuthContext::Admin { token: token.into() }
    }

    #[test]
    fn unset_tokens_allow_everything() {
        let p = StaticAuthProvider::new(StaticAuthConfig::default());
        assert!(p.check(&ctx_publish("anything")).is_allow());
        assert!(p.check(&ctx_subscribe(None)).is_allow());
        assert!(p.check(&ctx_admin("")).is_allow());
    }

    #[test]
    fn admin_token_enforced() {
        let p = StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            ..Default::default()
        });
        assert!(p.check(&ctx_admin("secret")).is_allow());
        assert!(!p.check(&ctx_admin("wrong")).is_allow());
        assert!(!p.check(&ctx_admin("")).is_allow());
    }

    #[test]
    fn publish_key_enforced() {
        let p = StaticAuthProvider::new(StaticAuthConfig {
            publish_key: Some("streamkey".into()),
            ..Default::default()
        });
        assert!(p.check(&ctx_publish("streamkey")).is_allow());
        assert!(!p.check(&ctx_publish("wrong")).is_allow());
        // Subscribe and admin remain open since their tokens aren't set.
        assert!(p.check(&ctx_subscribe(None)).is_allow());
        assert!(p.check(&ctx_admin("")).is_allow());
    }

    #[test]
    fn subscribe_token_required_when_set() {
        let p = StaticAuthProvider::new(StaticAuthConfig {
            subscribe_token: Some("viewer".into()),
            ..Default::default()
        });
        assert!(p.check(&ctx_subscribe(Some("viewer"))).is_allow());
        assert!(!p.check(&ctx_subscribe(Some("wrong"))).is_allow());
        assert!(!p.check(&ctx_subscribe(None)).is_allow());
    }

    #[test]
    fn has_any_reflects_state() {
        assert!(!StaticAuthConfig::default().has_any());
        assert!(
            StaticAuthConfig {
                admin_token: Some("x".into()),
                ..Default::default()
            }
            .has_any()
        );
    }
}

//! JWT-based authentication provider (behind the `jwt` feature flag).
//!
//! Tokens are HS256-signed with a shared secret. Claims include a `scope` field
//! indicating what the token grants (`subscribe`, `publish`, or `admin`). The
//! provider also validates `iss` and `aud` if configured.

use crate::error::AuthError;
use crate::provider::{AuthContext, AuthDecision, AuthProvider, AuthScope};
use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};

/// Configuration for the JWT auth provider.
#[derive(Debug, Clone)]
pub struct JwtAuthConfig {
    /// HMAC secret used for token verification.
    pub secret: String,
    /// Expected issuer claim. If `None`, issuer is not checked.
    pub issuer: Option<String>,
    /// Expected audience claim. If `None`, audience is not checked.
    pub audience: Option<String>,
}

/// Claims expected in an LVQR JWT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// Subject (user identifier).
    pub sub: String,
    /// Expiration timestamp (seconds since epoch).
    pub exp: usize,
    /// Granted scope.
    pub scope: AuthScope,
    /// Optional issuer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iss: Option<String>,
    /// Optional audience.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
    /// Optional broadcast name limiting publish scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broadcast: Option<String>,
}

/// JWT authentication provider.
#[derive(Clone)]
pub struct JwtAuthProvider {
    config: JwtAuthConfig,
    key: DecodingKey,
    validation: Validation,
}

impl JwtAuthProvider {
    pub fn new(config: JwtAuthConfig) -> Result<Self, AuthError> {
        if config.secret.is_empty() {
            return Err(AuthError::InvalidConfig("JWT secret is empty".into()));
        }
        let key = DecodingKey::from_secret(config.secret.as_bytes());
        let mut validation = Validation::default();
        if let Some(iss) = &config.issuer {
            validation.set_issuer(&[iss.as_str()]);
        }
        if let Some(aud) = &config.audience {
            validation.set_audience(&[aud.as_str()]);
        }
        Ok(Self {
            config,
            key,
            validation,
        })
    }

    pub fn config(&self) -> &JwtAuthConfig {
        &self.config
    }

    fn decode(&self, token: &str) -> Result<JwtClaims, AuthError> {
        decode::<JwtClaims>(token, &self.key, &self.validation)
            .map(|d| d.claims)
            .map_err(|e| AuthError::InvalidToken(e.to_string()))
    }
}

impl AuthProvider for JwtAuthProvider {
    fn check(&self, ctx: &AuthContext) -> AuthDecision {
        let (token, required_scope, broadcast_filter): (&str, AuthScope, Option<&str>) = match ctx {
            AuthContext::Publish { app, key, broadcast } => {
                // RTMP carries the JWT as the stream key and sets
                // `broadcast = None`; WHIP/SRT/RTSP/WS-ingest all know
                // the target broadcast and pass `Some(name)`, which
                // enables per-broadcast claim binding below.
                let _ = app;
                (key.as_str(), AuthScope::Publish, broadcast.as_deref())
            }
            AuthContext::Subscribe { token, broadcast } => {
                let Some(t) = token.as_deref() else {
                    return AuthDecision::deny("subscribe token missing");
                };
                (t, AuthScope::Subscribe, Some(broadcast.as_str()))
            }
            AuthContext::Admin { token } => (token.as_str(), AuthScope::Admin, None),
        };

        match self.decode(token) {
            Ok(claims) => {
                if !claims.scope.includes(required_scope) {
                    return AuthDecision::deny(format!(
                        "token scope {:?} insufficient for {:?}",
                        claims.scope, required_scope
                    ));
                }
                if let (Some(filter), Some(broadcast)) = (claims.broadcast.as_deref(), broadcast_filter) {
                    if filter != broadcast {
                        return AuthDecision::deny("token bound to different broadcast");
                    }
                }
                AuthDecision::Allow
            }
            Err(e) => AuthDecision::deny(format!("jwt decode failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_token(secret: &str, scope: AuthScope, broadcast: Option<&str>) -> String {
        let exp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as usize + 3600;
        let claims = JwtClaims {
            sub: "alice".into(),
            exp,
            scope,
            iss: None,
            aud: None,
            broadcast: broadcast.map(String::from),
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    #[test]
    fn admin_token_allows_admin() {
        let p = JwtAuthProvider::new(JwtAuthConfig {
            secret: "secret".into(),
            issuer: None,
            audience: None,
        })
        .unwrap();
        let token = make_token("secret", AuthScope::Admin, None);
        assert!(p.check(&AuthContext::Admin { token }).is_allow());
    }

    #[test]
    fn subscribe_scope_cannot_publish() {
        let p = JwtAuthProvider::new(JwtAuthConfig {
            secret: "secret".into(),
            issuer: None,
            audience: None,
        })
        .unwrap();
        let token = make_token("secret", AuthScope::Subscribe, None);
        let decision = p.check(&AuthContext::Publish {
            app: "live".into(),
            key: token,
            broadcast: None,
        });
        assert!(!decision.is_allow());
    }

    #[test]
    fn publish_broadcast_filter_enforced_when_present() {
        let p = JwtAuthProvider::new(JwtAuthConfig {
            secret: "secret".into(),
            issuer: None,
            audience: None,
        })
        .unwrap();
        let token = make_token("secret", AuthScope::Publish, Some("live/cam1"));
        assert!(
            p.check(&AuthContext::Publish {
                app: "whip".into(),
                key: token.clone(),
                broadcast: Some("live/cam1".into()),
            })
            .is_allow()
        );
        assert!(
            !p.check(&AuthContext::Publish {
                app: "whip".into(),
                key: token.clone(),
                broadcast: Some("live/other".into()),
            })
            .is_allow()
        );
        // Without an extractor-supplied broadcast, binding is skipped; the
        // scope still admits the token (the RTMP convention).
        assert!(
            p.check(&AuthContext::Publish {
                app: "live".into(),
                key: token,
                broadcast: None,
            })
            .is_allow()
        );
    }

    #[test]
    fn broadcast_filter_enforced() {
        let p = JwtAuthProvider::new(JwtAuthConfig {
            secret: "secret".into(),
            issuer: None,
            audience: None,
        })
        .unwrap();
        let token = make_token("secret", AuthScope::Subscribe, Some("live/cool"));
        assert!(
            p.check(&AuthContext::Subscribe {
                token: Some(token.clone()),
                broadcast: "live/cool".into()
            })
            .is_allow()
        );
        assert!(
            !p.check(&AuthContext::Subscribe {
                token: Some(token),
                broadcast: "live/other".into()
            })
            .is_allow()
        );
    }

    #[test]
    fn empty_secret_rejected() {
        assert!(
            JwtAuthProvider::new(JwtAuthConfig {
                secret: "".into(),
                issuer: None,
                audience: None,
            })
            .is_err()
        );
    }
}

use std::sync::Arc;

/// The kind of operation being authenticated.
///
/// `AuthContext` is intentionally lightweight and uses owned strings so the same
/// value can be passed across thread boundaries (RTMP callbacks, axum handlers,
/// MoQ session accept loops).
#[derive(Debug, Clone)]
pub enum AuthContext {
    /// RTMP publish: identified by the application name and stream key.
    Publish { app: String, key: String },
    /// Subscribe (viewer): optional bearer token plus the broadcast name.
    Subscribe { token: Option<String>, broadcast: String },
    /// Admin API access: bearer token from the `Authorization` header.
    Admin { token: String },
}

/// The high-level scope an authenticated principal has been granted.
///
/// Used by JWT claims to express what a token allows. The scope hierarchy is:
/// `Admin` implies `Publish` implies `Subscribe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthScope {
    Subscribe,
    Publish,
    Admin,
}

impl AuthScope {
    pub fn includes(self, other: AuthScope) -> bool {
        use AuthScope::*;
        matches!(
            (self, other),
            (Admin, _) | (Publish, Publish | Subscribe) | (Subscribe, Subscribe)
        )
    }
}

/// The result of an authentication check.
#[derive(Debug, Clone)]
pub enum AuthDecision {
    /// Access is permitted.
    Allow,
    /// Access is denied. The reason is logged and may be surfaced to the client.
    Deny { reason: String },
}

impl AuthDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, AuthDecision::Allow)
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        AuthDecision::Deny { reason: reason.into() }
    }
}

/// Trait for pluggable authentication backends.
///
/// `check` is synchronous because all built-in providers (static tokens, JWT)
/// only do CPU work. If a custom backend needs to make a network call, it should
/// cache results and update them out of band.
pub trait AuthProvider: Send + Sync + 'static {
    /// Inspect the context and return whether access is permitted.
    fn check(&self, ctx: &AuthContext) -> AuthDecision;
}

/// Convenience alias used throughout the workspace.
pub type SharedAuth = Arc<dyn AuthProvider>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_hierarchy() {
        assert!(AuthScope::Admin.includes(AuthScope::Publish));
        assert!(AuthScope::Admin.includes(AuthScope::Subscribe));
        assert!(AuthScope::Publish.includes(AuthScope::Subscribe));
        assert!(!AuthScope::Subscribe.includes(AuthScope::Publish));
        assert!(!AuthScope::Publish.includes(AuthScope::Admin));
    }

    #[test]
    fn decision_helpers() {
        assert!(AuthDecision::Allow.is_allow());
        assert!(!AuthDecision::deny("nope").is_allow());
    }
}

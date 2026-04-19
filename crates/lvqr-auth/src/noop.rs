use crate::provider::{AuthContext, AuthDecision, AuthProvider};

/// Auth provider that allows everything. This is the default when no
/// authentication is configured, preserving backward compatibility.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopAuthProvider;

impl AuthProvider for NoopAuthProvider {
    fn check(&self, _ctx: &AuthContext) -> AuthDecision {
        AuthDecision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_everything() {
        let p = NoopAuthProvider;
        assert!(
            p.check(&AuthContext::Publish {
                app: "live".into(),
                key: "x".into(),
                broadcast: None,
            })
            .is_allow()
        );
        assert!(
            p.check(&AuthContext::Subscribe {
                token: None,
                broadcast: "live/test".into()
            })
            .is_allow()
        );
        assert!(p.check(&AuthContext::Admin { token: "".into() }).is_allow());
    }
}

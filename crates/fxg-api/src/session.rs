//! Web UI session cookie (`fxg_session_<role>`).
//!
//! Design: `docs/03-protocol-and-api.md` §3.0.
//!
//! The cookie carries the role's Web access token, which has no expiry of its
//! own, so the cookie is persisted (`Max-Age`) instead of being a browser
//! session cookie. The name is role-scoped because cookies are host-scoped and
//! port-agnostic: a hub (`:8080`) and a local node (`:7860`) opened on the same
//! host would otherwise overwrite each other's `fxg_session` and force a
//! re-login on every switch.

use fxg_protocol::client_api::ConnectionRole;

/// Session cookie lifetime (30 days).
pub const SESSION_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;

/// Session cookie name for the given connection role.
pub const fn cookie_name(role: ConnectionRole) -> &'static str {
    match role {
        ConnectionRole::Hub => "fxg_session_hub",
        ConnectionRole::LocalNode => "fxg_session_node",
    }
}

/// `Set-Cookie` value that persists the Web access token for `role`.
pub(crate) fn set_cookie(role: ConnectionRole, token: &str) -> String {
    format!(
        "{}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={SESSION_MAX_AGE_SECS}",
        cookie_name(role)
    )
}

/// `Set-Cookie` value that expires the role's session cookie (`Max-Age=0`).
pub(crate) fn clear_cookie(role: ConnectionRole) -> String {
    format!(
        "{}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        cookie_name(role)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_names_are_role_scoped() {
        assert_eq!(cookie_name(ConnectionRole::Hub), "fxg_session_hub");
        assert_eq!(cookie_name(ConnectionRole::LocalNode), "fxg_session_node");
        assert_ne!(
            cookie_name(ConnectionRole::Hub),
            cookie_name(ConnectionRole::LocalNode)
        );
    }

    #[test]
    fn set_cookie_persists_the_token_in_a_restricted_cookie() {
        let cookie = set_cookie(ConnectionRole::Hub, "abc");
        assert!(cookie.starts_with("fxg_session_hub=abc;"), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Strict"), "{cookie}");
        assert!(cookie.contains("Path=/"), "{cookie}");
        assert!(
            cookie.contains(&format!("Max-Age={SESSION_MAX_AGE_SECS}")),
            "{cookie}"
        );
    }

    #[test]
    fn clear_cookie_expires_the_role_cookie() {
        let cookie = clear_cookie(ConnectionRole::LocalNode);
        assert!(cookie.starts_with("fxg_session_node=;"), "{cookie}");
        assert!(cookie.contains("Max-Age=0"), "{cookie}");
    }
}

//! Server-side resolution for the Git Credential Proxy.
//!
//! Design: `docs/01-architecture-and-sync.md` §6.4, `docs/05-cli-and-pwa-ui.md` §2.2.
//!
//! Used both for `GitCredentialRequest` from a temporary VM (over the stdio pipe) and for
//! short-lived token injection at provisioner startup (`FXG_GIT_TOKEN`). Tokens live only in
//! process memory and are never written to `server.db` or disk.

use std::time::Duration;

use fxg_protocol::config::{DEFAULT_GIT_USERNAME, GitCredentialConfig};

/// Resolved Git credentials (memory only).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CredentialResponse {
    /// Basic auth username
    pub username: String,
    /// Token (`None` when resolution failed)
    pub token: Option<String>,
    /// Message describing the failure
    pub error: Option<String>,
}

/// Execution timeout for `gh auth token`.
const GH_TIMEOUT: Duration = Duration::from_secs(15);

/// Resolves credentials from configuration (`[server.git_credentials.<host>]`).
pub async fn resolve_credential(
    config: &GitCredentialConfig,
) -> Result<CredentialResponse, String> {
    match config {
        GitCredentialConfig::GhCli { username } => {
            let token = tokio::time::timeout(GH_TIMEOUT, async {
                let output = tokio::process::Command::new("gh")
                    .args(["auth", "token"])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .await
                    .map_err(|err| format!("failed to run `gh auth token`: {err}"))?;
                if !output.status.success() {
                    return Err(format!(
                        "`gh auth token` failed: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
            })
            .await
            .map_err(|err| format!("`gh auth token` timed out: {err}"))??;
            if token.is_empty() {
                return Err("`gh auth token` returned an empty token".to_owned());
            }
            Ok(CredentialResponse {
                username: username
                    .clone()
                    .unwrap_or_else(|| DEFAULT_GIT_USERNAME.to_owned()),
                token: Some(token),
                error: None,
            })
        }
        GitCredentialConfig::Env {
            username,
            token_env,
        } => match std::env::var(token_env) {
            Ok(token) if !token.trim().is_empty() => Ok(CredentialResponse {
                username: username
                    .clone()
                    .unwrap_or_else(|| DEFAULT_GIT_USERNAME.to_owned()),
                token: Some(token),
                error: None,
            }),
            _ => Err(format!(
                "environment variable {token_env} is not set (git credentials)"
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn env_provider_reads_token_from_environment() {
        // Use a unique variable name (avoids collisions in parallel test runs)
        let key = "FXG_TEST_GIT_TOKEN_ENV";
        unsafe { std::env::set_var(key, "secret-token") };
        let config = GitCredentialConfig::Env {
            username: None,
            token_env: key.to_owned(),
        };
        let response = resolve_credential(&config).await.expect("resolve");
        assert_eq!(response.token.as_deref(), Some("secret-token"));
        assert_eq!(response.username, DEFAULT_GIT_USERNAME);
        unsafe { std::env::remove_var(key) };
    }

    #[tokio::test]
    async fn env_provider_reports_missing_variable() {
        let config = GitCredentialConfig::Env {
            username: Some("git".to_owned()),
            token_env: "FXG_TEST_MISSING_TOKEN_ENV".to_owned(),
        };
        let err = resolve_credential(&config).await.expect_err("error");
        assert!(err.contains("FXG_TEST_MISSING_TOKEN_ENV"));
    }
}

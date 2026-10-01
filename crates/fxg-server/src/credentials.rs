//! Git Credential Proxy のサーバー側解決。
//!
//! 設計: `docs/01-architecture-and-sync.md` §6.4、`docs/05-cli-and-pwa-ui.md` §2.2。
//!
//! 一時VMからの `GitCredentialRequest` (stdio パイプ経由) と、プロビジョナー起動時の
//! 短命トークン注入 (`FXG_GIT_TOKEN`) の双方で使用する。トークンはプロセスメモリ上の
//! みで扱い、`server.db` やディスクへは保存しない。

use std::time::Duration;

use fxg_protocol::config::{DEFAULT_GIT_USERNAME, GitCredentialConfig};

/// 解決した Git 資格情報 (オンメモリのみ)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CredentialResponse {
    /// Basic 認証ユーザー名
    pub username: String,
    /// トークン (解決失敗時は `None`)
    pub token: Option<String>,
    /// 失敗時のメッセージ
    pub error: Option<String>,
}

/// `gh auth token` の実行タイムアウト。
const GH_TIMEOUT: Duration = Duration::from_secs(15);

/// 設定 (`[server.git_credentials.<host>]`) から資格情報を解決する。
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
            .map_err(|_| "`gh auth token` timed out".to_owned())??;
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
        // 安全のためユニークな変数名を使う (テスト並列実行での衝突回避)
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

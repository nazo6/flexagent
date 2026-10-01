//! Git Credential Proxy (一時VM向けオンメモリ認証中継)。
//!
//! 設計: `docs/01-architecture-and-sync.md` §6.4。
//!
//! 一時VM・サンドボックスノードでは、個人の SSH 秘密鍵や恒久的な GitHub PAT を
//! VM内ディスクに一切保存しない。VM内で `git clone` / `git fetch` / `git push`
//! が走ると Git は `GIT_ASKPASS` ヘルパー ([`crate::daemon`] が生成する
//! `~/.flexagent/git-askpass.sh`) を呼び出し、ヘルパーはローカルIPC
//! (`fxg git-askpass`) 経由でデーモンへ問い合わせる。デーモンは
//! [`CredentialBroker`] で既存の Node ⇔ Server 接続 (WS / stdio パイプ) 上に
//! `GitCredentialRequest` を送り、`GitCredentialResponse` で受け取った
//! トークンを**オンメモリのみ**でヘルパーへ返す。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use fxg_protocol::config::DEFAULT_GIT_USERNAME;
use fxg_protocol::node_server::NodeToServerMsg;
use fxg_protocol::util::uuid_v7;
use tokio::sync::{mpsc, oneshot};

use crate::error::NodeError;

/// `GitCredentialRequest` の応答待ちタイムアウト。
pub const CREDENTIAL_TIMEOUT: Duration = Duration::from_secs(30);

/// 中央サーバーから返る Git 資格情報 (オンメモリのみ)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CredentialResponse {
    /// Basic 認証ユーザー名
    pub username: String,
    /// トークン (解決失敗時は `None`)
    pub token: Option<String>,
    /// 失敗時のメッセージ
    pub error: Option<String>,
}

/// Git Credential Proxy が利用できない場合のエラー (そのまま CLI へ返す)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    /// 中央サーバーへ接続していない
    NotConnected,
    /// 応答タイムアウト
    Timeout,
    /// サーバーが解決に失敗した
    Server(String),
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => f.write_str(
                "git credential proxy is unavailable (not connected to a central server)",
            ),
            Self::Timeout => f.write_str("git credential request timed out"),
            Self::Server(message) => write!(f, "git credential request failed: {message}"),
        }
    }
}

struct BrokerInner {
    /// Node ⇔ Server 接続の送信キュー (未接続時は `None`)。
    outbound: RwLock<Option<mpsc::Sender<NodeToServerMsg>>>,
    /// 応答待ちの要求 (`request_id` → oneshot)
    pending: Mutex<HashMap<String, oneshot::Sender<CredentialResponse>>>,
}

/// Git 資格情報の中継ブローカー (デーモンが 1 つ保持する)。
#[derive(Clone)]
pub struct CredentialBroker {
    inner: Arc<BrokerInner>,
}

impl std::fmt::Debug for CredentialBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialBroker").finish_non_exhaustive()
    }
}

impl Default for CredentialBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialBroker {
    /// 空のブローカーを作成する。
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BrokerInner {
                outbound: RwLock::new(None),
                pending: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// 中央サーバー接続時に送信キューを登録する (Sync Worker / stdio トランスポート専用)。
    pub fn attach(&self, outbound: mpsc::Sender<NodeToServerMsg>) {
        *self.inner.outbound.write().expect("outbound lock poisoned") = Some(outbound);
    }

    /// 接続断時に送信キューを解除する (待機中の要求はタイムアウトで失敗する)。
    pub fn detach(&self) {
        *self.inner.outbound.write().expect("outbound lock poisoned") = None;
    }

    /// `GitCredentialResponse` を待機中の要求へ配送する。
    pub fn complete(&self, request_id: &str, response: CredentialResponse) {
        if let Some(tx) = self
            .inner
            .pending
            .lock()
            .expect("pending lock poisoned")
            .remove(request_id)
        {
            let _ = tx.send(response);
        }
    }

    /// Git の askpass プロンプトから資格情報を解決する。
    ///
    /// プロンプトに含まれる URL からホストを抽出し、中央サーバーへ
    /// `GitCredentialRequest` を送って応答を待つ。
    pub async fn resolve(&self, prompt: &str) -> Result<CredentialResponse, CredentialError> {
        let host = parse_host(prompt).unwrap_or_default();
        let repo_url = parse_url(prompt).unwrap_or(host);
        let outbound = self
            .inner
            .outbound
            .read()
            .expect("outbound lock poisoned")
            .clone()
            .ok_or(CredentialError::NotConnected)?;

        let request_id = uuid_v7();
        let (tx, rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .expect("pending lock poisoned")
            .insert(request_id.clone(), tx);

        let message = NodeToServerMsg::GitCredentialRequest {
            request_id: request_id.clone(),
            repo_url,
            // GIT_ASKPASS は clone / fetch / push を区別できないため
            // 汎用値 `fetch` として送る (サーバー側の解決方法は同一)
            operation: "fetch".to_owned(),
        };
        if outbound.send(message).await.is_err() {
            self.inner
                .pending
                .lock()
                .expect("pending lock poisoned")
                .remove(&request_id);
            return Err(CredentialError::NotConnected);
        }

        match tokio::time::timeout(CREDENTIAL_TIMEOUT, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(CredentialError::NotConnected),
            Err(_) => {
                self.inner
                    .pending
                    .lock()
                    .expect("pending lock poisoned")
                    .remove(&request_id);
                Err(CredentialError::Timeout)
            }
        }
    }
}

/// Git の askpass プロンプトから URL を抽出する。
///
/// 例: `Password for 'https://user@github.com':` → `https://user@github.com`。
pub fn parse_url(prompt: &str) -> Option<String> {
    let start = prompt.find("'")?;
    let rest = &prompt[start + 1..];
    let end = rest.find("'")?;
    let url = &rest[..end];
    if url.contains("://") {
        Some(url.to_owned())
    } else {
        None
    }
}

/// Git の askpass プロンプトからホスト名のみを抽出する。
pub fn parse_host(prompt: &str) -> Option<String> {
    let url = parse_url(prompt)?;
    let rest = url.split("://").nth(1)?;
    let authority = rest.split('/').next()?;
    let host = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = host.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_owned())
    }
}

/// プロンプトがユーザー名を要求しているか (Password 要求と区別する)。
pub fn prompt_wants_username(prompt: &str) -> bool {
    prompt.to_ascii_lowercase().contains("username")
}

/// `GIT_ASKPASS` ヘルパースクリプトのパス (`~/.flexagent/git-askpass.sh` / `.cmd`)。
pub fn askpass_script_path(fxg_home: &Path) -> PathBuf {
    if cfg!(windows) {
        fxg_home.join("git-askpass.cmd")
    } else {
        fxg_home.join("git-askpass.sh")
    }
}

/// `GIT_ASKPASS` ヘルパースクリプトを生成する (毎回上書き)。
///
/// スクリプトは `fxg git-askpass <prompt>` を呼び出すだけの薄いラッパーであり、
/// 実処理 (中央サーバーへの `GitCredentialRequest` 中継) はデーモンが行う。
/// デーモン未起動時は非ゼロ終了し、Git は認証失敗として停止する
/// (トークンをディスクに残さない設計: docs/01 §6.4)。
pub fn write_askpass_script(fxg_home: &Path) -> Result<PathBuf, NodeError> {
    let binary = std::env::current_exe().map_err(NodeError::Plain)?;
    let path = askpass_script_path(fxg_home);
    let content = if cfg!(windows) {
        format!("@echo off\r\n\"{}\" git-askpass %*\r\n", binary.display())
    } else {
        format!(
            "#!/bin/sh\n# fxg GIT_ASKPASS proxy (auto-generated). Do not edit.\nexec \"{}\" git-askpass \"$1\"\n",
            binary.display()
        )
    };
    std::fs::write(&path, content).map_err(|source| NodeError::Io {
        path: path.clone(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(&path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o755);
            let _ = std::fs::set_permissions(&path, permissions);
        }
    }
    Ok(path)
}

/// エージェントプロセスへ注入する Git 環境変数を返す
/// (`GIT_ASKPASS` プロキシ + 対話プロンプト無効化)。
pub fn git_env(fxg_home: &Path) -> Vec<(String, String)> {
    vec![
        (
            "GIT_ASKPASS".to_owned(),
            askpass_script_path(fxg_home).to_string_lossy().into_owned(),
        ),
        // 端末が無い環境で Git がユーザー入力待ちでハングしないようにする
        ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
    ]
}

/// 資格情報のユーザー名の既定値。
pub fn default_username() -> &'static str {
    DEFAULT_GIT_USERNAME
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_host_extracts_github_from_askpass_prompt() {
        let prompt = "Password for 'https://user@github.com': ";
        assert_eq!(parse_host(prompt).as_deref(), Some("github.com"));
        assert_eq!(
            parse_url(prompt).as_deref(),
            Some("https://user@github.com")
        );
        assert_eq!(
            parse_host("Username for 'https://gitlab.example.com:8443/x/y':").as_deref(),
            Some("gitlab.example.com")
        );
        assert_eq!(parse_host("Password:"), None);
        assert!(prompt_wants_username("Username for 'https://github.com':"));
        assert!(!prompt_wants_username(
            "Password for 'https://x@github.com':"
        ));
    }

    #[tokio::test]
    async fn resolve_without_connection_fails() {
        let broker = CredentialBroker::new();
        let err = broker.resolve("Password for 'https://github.com':").await;
        assert_eq!(err, Err(CredentialError::NotConnected));
    }

    #[tokio::test]
    async fn resolve_forwards_request_and_completes() {
        let broker = CredentialBroker::new();
        let (tx, mut rx) = mpsc::channel(4);
        broker.attach(tx);

        let pending = {
            let broker = broker.clone();
            tokio::spawn(async move { broker.resolve("Password for 'https://github.com':").await })
        };
        let message = rx.recv().await.expect("request");
        let NodeToServerMsg::GitCredentialRequest {
            request_id,
            repo_url,
            ..
        } = message
        else {
            panic!("unexpected message: {message:?}");
        };
        assert_eq!(repo_url, "https://github.com");
        broker.complete(
            &request_id,
            CredentialResponse {
                username: "x-access-token".to_owned(),
                token: Some("secret".to_owned()),
                error: None,
            },
        );
        let response = pending.await.expect("join").expect("resolve");
        assert_eq!(response.token.as_deref(), Some("secret"));
    }

    #[test]
    fn askpass_script_is_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_askpass_script(dir.path()).expect("write");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).expect("read");
        assert!(content.contains("git-askpass"));
    }
}

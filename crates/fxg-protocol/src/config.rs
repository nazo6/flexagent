//! 設定ファイルスキーマ (`~/.flexagent/config.toml` / `.fxg.toml`)。
//!
//! 設計: `docs/05-cli-and-pwa-ui.md` §2。
//!
//! - グローバル設定 ([`GlobalConfig`]): ノード設定・サーバー設定・エージェント設定・
//!   プロビジョナー定義を単一の `config.toml` に統合し、環境変数 (`FXG_*`)
//!   による上書きをサポートする。
//! - プロジェクト設定 ([`ProjectConfig`]): リポジトリ内の `.fxg.toml` に配置する。
//!
//! これらはサーバー/CLI 内部型であり `ts-rs` の export 対象外とする
//! (クライアントに公開する型のみ `#[ts(export)]` を付与する規約)。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// データディレクトリ (`~/.flexagent`) を変更する環境変数。
pub const ENV_FXG_HOME: &str = "FXG_HOME";
/// グローバル設定ファイル名。
pub const CONFIG_FILE_NAME: &str = "config.toml";
/// プロジェクト設定ファイル名。
pub const PROJECT_CONFIG_FILE_NAME: &str = ".fxg.toml";
/// クライアント認証トークンのファイル名。
pub const AUTH_TOKEN_FILE_NAME: &str = "auth_token";
/// ノード個別ペアリングトークンのファイル名。
pub const NODE_TOKEN_FILE_NAME: &str = "node_token";
/// ノードローカルDBのファイル名。
pub const NODE_DB_FILE_NAME: &str = "node.db";
/// 中央サーバーDBのファイル名。
pub const SERVER_DB_FILE_NAME: &str = "server.db";
/// 一時VM破棄時に退避された git bundle の格納ディレクトリ名 (`~/.flexagent/bundles`)。
pub const BUNDLES_DIR_NAME: &str = "bundles";
/// 一時VMのブートストラップが生成する環境変数ファイル名 (`~/.flexagent/bootstrap.env`)。
pub const BOOTSTRAP_ENV_FILE_NAME: &str = "bootstrap.env";
/// `GitCredentialRequest` のデフォルト Basic 認証ユーザー名。
pub const DEFAULT_GIT_USERNAME: &str = "x-access-token";
/// `FXG_*` 環境変数名の定義。
pub mod env_keys {
    /// `[node] node_id`
    pub const NODE_ID: &str = "FXG_NODE_ID";
    /// `[node] name`
    pub const NODE_NAME: &str = "FXG_NODE_NAME";
    /// `[node] listen_addr`
    pub const NODE_LISTEN_ADDR: &str = "FXG_NODE_LISTEN_ADDR";
    /// `[node] central_server_url`
    pub const CENTRAL_SERVER_URL: &str = "FXG_CENTRAL_SERVER_URL";
    /// `[node] node_token`
    pub const NODE_TOKEN: &str = "FXG_NODE_TOKEN";
    /// `[node] allow_remote_pty`
    pub const ALLOW_REMOTE_PTY: &str = "FXG_ALLOW_REMOTE_PTY";
    /// ローカルIPCエンドポイントの上書き (テスト・同一ホストでの複数インスタンス用)
    pub const IPC_ENDPOINT: &str = "FXG_IPC_ENDPOINT";
    /// `[server] listen_addr`
    pub const SERVER_LISTEN_ADDR: &str = "FXG_SERVER_LISTEN_ADDR";
    /// `[server] allowed_hosts` (カンマ区切り)
    pub const SERVER_ALLOWED_HOSTS: &str = "FXG_SERVER_ALLOWED_HOSTS";
    /// `[server] allowed_origins` (カンマ区切り)
    pub const SERVER_ALLOWED_ORIGINS: &str = "FXG_SERVER_ALLOWED_ORIGINS";
    /// `[log] level` ("trace" | "debug" | "info" | "warn" | "error")
    pub const LOG_LEVEL: &str = "FXG_LOG_LEVEL";
    /// `[log] file`
    pub const LOG_FILE: &str = "FXG_LOG_FILE";
    /// `[log] format` ("text" | "json")
    pub const LOG_FORMAT: &str = "FXG_LOG_FORMAT";
    /// `[log] no_color`
    pub const LOG_NO_COLOR: &str = "FXG_LOG_NO_COLOR";
}

/// 設定の読み込み・パース時に発生するエラー。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// 設定ファイルの読み込み失敗
    #[error("failed to read config file {path}: {source}")]
    Io {
        /// 対象パス
        path: PathBuf,
        /// 元のI/Oエラー
        #[source]
        source: std::io::Error,
    },
    /// 設定ファイルの TOML パース失敗
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        /// 対象パス
        path: PathBuf,
        /// 元のパースエラー
        #[source]
        source: Box<toml::de::Error>,
    },
    /// 環境変数の値が不正
    #[error("invalid value for environment variable {key}: {value:?}")]
    InvalidEnvValue {
        /// 環境変数名
        key: &'static str,
        /// 不正な値
        value: String,
    },
}

/// 環境変数参照関数。
///
/// テストで差し替え可能にするため、設定ローダーは常にこの関数を通して
/// 環境変数を読む ([`process_env`] が実環境用の実装)。
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// 実環境の環境変数を読む [`EnvLookup`] 実装。
pub fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// FlexAgent のデータディレクトリ (`~/.flexagent`) を解決する。
///
/// 環境変数 [`ENV_FXG_HOME`] が設定されていればそれを優先し、
/// 未設定時は `~/.flexagent` を返す (ホームディレクトリ解決失敗時は `./.flexagent`)。
pub fn fxg_home(env: EnvLookup<'_>) -> PathBuf {
    if let Some(value) = env(ENV_FXG_HOME).filter(|v| !v.trim().is_empty()) {
        return PathBuf::from(value);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".flexagent")
}

fn parse_bool_env(key: &'static str, value: &str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidEnvValue {
            key,
            value: value.to_owned(),
        }),
    }
}

fn parse_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// グローバル設定 (`~/.flexagent/config.toml`)。
///
/// すべてのフィールドは省略可能 (デフォルト値あり)。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    /// ログ設定
    pub log: LogConfig,
    /// ローカルノードデーモン設定 (`fxg daemon`)
    pub node: NodeConfig,
    /// 中央サーバー設定 (`fxg server`)
    pub server: ServerConfig,
    /// エージェント & ACP Registry 設定
    pub agents: AgentsConfig,
    /// 一時VM・サンドボックスプロビジョナー定義 (`--provisioner <name>`)
    pub provisioners: BTreeMap<String, ProvisionerConfig>,
}

impl GlobalConfig {
    /// 実環境の環境変数を用いて [データディレクトリ](fxg_home) の
    /// `config.toml` を読み込み、`FXG_*` 上書きを適用する。
    ///
    /// ファイルが存在しない場合はデフォルト設定を返す。
    pub fn load(env: EnvLookup<'_>) -> Result<Self, ConfigError> {
        let home = fxg_home(env);
        Self::load_from_dir(&home, env)
    }

    /// 指定ディレクトリの `config.toml` を読み込み、`FXG_*` 上書きを適用する。
    pub fn load_from_dir(dir: &Path, env: EnvLookup<'_>) -> Result<Self, ConfigError> {
        let mut config = Self::load_from_path(&dir.join(CONFIG_FILE_NAME))?;
        config.apply_env_overrides(env)?;
        Ok(config)
    }

    /// 指定パスの `config.toml` を読み込む。
    ///
    /// ファイルが存在しない場合はデフォルト設定を返す。
    pub fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|source| ConfigError::Parse {
                path: path.to_path_buf(),
                source: Box::new(source),
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// `FXG_*` 環境変数による上書きを適用する。
    pub fn apply_env_overrides(&mut self, env: EnvLookup<'_>) -> Result<(), ConfigError> {
        if let Some(v) = env(env_keys::LOG_LEVEL) {
            self.log.level = Some(v);
        }
        if let Some(v) = env(env_keys::LOG_FILE) {
            self.log.file = Some(v);
        }
        if let Some(v) = env(env_keys::LOG_FORMAT) {
            self.log.format = Some(v);
        }
        if let Some(v) = env(env_keys::LOG_NO_COLOR) {
            self.log.no_color = parse_bool_env(env_keys::LOG_NO_COLOR, &v)?;
        }
        if let Some(v) = env(env_keys::NODE_ID) {
            self.node.node_id = Some(v);
        }
        if let Some(v) = env(env_keys::NODE_NAME) {
            self.node.name = Some(v);
        }
        if let Some(v) = env(env_keys::NODE_LISTEN_ADDR) {
            self.node.listen_addr = Some(v);
        }
        if let Some(v) = env(env_keys::CENTRAL_SERVER_URL) {
            self.node.central_server_url = Some(v);
        }
        if let Some(v) = env(env_keys::NODE_TOKEN) {
            self.node.node_token = Some(v);
        }
        if let Some(v) = env(env_keys::ALLOW_REMOTE_PTY) {
            self.node.allow_remote_pty = parse_bool_env(env_keys::ALLOW_REMOTE_PTY, &v)?;
        }
        if let Some(v) = env(env_keys::SERVER_LISTEN_ADDR) {
            self.server.listen_addr = Some(v);
        }
        if let Some(v) = env(env_keys::SERVER_ALLOWED_HOSTS) {
            self.server.allowed_hosts = parse_list(&v);
        }
        if let Some(v) = env(env_keys::SERVER_ALLOWED_ORIGINS) {
            self.server.allowed_origins = parse_list(&v);
        }
        Ok(())
    }
}

/// `[log]` セクション: ロギング設定。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// 既定ログレベル ("trace" | "debug" | "info" | "warn" | "error")
    pub level: Option<String>,
    /// ログファイル出力先 (相対パスは ~/.flexagent 起算、空文字で無効)
    pub file: Option<String>,
    /// ログ出力フォーマット ("text" | "json")
    pub format: Option<String>,
    /// ANSI カラー出力を無効化するか
    pub no_color: bool,
}

/// `[node]` セクション: ローカルノードデーモン設定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeConfig {
    /// ノードの一意識別子
    /// (省略時は初回起動時にホスト名ベースのスラッグ + 短IDを自動採番)。
    pub node_id: Option<String>,
    /// UI 上の表示名 (省略時は OS のホスト名)。
    pub name: Option<String>,
    /// ローカルHTTP/WSサーバーのバインドアドレス
    /// (セキュリティ原則によりループバック固定推奨)。
    pub listen_addr: Option<String>,
    /// 中央サーバーの Node Hub WebSocket URL
    /// (未指定時はスタンドアロン・ローカルのみで動作)。
    pub central_server_url: Option<String>,
    /// 中央サーバー接続時のペアリングトークン (ノード個別)。
    ///
    /// 省略時は `~/.flexagent/node_token` を読み込む。
    pub node_token: Option<String>,
    /// リモート (中央サーバー経由) からの対話型 Web PTY 起動を許可するか。
    pub allow_remote_pty: bool,
    /// Git Worktree 自動作成時のデフォルト配置先テンプレート。
    ///
    /// プレースホルダ: `{fxg_home}` / `{project}` / `{repo}` / `{repo_parent}` /
    /// `{branch}`。
    pub worktree_dir_template: Option<String>,
    /// デーモン起動時および `fxg project scan` 実行時に自動探索するディレクトリ一覧。
    pub project_scan_dirs: Vec<String>,
    /// 各ターンのプロンプト送信直前に Shadow Git Tree スナップショットを
    /// 自動取得するか。
    pub snapshot_enabled: bool,
    /// `fxg daemon` 起動時にタスクトレイへ常駐するか (既定: 無効)。
    ///
    /// CLI の `--tray` / `--no-tray` が指定された場合はそちらが優先される。
    pub tray: Option<bool>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            node_id: None,
            name: None,
            listen_addr: None,
            central_server_url: None,
            node_token: None,
            allow_remote_pty: false,
            worktree_dir_template: None,
            project_scan_dirs: Vec::new(),
            snapshot_enabled: true,
            tray: None,
        }
    }
}

impl NodeConfig {
    /// ローカルHTTP/WSサーバーの既定バインドアドレス (ループバック厳格バインド)。
    pub const DEFAULT_LISTEN_ADDR: &'static str = "127.0.0.1:7860";
    /// Worktree 配置先テンプレートの既定値。
    pub const DEFAULT_WORKTREE_DIR_TEMPLATE: &'static str =
        "{fxg_home}/worktrees/{project}/{branch}";

    /// 実効バインドアドレスを返す。
    pub fn resolved_listen_addr(&self) -> &str {
        self.listen_addr
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(Self::DEFAULT_LISTEN_ADDR)
    }

    /// 実効 Worktree 配置先テンプレートを返す。
    pub fn resolved_worktree_dir_template(&self) -> &str {
        self.worktree_dir_template
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(Self::DEFAULT_WORKTREE_DIR_TEMPLATE)
    }
}

/// `[server]` セクション: 中央サーバー設定。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// 中央サーバーのHTTP/WSバインドアドレス (LAN / VPN 内のみで待ち受けること)。
    pub listen_addr: Option<String>,
    /// DNS Rebinding 防御 (Host ヘッダ検証) で追加許可するホスト名一覧。
    pub allowed_hosts: Vec<String>,
    /// CSWSH 防御 (WebSocket Origin 検証) で同一オリジン以外に許可するオリジン一覧。
    pub allowed_origins: Vec<String>,
    /// Web Push (VAPID) の連絡先クレーム (`mailto:` または `https:`)。
    pub vapid_subject: Option<String>,
    /// 一時VM (`fxg daemon --stdio`) からの `GitCredentialRequest` に対する
    /// 認証プロキシ設定 (キーはホスト名。例: `github.com`)。
    pub git_credentials: BTreeMap<String, GitCredentialConfig>,
}

impl ServerConfig {
    /// 中央サーバーの既定バインドアドレス。
    ///
    /// パブリックインターネットへ直接露出せず、ファイアウォール / VPN で
    /// 隔離された LAN 内でのみ待ち受けること。
    pub const DEFAULT_LISTEN_ADDR: &'static str = "0.0.0.0:8080";

    /// 実効バインドアドレスを返す。
    pub fn resolved_listen_addr(&self) -> &str {
        self.listen_addr
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(Self::DEFAULT_LISTEN_ADDR)
    }
}

/// 一時VM・サンドボックスノードからの Git 認証取得方法。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum GitCredentialConfig {
    /// `gh auth token` からトークンを取得する。
    GhCli {
        /// Basic 認証ユーザー名 (既定: `x-access-token`)
        #[serde(default)]
        username: Option<String>,
    },
    /// 指定した環境変数からトークンを取得する。
    Env {
        /// Basic 認証ユーザー名 (既定: `x-access-token`)
        #[serde(default)]
        username: Option<String>,
        /// トークンを保持する環境変数名
        token_env: String,
    },
}

/// `[agents]` セクション: エージェント & ACP Registry 設定。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    /// `fxg run` 等でエージェント指定を省略した場合のデフォルトエージェント。
    pub default_agent: Option<String>,
    /// ACP 公式レジストリURL。
    pub registry_url: Option<String>,
    /// ACP レジストリキャッシュの有効期間 (秒)。
    pub registry_cache_ttl_secs: Option<u64>,
    /// `fxg run <alias>` で使用できるエージェント名エイリアス
    /// (内蔵デフォルト値の上書き・追加)。
    pub aliases: BTreeMap<String, String>,
    /// レジストリ外のカスタムACPエージェント定義。
    pub custom: BTreeMap<String, CustomAgentConfig>,
}

impl AgentsConfig {
    /// デフォルトエージェントの既定値。
    pub const DEFAULT_AGENT: &'static str = "opencode";
    /// ACP 公式レジストリURLの既定値。
    pub const DEFAULT_REGISTRY_URL: &'static str =
        "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
    /// レジストリキャッシュ TTL の既定値 (24時間)。
    pub const DEFAULT_REGISTRY_CACHE_TTL_SECS: u64 = 86_400;

    /// 内蔵エイリアスの既定値。
    pub fn default_aliases() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("claude".to_owned(), "claude-code-acp".to_owned()),
            ("antigravity".to_owned(), "antigravity-acp".to_owned()),
            ("gemini".to_owned(), "gemini-cli-acp".to_owned()),
            ("codex".to_owned(), "codex-acp".to_owned()),
        ])
    }

    /// 内蔵エイリアスに設定ファイルの上書き・追加をマージした実効エイリアスを返す。
    pub fn resolved_aliases(&self) -> BTreeMap<String, String> {
        let mut aliases = Self::default_aliases();
        for (key, value) in &self.aliases {
            aliases.insert(key.clone(), value.clone());
        }
        aliases
    }

    /// 実効デフォルトエージェントを返す。
    pub fn resolved_default_agent(&self) -> &str {
        self.default_agent
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(Self::DEFAULT_AGENT)
    }

    /// 実効 ACP レジストリURLを返す。
    pub fn resolved_registry_url(&self) -> &str {
        self.registry_url
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or(Self::DEFAULT_REGISTRY_URL)
    }

    /// 実効レジストリキャッシュ TTL (秒) を返す。
    pub fn resolved_registry_cache_ttl_secs(&self) -> u64 {
        self.registry_cache_ttl_secs
            .unwrap_or(Self::DEFAULT_REGISTRY_CACHE_TTL_SECS)
    }
}

/// レジストリ外のカスタムACPエージェント定義 (`[agents.custom.<id>]`)。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomAgentConfig {
    /// 表示名
    pub name: Option<String>,
    /// 実行コマンド
    pub command: String,
    /// コマンド引数
    pub args: Vec<String>,
    /// プロセスへ注入する環境変数
    pub env: BTreeMap<String, String>,
}

/// `[provisioners.<name>]`: 一時VM・サンドボックスプロビジョナー定義。
///
/// 「標準入出力で最終的に `fxg daemon --stdio` を起動するコマンド」を定義するだけで
/// 任意の環境をプロビジョナーとして登録できる。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProvisionerConfig {
    /// 説明 (UI 表示用)
    pub description: Option<String>,
    /// 実行コマンド (`docker` / `incus` / `uvx` / `ssh` 等)
    pub command: String,
    /// コマンド引数。
    ///
    /// プレースホルダ: `{BOOTSTRAP_SCRIPT}` / `{INSTANCE_NAME}` / `{SESSION_ID}`。
    pub args: Vec<String>,
    /// アイドル自動破棄までの秒数。
    pub idle_timeout_secs: Option<u64>,
    /// プロセスへ注入する環境変数
    pub env: BTreeMap<String, String>,
}

/// プロジェクト設定 (`.fxg.toml`)。
///
/// 各Gitリポジトリのルート (またはモノレポ内のサブディレクトリ) に任意で配置し、
/// プロジェクト固有の挙動を定義する。Git 管理可能。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    /// 論理プロジェクトIDの明示指定
    /// (省略時は `git remote.origin.url` から自動正規化)。
    pub project_key: Option<String>,
    /// UI 上のプロジェクト表示名 (省略時はディレクトリ名またはリポジトリ名)。
    pub name: Option<String>,
    /// プロジェクト別のデフォルトエージェント設定
    pub agent: ProjectAgentConfig,
    /// Git Worktree 作成時のプロジェクト固有ルール
    pub worktree: ProjectWorktreeConfig,
    /// 一時VM (`fxg bootstrap-workspace`) 用の追加セットアップ定義
    pub bootstrap: ProjectBootstrapConfig,
}

impl ProjectConfig {
    /// 指定ディレクトリの `.fxg.toml` を読み込む (存在しない場合は `None`)。
    pub fn load_from_dir(dir: &Path) -> Result<Option<Self>, ConfigError> {
        let path = dir.join(PROJECT_CONFIG_FILE_NAME);
        if !path.exists() {
            return Ok(None);
        }
        Self::load_from_path(&path).map(Some)
    }

    /// 指定パスの `.fxg.toml` を読み込む。
    pub fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    }
}

/// `[agent]` セクション: プロジェクト別のデフォルトエージェント設定。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectAgentConfig {
    /// デフォルトエージェントID
    pub default_agent: Option<String>,
    /// デフォルトモード (`code` / `plan` 等)
    pub default_mode: Option<String>,
    /// エージェントプロセスへの追加引数
    pub extra_args: Vec<String>,
}

/// `[worktree]` セクション: Git Worktree 作成時のプロジェクト固有ルール。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectWorktreeConfig {
    /// 新規Worktree作成時のデフォルト起点ブランチ (省略時は現在の HEAD)。
    pub base_branch: Option<String>,
    /// このプロジェクト専用のWorktree配置先テンプレート上書き。
    pub dir_template: Option<String>,
    /// 新規Worktree作成時にメインWorktreeから自動コピーする未追跡ファイル
    /// (`.env` 等)。
    pub copy_files: Vec<String>,
    /// 新規Worktree作成直後にそのディレクトリで自動実行する初期化コマンド。
    pub post_create: Vec<String>,
}

/// `[bootstrap]` セクション: 一時VM (`fxg bootstrap-workspace`) 用の追加定義。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectBootstrapConfig {
    /// リポジトリから自動検出されるツール以外に `mise install` で追加導入するツール。
    pub tools: Vec<String>,
    /// `git clone` とツール導入完了後に実行する初期セットアップコマンド。
    pub setup: Vec<String>,
    /// セットアップおよびエージェント実行時に注入する非機密の環境変数。
    pub env: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    const SAMPLE_GLOBAL_CONFIG: &str = r#"
[node]
node_id = "home-win"
name = "Home Windows PC"
listen_addr = "127.0.0.1:7860"
central_server_url = "ws://100.64.0.10:8080/api/v1/node/ws"
allow_remote_pty = false
worktree_dir_template = "{fxg_home}/worktrees/{project}/{branch}"
project_scan_dirs = ["D:/ghq/github.com"]
snapshot_enabled = true
tray = false

[server]
listen_addr = "0.0.0.0:8080"
allowed_hosts = ["home-server.tailnet-xxxx.ts.net:8080", "192.168.1.50:8080"]
allowed_origins = ["https://home-server.tailnet-xxxx.ts.net"]
vapid_subject = "mailto:admin@example.com"

[server.git_credentials."github.com"]
provider = "gh_cli"
username = "x-access-token"

[server.git_credentials."gitlab.example.com"]
provider = "env"
token_env = "FXG_GITLAB_PAT"

[agents]
default_agent = "opencode"
registry_url = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json"
registry_cache_ttl_secs = 86400

[agents.aliases]
antigravity = "antigravity-acp"

[agents.custom.my-local-agent]
name = "My Custom ACP Agent"
command = "node"
args = ["D:/tools/my-agent/dist/acp.js"]
env = { LOG_LEVEL = "info" }

[provisioners.local-docker]
description = "Local isolated Ubuntu container (Docker)"
command = "docker"
args = ["run", "--rm", "-i", "ubuntu:24.04", "sh", "-c", "{BOOTSTRAP_SCRIPT}"]
idle_timeout_secs = 900

[provisioners.colab-pro]
description = "Google Colab Pro (T4 GPU Runtime)"
command = "uvx"
args = ["google-colab-cli", "ssh", "--gpu", "t4", "--command", "{BOOTSTRAP_SCRIPT}"]
idle_timeout_secs = 900
env = {}
"#;

    #[test]
    fn parses_full_global_config() {
        let config: GlobalConfig = toml::from_str(SAMPLE_GLOBAL_CONFIG).unwrap();
        assert_eq!(config.node.node_id.as_deref(), Some("home-win"));
        assert_eq!(config.node.name.as_deref(), Some("Home Windows PC"));
        assert!(!config.node.allow_remote_pty);
        assert!(config.node.snapshot_enabled);
        assert_eq!(
            config.node.resolved_listen_addr(),
            NodeConfig::DEFAULT_LISTEN_ADDR
        );
        assert_eq!(
            config.node.resolved_worktree_dir_template(),
            "{fxg_home}/worktrees/{project}/{branch}"
        );
        assert_eq!(
            config.server.resolved_listen_addr(),
            ServerConfig::DEFAULT_LISTEN_ADDR
        );
        assert_eq!(config.server.allowed_hosts.len(), 2);
        assert_eq!(config.server.allowed_origins.len(), 1);
        assert_eq!(
            config.server.vapid_subject.as_deref(),
            Some("mailto:admin@example.com")
        );

        match config.server.git_credentials.get("github.com").unwrap() {
            GitCredentialConfig::GhCli { username } => {
                assert_eq!(username.as_deref(), Some("x-access-token"));
            }
            other => panic!("unexpected provider: {other:?}"),
        }
        match config
            .server
            .git_credentials
            .get("gitlab.example.com")
            .unwrap()
        {
            GitCredentialConfig::Env { token_env, .. } => {
                assert_eq!(token_env, "FXG_GITLAB_PAT");
            }
            other => panic!("unexpected provider: {other:?}"),
        }

        assert_eq!(config.agents.resolved_default_agent(), "opencode");
        assert_eq!(
            config.agents.resolved_registry_cache_ttl_secs(),
            AgentsConfig::DEFAULT_REGISTRY_CACHE_TTL_SECS
        );
        // エイリアス: 設定の上書き + 内蔵デフォルトのマージ
        let aliases = config.agents.resolved_aliases();
        assert_eq!(aliases.get("claude").unwrap(), "claude-code-acp");
        assert_eq!(aliases.len(), 4);

        let custom = config.agents.custom.get("my-local-agent").unwrap();
        assert_eq!(custom.command, "node");
        assert_eq!(custom.args.len(), 1);
        assert_eq!(
            custom.env.get("LOG_LEVEL").map(String::as_str),
            Some("info")
        );

        let docker = config.provisioners.get("local-docker").unwrap();
        assert_eq!(docker.command, "docker");
        assert_eq!(docker.idle_timeout_secs, Some(900));
        assert_eq!(config.provisioners.len(), 2);
    }

    #[test]
    fn defaults_are_applied_when_sections_are_missing() {
        let config: GlobalConfig = toml::from_str("").unwrap();
        assert_eq!(config, GlobalConfig::default());
        assert!(
            config.node.snapshot_enabled,
            "snapshot_enabled defaults true"
        );
        assert!(
            !config.node.allow_remote_pty,
            "allow_remote_pty defaults false"
        );
        assert_eq!(
            config.agents.resolved_registry_url(),
            AgentsConfig::DEFAULT_REGISTRY_URL
        );
        assert_eq!(config.agents.resolved_default_agent(), "opencode");
        assert!(config.provisioners.is_empty());
    }

    #[test]
    fn unknown_fields_are_ignored_for_forward_compatibility() {
        let config: GlobalConfig = toml::from_str(
            r#"
[node]
node_id = "n1"
future_option = "whatever"
"#,
        )
        .unwrap();
        assert_eq!(config.node.node_id.as_deref(), Some("n1"));
    }

    #[test]
    fn env_overrides_are_applied() {
        let mut config: GlobalConfig = toml::from_str(SAMPLE_GLOBAL_CONFIG).unwrap();
        let env = env_map(&[
            ("FXG_NODE_ID", "override-node"),
            ("FXG_NODE_NAME", "Overridden"),
            ("FXG_NODE_LISTEN_ADDR", "127.0.0.1:9000"),
            ("FXG_CENTRAL_SERVER_URL", "ws://example.test/ws"),
            ("FXG_NODE_TOKEN", "secret-token"),
            ("FXG_ALLOW_REMOTE_PTY", "true"),
            ("FXG_SERVER_LISTEN_ADDR", "0.0.0.0:9090"),
            ("FXG_SERVER_ALLOWED_HOSTS", "a.test, b.test ,, c.test"),
            ("FXG_SERVER_ALLOWED_ORIGINS", "http://a.test"),
            ("FXG_LOG_LEVEL", "debug"),
            ("FXG_LOG_FILE", "custom.log"),
            ("FXG_LOG_FORMAT", "json"),
            ("FXG_LOG_NO_COLOR", "true"),
        ]);
        config
            .apply_env_overrides(&|key| env.get(key).cloned())
            .unwrap();

        assert_eq!(config.log.level.as_deref(), Some("debug"));
        assert_eq!(config.log.file.as_deref(), Some("custom.log"));
        assert_eq!(config.log.format.as_deref(), Some("json"));
        assert!(config.log.no_color);
        assert_eq!(config.node.node_id.as_deref(), Some("override-node"));
        assert_eq!(config.node.name.as_deref(), Some("Overridden"));
        assert_eq!(config.node.resolved_listen_addr(), "127.0.0.1:9000");
        assert_eq!(
            config.node.central_server_url.as_deref(),
            Some("ws://example.test/ws")
        );
        assert_eq!(config.node.node_token.as_deref(), Some("secret-token"));
        assert!(config.node.allow_remote_pty);
        assert_eq!(config.server.resolved_listen_addr(), "0.0.0.0:9090");
        assert_eq!(
            config.server.allowed_hosts,
            vec!["a.test", "b.test", "c.test"]
        );
        assert_eq!(config.server.allowed_origins, vec!["http://a.test"]);
    }

    #[test]
    fn invalid_bool_env_is_rejected() {
        let mut config = GlobalConfig::default();
        let env = env_map(&[("FXG_ALLOW_REMOTE_PTY", "maybe")]);
        let err = config
            .apply_env_overrides(&|key| env.get(key).cloned())
            .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidEnvValue {
                key: "FXG_ALLOW_REMOTE_PTY",
                ..
            }
        ));
    }

    #[test]
    fn fxg_home_prefers_env_then_falls_back_to_home_dir() {
        let env = env_map(&[("FXG_HOME", "/tmp/fxg-test-home")]);
        assert_eq!(
            fxg_home(&|key| env.get(key).cloned()),
            PathBuf::from("/tmp/fxg-test-home")
        );

        let empty = env_map(&[]);
        let fallback = fxg_home(&|key| empty.get(key).cloned());
        assert!(fallback.ends_with(".flexagent"));
    }

    #[test]
    fn missing_config_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = GlobalConfig::load_from_dir(dir.path(), &|_| None).unwrap();
        assert_eq!(config, GlobalConfig::default());
    }

    #[test]
    fn loads_config_file_from_directory_with_env_overrides() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(CONFIG_FILE_NAME), SAMPLE_GLOBAL_CONFIG).unwrap();
        let env = env_map(&[("FXG_NODE_ID", "from-env")]);
        let config = GlobalConfig::load_from_dir(dir.path(), &|key| env.get(key).cloned()).unwrap();
        assert_eq!(config.node.node_id.as_deref(), Some("from-env"));
        assert_eq!(config.node.name.as_deref(), Some("Home Windows PC"));
    }

    #[test]
    fn invalid_toml_reports_parse_error_with_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE_NAME);
        std::fs::write(&path, "this is not = valid toml [").unwrap();
        let err = GlobalConfig::load_from_path(&path).unwrap_err();
        match err {
            ConfigError::Parse { path: p, .. } => assert_eq!(p, path),
            other => panic!("unexpected error: {other}"),
        }
    }

    const SAMPLE_PROJECT_CONFIG: &str = r#"
project_key = "github.com/nazo6/flexagent"
name = "flexagent"

[agent]
default_agent = "opencode2"
default_mode = "code"
extra_args = []

[worktree]
base_branch = "main"
dir_template = "{fxg_home}/worktrees/{project}/{branch}"
copy_files = [".env", ".env.local"]
post_create = ["pnpm install --prefer-offline"]

[bootstrap]
tools = ["rust@stable", "node@22", "pnpm@latest"]
setup = ["cargo fetch", "pnpm install --frozen-lockfile"]

[bootstrap.env]
RUST_BACKTRACE = "1"
"#;

    #[test]
    fn parses_project_config() {
        let config: ProjectConfig = toml::from_str(SAMPLE_PROJECT_CONFIG).unwrap();
        assert_eq!(
            config.project_key.as_deref(),
            Some("github.com/nazo6/flexagent")
        );
        assert_eq!(config.name.as_deref(), Some("flexagent"));
        assert_eq!(config.agent.default_mode.as_deref(), Some("code"));
        assert_eq!(config.worktree.base_branch.as_deref(), Some("main"));
        assert_eq!(config.worktree.copy_files, vec![".env", ".env.local"]);
        assert_eq!(config.worktree.post_create.len(), 1);
        assert_eq!(config.bootstrap.tools.len(), 3);
        assert_eq!(config.bootstrap.setup.len(), 2);
        assert_eq!(
            config
                .bootstrap
                .env
                .get("RUST_BACKTRACE")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn project_config_load_from_dir_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ProjectConfig::load_from_dir(dir.path()).unwrap().is_none());

        std::fs::write(dir.path().join(PROJECT_CONFIG_FILE_NAME), "name = \"app\"").unwrap();
        let config = ProjectConfig::load_from_dir(dir.path()).unwrap().unwrap();
        assert_eq!(config.name.as_deref(), Some("app"));
    }
}

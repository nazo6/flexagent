//! ACP Registry (`registry.json`) の取得・キャッシュ・起動解決と導入管理。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §2、
//! CLI コマンド: `docs/05-cli-and-pwa-ui.md` §1 (`fxg agents ...`)。
//!
//! - 公式インデックス (既定
//!   `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`)
//!   を `~/.flexagent/cache/registry.json` へキャッシュする
//!   (既定 TTL 24 時間。`fxg agents update` は強制更新)。
//! - 起動解決は「ローカル優先」: fxg 管理の導入済みバイナリ →
//!   `PATH` 上の既存インストール → `npx` / `uvx`。**起動時の暗黙ダウンロードは
//!   行わない** (導入は `fxg agents install` / UI の明示操作のみ)。
//! - カスタム定義は `config.toml` の `[agents.custom.<id>]` と、UI が管理する
//!   `~/.flexagent/custom_agents.json` をマージして解決する (ファイル側が優先)。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use fxg_protocol::config::{AgentsConfig, CustomAgentConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::driver::AgentLaunchSpec;

/// キャッシュファイル名 (`~/.flexagent/cache/registry.json`)。
pub const REGISTRY_CACHE_FILE: &str = "registry.json";

/// UI 管理のカスタムエージェント定義ファイル名 (`~/.flexagent/custom_agents.json`)。
pub const CUSTOM_AGENTS_FILE: &str = "custom_agents.json";

/// `npx` / `uvx` 配布を有効化したエージェントIDのファイル名
/// (`~/.flexagent/enabled_package_agents.json`)。
pub const ENABLED_PACKAGES_FILE: &str = "enabled_package_agents.json";

/// `agents[].distribution`。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RegistryDistribution {
    /// OS/Arch キー (`darwin-aarch64` / `windows-x86_64` 等) ごとのバイナリ配布
    pub binary: BTreeMap<String, BinaryDistribution>,
    /// `npx` 経由の配布
    pub npx: Option<PackageDistribution>,
    /// `uvx` 経由の配布
    pub uvx: Option<PackageDistribution>,
}

impl RegistryDistribution {
    /// 配布形態の名前一覧 (表示用)。
    pub fn kinds(&self) -> Vec<&'static str> {
        let mut kinds = Vec::new();
        if !self.binary.is_empty() {
            kinds.push("binary");
        }
        if self.npx.is_some() {
            kinds.push("npx");
        }
        if self.uvx.is_some() {
            kinds.push("uvx");
        }
        kinds
    }
}

/// バイナリ配布エントリ。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BinaryDistribution {
    /// アーカイブ URL (`.tar.gz` / `.zip`)
    pub archive: String,
    /// アーカイブ内の実行ファイル (相対パス。`./name` 形式を含む)
    pub cmd: String,
    /// 起動時の追加引数
    #[serde(default)]
    pub args: Vec<String>,
    /// 追加の環境変数
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// アーカイブの SHA-256 (任意)
    #[serde(default)]
    pub sha256: Option<String>,
}

/// `npx` / `uvx` 配布エントリ。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageDistribution {
    /// パッケージ指定 (`name@version` を含み得る)
    pub package: String,
    /// パッケージへ渡す追加引数
    #[serde(default)]
    pub args: Vec<String>,
    /// 追加の環境変数
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// レジストリのエージェント定義。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryAgent {
    /// エージェントID
    pub id: String,
    /// 表示名
    pub name: String,
    /// バージョン
    pub version: String,
    /// 説明
    #[serde(default)]
    pub description: Option<String>,
    /// リポジトリURL
    #[serde(default)]
    pub repository: Option<String>,
    /// 公式サイト
    #[serde(default)]
    pub website: Option<String>,
    /// 作者
    #[serde(default)]
    pub authors: Vec<String>,
    /// ライセンス
    #[serde(default)]
    pub license: Option<String>,
    /// アイコンURL
    #[serde(default)]
    pub icon: Option<String>,
    /// 配布形態
    #[serde(default)]
    pub distribution: RegistryDistribution,
}

/// `registry.json` 全体。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct RegistryIndex {
    /// インデックスのスキーマバージョン
    #[serde(default)]
    pub version: String,
    /// エージェント一覧
    #[serde(default)]
    pub agents: Vec<RegistryAgent>,
}

impl RegistryIndex {
    /// エージェントIDで引く。
    pub fn find(&self, id: &str) -> Option<&RegistryAgent> {
        self.agents.iter().find(|agent| agent.id == id)
    }
}

/// このノードで使える起動経路。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalSource {
    /// fxg が `~/.flexagent/agents/<id>/<version>` へ導入したバイナリ
    Managed(PathBuf),
    /// `PATH` 上に既にある実行ファイル
    External(PathBuf),
    /// `npx` で実行 (実行時に取得)
    Npx,
    /// `uvx` で実行 (実行時に取得)
    Uvx,
}

/// `fxg agents list` / カタログ表示用のエージェント情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentListEntry {
    /// エージェントID
    pub id: String,
    /// 表示名
    pub name: String,
    /// バージョン (カスタムは `-`)
    pub version: String,
    /// 説明
    pub description: Option<String>,
    /// ACP Registry カタログに存在するか
    pub registry: bool,
    /// 現在のプラットフォーム向け binary を fxg で導入できるか
    pub installable: bool,
    /// `npx` / `uvx` (PATH 上に存在) で実行できるか
    pub package_available: bool,
    /// このノードで使える起動経路 (優先順)
    pub sources: Vec<LocalSource>,
    /// fxg が導入済みのバージョン一覧
    pub managed_versions: Vec<String>,
    /// カスタム定義 (config.toml / UI 管理)
    pub custom: Option<CustomAgentConfig>,
}

impl AgentListEntry {
    /// このノードで起動できるか。
    pub fn is_available(&self) -> bool {
        self.custom.is_some() || !self.sources.is_empty()
    }
}

/// ACP Registry 管理 (取得・キャッシュ・解決・導入)。
#[derive(Debug, Clone)]
pub struct AcpRegistry {
    home: PathBuf,
    registry_url: String,
    cache_ttl: Duration,
    custom: BTreeMap<String, CustomAgentConfig>,
    aliases: BTreeMap<String, String>,
    http: reqwest::Client,
}

impl AcpRegistry {
    /// `~/.flexagent` と `[agents]` 設定から作成する。
    pub fn new(home: impl Into<PathBuf>, config: &AgentsConfig) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("fxg/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            home: home.into(),
            registry_url: config
                .registry_url
                .clone()
                .unwrap_or_else(|| AgentsConfig::DEFAULT_REGISTRY_URL.to_owned()),
            cache_ttl: Duration::from_secs(
                config
                    .registry_cache_ttl_secs
                    .unwrap_or(AgentsConfig::DEFAULT_REGISTRY_CACHE_TTL_SECS),
            ),
            custom: config.custom.clone(),
            aliases: config.resolved_aliases(),
            http,
        }
    }

    /// キャッシュファイルのパス (`~/.flexagent/cache/registry.json`)。
    pub fn cache_path(&self) -> PathBuf {
        self.home.join("cache").join(REGISTRY_CACHE_FILE)
    }

    /// エージェント導入ディレクトリ (`~/.flexagent/agents`)。
    pub fn agents_dir(&self) -> PathBuf {
        self.home.join("agents")
    }

    /// 指定エージェントのバージョンディレクトリ (`~/.flexagent/agents/<id>/<version>`)。
    pub fn agent_version_dir(&self, id: &str, version: &str) -> PathBuf {
        self.agents_dir().join(id).join(version)
    }

    /// UI 管理のカスタムエージェント定義ファイル (`~/.flexagent/custom_agents.json`)。
    pub fn custom_store_path(&self) -> PathBuf {
        self.home.join(CUSTOM_AGENTS_FILE)
    }

    /// `config.toml` と UI 管理ファイルをマージしたカスタム定義を返す
    /// (同名は UI 管理ファイルが優先)。
    pub fn custom_agents(&self) -> BTreeMap<String, CustomAgentConfig> {
        let mut agents = self.custom.clone();
        agents.extend(self.read_custom_store());
        agents
    }

    /// UI 管理ファイルを読む (存在しない・壊れている場合は空)。
    fn read_custom_store(&self) -> BTreeMap<String, CustomAgentConfig> {
        let path = self.custom_store_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return BTreeMap::new();
        };
        match serde_json::from_str(&text) {
            Ok(agents) => agents,
            Err(err) => {
                tracing::warn!(path = %path.display(), "failed to parse custom agents: {err}");
                BTreeMap::new()
            }
        }
    }

    /// UI 管理ファイルを書く (一時ファイル経由で原子的に置換)。
    fn write_custom_store(
        &self,
        agents: &BTreeMap<String, CustomAgentConfig>,
    ) -> anyhow::Result<()> {
        let path = self.custom_store_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(agents)?)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
        Ok(())
    }

    /// カスタムエージェント定義を作成・更新する。
    pub fn upsert_custom(&self, id: &str, config: CustomAgentConfig) -> anyhow::Result<()> {
        let id = Self::validate_custom_id(id)?;
        if config.command.trim().is_empty() {
            bail!("custom agent `{id}` requires a non-empty command");
        }
        let mut agents = self.read_custom_store();
        agents.insert(id.to_owned(), config);
        self.write_custom_store(&agents)
    }

    /// UI 管理のカスタム定義を削除する (`config.toml` 側の定義は残る)。
    pub fn remove_custom(&self, id: &str) -> anyhow::Result<bool> {
        let mut agents = self.read_custom_store();
        let removed = agents.remove(id).is_some();
        if removed {
            self.write_custom_store(&agents)?;
        }
        Ok(removed)
    }

    /// カスタムIDとして使える文字列か検証する。
    fn validate_custom_id(id: &str) -> anyhow::Result<&str> {
        let id = id.trim();
        if id.is_empty() || id.contains(|c: char| c.is_whitespace() || c == '/' || c == '\\') {
            bail!("invalid custom agent id: {id:?}");
        }
        Ok(id)
    }

    /// `npx` / `uvx` 有効化ファイル (`~/.flexagent/enabled_package_agents.json`)。
    pub fn enabled_packages_path(&self) -> PathBuf {
        self.home.join(ENABLED_PACKAGES_FILE)
    }

    /// `npx` / `uvx` 配布での利用を有効化したエージェントID一覧。
    ///
    /// `npx` / `uvx` は実体を持たず実行時に取得されるため、既定では一覧に出さず、
    /// ユーザーが明示的に有効化したものだけを候補にする。
    pub fn enabled_package_agents(&self) -> BTreeSet<String> {
        let path = self.enabled_packages_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return BTreeSet::new();
        };
        match serde_json::from_str::<Vec<String>>(&text) {
            Ok(ids) => ids.into_iter().collect(),
            Err(err) => {
                tracing::warn!(path = %path.display(), "failed to parse enabled package agents: {err}");
                BTreeSet::new()
            }
        }
    }

    /// `npx` / `uvx` 配布での利用を有効化・無効化する。
    pub fn set_package_enabled(&self, id: &str, enabled: bool) -> anyhow::Result<()> {
        let mut ids = self.enabled_package_agents();
        let changed = if enabled {
            ids.insert(id.to_owned())
        } else {
            ids.remove(id)
        };
        if !changed {
            return Ok(());
        }
        let path = self.enabled_packages_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let values: Vec<&String> = ids.iter().collect();
        std::fs::write(&tmp, serde_json::to_string_pretty(&values)?)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
        Ok(())
    }

    /// `npx` / `uvx` 配布で起動できるランナー (両方無ければ `None`)。
    pub fn package_runner(agent: &RegistryAgent) -> Option<&'static str> {
        if agent.distribution.npx.is_some() && which::which("npx").is_ok() {
            return Some("npx");
        }
        if agent.distribution.uvx.is_some() && which::which("uvx").is_ok() {
            return Some("uvx");
        }
        None
    }

    /// 現在のプラットフォームキー (`darwin-aarch64` / `windows-x86_64` 等)。
    pub fn platform_key() -> String {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        };
        format!("{os}-{}", std::env::consts::ARCH)
    }

    /// エージェント名 (エイリアス / カスタム名) を実エージェントIDへ解決する。
    pub fn resolve_alias(&self, name: &str) -> String {
        self.aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_owned())
    }

    /// レジストリインデックスを返す。
    ///
    /// キャッシュが TTL 内ならそれを用い、期限切れまたは `force_update` 時は
    /// 取得してキャッシュを更新する。取得失敗時に TTL 切れキャッシュしか無い
    /// 場合はそれをフォールバックとして返す (オフライン利用)。
    pub async fn index(&self, force_update: bool) -> anyhow::Result<RegistryIndex> {
        if !force_update && let Some(cached) = self.read_cache(true) {
            return Ok(cached);
        }
        match self.fetch().await {
            Ok(index) => {
                if let Err(err) = self.write_cache(&index) {
                    tracing::warn!("failed to write registry cache: {err}");
                }
                Ok(index)
            }
            Err(err) => {
                // オフラインフォールバック: TTL 切れキャッシュでも使う
                if let Some(stale) = self.read_cache(false) {
                    tracing::warn!("registry fetch failed ({err}); using stale cache");
                    Ok(stale)
                } else {
                    Err(err)
                }
            }
        }
    }

    /// レジストリを取得する (キャッシュは使わない)。
    pub async fn fetch(&self) -> anyhow::Result<RegistryIndex> {
        tracing::debug!(url = %self.registry_url, "fetching acp registry");
        let response = self
            .http
            .get(&self.registry_url)
            .send()
            .await
            .context("failed to fetch ACP registry")?
            .error_for_status()
            .context("ACP registry returned an error status")?;
        let text = response
            .text()
            .await
            .context("failed to read registry body")?;
        serde_json::from_str(&text).context("failed to parse ACP registry JSON")
    }

    /// キャッシュを読み込む (`ttl_fresh` が true なら TTL 内のもののみ)。
    fn read_cache(&self, ttl_fresh: bool) -> Option<RegistryIndex> {
        let path = self.cache_path();
        if ttl_fresh {
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            let age = std::time::SystemTime::now()
                .duration_since(modified)
                .unwrap_or_default();
            if age > self.cache_ttl {
                return None;
            }
        }
        let text = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// キャッシュを書き込む。
    fn write_cache(&self, index: &RegistryIndex) -> anyhow::Result<()> {
        let path = self.cache_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(index)?;
        std::fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))
    }

    /// エージェント一覧を作る。
    ///
    /// `include_unavailable = false` の場合はこのノードで起動できるものだけ返す。
    pub fn list(&self, index: &RegistryIndex, include_unavailable: bool) -> Vec<AgentListEntry> {
        let mut entries = Vec::new();
        let custom = self.custom_agents();
        // `npx` / `uvx` は明示的に有効化されたものだけ候補にする
        let enabled = self.enabled_package_agents();

        // カスタム定義 (config.toml / UI 管理)
        for (id, config) in &custom {
            entries.push(AgentListEntry {
                id: id.clone(),
                name: config.name.clone().unwrap_or_else(|| id.clone()),
                version: "-".to_owned(),
                description: None,
                registry: index.find(id).is_some(),
                installable: false,
                package_available: false,
                sources: Vec::new(),
                managed_versions: Vec::new(),
                custom: Some(config.clone()),
            });
        }

        // レジストリ
        for agent in &index.agents {
            // カスタム定義が同名で存在する場合はそちらを優先する (起動解決と同じ)
            if custom.contains_key(&agent.id) {
                continue;
            }
            let sources = self.local_sources(agent, enabled.contains(&agent.id));
            if sources.is_empty() && !include_unavailable {
                continue;
            }
            entries.push(AgentListEntry {
                id: agent.id.clone(),
                name: agent.name.clone(),
                version: agent.version.clone(),
                description: agent.description.clone(),
                registry: true,
                installable: Self::binary_distribution(agent).is_ok(),
                package_available: Self::package_runner(agent).is_some(),
                sources,
                managed_versions: self.installed_versions(&agent.id),
                custom: None,
            });
        }

        entries
    }

    /// 現在のレジストリバージョンの fxg 管理バイナリ (未導入なら `None`)。
    fn managed_command(&self, agent: &RegistryAgent) -> Option<PathBuf> {
        let dist = Self::binary_distribution(agent).ok()?;
        let path = self
            .agent_version_dir(&agent.id, &agent.version)
            .join(clean_cmd(&dist.cmd));
        path.is_file().then_some(path)
    }

    /// このノードで使える起動経路を優先順に返す。
    ///
    /// `include_package` が `false` のときは `npx` / `uvx` を候補に含めない
    /// (一覧表示用。起動解決では常に許可する)。
    fn local_sources(&self, agent: &RegistryAgent, include_package: bool) -> Vec<LocalSource> {
        let mut sources = Vec::new();
        if let Some(path) = self.managed_command(agent) {
            sources.push(LocalSource::Managed(path));
        }
        if let Ok(dist) = Self::binary_distribution(agent) {
            let cmd = clean_cmd(&dist.cmd);
            // ディレクトリを含む `cmd` はカレントディレクトリ相対と解釈されるため
            // PATH 探索の対象にしない。
            if cmd.components().count() == 1
                && let Ok(path) = which::which(&cmd)
            {
                sources.push(LocalSource::External(path));
            }
        }
        if include_package {
            match Self::package_runner(agent) {
                Some("npx") => sources.push(LocalSource::Npx),
                Some(_) => sources.push(LocalSource::Uvx),
                None => {}
            }
        }
        sources
    }

    /// エージェントの起動スペックを解決する。
    ///
    /// 解決順は「カスタム定義 → fxg 管理バイナリ → `PATH` 上の既存インストール →
    /// `npx` → `uvx`」。**未導入でも暗黙にダウンロードはしない**
    /// (導入は `fxg agents install` / UI の明示操作のみ)。
    pub async fn launch_spec(
        &self,
        name: &str,
        index: &RegistryIndex,
        extra_args: &[String],
    ) -> anyhow::Result<AgentLaunchSpec> {
        let id = self.resolve_alias(name);

        // 1) カスタム定義 (レジストリより優先)
        if let Some(custom) = self.custom_agents().remove(&id) {
            let mut args = custom.args.clone();
            args.extend(extra_args.iter().cloned());
            return Ok(AgentLaunchSpec {
                agent_id: id.clone(),
                display_name: custom.name.clone().unwrap_or_else(|| id.clone()),
                driver_kind: "acp".to_owned(),
                program: PathBuf::from(&custom.command),
                args,
                env: custom
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            });
        }

        // 2) レジストリ (このノードで使える経路のみ。`npx` / `uvx` は明示有効化に依存しない)
        let agent = index
            .find(&id)
            .ok_or_else(|| anyhow!("unknown agent: {name} (registry: {id})"))?;
        let source = self
            .local_sources(agent, true)
            .into_iter()
            .next()
            .ok_or_else(|| {
            anyhow!(
                "agent {id} is not available on this node: install it with `fxg agents install {id}` or put its binary on PATH (distributions: {:?})",
                agent.distribution.kinds()
            )
        })?;
        self.spec_for_source(agent, source, extra_args)
            .ok_or_else(|| {
                anyhow!(
                    "agent {id} has no usable distribution for {} (available: {:?})",
                    Self::platform_key(),
                    agent.distribution.kinds()
                )
            })
    }

    /// 起動経路から起動スペックを組み立てる。
    fn spec_for_source(
        &self,
        agent: &RegistryAgent,
        source: LocalSource,
        extra_args: &[String],
    ) -> Option<AgentLaunchSpec> {
        let (program, args, env) = match source {
            LocalSource::Managed(path) | LocalSource::External(path) => {
                let dist = Self::binary_distribution(agent).ok()?;
                let mut args = dist.args.clone();
                args.extend(extra_args.iter().cloned());
                (path, args, dist.env.clone())
            }
            LocalSource::Npx => {
                let npx = agent.distribution.npx.as_ref()?;
                let mut args = vec!["-y".to_owned(), npx.package.clone()];
                args.extend(npx.args.iter().cloned());
                args.extend(extra_args.iter().cloned());
                (PathBuf::from("npx"), args, npx.env.clone())
            }
            LocalSource::Uvx => {
                let uvx = agent.distribution.uvx.as_ref()?;
                let mut args = vec![uvx.package.clone()];
                args.extend(uvx.args.iter().cloned());
                args.extend(extra_args.iter().cloned());
                (PathBuf::from("uvx"), args, uvx.env.clone())
            }
        };
        Some(AgentLaunchSpec {
            agent_id: agent.id.clone(),
            display_name: agent.name.clone(),
            driver_kind: "acp".to_owned(),
            program,
            args,
            env: env.into_iter().collect(),
        })
    }

    /// 現在のプラットフォーム向け binary 配布を返す。
    fn binary_distribution(agent: &RegistryAgent) -> anyhow::Result<&BinaryDistribution> {
        let platform = AcpRegistry::platform_key();
        agent.distribution.binary.get(&platform).ok_or_else(|| {
            anyhow!(
                "agent {} has no binary distribution for {platform}",
                agent.id
            )
        })
    }

    /// binary 配布を導入済みにし、実行ファイルの絶対パスを返す。
    ///
    /// 既に展開済みならダウンロードせずにそのパスを返す。`PATH` 上の同名
    /// コマンドは使用しない (外部インストールは起動解決側で扱う)。
    pub async fn install(&self, id: &str, index: &RegistryIndex) -> anyhow::Result<PathBuf> {
        let agent = index
            .find(id)
            .ok_or_else(|| anyhow!("unknown agent in registry: {id}"))?;
        let dist = Self::binary_distribution(agent)?.clone();
        let target_dir = self.agent_version_dir(&agent.id, &agent.version);
        let cmd_name = clean_cmd(&dist.cmd);
        let cmd_path = target_dir.join(&cmd_name);
        if cmd_path.is_file() {
            return Ok(cmd_path);
        }

        tracing::info!(agent = %agent.id, version = %agent.version, "installing agent binary");
        let bytes = self
            .http
            .get(&dist.archive)
            .send()
            .await
            .with_context(|| format!("failed to download {}", dist.archive))?
            .error_for_status()
            .with_context(|| format!("download failed: {}", dist.archive))?
            .bytes()
            .await
            .context("failed to read archive body")?;

        if let Some(expected) = &dist.sha256 {
            let actual = sha256_hex(&bytes);
            if !actual.eq_ignore_ascii_case(expected) {
                bail!(
                    "sha256 mismatch for {}: expected {expected}, got {actual}",
                    dist.archive
                );
            }
        }

        // 一時ディレクトリへ展開してから rename する (中断時の部分展開を防ぐ)
        let tmp_dir = self
            .agents_dir()
            .join(&agent.id)
            .join(format!(".tmp-{}", agent.version));
        if tmp_dir.exists() {
            std::fs::remove_dir_all(&tmp_dir).ok();
        }
        extract_archive_bytes(&dist.archive, &bytes, &tmp_dir)
            .with_context(|| format!("failed to extract {}", dist.archive))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = tmp_dir.join(&cmd_name);
            if path.is_file() {
                let mut perms = std::fs::metadata(&path)?.permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&path, perms)?;
            }
        }

        if target_dir.exists() {
            std::fs::remove_dir_all(&target_dir).ok();
        }
        if let Some(parent) = target_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&tmp_dir, &target_dir)
            .with_context(|| format!("failed to move into {}", target_dir.display()))?;

        if !cmd_path.is_file() {
            bail!(
                "archive {} did not contain expected command {}",
                dist.archive,
                cmd_name.display()
            );
        }
        Ok(cmd_path)
    }

    /// 導入済みバージョン一覧 (`~/.flexagent/agents/<id>/<version>`)。
    pub fn installed_versions(&self, id: &str) -> Vec<String> {
        let dir = self.agents_dir().join(id);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut versions: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .collect();
        versions.sort();
        versions
    }

    /// 導入済み binary (全バージョン) を削除する。
    pub fn remove(&self, id: &str) -> anyhow::Result<bool> {
        let dir = self.agents_dir().join(id);
        if !dir.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("failed to remove {}", dir.display()))?;
        Ok(true)
    }
}

/// `./name` 形式の `cmd` を相対パスへ正規化する。
fn clean_cmd(cmd: &str) -> PathBuf {
    PathBuf::from(cmd.trim_start_matches("./"))
}

/// SHA-256 を 16 進文字列で返す。
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// アーカイブバイト列 (`.tar.gz` / `.zip`) を `dest` へ展開する。
fn extract_archive_bytes(url: &str, bytes: &[u8], dest: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dest)?;
    if url.ends_with(".zip") {
        let reader = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(reader).context("invalid zip archive")?;
        archive
            .extract(dest)
            .context("failed to extract zip archive")?;
    } else {
        let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
        let mut archive = tar::Archive::new(decoder);
        archive
            .unpack(dest)
            .context("failed to extract tar.gz archive")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
    {
      "version": "1.0.0",
      "agents": [
        {
          "id": "demo-npx",
          "name": "Demo Npx",
          "version": "1.2.3",
          "description": "npx based agent",
          "authors": ["tester"],
          "distribution": {
            "npx": { "package": "demo-agent@1.2.3", "args": ["--acp"], "env": { "DEMO": "1" } }
          }
        },
        {
          "id": "demo-bin",
          "name": "Demo Binary",
          "version": "2.0.0",
          "authors": ["tester"],
          "distribution": {
            "binary": {
              "darwin-aarch64": {
                "archive": "https://example.com/demo.tar.gz",
                "cmd": "./bin/demo",
                "args": ["--stdio"],
                "sha256": "00"
              }
            }
          }
        }
      ]
    }
    "#;

    fn config() -> AgentsConfig {
        let mut custom = BTreeMap::new();
        custom.insert(
            "my-agent".to_owned(),
            CustomAgentConfig {
                name: Some("My Agent".to_owned()),
                command: "my-agent".to_owned(),
                args: vec!["--flag".to_owned()],
                env: BTreeMap::from([("KEY".to_owned(), "value".to_owned())]),
            },
        );
        AgentsConfig {
            custom,
            ..AgentsConfig::default()
        }
    }

    fn registry(dir: &Path) -> AcpRegistry {
        AcpRegistry::new(dir, &config())
    }

    #[test]
    fn parses_registry_fixture() {
        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");
        assert_eq!(index.version, "1.0.0");
        assert_eq!(index.agents.len(), 2);
        let npx = index.find("demo-npx").expect("demo-npx");
        assert_eq!(npx.distribution.kinds(), vec!["npx"]);
        let npx_dist = npx.distribution.npx.as_ref().expect("npx dist");
        assert_eq!(npx_dist.package, "demo-agent@1.2.3");
        assert_eq!(npx_dist.args, vec!["--acp"]);
        let bin = index.find("demo-bin").expect("demo-bin");
        assert_eq!(bin.distribution.kinds(), vec!["binary"]);
        let binary = bin
            .distribution
            .binary
            .get("darwin-aarch64")
            .expect("platform");
        assert_eq!(binary.cmd, "./bin/demo");
        assert_eq!(clean_cmd(&binary.cmd), PathBuf::from("bin/demo"));
    }

    #[test]
    fn platform_key_matches_registry_format() {
        let key = AcpRegistry::platform_key();
        let (os, arch) = key.split_once('-').expect("os-arch");
        assert!(
            ["darwin", "linux", "windows"].contains(&os),
            "unexpected os: {key}"
        );
        assert!(!arch.is_empty());
    }

    #[test]
    fn resolves_alias_and_custom_priority() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(dir.path());
        // 内蔵エイリアス
        assert_eq!(registry.resolve_alias("claude"), "claude-code-acp");
        // 未登録名はそのまま
        assert_eq!(registry.resolve_alias("unknown-agent"), "unknown-agent");

        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");
        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(registry.launch_spec("my-agent", &index, &["--extra".to_owned()]))
            .expect("custom spec");
        assert_eq!(spec.driver_kind, "acp");
        assert_eq!(spec.program, PathBuf::from("my-agent"));
        assert_eq!(spec.args, vec!["--flag", "--extra"]);
        assert_eq!(spec.env, vec![("KEY".to_owned(), "value".to_owned())]);
    }

    #[test]
    fn resolves_npx_specs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(dir.path());
        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");
        let agent = index.find("demo-npx").expect("demo-npx");

        let spec = registry
            .spec_for_source(agent, LocalSource::Npx, &[])
            .expect("npx spec");
        assert_eq!(spec.program, PathBuf::from("npx"));
        assert_eq!(spec.args, vec!["-y", "demo-agent@1.2.3", "--acp"]);
        assert_eq!(spec.env, vec![("DEMO".to_owned(), "1".to_owned())]);
    }

    #[test]
    fn unavailable_agent_reports_install_hint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(dir.path());
        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(registry.launch_spec("demo-bin", &index, &[]))
            .expect_err("must fail");
        assert!(
            err.to_string().contains("not available"),
            "unexpected: {err}"
        );
        assert!(err.to_string().contains("install"), "unexpected: {err}");
    }

    #[test]
    fn list_hides_unavailable_and_prioritizes_custom() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(dir.path());
        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");

        // 既定では利用可能なものだけが返る (npx の有無は環境依存なので個別には見ない)
        let available = registry.list(&index, false);
        assert!(
            available
                .iter()
                .any(|entry| entry.id == "my-agent" && entry.custom.is_some())
        );
        assert!(available.iter().all(|entry| entry.is_available()));
        assert!(!available.iter().any(|entry| entry.id == "demo-bin"));

        // 全件表示ではレジストリ分も含む
        let all = registry.list(&index, true);
        assert_eq!(all.len(), 3);
        let demo = all
            .iter()
            .find(|entry| entry.id == "demo-bin")
            .expect("demo-bin");
        assert!(!demo.is_available());
    }

    #[test]
    fn custom_store_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(dir.path());
        let config = CustomAgentConfig {
            name: Some("Store Agent".to_owned()),
            command: "node".to_owned(),
            args: vec!["agent.js".to_owned()],
            env: BTreeMap::new(),
        };
        registry
            .upsert_custom("store-agent", config)
            .expect("upsert custom");

        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");
        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(registry.launch_spec("store-agent", &index, &[]))
            .expect("custom spec");
        assert_eq!(spec.display_name, "Store Agent");
        assert_eq!(spec.program, PathBuf::from("node"));
        assert_eq!(spec.args, vec!["agent.js"]);

        assert!(registry.remove_custom("store-agent").expect("remove"));
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(registry.launch_spec("store-agent", &index, &[]))
            .expect_err("removed custom must fail");
        assert!(err.to_string().contains("unknown agent"));
    }

    #[test]
    fn custom_store_rejects_invalid_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(dir.path());
        let config = CustomAgentConfig {
            command: "node".to_owned(),
            ..CustomAgentConfig::default()
        };
        assert!(registry.upsert_custom("bad id", config.clone()).is_err());
        assert!(registry.upsert_custom("", config).is_err());
    }

    #[test]
    fn package_enablement_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(dir.path());
        assert!(registry.enabled_package_agents().is_empty());

        registry
            .set_package_enabled("demo-npx", true)
            .expect("enable");
        assert!(registry.enabled_package_agents().contains("demo-npx"));

        // 同じ値の再設定は no-op で壊れない
        registry
            .set_package_enabled("demo-npx", true)
            .expect("idempotent");
        assert_eq!(registry.enabled_package_agents().len(), 1);

        registry
            .set_package_enabled("demo-npx", false)
            .expect("disable");
        assert!(registry.enabled_package_agents().is_empty());
    }

    #[test]
    fn unknown_agent_reports_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(dir.path());
        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(registry.launch_spec("missing", &index, &[]))
            .expect_err("must fail");
        assert!(err.to_string().contains("unknown agent"));
    }

    #[test]
    fn extracts_tar_gz_archive() {
        let dir = tempfile::tempdir().expect("tempdir");
        // tar.gz を組み立てる
        let mut tar_bytes = Vec::new();
        {
            let encoder =
                flate2::write::GzEncoder::new(&mut tar_bytes, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            let data = b"#!/bin/sh\necho demo\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "bin/demo", &data[..])
                .expect("append");
            builder.finish().expect("finish");
            // GzEncoder は finish で内部 writer を閉じる
            let encoder = builder.into_inner().expect("into inner");
            encoder.finish().expect("gz finish");
        }
        let dest = dir.path().join("out");
        extract_archive_bytes("https://example.com/demo.tar.gz", &tar_bytes, &dest)
            .expect("extract");
        assert!(dest.join("bin/demo").is_file());
    }

    #[test]
    fn extracts_zip_archive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut zip_bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_bytes));
            writer
                .start_file("bin/demo.exe", zip::write::SimpleFileOptions::default())
                .expect("start_file");
            std::io::Write::write_all(&mut writer, b"demo").expect("write");
            writer.finish().expect("finish");
        }
        let dest = dir.path().join("out");
        extract_archive_bytes("https://example.com/demo.zip", &zip_bytes, &dest).expect("extract");
        assert!(dest.join("bin/demo.exe").is_file());
    }

    #[test]
    fn sha256_hex_formats_digest() {
        // 空文字列の SHA-256
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}

//! ACP Registry (`registry.json`) の取得・キャッシュ・起動解決と導入管理。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §2.1、
//! CLI コマンド: `docs/05-cli-and-pwa-ui.md` §1 (`fxg agents ...`)。
//!
//! - 公式インデックス (既定
//!   `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`)
//!   を `~/.flexagent/cache/registry.json` へキャッシュする
//!   (既定 TTL 24 時間。`fxg agents update` は強制更新)。
//! - `config.toml` の `[agents.custom.<id>]` はレジストリより優先して解決する。
//! - `distribution` (binary / npx / uvx) から現在の OS / Arch で起動可能な
//!   [`AgentLaunchSpec`] を解決する。binary は
//!   `~/.flexagent/agents/<id>/<version>/` へ展開して使用する
//!   (SHA-256 検証付き。未導入なら [`AcpRegistry::install`] が自動取得する)。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use fxg_protocol::config::{AgentsConfig, CustomAgentConfig, OpencodeMode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::driver::AgentLaunchSpec;

/// キャッシュファイル名 (`~/.flexagent/cache/registry.json`)。
pub const REGISTRY_CACHE_FILE: &str = "registry.json";

/// ビルトインエージェントID (レジストリ外。OpenCode2 ブリッジ/ACP)。
pub const OPENCODE2_ID: &str = "opencode2";

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

/// `fxg agents list` 表示用のエージェント情報。
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
    /// 導入済みか (binary 展開済み / npx・uvx / カスタム)
    pub installed: bool,
    /// 配布形態
    pub distributions: Vec<&'static str>,
    /// `config.toml` のカスタム定義か
    pub custom: bool,
    /// ビルトイン (`opencode2`) か
    pub builtin: bool,
}

/// ACP Registry 管理 (取得・キャッシュ・解決・導入)。
#[derive(Debug, Clone)]
pub struct AcpRegistry {
    home: PathBuf,
    registry_url: String,
    cache_ttl: Duration,
    custom: BTreeMap<String, CustomAgentConfig>,
    aliases: BTreeMap<String, String>,
    opencode_mode: OpencodeMode,
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
            opencode_mode: config.opencode_mode.unwrap_or(OpencodeMode::Bridge),
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

    /// `fxg agents list` 用の一覧を作る。
    ///
    /// `include_uninstalled = false` の場合は導入済み (`installed = true`) のみ返す。
    pub fn list(&self, index: &RegistryIndex, include_uninstalled: bool) -> Vec<AgentListEntry> {
        let mut entries = Vec::new();

        // ビルトイン (opencode2)
        entries.push(AgentListEntry {
            id: OPENCODE2_ID.to_owned(),
            name: "OpenCode2".to_owned(),
            version: "-".to_owned(),
            description: Some("opencode2 serve ブリッジ + 純正TUI Attach".to_owned()),
            installed: true,
            distributions: vec!["builtin"],
            custom: false,
            builtin: true,
        });

        // カスタム定義
        for (id, custom) in &self.custom {
            entries.push(AgentListEntry {
                id: id.clone(),
                name: custom.name.clone().unwrap_or_else(|| id.clone()),
                version: "-".to_owned(),
                description: None,
                installed: true,
                distributions: vec!["custom"],
                custom: true,
                builtin: false,
            });
        }

        // レジストリ
        for agent in &index.agents {
            let installed = self.is_installed(agent);
            if !installed && !include_uninstalled {
                continue;
            }
            entries.push(AgentListEntry {
                id: agent.id.clone(),
                name: agent.name.clone(),
                version: agent.version.clone(),
                description: agent.description.clone(),
                installed,
                distributions: agent.distribution.kinds(),
                custom: false,
                builtin: false,
            });
        }

        entries
    }

    /// binary 配布が導入済みか。
    pub fn is_installed(&self, agent: &RegistryAgent) -> bool {
        if agent.distribution.binary.is_empty() {
            return agent.distribution.npx.is_some() || agent.distribution.uvx.is_some();
        }
        match Self::binary_distribution(agent) {
            Ok(dist) => self
                .agent_version_dir(&agent.id, &agent.version)
                .join(clean_cmd(&dist.cmd))
                .is_file(),
            // 他プラットフォーム向け binary のみの場合は npx/uvx の有無で判定する
            Err(_) => agent.distribution.npx.is_some() || agent.distribution.uvx.is_some(),
        }
    }

    /// エージェントの起動スペックを解決する。
    ///
    /// binary 配布で未導入の場合は自動的にダウンロード・展開する
    /// (`fxg run antigravity` が初回に自動取得する挙動)。
    pub async fn launch_spec(
        &self,
        name: &str,
        index: &RegistryIndex,
        extra_args: &[String],
    ) -> anyhow::Result<AgentLaunchSpec> {
        let id = self.resolve_alias(name);

        // 1) ビルトイン: opencode2
        if id == OPENCODE2_ID {
            let (args, driver_kind) = match self.opencode_mode {
                OpencodeMode::Bridge => (vec!["serve".to_owned()], "opencode2"),
                OpencodeMode::Acp => (vec!["acp".to_owned()], "acp"),
            };
            let mut args = args;
            args.extend(extra_args.iter().cloned());
            return Ok(AgentLaunchSpec {
                agent_id: id,
                display_name: "OpenCode2".to_owned(),
                driver_kind: driver_kind.to_owned(),
                // v1 / v2 ともにコマンド名は `opencode` (v2 判定は起動時に実施)
                program: PathBuf::from("opencode"),
                args,
                env: Vec::new(),
            });
        }

        // 2) カスタム定義 (レジストリより優先)
        if let Some(custom) = self.custom.get(&id) {
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

        // 3) レジストリ
        let agent = index
            .find(&id)
            .ok_or_else(|| anyhow!("unknown agent: {name} (registry: {id})"))?;

        // 3a) binary (現在のプラットフォーム向けがある場合は優先)
        if let Ok(dist) = Self::binary_distribution(agent) {
            let dist = dist.clone();
            let cmd_path = self.install(&id, index).await?;
            let mut args = dist.args.clone();
            args.extend(extra_args.iter().cloned());
            return Ok(AgentLaunchSpec {
                agent_id: id,
                display_name: agent.name.clone(),
                driver_kind: "acp".to_owned(),
                program: cmd_path,
                args,
                env: dist
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            });
        }

        // 3b) npx
        if let Some(npx) = &agent.distribution.npx {
            let mut args = vec!["-y".to_owned(), npx.package.clone()];
            args.extend(npx.args.iter().cloned());
            args.extend(extra_args.iter().cloned());
            return Ok(AgentLaunchSpec {
                agent_id: id,
                display_name: agent.name.clone(),
                driver_kind: "acp".to_owned(),
                program: PathBuf::from("npx"),
                args,
                env: npx
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            });
        }

        // 3c) uvx
        if let Some(uvx) = &agent.distribution.uvx {
            let mut args = vec![uvx.package.clone()];
            args.extend(uvx.args.iter().cloned());
            args.extend(extra_args.iter().cloned());
            return Ok(AgentLaunchSpec {
                agent_id: id,
                display_name: agent.name.clone(),
                driver_kind: "acp".to_owned(),
                program: PathBuf::from("uvx"),
                args,
                env: uvx
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            });
        }

        bail!(
            "agent {id} has no distribution for {} (available: {:?})",
            Self::platform_key(),
            agent.distribution.kinds()
        )
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
    /// 既に展開済みならダウンロードせずにそのパスを返す。
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
    fn resolves_npx_and_opencode2_specs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(dir.path());
        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let spec = runtime
            .block_on(registry.launch_spec("demo-npx", &index, &[]))
            .expect("npx spec");
        assert_eq!(spec.program, PathBuf::from("npx"));
        assert_eq!(spec.args, vec!["-y", "demo-agent@1.2.3", "--acp"]);
        assert_eq!(spec.env, vec![("DEMO".to_owned(), "1".to_owned())]);

        let spec = runtime
            .block_on(registry.launch_spec("opencode", &index, &["--model".to_owned()]))
            .expect("opencode spec");
        assert_eq!(spec.driver_kind, "opencode2");
        assert_eq!(spec.program, PathBuf::from("opencode"));
        assert_eq!(spec.args, vec!["serve", "--model"]);

        // opencode_mode = acp の場合は `opencode acp`
        let mut config = config();
        config.opencode_mode = Some(OpencodeMode::Acp);
        let registry = AcpRegistry::new(dir.path(), &config);
        let spec = runtime
            .block_on(registry.launch_spec("opencode2", &index, &[]))
            .expect("acp spec");
        assert_eq!(spec.driver_kind, "acp");
        assert_eq!(spec.program, PathBuf::from("opencode"));
        assert_eq!(spec.args, vec!["acp"]);
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
    fn list_includes_builtin_custom_and_registry_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(dir.path());
        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");

        let installed_only = registry.list(&index, false);
        // builtin + custom + demo-npx (npx 配布はダウンロード不要のため導入済み扱い)
        assert_eq!(installed_only.len(), 3);
        assert!(installed_only.iter().any(|e| e.builtin));
        assert!(
            installed_only
                .iter()
                .any(|e| e.custom && e.id == "my-agent")
        );
        assert!(
            installed_only
                .iter()
                .any(|e| e.id == "demo-npx" && e.installed)
        );

        let all = registry.list(&index, true);
        assert_eq!(all.len(), 4);
        let demo = all.iter().find(|e| e.id == "demo-bin").expect("demo-bin");
        assert!(!demo.installed);
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

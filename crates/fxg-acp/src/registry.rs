//! Fetching, caching and launch resolution for the ACP Registry (`registry.json`),
//! plus custom agent management.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §2,
//! catalog commands: `docs/05-cli-and-pwa-ui.md` §1 (Web UI agent settings).
//!
//! - Caches the official index (default
//!   `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`)
//!   to `~/.flexagent/cache/registry.json`
//!   (default TTL 24 hours; the Web UI catalog refresh forces a refresh).
//! - Launch resolution is local-first: custom agent →
//!   existing install on `PATH` → `npx` / `uvx`.
//!   fxg itself never downloads or extracts binary archives
//!   (delegated to external package managers / the system PATH / custom definitions).
//! - Custom definitions and package enablement are owned by `node.db`; the
//!   registry only holds an in-memory snapshot applied by the daemon.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, anyhow};
use fxg_protocol::settings::{AgentSettings, CustomAgentConfig};
use serde::{Deserialize, Serialize};

use crate::driver::{AgentLaunchSpec, DriverKind};

/// Cache file name (`~/.flexagent/cache/registry.json`).
pub const REGISTRY_CACHE_FILE: &str = "registry.json";

/// In-memory catalog state owned by `node.db`.
#[derive(Debug, Clone, Default)]
pub struct AgentCatalogData {
    /// Registry URL (`None` = built-in default)
    pub registry_url: Option<String>,
    /// Registry cache TTL in seconds (`None` = built-in default)
    pub cache_ttl_secs: Option<u64>,
    /// Effective aliases (built-ins merged with the configured ones)
    pub aliases: BTreeMap<String, String>,
    /// Custom agent definitions
    pub custom: BTreeMap<String, CustomAgentConfig>,
    /// Agent IDs enabled through `npx` / `uvx`
    pub enabled_packages: BTreeSet<String>,
}

impl AgentCatalogData {
    /// Builds the catalog from the persisted settings.
    pub fn from_settings(
        settings: &AgentSettings,
        custom: BTreeMap<String, CustomAgentConfig>,
        enabled_packages: Vec<String>,
    ) -> Self {
        Self {
            registry_url: settings.registry_url.clone(),
            cache_ttl_secs: settings.registry_cache_ttl_secs,
            aliases: settings.resolved_aliases(),
            custom,
            enabled_packages: enabled_packages.into_iter().collect(),
        }
    }
}

/// `agents[].distribution`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RegistryDistribution {
    /// Binary distribution per OS/Arch key (`darwin-aarch64` / `windows-x86_64`, etc.)
    pub binary: BTreeMap<String, BinaryDistribution>,
    /// Distribution via `npx`
    pub npx: Option<PackageDistribution>,
    /// Distribution via `uvx`
    pub uvx: Option<PackageDistribution>,
}

impl RegistryDistribution {
    /// Names of the available distribution kinds (for display).
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

/// Binary distribution entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BinaryDistribution {
    /// Archive URL (`.tar.gz` / `.zip`)
    pub archive: String,
    /// Executable inside the archive (relative path; may have the `./name` form)
    pub cmd: String,
    /// Additional arguments at launch
    #[serde(default)]
    pub args: Vec<String>,
    /// Additional environment variables
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// SHA-256 of the archive (optional)
    #[serde(default)]
    pub sha256: Option<String>,
}

/// `npx` / `uvx` distribution entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageDistribution {
    /// Package specification (may include `name@version`)
    pub package: String,
    /// Additional arguments passed to the package
    #[serde(default)]
    pub args: Vec<String>,
    /// Additional environment variables
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Agent definition in the registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryAgent {
    /// Agent ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Version
    pub version: String,
    /// Description
    #[serde(default)]
    pub description: Option<String>,
    /// Repository URL
    #[serde(default)]
    pub repository: Option<String>,
    /// Website
    #[serde(default)]
    pub website: Option<String>,
    /// Authors
    #[serde(default)]
    pub authors: Vec<String>,
    /// License
    #[serde(default)]
    pub license: Option<String>,
    /// Icon URL
    #[serde(default)]
    pub icon: Option<String>,
    /// Distribution kinds
    #[serde(default)]
    pub distribution: RegistryDistribution,
}

/// The whole `registry.json` file.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct RegistryIndex {
    /// Index schema version
    #[serde(default)]
    pub version: String,
    /// Agent list
    #[serde(default)]
    pub agents: Vec<RegistryAgent>,
}

impl RegistryIndex {
    /// Looks up an agent by ID.
    pub fn find(&self, id: &str) -> Option<&RegistryAgent> {
        self.agents.iter().find(|agent| agent.id == id)
    }
}

/// Launch sources available on this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalSource {
    /// An executable already present on `PATH`
    External(PathBuf),
    /// Run via `npx` (fetched at run time)
    Npx,
    /// Run via `uvx` (fetched at run time)
    Uvx,
}

/// Agent information for the catalog display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentListEntry {
    /// Agent ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Version (`-` for custom agents)
    pub version: String,
    /// Description
    pub description: Option<String>,
    /// Whether it exists in the ACP Registry catalog
    pub registry: bool,
    /// Whether it can run via `npx` / `uvx` (present on PATH)
    pub package_available: bool,
    /// Launch sources available on this node (in priority order)
    pub sources: Vec<LocalSource>,
    /// Custom definition (config.toml / UI-managed)
    pub custom: Option<CustomAgentConfig>,
}

impl AgentListEntry {
    /// Whether it can be launched on this node.
    pub fn is_available(&self) -> bool {
        self.custom.is_some() || !self.sources.is_empty()
    }
}

/// A built-in launch preset for a well-known ACP CLI.
///
/// A preset is not a distribution channel: it only supplies the correct
/// arguments when the CLI is already installed on `PATH` (for example
/// `agy acp`). User custom definitions always take priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentPreset {
    /// Stable agent ID (used by `fxg run` and `AgentSummary.id`)
    pub id: &'static str,
    /// Display name
    pub display_name: &'static str,
    /// Executable name on `PATH` (resolved with `which` / `PATHEXT`)
    pub program: &'static str,
    /// Arguments that start the CLI's ACP server
    pub args: &'static [&'static str],
}

/// Built-in presets for well-known ACP CLIs.
///
/// Keep this list small and verified. Agents that are distributed through the
/// ACP Registry do not need a preset unless their registry `cmd` is not the
/// user-facing CLI (for example `antigravity-acp` expects a bundled
/// `agy_acp_server.par`, while the `agy` CLI exposes `agy acp`).
pub const AGENT_PRESETS: &[AgentPreset] = &[
    AgentPreset {
        id: "opencode",
        display_name: "OpenCode",
        program: "opencode",
        args: &["acp"],
    },
    AgentPreset {
        id: "agy",
        display_name: "Antigravity",
        program: "agy",
        args: &["acp"],
    },
];

/// Default preset lookup: resolves the executable name on `PATH`.
fn default_preset_lookup(preset: &AgentPreset) -> Option<PathBuf> {
    match which::which(preset.program) {
        Ok(path) => Some(path),
        Err(err) => {
            tracing::debug!(
                program = preset.program,
                "preset executable was not found on PATH: {err}"
            );
            None
        }
    }
}

/// ACP Registry management (fetch, cache, resolve, install).
#[derive(Debug, Clone)]
pub struct AcpRegistry {
    home: PathBuf,
    catalog: Arc<RwLock<AgentCatalogData>>,
    /// Preset executable lookup (injectable for tests)
    preset_lookup: fn(&AgentPreset) -> Option<PathBuf>,
    http: reqwest::Client,
}

impl AcpRegistry {
    /// Returns the built-in preset for an agent ID.
    fn preset(id: &str) -> Option<&'static AgentPreset> {
        AGENT_PRESETS.iter().find(|preset| preset.id == id)
    }

    /// Resolves a preset's executable on `PATH`.
    fn preset_path(&self, preset: &AgentPreset) -> Option<PathBuf> {
        (self.preset_lookup)(preset)
    }

    /// Builds a launch spec from a built-in preset without requiring the registry.
    pub fn preset_launch_spec(&self, name: &str, extra_args: &[String]) -> Option<AgentLaunchSpec> {
        let id = self.resolve_alias(name);
        let preset = Self::preset(&id)?;
        let path = self.preset_path(preset)?;
        Some(Self::preset_spec(preset, path, extra_args))
    }

    /// Builds a launch spec from a built-in preset.
    fn preset_spec(
        preset: &AgentPreset,
        program: PathBuf,
        extra_args: &[String],
    ) -> AgentLaunchSpec {
        let mut args = preset
            .args
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect::<Vec<_>>();
        args.extend(extra_args.iter().cloned());
        AgentLaunchSpec {
            agent_id: preset.id.to_owned(),
            display_name: preset.display_name.to_owned(),
            driver_kind: DriverKind::Acp,
            program,
            args,
            env: Vec::new(),
        }
    }

    /// Creates from `~/.flexagent` and the persisted agent catalog.
    pub fn new(
        home: impl Into<PathBuf>,
        settings: &AgentSettings,
        custom: BTreeMap<String, CustomAgentConfig>,
        enabled_packages: Vec<String>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("fxg/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            home: home.into(),
            catalog: Arc::new(RwLock::new(AgentCatalogData::from_settings(
                settings,
                custom,
                enabled_packages,
            ))),
            preset_lookup: default_preset_lookup,
            http,
        }
    }

    /// Replaces the in-memory catalog after a DB update.
    pub fn apply_catalog(
        &self,
        settings: &AgentSettings,
        custom: BTreeMap<String, CustomAgentConfig>,
        enabled_packages: Vec<String>,
    ) {
        let mut catalog = self.catalog.write().expect("agent catalog lock poisoned");
        *catalog = AgentCatalogData::from_settings(settings, custom, enabled_packages);
    }

    fn catalog(&self) -> std::sync::RwLockReadGuard<'_, AgentCatalogData> {
        self.catalog.read().expect("agent catalog lock poisoned")
    }

    /// Cache file path (`~/.flexagent/cache/registry.json`).
    pub fn cache_path(&self) -> PathBuf {
        self.home.join("cache").join(REGISTRY_CACHE_FILE)
    }

    /// Returns custom agent definitions from the DB catalog.
    pub fn custom_agents(&self) -> BTreeMap<String, CustomAgentConfig> {
        self.catalog().custom.clone()
    }

    /// Agent IDs enabled for use via `npx` / `uvx` distribution.
    pub fn enabled_package_agents(&self) -> BTreeSet<String> {
        self.catalog().enabled_packages.clone()
    }

    /// Runner that can launch the `npx` / `uvx` distribution (`None` if neither exists).
    pub fn package_runner(agent: &RegistryAgent) -> Option<&'static str> {
        if agent.distribution.npx.is_some() && which::which("npx").is_ok() {
            return Some("npx");
        }
        if agent.distribution.uvx.is_some() && which::which("uvx").is_ok() {
            return Some("uvx");
        }
        None
    }

    /// Current platform key (`darwin-aarch64` / `windows-x86_64`, etc.).
    pub fn platform_key() -> String {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        };
        format!("{os}-{}", std::env::consts::ARCH)
    }

    /// Resolves an agent name (alias / custom name) to the real agent ID.
    pub fn resolve_alias(&self, name: &str) -> String {
        self.catalog()
            .aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_owned())
    }

    /// Returns the registry index.
    ///
    /// Uses the cache while it is within the TTL; when expired or when `force_update`
    /// is set, fetches and refreshes the cache. If a fetch fails and only an expired
    /// cache exists, returns it as a fallback (offline use).
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
                // Offline fallback: use even an expired cache
                if let Some(stale) = self.read_cache(false) {
                    tracing::warn!("registry fetch failed ({err}); using stale cache");
                    Ok(stale)
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Fetches the registry (bypassing the cache).
    pub async fn fetch(&self) -> anyhow::Result<RegistryIndex> {
        let registry_url = self
            .catalog()
            .registry_url
            .clone()
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| AgentSettings::DEFAULT_REGISTRY_URL.to_owned());
        tracing::debug!(url = %registry_url, "fetching acp registry");
        let response = self
            .http
            .get(&registry_url)
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

    /// Reads the cache (`ttl_fresh` = only entries within the TTL).
    ///
    /// A missing or corrupt cache is a normal miss (logged at debug), not an
    /// error for the caller.
    fn read_cache(&self, ttl_fresh: bool) -> Option<RegistryIndex> {
        let path = self.cache_path();
        if ttl_fresh {
            let modified = match std::fs::metadata(&path) {
                Ok(metadata) => match metadata.modified() {
                    Ok(modified) => modified,
                    Err(err) => {
                        tracing::debug!(
                            path = %path.display(),
                            "registry cache has no modification time: {err}"
                        );
                        return None;
                    }
                },
                Err(err) => {
                    tracing::debug!(
                        path = %path.display(),
                        "registry cache is not readable: {err}"
                    );
                    return None;
                }
            };
            let age = match std::time::SystemTime::now().duration_since(modified) {
                Ok(age) => age,
                Err(err) => {
                    tracing::debug!(
                        path = %path.display(),
                        "registry cache mtime is in the future: {err}"
                    );
                    std::time::Duration::ZERO
                }
            };
            let ttl_secs = self
                .catalog()
                .cache_ttl_secs
                .unwrap_or(AgentSettings::DEFAULT_REGISTRY_CACHE_TTL_SECS);
            if age > Duration::from_secs(ttl_secs) {
                return None;
            }
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                tracing::debug!(
                    path = %path.display(),
                    "failed to read the registry cache: {err}"
                );
                return None;
            }
        };
        match serde_json::from_str(&text) {
            Ok(index) => Some(index),
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    "registry cache is corrupt; ignoring it: {err}"
                );
                None
            }
        }
    }

    /// Writes the cache.
    fn write_cache(&self, index: &RegistryIndex) -> anyhow::Result<()> {
        let path = self.cache_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(index)?;
        std::fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))
    }

    /// Builds the agent list.
    ///
    /// With `include_unavailable = false`, only agents launchable on this node are returned.
    pub fn list(&self, index: &RegistryIndex, include_unavailable: bool) -> Vec<AgentListEntry> {
        let mut entries = Vec::new();
        let custom = self.custom_agents();
        // `npx` / `uvx` are candidates only when explicitly enabled
        let enabled = self.enabled_package_agents();

        // Custom definitions (config.toml / UI-managed)
        for (id, config) in &custom {
            entries.push(AgentListEntry {
                id: id.clone(),
                name: config.name.clone().unwrap_or_else(|| id.clone()),
                version: "-".to_owned(),
                description: None,
                registry: index.find(id).is_some(),
                package_available: false,
                sources: Vec::new(),
                custom: Some(config.clone()),
            });
        }

        // Built-in presets (known CLI on PATH). Skip IDs already handled by a
        // custom definition or the registry to avoid duplicate rows.
        for preset in AGENT_PRESETS {
            if custom.contains_key(preset.id) || index.find(preset.id).is_some() {
                continue;
            }
            let Some(path) = self.preset_path(preset) else {
                continue;
            };
            entries.push(AgentListEntry {
                id: preset.id.to_owned(),
                name: preset.display_name.to_owned(),
                version: "-".to_owned(),
                description: Some("Known ACP CLI".to_owned()),
                registry: false,
                package_available: false,
                sources: vec![LocalSource::External(path)],
                custom: None,
            });
        }

        // Registry
        for agent in &index.agents {
            // Prefer a custom definition with the same name (same as launch resolution)
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
                package_available: Self::package_runner(agent).is_some(),
                sources,
                custom: None,
            });
        }

        entries
    }

    /// Returns the launch sources available on this node in priority order.
    ///
    /// When `include_package` is `false`, `npx` / `uvx` are excluded
    /// (for list display; launch resolution always allows them).
    fn local_sources(&self, agent: &RegistryAgent, include_package: bool) -> Vec<LocalSource> {
        let mut sources = Vec::new();
        if let Ok(dist) = Self::binary_distribution(agent) {
            let cmd = clean_cmd(&dist.cmd);
            // A `cmd` containing a directory is interpreted relative to the current
            // directory, so it is not searched on PATH.
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

    /// Resolves the launch spec for an agent.
    ///
    /// Resolution order: custom definition → built-in preset → existing install
    /// on `PATH` → `npx` → `uvx`.
    pub async fn launch_spec(
        &self,
        name: &str,
        index: &RegistryIndex,
        extra_args: &[String],
    ) -> anyhow::Result<AgentLaunchSpec> {
        let id = self.resolve_alias(name);

        // 1) Custom definition (takes priority over the registry)
        if let Some(custom) = self.custom_agents().remove(&id) {
            let mut args = custom.args.clone();
            args.extend(extra_args.iter().cloned());
            let mut env: Vec<(String, String)> = custom
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            // Resolve `env_secrets` from ~/.flexagent/secrets/node/<id>. A missing
            // value aborts the launch (never start with an empty token).
            for (key, secret_id) in &custom.env_secrets {
                let path = self
                    .home
                    .join("secrets")
                    .join("node")
                    .join(secret_id.as_str());
                let value = std::fs::read_to_string(&path).map_err(|err| {
                    anyhow!(
                        "secret {} referenced by {key} is missing ({}): {err}",
                        secret_id.as_str(),
                        path.display()
                    )
                })?;
                env.push((key.clone(), value.trim_end_matches(['\r', '\n']).to_owned()));
            }
            return Ok(AgentLaunchSpec {
                agent_id: id.clone(),
                display_name: custom.name.clone().unwrap_or_else(|| id.clone()),
                driver_kind: DriverKind::Acp,
                program: PathBuf::from(&custom.command),
                args,
                env,
            });
        }

        // 2) Built-in preset: a known CLI already installed on PATH.
        if let Some(spec) = self.preset_launch_spec(&id, extra_args) {
            return Ok(spec);
        }

        // 3) Registry (only sources usable on this node; `npx` / `uvx` do not require
        // explicit enablement)
        let agent = index
            .find(&id)
            .ok_or_else(|| anyhow!("unknown agent: {name} (registry: {id})"))?;
        let source = self
            .local_sources(agent, true)
            .into_iter()
            .next()
            .ok_or_else(|| {
            anyhow!(
                "agent {id} is not available on this node: put its binary on PATH or configure custom agent (distributions: {:?})",
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

    /// Builds a launch spec from a launch source.
    fn spec_for_source(
        &self,
        agent: &RegistryAgent,
        source: LocalSource,
        extra_args: &[String],
    ) -> Option<AgentLaunchSpec> {
        let (program, args, env) = match source {
            LocalSource::External(path) => {
                let dist = match Self::binary_distribution(agent) {
                    Ok(dist) => dist,
                    Err(err) => {
                        tracing::warn!(
                            agent_id = %agent.id,
                            "cannot launch external agent; its binary distribution is invalid: {err}"
                        );
                        return None;
                    }
                };
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
            driver_kind: DriverKind::Acp,
            program,
            args,
            env: env.into_iter().collect(),
        })
    }

    /// Returns the binary distribution for the current platform.
    fn binary_distribution(agent: &RegistryAgent) -> anyhow::Result<&BinaryDistribution> {
        let platform = AcpRegistry::platform_key();
        agent.distribution.binary.get(&platform).ok_or_else(|| {
            anyhow!(
                "agent {} has no binary distribution for {platform}",
                agent.id
            )
        })
    }
}

/// Normalizes a `cmd` in `./name` form to a relative path.
fn clean_cmd(cmd: &str) -> PathBuf {
    PathBuf::from(cmd.trim_start_matches("./"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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

    fn settings() -> AgentSettings {
        AgentSettings {
            default_agent: None,
            registry_url: None,
            registry_cache_ttl_secs: None,
            aliases: BTreeMap::new(),
            revision: 1,
            updated_at: 0,
        }
    }

    fn custom_agents() -> BTreeMap<String, CustomAgentConfig> {
        BTreeMap::from([(
            "my-agent".to_owned(),
            CustomAgentConfig {
                name: Some("My Agent".to_owned()),
                command: "my-agent".to_owned(),
                args: vec!["--flag".to_owned()],
                env: BTreeMap::from([("KEY".to_owned(), "value".to_owned())]),
                env_secrets: BTreeMap::new(),
            },
        )])
    }

    fn registry(dir: &Path) -> AcpRegistry {
        let mut registry = AcpRegistry::new(dir, &settings(), custom_agents(), Vec::new());
        // Keep tests independent of the host's PATH.
        registry.preset_lookup = |_| None;
        registry
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
        // Built-in alias
        assert_eq!(registry.resolve_alias("claude"), "claude-code-acp");
        assert_eq!(registry.resolve_alias("antigravity"), "agy");
        // Unknown names pass through unchanged
        assert_eq!(registry.resolve_alias("unknown-agent"), "unknown-agent");

        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");
        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(registry.launch_spec("my-agent", &index, &["--extra".to_owned()]))
            .expect("custom spec");
        assert_eq!(spec.driver_kind, DriverKind::Acp);
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
        assert!(err.to_string().contains("PATH"), "unexpected: {err}");
    }

    #[test]
    fn list_hides_unavailable_and_prioritizes_custom() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry(dir.path());
        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");

        // By default only available agents are returned (npx presence is environment-dependent,
        // so it is not asserted individually)
        let available = registry.list(&index, false);
        assert!(
            available
                .iter()
                .any(|entry| entry.id == "my-agent" && entry.custom.is_some())
        );
        assert!(available.iter().all(|entry| entry.is_available()));
        assert!(!available.iter().any(|entry| entry.id == "demo-bin"));

        // Listing all includes registry entries
        let all = registry.list(&index, true);
        assert_eq!(all.len(), 3);
        let demo = all
            .iter()
            .find(|entry| entry.id == "demo-bin")
            .expect("demo-bin");
        assert!(!demo.is_available());
    }

    #[tokio::test]
    async fn custom_agent_env_secrets_are_resolved_from_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("secrets/node")).expect("mkdir");
        std::fs::write(dir.path().join("secrets/node/key-1"), "secret-value\n").expect("write");
        let custom = BTreeMap::from([
            (
                "secret-agent".to_owned(),
                CustomAgentConfig {
                    name: Some("Secret Agent".to_owned()),
                    command: "node".to_owned(),
                    args: Vec::new(),
                    env: BTreeMap::from([("PLAIN".to_owned(), "value".to_owned())]),
                    env_secrets: BTreeMap::from([(
                        "API_KEY".to_owned(),
                        fxg_protocol::SecretId::new("key-1"),
                    )]),
                },
            ),
            (
                "missing-secret-agent".to_owned(),
                CustomAgentConfig {
                    command: "node".to_owned(),
                    env_secrets: BTreeMap::from([(
                        "API_KEY".to_owned(),
                        fxg_protocol::SecretId::new("missing"),
                    )]),
                    ..Default::default()
                },
            ),
        ]);
        let registry = AcpRegistry::new(dir.path(), &settings(), custom, Vec::new());
        let index = RegistryIndex::default();
        let spec = registry
            .launch_spec("secret-agent", &index, &[])
            .await
            .expect("spec");
        assert!(spec.env.contains(&("PLAIN".to_owned(), "value".to_owned())));
        assert!(
            spec.env
                .contains(&("API_KEY".to_owned(), "secret-value".to_owned()))
        );

        // A missing secret must abort the launch instead of starting with an empty value.
        let err = registry
            .launch_spec("missing-secret-agent", &index, &[])
            .await
            .expect_err("missing secret");
        assert!(
            err.to_string().contains("is missing"),
            "unexpected error: {err}"
        );
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
        assert!(
            err.to_string().contains("unknown agent"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn preset_definitions_are_unique_and_use_acp_arguments() {
        let mut ids = BTreeSet::new();
        for preset in AGENT_PRESETS {
            assert!(ids.insert(preset.id), "duplicate preset id: {}", preset.id);
            assert!(!preset.program.is_empty());
            assert_eq!(preset.args, ["acp"]);
        }
        assert!(AcpRegistry::preset("agy").is_some());
        assert!(AcpRegistry::preset("opencode").is_some());
    }

    #[test]
    fn builds_preset_spec_with_passthrough_arguments() {
        let preset = AcpRegistry::preset("agy").expect("agy preset");
        let spec = AcpRegistry::preset_spec(
            preset,
            PathBuf::from("/opt/agy/bin/agy"),
            &["--verbose".to_owned()],
        );
        assert_eq!(spec.agent_id, "agy");
        assert_eq!(spec.display_name, "Antigravity");
        assert_eq!(spec.driver_kind, DriverKind::Acp);
        assert_eq!(spec.program, PathBuf::from("/opt/agy/bin/agy"));
        assert_eq!(spec.args, vec!["acp", "--verbose"]);
        assert!(spec.env.is_empty());
    }

    #[test]
    fn resolves_preset_alias_to_installed_cli() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut registry = AcpRegistry::new(dir.path(), &settings(), custom_agents(), Vec::new());
        registry.preset_lookup =
            |preset| (preset.id == "agy").then(|| PathBuf::from("/fake/bin/agy"));
        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(registry.launch_spec("antigravity", &index, &[]))
            .expect("preset spec");
        assert_eq!(spec.agent_id, "agy");
        assert_eq!(spec.program, PathBuf::from("/fake/bin/agy"));
        assert_eq!(spec.args, vec!["acp"]);
    }

    #[test]
    fn lists_installed_presets_without_registry_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut registry = AcpRegistry::new(dir.path(), &settings(), custom_agents(), Vec::new());
        registry.preset_lookup =
            |preset| (preset.id == "agy").then(|| PathBuf::from("/fake/bin/agy"));
        let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");

        let entries = registry.list(&index, false);
        let agy = entries
            .iter()
            .find(|entry| entry.id == "agy")
            .expect("agy preset entry");
        assert!(!agy.registry);
        assert!(agy.is_available());
        assert_eq!(
            agy.sources,
            vec![LocalSource::External(PathBuf::from("/fake/bin/agy"))]
        );
    }
}

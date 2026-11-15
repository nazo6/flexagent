//! Fetching, caching and launch resolution for the ACP Registry (`registry.json`),
//! plus custom agent management.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §2,
//! catalog commands: `docs/05-cli-and-config.md` §1 (Web UI agent settings).
//!
//! - Caches the official index (default
//!   `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`)
//!   to `<cache>/node/registry.json`
//!   (default TTL 24 hours; the Web UI catalog refresh forces a refresh).
//! - Launch resolution is local-first: custom agent →
//!   built-in preset → registry import.
//!   fxg itself never downloads or extracts binary archives
//!   (delegated to external package managers / the system PATH / custom definitions).
//! - Custom definitions (including registry imports) are owned by `node.db`;
//!   the registry only holds an in-memory snapshot used by the import dialog.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, anyhow};
use fxg_protocol::client_api::{RegistryAgentSummary, RegistryDistributionKind};
use fxg_protocol::paths::{Paths, REGISTRY_CACHE_FILE_NAME};
use fxg_protocol::settings::{AgentSettings, CustomAgentConfig, CustomAgentOrigin};

use crate::driver::{AgentLaunchSpec, LaunchBackend};

mod types;

pub use types::*;

/// Registry fetch/cache settings (owned by the node settings).
#[derive(Debug, Clone, Default)]
struct RegistryOptions {
    /// Registry URL (`None` = built-in default)
    registry_url: Option<String>,
    /// Registry cache TTL in seconds (`None` = built-in default)
    cache_ttl_secs: Option<u64>,
}

/// ACP Registry client (fetch, cache, import).
///
/// Registry entries are not part of the runtime catalog: the catalog keeps the
/// custom definitions and hands the registry only the fetch settings.
#[derive(Debug, Clone)]
pub struct AcpRegistry {
    paths: Paths,
    options: Arc<RwLock<RegistryOptions>>,
    http: reqwest::Client,
}

impl AcpRegistry {
    /// Creates a client for the node's role paths.
    pub fn new(paths: Paths) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("fxg/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            paths,
            options: Arc::new(RwLock::new(RegistryOptions::default())),
            http,
        }
    }

    /// Applies the persisted registry settings (owned by `node.db`).
    pub fn configure(&self, registry_url: Option<String>, cache_ttl_secs: Option<u64>) {
        let mut options = self.options.write().expect("registry options poisoned");
        options.registry_url = registry_url;
        options.cache_ttl_secs = cache_ttl_secs;
    }

    fn options(&self) -> std::sync::RwLockReadGuard<'_, RegistryOptions> {
        self.options.read().expect("registry options poisoned")
    }

    /// Cache file path (`<cache>/node/registry.json`).
    pub fn cache_path(&self) -> PathBuf {
        self.paths.cache_dir().join(REGISTRY_CACHE_FILE_NAME)
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
            .options()
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
                .options()
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

    /// Builds an import preview for one registry entry.
    pub fn import_summary(&self, agent: &RegistryAgent, imported: bool) -> RegistryAgentSummary {
        let mut distribution = Vec::new();
        if !agent.distribution.binary.is_empty() {
            distribution.push(RegistryDistributionKind::Binary);
        }
        if agent.distribution.npx.is_some() {
            distribution.push(RegistryDistributionKind::Npx);
        }
        if agent.distribution.uvx.is_some() {
            distribution.push(RegistryDistributionKind::Uvx);
        }
        let (available, unavailable_reason) = match self.spec_for_registry_agent(agent, &[]) {
            Ok(_) => (true, None),
            Err(err) => (false, Some(err.to_string())),
        };
        RegistryAgentSummary {
            id: agent.id.clone(),
            name: agent.name.clone(),
            version: agent.version.clone(),
            description: agent.description.clone(),
            icon: agent.icon.clone(),
            distribution,
            available,
            unavailable_reason,
            imported,
        }
    }

    /// Converts a registry entry into a custom agent definition.
    ///
    /// The command is pinned to the distribution that is usable on this node
    /// (an executable on `PATH`, or `npx` / `uvx`).
    pub fn import_custom_config(
        &self,
        agent: &RegistryAgent,
        extension: Option<String>,
    ) -> anyhow::Result<CustomAgentConfig> {
        let spec = self.spec_for_registry_agent(agent, &[])?;
        let program = spec
            .program
            .ok_or_else(|| anyhow!("registry agent {} has no program", agent.id))?;
        Ok(CustomAgentConfig {
            name: Some(agent.name.clone()),
            command: program.to_string_lossy().into_owned(),
            args: spec.args,
            env: spec.env.into_iter().collect(),
            env_secrets: BTreeMap::new(),
            extension,
            description: agent.description.clone(),
            icon: agent.icon.clone(),
            capabilities: Default::default(),
            origin: Some(CustomAgentOrigin::Registry {
                id: agent.id.clone(),
                version: agent.version.clone(),
            }),
        })
    }

    /// Returns the launch spec for one registry entry (binary → npx → uvx).
    fn spec_for_registry_agent(
        &self,
        agent: &RegistryAgent,
        extra_args: &[String],
    ) -> anyhow::Result<AgentLaunchSpec> {
        let source = self
            .local_sources(agent, true)
            .into_iter()
            .next()
            .ok_or_else(|| {
                anyhow!(
                    "agent {} is not runnable on this node: put its binary on PATH or install npx / uvx (distributions: {:?})",
                    agent.id,
                    agent.distribution.kinds()
                )
            })?;
        self.spec_for_source(agent, source, extra_args)
            .ok_or_else(|| {
                anyhow!(
                    "agent {} has no usable distribution for {} (available: {:?})",
                    agent.id,
                    Self::platform_key(),
                    agent.distribution.kinds()
                )
            })
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
            backend: LaunchBackend::Acp(Default::default()),
            program: Some(program),
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
mod registry_tests;

//! Agent catalog: definitions, launch resolution and driver selection.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.1.
//!
//! The catalog owns the runtime agent state (custom definitions and aliases
//! from `node.db`) and the built-in agent definitions. `AcpRegistry` is only a
//! client for the ACP Registry (`registry.json`); it never owns catalog state.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::anyhow;
use fxg_protocol::client_api::AgentBackendKind;
use fxg_protocol::common::AgentUsageLimits;
use fxg_protocol::paths::Paths;
use fxg_protocol::settings::{
    AgentAcpCapabilities, AgentSettings, CustomAgentConfig, CustomAgentOrigin,
};

use self::builtin::{AgentAuth, AgentBackend, AgentDefinition, BuiltinAgent, LaunchPreset};
use crate::acp::AcpDriver;
use crate::acp::extensions::{AcpCapability, AgentExtension};
use crate::acp::registry::{AcpRegistry, AgentListEntry, LocalSource, RegistryIndex};
use crate::driver::{AcpLaunchOptions, AgentDriver, AgentLaunchSpec, LaunchBackend};

pub mod builtin;

/// In-memory catalog state owned by `node.db`.
#[derive(Debug, Clone, Default)]
pub struct AgentCatalogState {
    /// Registry URL (`None` = built-in default)
    pub registry_url: Option<String>,
    /// Registry cache TTL in seconds (`None` = built-in default)
    pub cache_ttl_secs: Option<u64>,
    /// Effective aliases (built-ins merged with the configured ones)
    pub aliases: BTreeMap<String, String>,
    /// Custom agent definitions
    pub custom: BTreeMap<String, CustomAgentConfig>,
}

impl AgentCatalogState {
    /// Builds the catalog state from the persisted settings.
    pub fn from_settings(
        settings: &AgentSettings,
        custom: BTreeMap<String, CustomAgentConfig>,
    ) -> Self {
        Self {
            registry_url: settings.registry_url.clone(),
            cache_ttl_secs: settings.registry_cache_ttl_secs,
            aliases: settings.aliases.clone(),
            custom,
        }
    }
}

/// Agent catalog used by the node session manager and the CLI.
pub struct AgentCatalog {
    paths: Paths,
    /// Runtime state owned by `node.db` (aliases, custom definitions, registry settings)
    state: RwLock<AgentCatalogState>,
    /// Client for the ACP Registry (`registry.json`)
    registry: AcpRegistry,
    /// Built-in agent definitions keyed by their stable ID
    agents: BTreeMap<String, BuiltinAgent>,
    /// Built-in extensions keyed by their stable extension ID.
    extensions: BTreeMap<String, Arc<dyn AgentExtension>>,
    acp: Arc<AcpDriver>,
    /// Tier 2 drivers (one per extension), cached so the driver identity stays
    /// stable per agent.
    extended: Mutex<BTreeMap<String, Arc<dyn AgentDriver>>>,
    /// Preset executable lookup (injectable for tests).
    preset_lookup: fn(&LaunchPreset) -> Option<PathBuf>,
}

impl AgentCatalog {
    /// Creates a catalog with the built-in agents.
    pub fn new(
        paths: Paths,
        settings: &AgentSettings,
        custom: BTreeMap<String, CustomAgentConfig>,
    ) -> Self {
        Self::with_agents(paths, settings, custom, builtin::agents())
    }

    /// Creates a catalog with an explicit built-in agent list (tests / plugins).
    pub fn with_agents(
        paths: Paths,
        settings: &AgentSettings,
        custom: BTreeMap<String, CustomAgentConfig>,
        agents: Vec<BuiltinAgent>,
    ) -> Self {
        let registry = AcpRegistry::new(paths.clone());
        registry.configure(
            settings.registry_url.clone(),
            settings.registry_cache_ttl_secs,
        );

        let mut extensions = BTreeMap::new();
        let mut agent_map = BTreeMap::new();
        for agent in agents {
            if let AgentBackend::AcpExtended { id, extension } = agent.backend() {
                extensions.insert((*id).to_owned(), Arc::clone(extension));
            }
            agent_map.insert(agent.definition().id.clone(), agent);
        }

        Self {
            paths,
            state: RwLock::new(AgentCatalogState::from_settings(settings, custom)),
            registry,
            agents: agent_map,
            extensions,
            acp: Arc::new(AcpDriver::new()),
            extended: Mutex::new(BTreeMap::new()),
            preset_lookup: default_preset_lookup,
        }
    }

    fn state(&self) -> std::sync::RwLockReadGuard<'_, AgentCatalogState> {
        self.state.read().expect("agent catalog lock poisoned")
    }

    /// Returns a built-in extension by ID.
    pub fn extension(&self, id: &str) -> Option<Arc<dyn AgentExtension>> {
        self.extensions.get(id).cloned()
    }

    /// Whether the ID is owned by a full custom driver.
    ///
    /// Such IDs are reserved: a user custom definition cannot replace the
    /// built-in driver implementation.
    pub fn is_driver_backed(&self, id: &str) -> bool {
        self.agents
            .get(id)
            .is_some_and(|agent| matches!(agent.backend(), AgentBackend::Custom(_)))
    }

    /// Returns the agent-native management integration, if the agent has one.
    ///
    /// Custom definitions always resolve through ACP and have no management
    /// integration, even when their ID matches a built-in agent.
    pub fn management(
        &self,
        agent_id: &str,
    ) -> Option<Arc<dyn crate::management::AgentManagement>> {
        if self.custom_agent(agent_id).is_some() {
            return None;
        }
        self.agents.get(agent_id)?.management()
    }

    /// Returns the native authentication surface of a built-in agent.
    ///
    /// Custom definitions resolve through ACP, so they keep using
    /// `initialize` instead of an agent-declared command.
    pub fn native_auth(&self, agent_id: &str) -> Option<&AgentAuth> {
        if self.custom_agent(agent_id).is_some() {
            return None;
        }
        self.agents.get(agent_id)?.auth()
    }

    /// The underlying ACP Registry client (catalog fetch / cache / import).
    pub fn registry(&self) -> &AcpRegistry {
        &self.registry
    }

    /// Returns the registry index (cached unless `force_update`).
    pub async fn index(&self, force_update: bool) -> anyhow::Result<RegistryIndex> {
        self.registry.index(force_update).await
    }

    /// Custom agent definitions owned by `node.db`.
    pub fn custom_agents(&self) -> BTreeMap<String, CustomAgentConfig> {
        self.state().custom.clone()
    }

    /// Returns one custom agent definition.
    pub fn custom_agent(&self, id: &str) -> Option<CustomAgentConfig> {
        self.state().custom.get(id).cloned()
    }

    /// Resolves an agent alias to the real agent ID.
    pub fn resolve_alias(&self, name: &str) -> String {
        self.state()
            .aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_owned())
    }

    /// Replaces the in-memory catalog after a DB update.
    pub fn apply_catalog(
        &self,
        settings: &AgentSettings,
        custom: BTreeMap<String, CustomAgentConfig>,
    ) {
        {
            let mut state = self.state.write().expect("agent catalog lock poisoned");
            *state = AgentCatalogState::from_settings(settings, custom);
        }
        self.registry.configure(
            settings.registry_url.clone(),
            settings.registry_cache_ttl_secs,
        );
    }

    /// Builds the catalog list in display order: custom → built-in presets.
    ///
    /// Registry entries are *not* part of the runtime catalog; they are only
    /// import candidates exposed through [`AcpRegistry::import_summary`].
    pub fn list(&self) -> Vec<AgentListEntry> {
        let mut entries = self.custom_entries();
        let overridden: BTreeSet<String> = entries.iter().map(|entry| entry.id.clone()).collect();

        for agent in self.agents.values() {
            let definition = agent.definition();
            if overridden.contains(&definition.id) {
                continue;
            }
            let backend = agent.backend();
            let is_custom = matches!(backend, AgentBackend::Custom(_));
            let preset_program = agent
                .preset()
                .and_then(|preset| (self.preset_lookup)(preset));

            let mut sources = Vec::new();
            if let Some(program) = preset_program {
                sources.push(LocalSource::External(program));
            } else if !is_custom {
                // Built-in ACP agents need their CLI on PATH.
                continue;
            }

            entries.push(AgentListEntry {
                id: definition.id.clone(),
                name: definition.name.clone(),
                version: "-".to_owned(),
                description: definition.description.clone(),
                sources,
                custom: None,
                extension: backend.extension_id().map(str::to_owned),
                capabilities: if is_custom {
                    AgentAcpCapabilities { subagents: false }
                } else {
                    AgentAcpCapabilities::default()
                },
                backend: backend.kind(),
                has_management: agent.management().is_some(),
                icon: definition.icon.clone(),
            });
        }

        // Merge extension metadata into custom and built-in entries.
        for entry in &mut entries {
            if let Some(custom) = &entry.custom {
                let Some(extension_id) = custom.extension.as_deref() else {
                    continue;
                };
                let Some(extension) = self.extensions.get(extension_id) else {
                    continue;
                };
                entry.backend = AgentBackendKind::AcpExtended;
                let metadata = extension.metadata();
                if custom.name.is_none()
                    && let Some(name) = metadata.name
                {
                    entry.name = name;
                }
                if metadata.description.is_some() {
                    entry.description = metadata.description;
                }
                if metadata.icon.is_some() {
                    entry.icon = metadata.icon;
                }
                continue;
            }
            let Some(agent) = self.agents.get(&entry.id) else {
                continue;
            };
            match agent.backend() {
                AgentBackend::AcpExtended { extension, .. } => {
                    entry.backend = AgentBackendKind::AcpExtended;
                    let metadata = extension.metadata();
                    if let Some(name) = metadata.name {
                        entry.name = name;
                    }
                    if metadata.description.is_some() {
                        entry.description = metadata.description;
                    }
                    if metadata.icon.is_some() {
                        entry.icon = metadata.icon;
                    }
                }
                AgentBackend::Custom(_) => {
                    entry.backend = AgentBackendKind::Custom;
                    let definition = agent.definition();
                    entry.name = definition.name.clone();
                    entry.description = definition.description.clone();
                    entry.icon = definition.icon.clone();
                }
                AgentBackend::Acp => {}
            }
        }
        entries
    }

    /// Resolves the launch spec for an agent.
    ///
    /// Order: custom definition → full custom driver → built-in preset.
    /// Registry entries are not launched directly; they must be imported as
    /// custom agents first.
    pub async fn launch_spec(
        &self,
        name: &str,
        _cwd: &Path,
        extra_args: &[String],
    ) -> anyhow::Result<AgentLaunchSpec> {
        let id = self.resolve_alias(name);

        if let Some(custom) = self.custom_agent(&id) {
            let Some(mut spec) = self.custom_launch_spec(&id, extra_args)? else {
                return Err(anyhow!("custom agent {id} disappeared while resolving"));
            };
            let capabilities = custom.capabilities.clone();
            if let Some(extension_id) = custom.extension.as_deref() {
                let extension = self.extension(extension_id).ok_or_else(|| {
                    anyhow!("custom agent {id} references an unknown extension: {extension_id}")
                })?;
                spec.backend = LaunchBackend::Acp(AcpLaunchOptions {
                    extension: Some(extension_id.to_owned()),
                    options: extension.acp_options(&capabilities),
                });
                if custom.name.is_none()
                    && let Some(name) = extension.metadata().name
                {
                    spec.display_name = name;
                }
            }
            Self::apply_capabilities(&mut spec, capabilities);
            return Ok(spec);
        }

        let Some(agent) = self.agents.get(&id) else {
            return Err(anyhow!(
                "unknown agent: {id} (import it from the ACP Registry in the agent settings)"
            ));
        };
        let backend = agent.backend();
        if matches!(backend, AgentBackend::Custom(_)) {
            return Ok(self.custom_driver_spec(agent, extra_args));
        }

        let preset = agent
            .preset()
            .ok_or_else(|| anyhow!("agent {id} has no launch preset"))?;
        let program = (self.preset_lookup)(preset).ok_or_else(|| {
            anyhow!(
                "agent {id} is not installed: put {} on PATH",
                preset.program
            )
        })?;
        let capabilities = AgentAcpCapabilities::default();
        let mut spec = Self::preset_spec(agent.definition(), preset, program, extra_args);
        if let AgentBackend::AcpExtended {
            id: extension_id,
            extension,
        } = backend
        {
            spec.backend = LaunchBackend::Acp(AcpLaunchOptions {
                extension: Some((*extension_id).to_owned()),
                options: extension.acp_options(&capabilities),
            });
            if let Some(name) = extension.metadata().name {
                spec.display_name = name;
            }
        } else {
            Self::apply_capabilities(&mut spec, capabilities);
        }
        Ok(spec)
    }

    /// Applies the per-agent ACP capability selection to a launch spec.
    ///
    /// Only capabilities the setting knows about are touched, so capabilities
    /// contributed by an extension survive untouched.
    fn apply_capabilities(spec: &mut AgentLaunchSpec, capabilities: AgentAcpCapabilities) {
        let LaunchBackend::Acp(options) = &mut spec.backend else {
            return;
        };
        if capabilities.subagents {
            options
                .options
                .capabilities
                .insert(AcpCapability::Subagents);
        } else {
            options
                .options
                .capabilities
                .remove(&AcpCapability::Subagents);
        }
    }

    /// Selects the driver for a resolved launch spec.
    pub fn driver_for(&self, spec: &AgentLaunchSpec) -> anyhow::Result<Arc<dyn AgentDriver>> {
        match &spec.backend {
            LaunchBackend::Acp(options) => match options.extension.as_deref() {
                Some(extension_id) => self.extended_driver(extension_id),
                None => Ok(Arc::clone(&self.acp) as Arc<dyn AgentDriver>),
            },
            LaunchBackend::Custom => self.custom_driver(&spec.agent_id),
            LaunchBackend::Other(other) => Err(anyhow!("unknown launch backend: {other}")),
        }
    }

    /// Resolves an agent and reads its usage limits (best-effort).
    ///
    /// The driver decides whether the agent exposes a quota surface; `None`
    /// means "not available" and should not be shown as an error.
    pub async fn usage_limits(
        &self,
        agent_id: &str,
        cwd: &Path,
    ) -> anyhow::Result<Option<AgentUsageLimits>> {
        let spec = self.launch_spec(agent_id, cwd, &[]).await?;
        let driver = self.driver_for(&spec)?;
        driver.usage_limits(&spec, cwd).await
    }

    /// Returns the built-in driver of a full custom agent.
    fn custom_driver(&self, agent_id: &str) -> anyhow::Result<Arc<dyn AgentDriver>> {
        let agent = self
            .agents
            .get(agent_id)
            .ok_or_else(|| anyhow!("no built-in agent is registered as {agent_id}"))?;
        let AgentBackend::Custom(driver) = agent.backend() else {
            return Err(anyhow!("agent {agent_id} is not a full custom driver"));
        };
        Ok(Arc::clone(driver))
    }

    /// Builds the launch spec for a full custom driver.
    ///
    /// The driver owns the actual launch; the spec only carries the optional
    /// preset command and pass-through arguments.
    fn custom_driver_spec(&self, agent: &BuiltinAgent, extra_args: &[String]) -> AgentLaunchSpec {
        let definition = agent.definition();
        let mut spec = AgentLaunchSpec {
            agent_id: definition.id.clone(),
            display_name: definition.name.clone(),
            backend: LaunchBackend::Custom,
            program: None,
            args: extra_args.to_vec(),
            env: Vec::new(),
        };
        if let Some(preset) = agent.preset()
            && let Some(program) = (self.preset_lookup)(preset)
        {
            let mut args = preset
                .args
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect::<Vec<_>>();
            args.extend(extra_args.iter().cloned());
            spec.program = Some(program);
            spec.args = args;
        }
        spec
    }

    /// Returns the cached Tier 2 driver for an extension.
    fn extended_driver(&self, extension_id: &str) -> anyhow::Result<Arc<dyn AgentDriver>> {
        let mut cache = self
            .extended
            .lock()
            .expect("extended driver cache poisoned");
        if let Some(driver) = cache.get(extension_id) {
            return Ok(Arc::clone(driver));
        }

        let extension = self
            .extensions
            .get(extension_id)
            .ok_or_else(|| anyhow!("no extension is registered as {extension_id}"))?;
        let driver: Arc<dyn AgentDriver> =
            Arc::new(AcpDriver::with_extension(Arc::clone(extension)));
        cache.insert(extension_id.to_owned(), Arc::clone(&driver));
        Ok(driver)
    }

    /// Builds a launch spec from a built-in preset.
    fn preset_spec(
        definition: &AgentDefinition,
        preset: &LaunchPreset,
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
            agent_id: definition.id.clone(),
            display_name: definition.name.clone(),
            backend: LaunchBackend::Acp(AcpLaunchOptions::default()),
            program: Some(program),
            args,
            env: Vec::new(),
        }
    }

    /// Lists custom agent definitions from `node.db`.
    fn custom_entries(&self) -> Vec<AgentListEntry> {
        self.custom_agents()
            .into_iter()
            .map(|(id, config)| {
                let version = match &config.origin {
                    Some(CustomAgentOrigin::Registry { version, .. }) => version.clone(),
                    _ => "-".to_owned(),
                };
                let extension = config.extension.clone();
                let capabilities = config.capabilities.clone();
                AgentListEntry {
                    name: config.name.clone().unwrap_or_else(|| id.clone()),
                    backend: AgentBackendKind::Acp,
                    has_management: false,
                    icon: config.icon.clone(),
                    id,
                    version,
                    description: config.description.clone(),
                    sources: Vec::new(),
                    custom: Some(config),
                    extension,
                    capabilities,
                }
            })
            .collect()
    }

    /// Builds a launch spec from a custom definition (`None` when not defined).
    fn custom_launch_spec(
        &self,
        id: &str,
        extra_args: &[String],
    ) -> anyhow::Result<Option<AgentLaunchSpec>> {
        let Some(custom) = self.custom_agent(id) else {
            return Ok(None);
        };
        let mut args = custom.args.clone();
        args.extend(extra_args.iter().cloned());
        let mut env: Vec<(String, String)> = custom
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // Resolve `env_secrets` from `<data>/node/secrets/<id>`. A missing
        // value aborts the launch (never start with an empty token).
        for (key, secret_id) in &custom.env_secrets {
            let path = self.paths.secrets_dir().join(secret_id.as_str());
            let value = std::fs::read_to_string(&path).map_err(|err| {
                anyhow!(
                    "secret {} referenced by {key} is missing ({}): {err}",
                    secret_id.as_str(),
                    path.display()
                )
            })?;
            env.push((key.clone(), value.trim_end_matches(['\r', '\n']).to_owned()));
        }
        Ok(Some(AgentLaunchSpec {
            agent_id: id.to_owned(),
            display_name: custom.name.clone().unwrap_or_else(|| id.to_owned()),
            backend: LaunchBackend::Acp(AcpLaunchOptions::default()),
            program: Some(PathBuf::from(&custom.command)),
            args,
            env,
        }))
    }
}

/// Default preset lookup: resolves the executable name on `PATH`.
fn default_preset_lookup(preset: &LaunchPreset) -> Option<PathBuf> {
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

#[cfg(test)]
mod catalog_tests;

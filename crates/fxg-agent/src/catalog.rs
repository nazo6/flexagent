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
use fxg_protocol::paths::Paths;
use fxg_protocol::settings::{
    AgentAcpCapabilities, AgentSettings, CustomAgentConfig, CustomAgentOrigin,
};

use crate::acp::AcpDriver;
use crate::builtin::{self, AgentAuth, AgentBackend, AgentDefinition, BuiltinAgent, LaunchPreset};
use crate::driver::{AcpLaunchOptions, AgentDriver, AgentLaunchSpec, LaunchBackend};
use crate::extension::{AcpCapability, AgentExtension};
use crate::registry::{AcpRegistry, AgentListEntry, LocalSource, RegistryIndex};

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
mod tests {
    use super::*;
    use crate::driver::{DriverEvent, DriverKind, StartSessionRequest, StartedSession};
    use crate::extension::{AgentExtension, AgentMetadata};
    use fxg_protocol::client_api::AgentBackendKind;
    use fxg_protocol::paths::{Paths, Role};
    use fxg_protocol::settings::CustomAgentConfig;
    use tokio::sync::mpsc;

    fn settings() -> AgentSettings {
        AgentSettings {
            registry_url: None,
            registry_cache_ttl_secs: None,
            aliases: BTreeMap::new(),
            revision: 1,
            updated_at: 0,
        }
    }

    fn paths(dir: &std::path::Path) -> Paths {
        Paths::portable(dir, Role::Node)
    }

    struct DummyExtension;

    struct DummyDriver;

    #[async_trait::async_trait]
    impl AgentDriver for DummyDriver {
        fn driver_kind(&self) -> DriverKind {
            DriverKind::Custom
        }

        async fn start_session(
            &self,
            _req: StartSessionRequest,
            _event_tx: mpsc::UnboundedSender<DriverEvent>,
        ) -> anyhow::Result<StartedSession> {
            anyhow::bail!("DummyDriver is not started in catalog tests")
        }
    }

    impl AgentExtension for DummyExtension {
        fn metadata(&self) -> AgentMetadata {
            AgentMetadata {
                name: Some("Extended (enriched)".to_owned()),
                description: None,
                icon: Some("builtin:extended".to_owned()),
            }
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
                extension: None,
                ..Default::default()
            },
        )])
    }

    fn catalog(dir: &std::path::Path) -> AgentCatalog {
        let mut catalog = AgentCatalog::new(paths(dir), &settings(), custom_agents());
        // Keep tests independent of the host's PATH.
        catalog.preset_lookup = |_| None;
        catalog
    }

    fn catalog_with_agents(dir: &std::path::Path, agents: Vec<BuiltinAgent>) -> AgentCatalog {
        let mut catalog =
            AgentCatalog::with_agents(paths(dir), &settings(), BTreeMap::new(), agents);
        catalog.preset_lookup =
            |preset| Some(PathBuf::from(format!("/fake/bin/{}", preset.program)));
        catalog
    }

    fn demo_agent(id: &str, name: &str, program: &'static str) -> BuiltinAgent {
        BuiltinAgent::new(
            AgentDefinition {
                id: id.to_owned(),
                name: name.to_owned(),
                description: None,
                icon: None,
            },
            Some(LaunchPreset {
                program,
                args: &["acp"],
            }),
        )
    }

    #[test]
    fn custom_definitions_take_priority() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = catalog(dir.path());
        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("my-agent", Path::new("."), &["--extra".to_owned()]))
            .expect("custom spec");
        assert_eq!(spec.backend, LaunchBackend::Acp(Default::default()));
        assert_eq!(spec.program, Some(PathBuf::from("my-agent")));
        assert_eq!(spec.args, vec!["--flag", "--extra"]);
        assert_eq!(spec.env, vec![("KEY".to_owned(), "value".to_owned())]);
    }

    #[test]
    fn resolves_alias_and_custom_priority() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut settings = settings();
        settings
            .aliases
            .insert("fast".to_owned(), "my-agent".to_owned());
        let catalog = AgentCatalog::new(paths(dir.path()), &settings, custom_agents());
        // User-defined alias
        assert_eq!(catalog.resolve_alias("fast"), "my-agent");
        // Removed built-in aliases pass through unchanged
        assert_eq!(catalog.resolve_alias("antigravity"), "antigravity");
        // Unknown names pass through unchanged
        assert_eq!(catalog.resolve_alias("unknown-agent"), "unknown-agent");
    }

    #[test]
    fn preset_spec_includes_passthrough_arguments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = catalog_with_agents(dir.path(), vec![demo_agent("demo", "Demo", "demo")]);

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("demo", Path::new("."), &["--verbose".to_owned()]))
            .expect("preset spec");
        assert_eq!(spec.agent_id, "demo");
        assert_eq!(spec.display_name, "Demo");
        assert_eq!(spec.program, Some(PathBuf::from("/fake/bin/demo")));
        assert_eq!(spec.args, vec!["acp", "--verbose"]);
        assert!(spec.env.is_empty());

        let entries = catalog.list();
        let entry = entries
            .iter()
            .find(|entry| entry.id == "demo")
            .expect("demo");
        assert_eq!(entry.backend, AgentBackendKind::Acp);
    }

    #[test]
    fn per_agent_capabilities_apply_to_the_launch_spec() {
        let dir = tempfile::tempdir().expect("tempdir");
        let custom = BTreeMap::from([(
            "demo".to_owned(),
            CustomAgentConfig {
                command: "demo".to_owned(),
                capabilities: AgentAcpCapabilities { subagents: false },
                ..Default::default()
            },
        )]);
        let mut catalog = AgentCatalog::with_agents(
            paths(dir.path()),
            &settings(),
            custom,
            vec![demo_agent("demo", "Demo", "demo")],
        );
        catalog.preset_lookup = |_| Some(PathBuf::from("/fake/bin/demo"));

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("demo", Path::new("."), &[]))
            .expect("spec");
        assert!(
            !spec
                .acp_options()
                .expect("acp options")
                .capabilities
                .contains(&AcpCapability::Subagents)
        );

        let entry = catalog
            .list()
            .into_iter()
            .find(|entry| entry.id == "demo")
            .expect("demo entry");
        assert!(!entry.capabilities.subagents);
        assert_eq!(entry.extension, None);
    }

    #[test]
    fn agents_declare_their_extension_id_explicitly() {
        struct SharedExtension;

        impl AgentExtension for SharedExtension {}

        let dir = tempfile::tempdir().expect("tempdir");
        let agent = demo_agent("demo", "Demo", "demo")
            .with_extension("shared-extension", Arc::new(SharedExtension));
        let catalog = catalog_with_agents(dir.path(), vec![agent]);

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("demo", Path::new("."), &[]))
            .expect("spec");
        assert_eq!(spec.extension_id(), Some("shared-extension"));
        assert!(catalog.extension("shared-extension").is_some());

        let entry = catalog
            .list()
            .into_iter()
            .find(|entry| entry.id == "demo")
            .expect("demo entry");
        assert_eq!(entry.extension.as_deref(), Some("shared-extension"));
    }

    #[test]
    fn subagents_toggle_gates_vendor_child_notifications() {
        use crate::opencode::OpenCodeExtension;

        let dir = tempfile::tempdir().expect("tempdir");
        let agent = demo_agent(builtin::OPENCODE_ACP, "OpenCode", "opencode")
            .with_extension(builtin::OPENCODE_EXTENSION, Arc::new(OpenCodeExtension));
        let on_custom = BTreeMap::from([(
            "opencode".to_owned(),
            CustomAgentConfig {
                command: "opencode".to_owned(),
                extension: Some(builtin::OPENCODE_EXTENSION.to_owned()),
                capabilities: AgentAcpCapabilities { subagents: true },
                ..Default::default()
            },
        )]);
        let mut on =
            AgentCatalog::with_agents(paths(dir.path()), &settings(), on_custom, vec![agent]);
        on.preset_lookup = |_| Some(PathBuf::from("/fake/bin/opencode"));
        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(on.launch_spec("opencode", Path::new("."), &[]))
            .expect("spec");
        let options = spec.acp_options().expect("acp options");
        assert!(options.capabilities.contains(&AcpCapability::Subagents));
        assert!(options.client_meta.is_some());
        assert!(
            options
                .notifications
                .iter()
                .any(|method| method == "opencode/session/child_update")
        );

        let off_custom = BTreeMap::from([(
            "opencode".to_owned(),
            CustomAgentConfig {
                command: "opencode".to_owned(),
                extension: Some(builtin::OPENCODE_EXTENSION.to_owned()),
                capabilities: AgentAcpCapabilities { subagents: false },
                ..Default::default()
            },
        )]);
        let agent = demo_agent(builtin::OPENCODE_ACP, "OpenCode", "opencode")
            .with_extension(builtin::OPENCODE_EXTENSION, Arc::new(OpenCodeExtension));
        let mut off =
            AgentCatalog::with_agents(paths(dir.path()), &settings(), off_custom, vec![agent]);
        off.preset_lookup = |_| Some(PathBuf::from("/fake/bin/opencode"));
        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(off.launch_spec("opencode", Path::new("."), &[]))
            .expect("spec");
        let options = spec.acp_options().expect("acp options");
        assert!(!options.capabilities.contains(&AcpCapability::Subagents));
        assert!(options.client_meta.is_none());
        assert!(options.notifications.is_empty());
    }

    #[test]
    fn custom_definition_shadows_a_builtin_preset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let custom = BTreeMap::from([(
            "my-agent".to_owned(),
            CustomAgentConfig {
                name: Some("My Agent".to_owned()),
                command: "my-agent".to_owned(),
                ..Default::default()
            },
        )]);
        let mut catalog = AgentCatalog::with_agents(
            paths(dir.path()),
            &settings(),
            custom,
            vec![demo_agent("my-agent", "Builtin", "builtin")],
        );
        catalog.preset_lookup = |_| Some(PathBuf::from("/fake/bin/builtin"));

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("my-agent", Path::new("."), &[]))
            .expect("custom");
        assert_eq!(spec.display_name, "My Agent");
        assert_eq!(spec.program, Some(PathBuf::from("my-agent")));

        let entries = catalog.list();
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.id == "my-agent")
                .count(),
            1
        );
        assert!(entries[0].custom.is_some());
    }

    #[test]
    fn driver_selection_uses_the_shared_acp_driver() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = catalog(dir.path());
        let spec = AgentLaunchSpec {
            agent_id: "my-agent".to_owned(),
            display_name: "My Agent".to_owned(),
            backend: LaunchBackend::Acp(Default::default()),
            program: Some(PathBuf::from("my-agent")),
            args: Vec::new(),
            env: Vec::new(),
        };
        let first = catalog.driver_for(&spec).expect("driver");
        let second = catalog.driver_for(&spec).expect("driver");
        assert!(Arc::ptr_eq(&first, &second));

        let mut other = spec;
        other.backend = LaunchBackend::Other("nope".to_owned());
        assert!(catalog.driver_for(&other).is_err());
    }

    #[test]
    fn extended_agent_selects_and_caches_the_driver() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = demo_agent("extended", "Extended", "extended")
            .with_extension("extended", Arc::new(DummyExtension));
        let catalog = catalog_with_agents(dir.path(), vec![agent]);

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("extended", Path::new("."), &[]))
            .expect("extended spec");
        assert_eq!(spec.extension_id(), Some("extended"));
        assert_eq!(spec.display_name, "Extended (enriched)");
        assert_eq!(spec.args, vec!["acp"]);

        let driver = catalog.driver_for(&spec).expect("extended driver");
        assert_eq!(driver.driver_kind(), DriverKind::AcpExtended);
        let second = catalog.driver_for(&spec).expect("extended driver");
        assert!(Arc::ptr_eq(&driver, &second));

        let entry = catalog
            .list()
            .into_iter()
            .find(|entry| entry.id == "extended")
            .expect("extended entry");
        assert_eq!(entry.backend, AgentBackendKind::AcpExtended);
        assert_eq!(entry.name, "Extended (enriched)");
        assert_eq!(entry.icon.as_deref(), Some("builtin:extended"));
    }

    #[test]
    fn custom_backend_is_available_without_a_program() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = BuiltinAgent::new(
            AgentDefinition {
                id: "custom".to_owned(),
                name: "Custom".to_owned(),
                description: Some("Built-in custom driver".to_owned()),
                icon: None,
            },
            None,
        )
        .with_backend(AgentBackend::Custom(Arc::new(DummyDriver)));
        let catalog = catalog_with_agents(dir.path(), vec![agent]);

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("custom", Path::new("."), &["--verbose".to_owned()]))
            .expect("custom spec");
        assert_eq!(spec.backend, LaunchBackend::Custom);
        assert!(spec.program.is_none());
        assert_eq!(spec.args, vec!["--verbose"]);

        let driver = catalog.driver_for(&spec).expect("custom driver");
        assert_eq!(driver.driver_kind(), DriverKind::Custom);

        let entry = catalog
            .list()
            .into_iter()
            .find(|entry| entry.id == "custom")
            .expect("custom entry");
        assert_eq!(entry.backend, AgentBackendKind::Custom);
        assert!(entry.is_available());
        assert!(entry.sources.is_empty());
        assert!(catalog.is_driver_backed("custom"));
        assert!(!catalog.is_driver_backed("my-agent"));
    }

    #[test]
    fn custom_backend_uses_its_preset_when_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = BuiltinAgent::new(
            AgentDefinition {
                id: "custom".to_owned(),
                name: "Custom".to_owned(),
                description: None,
                icon: None,
            },
            Some(LaunchPreset {
                program: "custom-cli",
                args: &["serve"],
            }),
        )
        .with_backend(AgentBackend::Custom(Arc::new(DummyDriver)));
        let catalog = catalog_with_agents(dir.path(), vec![agent]);

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("custom", Path::new("."), &["--verbose".to_owned()]))
            .expect("custom spec");
        assert_eq!(spec.backend, LaunchBackend::Custom);
        assert_eq!(spec.program, Some(PathBuf::from("/fake/bin/custom-cli")));
        assert_eq!(spec.args, vec!["serve", "--verbose"]);
    }

    #[test]
    fn custom_definition_can_reuse_a_builtin_extension() {
        let dir = tempfile::tempdir().expect("tempdir");
        let custom = BTreeMap::from([(
            "my-opencode".to_owned(),
            CustomAgentConfig {
                name: Some("My OpenCode".to_owned()),
                command: "opencode".to_owned(),
                args: vec!["acp".to_owned()],
                env: BTreeMap::new(),
                env_secrets: BTreeMap::new(),
                extension: Some(builtin::OPENCODE_EXTENSION.to_owned()),
                ..Default::default()
            },
        )]);
        let catalog = AgentCatalog::new(paths(dir.path()), &settings(), custom);

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("my-opencode", Path::new("."), &[]))
            .expect("custom + extension");
        assert_eq!(spec.extension_id(), Some(builtin::OPENCODE_EXTENSION));
        assert_eq!(spec.display_name, "My OpenCode");

        let driver = catalog.driver_for(&spec).expect("extended driver");
        assert_eq!(driver.driver_kind(), DriverKind::AcpExtended);

        let entry = catalog
            .list()
            .into_iter()
            .find(|entry| entry.id == "my-opencode")
            .expect("custom entry");
        assert_eq!(entry.backend, AgentBackendKind::AcpExtended);
        assert_eq!(entry.name, "My OpenCode");
        assert_eq!(entry.icon.as_deref(), Some("builtin:opencode"));
    }

    #[test]
    fn custom_definition_unknown_extension_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let custom = BTreeMap::from([(
            "broken".to_owned(),
            CustomAgentConfig {
                command: "opencode".to_owned(),
                args: vec!["acp".to_owned()],
                extension: Some("missing".to_owned()),
                ..Default::default()
            },
        )]);
        let catalog = AgentCatalog::new(paths(dir.path()), &settings(), custom);

        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("broken", Path::new("."), &[]))
            .expect_err("unknown extension");
        assert!(
            err.to_string().contains("unknown extension"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn custom_entries_list_definitions_with_registry_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = catalog(dir.path());

        let custom = catalog.custom_entries();
        assert_eq!(custom.len(), 1);
        assert_eq!(custom[0].id, "my-agent");
        assert!(custom[0].custom.is_some());
        assert!(custom[0].is_available());
        assert_eq!(custom[0].version, "-");
    }

    #[test]
    fn native_management_is_registered_only_for_the_native_agent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = catalog(dir.path());
        let management = catalog
            .management(builtin::OPENCODE)
            .expect("native management");
        assert_eq!(management.agent_id(), builtin::OPENCODE);
        assert!(
            catalog.management(builtin::OPENCODE_ACP).is_none(),
            "the ACP fallback has no out-of-session management"
        );
        assert!(catalog.management("unknown").is_none());
    }

    #[test]
    fn native_auth_is_available_only_for_the_builtin_driver() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = catalog(dir.path());
        let auth = catalog.native_auth(builtin::OPENCODE).expect("native auth");
        assert_eq!(auth.methods.len(), 1);
        assert_eq!(auth.methods[0].id, "opencode-login");
        assert!(
            catalog.native_auth(builtin::OPENCODE_ACP).is_none(),
            "the ACP fallback discovers auth through initialize"
        );

        // A custom definition shadows the builtin and must keep using ACP.
        let mut custom = custom_agents();
        custom.insert(
            builtin::OPENCODE.to_owned(),
            CustomAgentConfig {
                command: "opencode".to_owned(),
                ..Default::default()
            },
        );
        let shadowed = AgentCatalog::new(paths(dir.path()), &settings(), custom);
        assert!(shadowed.native_auth(builtin::OPENCODE).is_none());
    }

    #[test]
    fn builtin_agents_are_unique_and_use_acp_arguments() {
        let agents = builtin::agents();
        let mut ids = BTreeSet::new();
        for agent in &agents {
            let definition = agent.definition();
            assert!(ids.insert(definition.id.clone()));
            assert!(!definition.name.is_empty());
            let preset = agent.preset().expect("preset");
            assert!(!preset.program.is_empty());
            assert!(!preset.args.is_empty());
        }
        assert!(ids.contains(builtin::OPENCODE));
        assert!(ids.contains(builtin::AGY));
    }

    #[tokio::test]
    async fn custom_agent_env_secrets_are_resolved_from_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths(dir.path());
        std::fs::create_dir_all(paths.secrets_dir()).expect("mkdir");
        std::fs::write(paths.secrets_dir().join("key-1"), "secret-value\n").expect("write");
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
                    extension: None,
                    ..Default::default()
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
        let catalog = AgentCatalog::new(paths, &settings(), custom);
        let spec = catalog
            .launch_spec("secret-agent", Path::new("."), &[])
            .await
            .expect("spec");
        assert!(spec.env.contains(&("PLAIN".to_owned(), "value".to_owned())));
        assert!(
            spec.env
                .contains(&("API_KEY".to_owned(), "secret-value".to_owned()))
        );

        // A missing secret must abort the launch instead of starting with an empty value.
        let err = catalog
            .launch_spec("missing-secret-agent", Path::new("."), &[])
            .await
            .expect_err("missing secret");
        assert!(
            err.to_string().contains("is missing"),
            "unexpected error: {err}"
        );
    }
}

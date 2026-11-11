//! Agent catalog: definitions, launch resolution and driver selection.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.1 (planned in
//! `docs/changelog/2026-11-11-agent-extension-architecture.md`).
//!
//! The catalog is the only place that knows about agent-specific launch
//! definitions. `SessionManager` depends on it instead of on `AcpRegistry` and
//! a separate driver factory.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use fxg_protocol::client_api::AgentBackendKind;
use fxg_protocol::settings::{AgentSettings, CustomAgentConfig};

use super::builtin;
use super::extended::ExtendedAcpDriver;
use super::extension::AgentExtension;
use super::provider::{AgentBackend, AgentDefinition, AgentProvider, LaunchPreset};
use crate::acp::AcpDriver;
use crate::driver::{AgentDriver, AgentLaunchSpec, DriverKind};
use crate::registry::{AcpRegistry, AgentListEntry, LocalSource, RegistryIndex};

/// Agent catalog used by the node session manager and the CLI.
pub struct AgentCatalog {
    registry: AcpRegistry,
    providers: BTreeMap<String, Arc<dyn AgentProvider>>,
    /// Built-in extensions keyed by their stable extension ID.
    extensions: BTreeMap<String, Arc<dyn AgentExtension>>,
    acp: Arc<dyn AgentDriver>,
    /// Tier 2 wrappers, cached so the driver identity stays stable per agent.
    extended: Mutex<BTreeMap<String, Arc<dyn AgentDriver>>>,
    /// Preset executable lookup (injectable for tests).
    preset_lookup: fn(&LaunchPreset) -> Option<PathBuf>,
}

impl AgentCatalog {
    /// Creates a catalog with the built-in providers.
    pub fn new(registry: AcpRegistry) -> Self {
        Self::with_providers(registry, builtin::providers())
    }

    /// Creates a catalog with an explicit provider list (tests / future plugins).
    pub fn with_providers(registry: AcpRegistry, providers: Vec<Arc<dyn AgentProvider>>) -> Self {
        let mut extensions = BTreeMap::new();
        let mut provider_map = BTreeMap::new();
        for provider in providers {
            if let AgentBackend::AcpExtended(extension) = provider.backend() {
                extensions.insert(provider.definition().id.clone(), extension);
            }
            provider_map.insert(provider.definition().id.clone(), provider);
        }
        Self {
            registry,
            providers: provider_map,
            extensions,
            acp: Arc::new(AcpDriver::new()),
            extended: Mutex::new(BTreeMap::new()),
            preset_lookup: default_preset_lookup,
        }
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
        self.providers
            .get(id)
            .is_some_and(|provider| matches!(provider.backend(), AgentBackend::Custom(_)))
    }

    /// The underlying ACP Registry (catalog fetch / cache).
    pub fn registry(&self) -> &AcpRegistry {
        &self.registry
    }

    /// Returns the registry index (cached unless `force_update`).
    pub async fn index(&self, force_update: bool) -> anyhow::Result<RegistryIndex> {
        self.registry.index(force_update).await
    }

    /// Custom agent definitions owned by `node.db`.
    pub fn custom_agents(&self) -> BTreeMap<String, CustomAgentConfig> {
        self.registry.custom_agents()
    }

    /// Agent IDs enabled for use via the `npx` / `uvx` distribution.
    pub fn enabled_package_agents(&self) -> BTreeSet<String> {
        self.registry.enabled_package_agents()
    }

    /// Resolves an agent alias to the real agent ID.
    pub fn resolve_alias(&self, name: &str) -> String {
        self.registry.resolve_alias(name)
    }

    /// Replaces the in-memory catalog after a DB update.
    pub fn apply_catalog(
        &self,
        settings: &AgentSettings,
        custom: BTreeMap<String, CustomAgentConfig>,
        enabled_packages: Vec<String>,
    ) {
        self.registry
            .apply_catalog(settings, custom, enabled_packages);
    }

    /// Builds the catalog list in display order: custom → built-in presets →
    /// ACP Registry.
    pub fn list(&self, index: &RegistryIndex, include_unavailable: bool) -> Vec<AgentListEntry> {
        let mut entries = self.registry.custom_entries(index);
        let overridden: BTreeSet<String> = entries.iter().map(|entry| entry.id.clone()).collect();
        // Full custom providers own their catalog entry and launch path; the
        // registry entry for the same ID is not used.
        let mut custom_backends = BTreeSet::new();
        // Providers whose native preset is available replace the registry row
        // (the registry recipe often only knows `npx` for the same ID).
        let mut preset_backed = BTreeSet::new();

        for provider in self.providers.values() {
            let definition = provider.definition();
            if overridden.contains(&definition.id) {
                continue;
            }
            let backend = provider.backend();
            let is_custom = matches!(&backend, AgentBackend::Custom(_));
            if is_custom {
                custom_backends.insert(definition.id.clone());
            }

            let registry_agent = index.find(&definition.id);
            let preset_program = provider
                .launch_preset()
                .and_then(|preset| (self.preset_lookup)(preset));
            if !is_custom && registry_agent.is_some() && preset_program.is_none() {
                // Keep the registry entry (it may be available via npx/uvx).
                continue;
            }

            let mut sources = Vec::new();
            if let Some(program) = preset_program {
                sources.push(LocalSource::External(program));
            } else if !is_custom {
                // ACP providers need a preset when the registry has no entry.
                continue;
            }

            if !is_custom && registry_agent.is_some() {
                preset_backed.insert(definition.id.clone());
            }

            entries.push(AgentListEntry {
                id: definition.id.clone(),
                name: definition.name.clone(),
                version: registry_agent
                    .map(|agent| agent.version.clone())
                    .unwrap_or_else(|| "-".to_owned()),
                description: registry_agent
                    .and_then(|agent| agent.description.clone())
                    .or_else(|| definition.description.clone()),
                registry: registry_agent.is_some(),
                package_available: false,
                sources,
                custom: None,
                backend: backend.kind(),
                icon: definition.icon.clone(),
            });
        }

        entries.extend(
            self.registry
                .registry_entries(index, include_unavailable)
                .into_iter()
                .filter(|entry| {
                    !custom_backends.contains(&entry.id) && !preset_backed.contains(&entry.id)
                }),
        );

        // Merge Tier 2 metadata into preset / registry / custom entries.
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
            let Some(provider) = self.providers.get(&entry.id) else {
                continue;
            };
            match provider.backend() {
                AgentBackend::AcpExtended(extension) => {
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
                    let definition = provider.definition();
                    entry.name = definition.name.clone();
                    entry.description = definition.description.clone();
                    entry.icon = definition.icon.clone();
                }
                AgentBackend::Acp => {
                    // Prefer the built-in display name and bundled icon when
                    // the registry entry has the same ID. The registry still
                    // supplies the version and (richer) description.
                    let definition = provider.definition();
                    entry.name = definition.name.clone();
                    if definition.icon.is_some() {
                        entry.icon = definition.icon.clone();
                    }
                }
            }
        }
        entries
    }

    /// Resolves the launch spec for an agent.
    ///
    /// Order: custom definition → full custom provider → built-in preset →
    /// ACP Registry.
    pub async fn launch_spec(
        &self,
        name: &str,
        index: &RegistryIndex,
        cwd: &Path,
        extra_args: &[String],
    ) -> anyhow::Result<AgentLaunchSpec> {
        let id = self.registry.resolve_alias(name);

        if let Some(custom) = self.registry.custom_agent(&id) {
            let Some(mut spec) = self.registry.custom_launch_spec(&id, extra_args)? else {
                return Err(anyhow!("custom agent {id} disappeared while resolving"));
            };
            if let Some(extension_id) = custom.extension.as_deref() {
                let extension = self.extension(extension_id).ok_or_else(|| {
                    anyhow!("custom agent {id} references an unknown extension: {extension_id}")
                })?;
                spec.driver_kind = DriverKind::AcpExtended;
                spec.extension = Some(extension_id.to_owned());
                extension.prepare_launch(cwd, &mut spec)?;
                if custom.name.is_none()
                    && let Some(name) = extension.metadata().name
                {
                    spec.display_name = name;
                }
            }
            return Ok(spec);
        }

        let provider = self.providers.get(&id);
        let backend = provider.map(|provider| provider.backend());
        if let Some(provider) = provider
            && matches!(&backend, Some(AgentBackend::Custom(_)))
        {
            return Ok(self.custom_driver_spec(provider.as_ref(), extra_args));
        }
        let driver_kind = backend.as_ref().map(Self::driver_kind);

        let mut spec = if let Some(provider) = provider
            && let Some(preset) = provider.launch_preset()
            && let Some(program) = (self.preset_lookup)(preset)
        {
            Self::preset_spec(provider.definition(), preset, program, extra_args)
        } else {
            self.registry
                .registry_launch_spec(&id, index, extra_args)
                .await?
        };

        if let Some(driver_kind) = driver_kind {
            spec.driver_kind = driver_kind;
        }
        if let Some(AgentBackend::AcpExtended(extension)) = &backend {
            spec.extension = Some(id.clone());
            extension.prepare_launch(cwd, &mut spec)?;
            if let Some(name) = extension.metadata().name {
                spec.display_name = name;
            }
        }
        Ok(spec)
    }

    /// Selects the driver for a resolved launch spec.
    pub fn driver_for(&self, spec: &AgentLaunchSpec) -> anyhow::Result<Arc<dyn AgentDriver>> {
        match &spec.driver_kind {
            DriverKind::Acp => Ok(Arc::clone(&self.acp)),
            DriverKind::AcpExtended => {
                let extension_id = spec.extension.as_deref().unwrap_or(&spec.agent_id);
                self.extended_driver(extension_id)
            }
            DriverKind::Custom => self.custom_driver(&spec.agent_id),
            DriverKind::Other(other) => Err(anyhow!("unknown driver kind: {other}")),
        }
    }

    /// Maps a provider backend to a driver kind.
    fn driver_kind(backend: &AgentBackend) -> DriverKind {
        match backend {
            AgentBackend::Acp => DriverKind::Acp,
            AgentBackend::AcpExtended(_) => DriverKind::AcpExtended,
            AgentBackend::Custom(_) => DriverKind::Custom,
        }
    }

    /// Returns the built-in driver of a full custom provider.
    fn custom_driver(&self, agent_id: &str) -> anyhow::Result<Arc<dyn AgentDriver>> {
        let provider = self
            .providers
            .get(agent_id)
            .ok_or_else(|| anyhow!("no provider is registered for agent {agent_id}"))?;
        let AgentBackend::Custom(driver) = provider.backend() else {
            return Err(anyhow!("agent {agent_id} is not a full custom driver"));
        };
        Ok(driver)
    }

    /// Builds the launch spec for a full custom driver.
    ///
    /// The driver owns the actual launch; the spec only carries the optional
    /// preset command and pass-through arguments.
    fn custom_driver_spec(
        &self,
        provider: &dyn AgentProvider,
        extra_args: &[String],
    ) -> AgentLaunchSpec {
        let definition = provider.definition();
        let mut spec = AgentLaunchSpec {
            agent_id: definition.id.clone(),
            display_name: definition.name.clone(),
            driver_kind: DriverKind::Custom,
            extension: None,
            program: None,
            args: extra_args.to_vec(),
            env: Vec::new(),
        };
        if let Some(preset) = provider.launch_preset()
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

    /// Returns the cached Tier 2 wrapper for an extension.
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
        let driver: Arc<dyn AgentDriver> = Arc::new(ExtendedAcpDriver::new(
            Arc::clone(&self.acp),
            Arc::clone(extension),
        ));
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
            driver_kind: DriverKind::Acp,
            extension: None,
            program: Some(program),
            args,
            env: Vec::new(),
        }
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
    use crate::agent::extension::{AgentExtension, AgentMetadata};
    use crate::agent::provider::StaticProvider;
    use crate::driver::{DriverEvent, StartSessionRequest, StartedSession};
    use crate::registry::{PackageDistribution, RegistryAgent, RegistryDistribution};
    use fxg_protocol::client_api::AgentBackendKind;
    use fxg_protocol::settings::CustomAgentConfig;
    use tokio::sync::mpsc;

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

    #[async_trait::async_trait]
    impl AgentExtension for DummyExtension {
        fn metadata(&self) -> AgentMetadata {
            AgentMetadata {
                name: Some("Extended (enriched)".to_owned()),
                description: None,
                icon: Some("builtin:extended".to_owned()),
            }
        }

        fn prepare_launch(&self, cwd: &Path, launch: &mut AgentLaunchSpec) -> anyhow::Result<()> {
            launch.args.push(format!("--cwd={}", cwd.display()));
            Ok(())
        }
    }

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
                extension: None,
            },
        )])
    }

    fn registry(dir: &std::path::Path) -> AcpRegistry {
        AcpRegistry::new(dir, &settings(), custom_agents(), Vec::new())
    }

    fn catalog(dir: &std::path::Path) -> AgentCatalog {
        let mut catalog = AgentCatalog::new(registry(dir));
        // Keep tests independent of the host's PATH.
        catalog.preset_lookup = |_| None;
        catalog
    }

    fn index() -> RegistryIndex {
        RegistryIndex::default()
    }

    #[test]
    fn custom_definitions_take_priority() {
        let dir = tempfile::tempdir().expect("tempdir");
        let catalog = catalog(dir.path());
        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec(
                "my-agent",
                &index(),
                Path::new("."),
                &["--extra".to_owned()],
            ))
            .expect("custom spec");
        assert_eq!(spec.driver_kind, DriverKind::Acp);
        assert_eq!(spec.program, Some(PathBuf::from("my-agent")));
        assert_eq!(spec.args, vec!["--flag", "--extra"]);
        assert_eq!(spec.env, vec![("KEY".to_owned(), "value".to_owned())]);
    }

    #[test]
    fn preset_spec_includes_passthrough_arguments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider = StaticProvider::new(
            AgentDefinition {
                id: "demo".to_owned(),
                name: "Demo".to_owned(),
                description: None,
                icon: Some("builtin:demo".to_owned()),
            },
            Some(LaunchPreset {
                program: "demo",
                args: &["acp"],
            }),
        );
        let mut catalog =
            AgentCatalog::with_providers(registry(dir.path()), vec![Arc::new(provider)]);
        catalog.preset_lookup = |_| Some(PathBuf::from("/fake/bin/demo"));

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec(
                "demo",
                &index(),
                Path::new("."),
                &["--verbose".to_owned()],
            ))
            .expect("preset spec");
        assert_eq!(spec.agent_id, "demo");
        assert_eq!(spec.display_name, "Demo");
        assert_eq!(spec.program, Some(PathBuf::from("/fake/bin/demo")));
        assert_eq!(spec.args, vec!["acp", "--verbose"]);
        assert!(spec.env.is_empty());

        let entries = catalog.list(&index(), false);
        let entry = entries
            .iter()
            .find(|entry| entry.id == "demo")
            .expect("demo");
        assert_eq!(entry.backend, AgentBackendKind::Acp);
        assert_eq!(entry.icon.as_deref(), Some("builtin:demo"));
        assert!(!entry.registry);
    }

    #[test]
    fn custom_definition_shadows_a_builtin_preset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider = StaticProvider::new(
            AgentDefinition {
                id: "my-agent".to_owned(),
                name: "Builtin".to_owned(),
                description: None,
                icon: None,
            },
            Some(LaunchPreset {
                program: "builtin",
                args: &["acp"],
            }),
        );
        let mut catalog =
            AgentCatalog::with_providers(registry(dir.path()), vec![Arc::new(provider)]);
        catalog.preset_lookup = |_| Some(PathBuf::from("/fake/bin/builtin"));

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("my-agent", &index(), Path::new("."), &[]))
            .expect("custom");
        assert_eq!(spec.display_name, "My Agent");
        assert_eq!(spec.program, Some(PathBuf::from("my-agent")));

        let entries = catalog.list(&index(), false);
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
            driver_kind: DriverKind::Acp,
            extension: None,
            program: Some(PathBuf::from("my-agent")),
            args: Vec::new(),
            env: Vec::new(),
        };
        let first = catalog.driver_for(&spec).expect("driver");
        let second = catalog.driver_for(&spec).expect("driver");
        assert!(Arc::ptr_eq(&first, &second));

        let mut other = spec;
        other.driver_kind = DriverKind::Other("nope".to_owned());
        assert!(catalog.driver_for(&other).is_err());
    }

    #[test]
    fn extended_provider_selects_and_caches_the_wrapper() {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider = StaticProvider::new(
            AgentDefinition {
                id: "extended".to_owned(),
                name: "Extended".to_owned(),
                description: None,
                icon: None,
            },
            Some(LaunchPreset {
                program: "extended",
                args: &["acp"],
            }),
        )
        .with_backend(AgentBackend::AcpExtended(Arc::new(DummyExtension)));
        let mut catalog =
            AgentCatalog::with_providers(registry(dir.path()), vec![Arc::new(provider)]);
        catalog.preset_lookup = |_| Some(PathBuf::from("/fake/bin/extended"));

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("extended", &index(), Path::new("."), &[]))
            .expect("extended spec");
        assert_eq!(spec.driver_kind, DriverKind::AcpExtended);
        assert_eq!(spec.display_name, "Extended (enriched)");
        assert_eq!(spec.args, vec!["acp", "--cwd=."]);

        let driver = catalog.driver_for(&spec).expect("extended driver");
        assert_eq!(driver.driver_kind(), DriverKind::AcpExtended);
        let second = catalog.driver_for(&spec).expect("extended driver");
        assert!(Arc::ptr_eq(&driver, &second));

        let entry = catalog
            .list(&index(), false)
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
        let provider = StaticProvider::new(
            AgentDefinition {
                id: "custom".to_owned(),
                name: "Custom".to_owned(),
                description: Some("Built-in custom driver".to_owned()),
                icon: None,
            },
            None,
        )
        .with_backend(AgentBackend::Custom(Arc::new(DummyDriver)));
        let catalog = AgentCatalog::with_providers(registry(dir.path()), vec![Arc::new(provider)]);

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec(
                "custom",
                &index(),
                Path::new("."),
                &["--verbose".to_owned()],
            ))
            .expect("custom spec");
        assert_eq!(spec.driver_kind, DriverKind::Custom);
        assert!(spec.program.is_none());
        assert_eq!(spec.args, vec!["--verbose"]);

        let driver = catalog.driver_for(&spec).expect("custom driver");
        assert_eq!(driver.driver_kind(), DriverKind::Custom);

        let entry = catalog
            .list(&index(), false)
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
        let provider = StaticProvider::new(
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
        let mut catalog =
            AgentCatalog::with_providers(registry(dir.path()), vec![Arc::new(provider)]);
        catalog.preset_lookup = |_| Some(PathBuf::from("/fake/bin/custom-cli"));

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec(
                "custom",
                &index(),
                Path::new("."),
                &["--verbose".to_owned()],
            ))
            .expect("custom spec");
        assert_eq!(spec.driver_kind, DriverKind::Custom);
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
                extension: Some("opencode".to_owned()),
            },
        )]);
        let catalog = AgentCatalog::new(AcpRegistry::new(
            dir.path(),
            &settings(),
            custom,
            Vec::new(),
        ));

        let spec = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("my-opencode", &index(), Path::new("."), &[]))
            .expect("custom + extension");
        assert_eq!(spec.driver_kind, DriverKind::AcpExtended);
        assert_eq!(spec.extension.as_deref(), Some("opencode"));
        assert_eq!(spec.display_name, "My OpenCode");

        let driver = catalog.driver_for(&spec).expect("extended driver");
        assert_eq!(driver.driver_kind(), DriverKind::AcpExtended);

        let entry = catalog
            .list(&index(), false)
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
        let catalog = AgentCatalog::new(AcpRegistry::new(
            dir.path(),
            &settings(),
            custom,
            Vec::new(),
        ));

        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(catalog.launch_spec("broken", &index(), Path::new("."), &[]))
            .expect_err("unknown extension");
        assert!(
            err.to_string().contains("unknown extension"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn preset_backed_registry_entry_uses_the_native_cli() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut catalog = AgentCatalog::new(registry(dir.path()));
        catalog.preset_lookup =
            |preset| (preset.program == "gemini").then(|| PathBuf::from("/fake/bin/gemini"));
        let index = RegistryIndex {
            version: "1".to_owned(),
            agents: vec![RegistryAgent {
                id: "gemini".to_owned(),
                name: "Gemini CLI".to_owned(),
                version: "1.2.3".to_owned(),
                description: Some("Registry description".to_owned()),
                repository: None,
                website: None,
                authors: Vec::new(),
                license: None,
                icon: Some("https://example.com/gemini.svg".to_owned()),
                distribution: RegistryDistribution {
                    binary: BTreeMap::new(),
                    npx: Some(PackageDistribution {
                        package: "@google/gemini-cli".to_owned(),
                        args: vec!["--acp".to_owned()],
                        env: BTreeMap::new(),
                    }),
                    uvx: None,
                },
            }],
        };

        let entries = catalog.list(&index, false);
        let entry = entries
            .iter()
            .find(|entry| entry.id == "gemini")
            .expect("gemini entry");
        assert_eq!(entry.version, "1.2.3");
        assert_eq!(entry.description.as_deref(), Some("Registry description"));
        assert_eq!(entry.icon.as_deref(), Some("builtin:gemini"));
        assert_eq!(entry.backend, AgentBackendKind::Acp);
        assert_eq!(
            entry.sources,
            vec![LocalSource::External(PathBuf::from("/fake/bin/gemini"))]
        );
        // The registry row is replaced, not duplicated.
        assert_eq!(
            entries.iter().filter(|entry| entry.id == "gemini").count(),
            1
        );
    }

    #[test]
    fn builtin_providers_are_unique_and_use_acp_arguments() {
        let providers = builtin::providers();
        let mut ids = BTreeSet::new();
        for provider in &providers {
            let definition = provider.definition();
            assert!(ids.insert(definition.id.clone()));
            assert!(!definition.name.is_empty());
            let preset = provider.launch_preset().expect("preset");
            assert!(!preset.program.is_empty());
            assert!(!preset.args.is_empty());
        }
        assert!(ids.contains(builtin::OPENCODE));
        assert!(ids.contains(builtin::OPENCODE_V2));
        assert!(ids.contains(builtin::AGY));
        assert!(ids.contains(builtin::GEMINI));
    }
}

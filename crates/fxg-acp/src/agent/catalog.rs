//! Agent catalog: definitions, launch resolution and driver selection.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.1 (planned in
//! `docs/changelog/2026-11-11-agent-extension-architecture.md`).
//!
//! The catalog is the only place that knows about agent-specific launch
//! definitions. `SessionManager` depends on it instead of on `AcpRegistry` and
//! a separate driver factory.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::anyhow;
use fxg_protocol::settings::{AgentSettings, CustomAgentConfig};

use super::builtin;
use super::provider::{AgentDefinition, AgentProvider, LaunchPreset};
use crate::acp::AcpDriver;
use crate::driver::{AgentDriver, AgentLaunchSpec, DriverKind};
use crate::registry::{AcpRegistry, AgentListEntry, LocalSource, RegistryIndex};

/// Agent catalog used by the node session manager and the CLI.
pub struct AgentCatalog {
    registry: AcpRegistry,
    providers: BTreeMap<String, Arc<dyn AgentProvider>>,
    acp: Arc<dyn AgentDriver>,
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
        let providers = providers
            .into_iter()
            .map(|provider| (provider.definition().id.clone(), provider))
            .collect();
        Self {
            registry,
            providers,
            acp: Arc::new(AcpDriver::new()),
            preset_lookup: default_preset_lookup,
        }
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

        for provider in self.providers.values() {
            let definition = provider.definition();
            if overridden.contains(&definition.id) || index.find(&definition.id).is_some() {
                continue;
            }
            let Some(preset) = provider.launch_preset() else {
                continue;
            };
            let Some(program) = (self.preset_lookup)(preset) else {
                continue;
            };
            entries.push(AgentListEntry {
                id: definition.id.clone(),
                name: definition.name.clone(),
                version: "-".to_owned(),
                description: definition.description.clone(),
                registry: false,
                package_available: false,
                sources: vec![LocalSource::External(program)],
                custom: None,
                backend: provider.backend().kind(),
                icon: definition.icon.clone(),
            });
        }

        entries.extend(self.registry.registry_entries(index, include_unavailable));
        entries
    }

    /// Resolves the launch spec for an agent.
    ///
    /// Order: custom definition → built-in preset → ACP Registry.
    pub async fn launch_spec(
        &self,
        name: &str,
        index: &RegistryIndex,
        extra_args: &[String],
    ) -> anyhow::Result<AgentLaunchSpec> {
        let id = self.registry.resolve_alias(name);

        if let Some(spec) = self.registry.custom_launch_spec(&id, extra_args)? {
            return Ok(spec);
        }

        if let Some(provider) = self.providers.get(&id)
            && let Some(preset) = provider.launch_preset()
            && let Some(program) = (self.preset_lookup)(preset)
        {
            return Ok(Self::preset_spec(
                provider.definition(),
                preset,
                program,
                extra_args,
            ));
        }

        self.registry
            .registry_launch_spec(&id, index, extra_args)
            .await
    }

    /// Selects the driver for a resolved launch spec.
    pub fn driver_for(&self, spec: &AgentLaunchSpec) -> anyhow::Result<Arc<dyn AgentDriver>> {
        match &spec.driver_kind {
            DriverKind::Acp => Ok(Arc::clone(&self.acp)),
            DriverKind::Other(other) => Err(anyhow!("unknown driver kind: {other}")),
        }
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
            program,
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
    use crate::agent::provider::StaticProvider;
    use fxg_protocol::client_api::AgentBackendKind;
    use fxg_protocol::settings::CustomAgentConfig;

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
            .block_on(catalog.launch_spec("my-agent", &index(), &["--extra".to_owned()]))
            .expect("custom spec");
        assert_eq!(spec.driver_kind, DriverKind::Acp);
        assert_eq!(spec.program, PathBuf::from("my-agent"));
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
            .block_on(catalog.launch_spec("demo", &index(), &["--verbose".to_owned()]))
            .expect("preset spec");
        assert_eq!(spec.agent_id, "demo");
        assert_eq!(spec.display_name, "Demo");
        assert_eq!(spec.program, PathBuf::from("/fake/bin/demo"));
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
            .block_on(catalog.launch_spec("my-agent", &index(), &[]))
            .expect("custom");
        assert_eq!(spec.display_name, "My Agent");
        assert_eq!(spec.program, PathBuf::from("my-agent"));

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
            program: PathBuf::from("my-agent"),
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
    fn builtin_providers_are_unique_and_use_acp_arguments() {
        let providers = builtin::providers();
        let mut ids = BTreeSet::new();
        for provider in &providers {
            let definition = provider.definition();
            assert!(ids.insert(definition.id.clone()));
            assert!(!definition.name.is_empty());
            assert_eq!(provider.launch_preset().expect("preset").args, ["acp"]);
        }
        assert!(ids.contains(builtin::OPENCODE));
        assert!(ids.contains(builtin::AGY));
    }
}

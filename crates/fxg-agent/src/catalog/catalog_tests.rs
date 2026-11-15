//! `AgentCatalog` unit tests.

use super::*;
use crate::acp::extensions::{AgentExtension, AgentMetadata};
use crate::driver::{DriverEvent, DriverKind, StartSessionRequest, StartedSession};
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
    let mut catalog = AgentCatalog::with_agents(paths(dir), &settings(), BTreeMap::new(), agents);
    catalog.preset_lookup = |preset| Some(PathBuf::from(format!("/fake/bin/{}", preset.program)));
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
fn usage_limits_delegate_to_the_agent_extension() {
    struct UsageExtension;

    #[async_trait::async_trait]
    impl AgentExtension for UsageExtension {
        async fn usage_limits(
            &self,
            _launch: &AgentLaunchSpec,
            _cwd: &Path,
        ) -> anyhow::Result<Option<AgentUsageLimits>> {
            Ok(Some(AgentUsageLimits {
                source: fxg_protocol::common::UsageSource {
                    id: "demo".to_owned(),
                    label: "Demo".to_owned(),
                    plan: None,
                    account: None,
                    icon: None,
                },
                groups: Vec::new(),
                fetched_at: 1,
            }))
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let agent =
        demo_agent("demo", "Demo", "demo").with_extension("demo-usage", Arc::new(UsageExtension));
    let mut catalog = catalog_with_agents(dir.path(), vec![agent]);
    catalog.preset_lookup = |_| Some(PathBuf::from("/fake/bin/demo"));

    let usage = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(catalog.usage_limits("demo", Path::new(".")))
        .expect("usage")
        .expect("limits");
    assert_eq!(usage.source.id, "demo");
}

#[test]
fn antigravity_uses_its_usage_extension() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = builtin::agents()
        .into_iter()
        .find(|agent| agent.definition().id == builtin::AGY)
        .expect("Antigravity built-in");
    let mut catalog = catalog_with_agents(dir.path(), vec![agent]);
    catalog.preset_lookup = |_| Some(PathBuf::from("/fake/bin/agy"));

    let spec = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(catalog.launch_spec("agy", Path::new("."), &[]))
        .expect("spec");
    assert_eq!(spec.extension_id(), Some(builtin::ANTIGRAVITY_EXTENSION));
    assert!(catalog.extension(builtin::ANTIGRAVITY_EXTENSION).is_some());
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
    use crate::acp::extensions::opencode::OpenCodeExtension;

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
    let mut on = AgentCatalog::with_agents(paths(dir.path()), &settings(), on_custom, vec![agent]);
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

//! `AcpRegistry` unit tests.

use std::path::Path;

use fxg_protocol::paths::Role;

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

fn registry(dir: &Path) -> AcpRegistry {
    AcpRegistry::new(Paths::portable(dir, Role::Node))
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
fn resolves_npx_specs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = registry(dir.path());
    let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");
    let agent = index.find("demo-npx").expect("demo-npx");

    let spec = registry
        .spec_for_source(agent, LocalSource::Npx, &[])
        .expect("npx spec");
    assert_eq!(spec.program, Some(PathBuf::from("npx")));
    assert_eq!(spec.args, vec!["-y", "demo-agent@1.2.3", "--acp"]);
    assert_eq!(spec.env, vec![("DEMO".to_owned(), "1".to_owned())]);
    assert_eq!(spec.backend, LaunchBackend::Acp(Default::default()));
}

#[test]
fn unavailable_agent_reports_install_hint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = registry(dir.path());
    let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");
    let agent = index.find("demo-bin").expect("demo-bin");
    let err = registry
        .spec_for_registry_agent(agent, &[])
        .expect_err("must fail");
    assert!(
        err.to_string().contains("not runnable"),
        "unexpected: {err}"
    );
    assert!(err.to_string().contains("PATH"), "unexpected: {err}");
}

#[test]
fn import_builds_a_custom_definition_from_npx() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = registry(dir.path());
    let index: RegistryIndex = serde_json::from_str(FIXTURE).expect("fixture");
    let agent = index.find("demo-npx").expect("demo-npx");

    let config = registry
        .import_custom_config(agent, Some("demo-ext".to_owned()))
        .expect("import config");
    assert_eq!(config.name.as_deref(), Some("Demo Npx"));
    assert_eq!(config.command, "npx");
    assert_eq!(config.args, vec!["-y", "demo-agent@1.2.3", "--acp"]);
    assert_eq!(config.env.get("DEMO").map(String::as_str), Some("1"));
    assert_eq!(config.extension.as_deref(), Some("demo-ext"));
    assert_eq!(config.description.as_deref(), Some("npx based agent"));
    assert_eq!(
        config.origin,
        Some(CustomAgentOrigin::Registry {
            id: "demo-npx".to_owned(),
            version: "1.2.3".to_owned(),
        })
    );

    let summary = registry.import_summary(agent, false);
    assert!(summary.available);
    assert!(!summary.imported);
    assert!(
        summary
            .distribution
            .contains(&RegistryDistributionKind::Npx)
    );
}

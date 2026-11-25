//! Agent authentication discovery and agent-managed login.
//!
//! Shared by the local node's Client API and the hub-relayed Node RPC so both
//! paths resolve the launch spec and ACP methods identically.

use std::path::PathBuf;

use anyhow::{Context, bail};
use fxg_agent::AgentCatalog;
use fxg_protocol::common::{AgentAuthMethodInfo, AgentAuthMethodKind};

/// Directory used to resolve an agent launch spec outside a project.
fn launch_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Reads the methods advertised by a built-in definition or ACP `initialize`.
pub(crate) async fn discover(
    catalog: &AgentCatalog,
    agent_id: &str,
) -> anyhow::Result<Vec<AgentAuthMethodInfo>> {
    let id = catalog.resolve_alias(agent_id);
    if let Some(auth) = catalog.native_auth(&id) {
        return Ok(auth.methods.clone());
    }
    let spec = catalog
        .launch_spec(&id, &launch_cwd(), &[])
        .await
        .with_context(|| format!("cannot launch {id}"))?;
    fxg_agent::acp::auth::discover_auth_methods(&spec)
        .await
        .with_context(|| format!("failed to query authentication methods for {id}"))
}

/// Runs an agent-managed method (`authenticate`).
///
/// Terminal methods need an interactive shell, so they stay on
/// `fxg agents login <id> --method <id>`.
pub(crate) async fn login(
    catalog: &AgentCatalog,
    agent_id: &str,
    method_id: &str,
) -> anyhow::Result<()> {
    let id = catalog.resolve_alias(agent_id);
    let methods = discover(catalog, &id).await?;
    let method = methods
        .iter()
        .find(|method| method.id == method_id)
        .with_context(|| format!("unknown authentication method {method_id} for {id}"))?;
    if method.kind != AgentAuthMethodKind::Agent {
        bail!(
            "authentication method {method_id} is {}; only agent-managed methods can be run from the UI (run `fxg agents login {id} --method {method_id}` in a terminal)",
            method.kind
        );
    }
    let spec = catalog
        .launch_spec(&id, &launch_cwd(), &[])
        .await
        .with_context(|| format!("cannot launch {id}"))?;
    fxg_agent::acp::auth::authenticate(&spec, &method.id)
        .await
        .with_context(|| format!("failed to log in to {id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxg_protocol::paths::{Paths, Role};
    use fxg_protocol::settings::AgentSettings;
    use std::collections::BTreeMap;

    fn catalog(dir: &std::path::Path) -> AgentCatalog {
        let settings = AgentSettings {
            registry_url: None,
            registry_cache_ttl_secs: None,
            warm_idle_secs: None,
            aliases: BTreeMap::new(),
            revision: 1,
            updated_at: 0,
        };
        AgentCatalog::new(Paths::portable(dir, Role::Node), &settings, BTreeMap::new())
    }

    #[tokio::test]
    async fn discovers_builtin_terminal_methods_without_spawning() {
        let dir = tempfile::tempdir().expect("tempdir");
        let methods = discover(&catalog(dir.path()), "opencode")
            .await
            .expect("native methods");
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].kind, AgentAuthMethodKind::Terminal);
    }

    #[tokio::test]
    async fn rejects_terminal_methods_for_ui_login() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = login(&catalog(dir.path()), "opencode", "opencode-login")
            .await
            .expect_err("terminal methods cannot run from the UI");
        let message = err.to_string();
        assert!(message.contains("terminal"), "got: {message}");
        assert!(message.contains("fxg agents login"), "got: {message}");
    }
}

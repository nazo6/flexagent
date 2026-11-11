//! Agent authentication helpers (`fxg agents login` / `logout`).
//!
//! The ACP `initialize` response advertises authentication methods. Terminal
//! methods are executed as an interactive child process (the agent CLI) with
//! the method's extra arguments, while agent methods are forwarded as an
//! `authenticate` request on an initialized connection.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    AuthMethodId, AuthenticateRequest, Implementation, InitializeRequest, LogoutRequest,
};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Client};
use anyhow::{Context, Result, anyhow, bail};
use fxg_protocol::common::{AgentAuthMethodInfo, AgentAuthMethodKind};
use tokio::sync::oneshot;

use crate::acp::{client_capabilities, map_auth_method};
use crate::driver::AgentLaunchSpec;

/// Builds the spawn configuration from a resolved launch spec.
fn agent_config(spec: &AgentLaunchSpec) -> Result<AcpAgentConfig> {
    let program = spec
        .program
        .clone()
        .ok_or_else(|| anyhow!("agent {} has no executable configured", spec.agent_id))?;
    Ok(AcpAgentConfig::new(program)
        .args(spec.args.iter().cloned())
        .envs(spec.env.iter().cloned()))
}

/// Initializes a fresh agent process and returns its advertised auth methods.
pub async fn discover_auth_methods(spec: &AgentLaunchSpec) -> Result<Vec<AgentAuthMethodInfo>> {
    let (tx, rx) = oneshot::channel();
    let connected = Client
        .v1()
        .connect_with(AcpAgent::new(agent_config(spec)?), async move |cx| {
            let outcome = async {
                let initialize = cx
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(client_capabilities())
                            .client_info(Implementation::new("fxg", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
                    .await
                    .map_err(|err| err.to_string())?;
                Ok::<_, String>(
                    initialize
                        .auth_methods
                        .iter()
                        .map(map_auth_method)
                        .collect::<Vec<_>>(),
                )
            }
            .await;
            fxg_protocol::discard_with!(tx.send(outcome), "return auth methods to the caller");
            Ok(())
        })
        .await;
    match connected {
        Ok(()) => rx
            .await
            .map_err(|err| anyhow!("agent process ended before returning auth methods: {err}"))?
            .map_err(|err| anyhow!("acp initialize failed: {err}")),
        Err(err) => Err(anyhow!("failed to start the agent: {err}")),
    }
}

/// Authenticates with an agent-managed method (`AuthMethod::Agent`).
pub async fn authenticate(spec: &AgentLaunchSpec, method_id: &str) -> Result<()> {
    let method_id = method_id.to_owned();
    let (tx, rx) = oneshot::channel();
    let connected = Client
        .v1()
        .connect_with(AcpAgent::new(agent_config(spec)?), async move |cx| {
            let outcome = async {
                let _initialize = cx
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(client_capabilities())
                            .client_info(Implementation::new("fxg", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
                    .await
                    .map_err(|err| err.to_string())?;
                cx.send_request(AuthenticateRequest::new(AuthMethodId::new(method_id)))
                    .block_task()
                    .await
                    .map_err(|err| err.to_string())?;
                Ok::<_, String>(())
            }
            .await;
            fxg_protocol::discard_with!(tx.send(outcome), "return the auth result to the caller");
            Ok(())
        })
        .await;
    match connected {
        Ok(()) => rx
            .await
            .map_err(|err| anyhow!("agent process ended before authentication completed: {err}"))?
            .map_err(|err| anyhow!("authentication failed: {err}")),
        Err(err) => Err(anyhow!("failed to start the agent: {err}")),
    }
}

/// Logs out from an agent (`auth/logout`), when advertised.
pub async fn logout(spec: &AgentLaunchSpec) -> Result<()> {
    let (tx, rx) = oneshot::channel();
    let connected = Client
        .v1()
        .connect_with(AcpAgent::new(agent_config(spec)?), async move |cx| {
            let outcome = async {
                let initialize = cx
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(client_capabilities())
                            .client_info(Implementation::new("fxg", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
                    .await
                    .map_err(|err| err.to_string())?;
                if initialize.agent_capabilities.auth.logout.is_none() {
                    return Err("the agent does not advertise logout support".to_owned());
                }
                cx.send_request(LogoutRequest::new())
                    .block_task()
                    .await
                    .map_err(|err| err.to_string())?;
                Ok::<_, String>(())
            }
            .await;
            fxg_protocol::discard_with!(tx.send(outcome), "return the logout result to the caller");
            Ok(())
        })
        .await;
    match connected {
        Ok(()) => rx
            .await
            .map_err(|err| anyhow!("agent process ended before logout completed: {err}"))?
            .map_err(|err| anyhow!("logout failed: {err}")),
        Err(err) => Err(anyhow!("failed to start the agent: {err}")),
    }
}

/// Runs a terminal authentication method interactively.
///
/// The ACP spec defines terminal methods as "the configured agent invocation
/// plus the method's arguments", so both the preset / registry args (`acp`) and
/// the method args (`--login`) are passed.
pub async fn run_terminal_login(
    spec: &AgentLaunchSpec,
    method: &AgentAuthMethodInfo,
) -> Result<()> {
    if method.kind != AgentAuthMethodKind::Terminal {
        bail!("{} is not a terminal authentication method", method.id);
    }
    let program_path = spec
        .program
        .as_deref()
        .ok_or_else(|| anyhow!("agent {} has no executable configured", spec.agent_id))?;
    let program = resolve_program(program_path)?;
    let mut args = spec.args.clone();
    args.extend(method.args.iter().cloned());

    let mut command = terminal_command(&program, &args);
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    for (key, value) in &method.env {
        command.env(key, value);
    }
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = command
        .status()
        .await
        .with_context(|| format!("failed to run {}", program.display()))?;
    if !status.success() {
        bail!("login command exited with {status}");
    }
    Ok(())
}

/// Resolves a launch program (absolute path or a name on `PATH`).
fn resolve_program(program: &Path) -> Result<PathBuf> {
    if program.is_absolute() {
        return Ok(program.to_path_buf());
    }
    let cwd = std::env::current_dir().context("failed to resolve the current directory")?;
    fxg_pty::resolve_command(&program.to_string_lossy(), cwd)
        .map_err(|err| anyhow!("failed to resolve {}: {err}", program.display()))
}

/// Builds the interactive login command.
///
/// On Windows, `.cmd` / `.bat` launchers must run through `cmd /C`.
fn terminal_command(program: &std::path::Path, args: &[String]) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let extension = program
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension == "cmd" || extension == "bat" {
            let mut command = tokio::process::Command::new("cmd");
            command.arg("/C").arg(program);
            command.args(args);
            return command;
        }
    }
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    command
}

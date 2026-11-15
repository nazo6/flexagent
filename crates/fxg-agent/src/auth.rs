//! Agent authentication helpers (`fxg agents login` / `logout`).
//!
//! Native agents have no handshake: their login / logout commands are declared
//! statically on the built-in agent definition and run as an interactive child
//! process with inherited stdio.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, anyhow, bail};

use crate::catalog::builtin::AgentCommand;

/// Runs a native provider's interactive login / logout command.
pub async fn run_agent_command(command: &AgentCommand, env: &[(String, String)]) -> Result<()> {
    let args = command
        .args
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect::<Vec<_>>();
    run_interactive(Path::new(command.program), &args, env).await
}

/// Runs an interactive child process with inherited stdio.
pub(crate) async fn run_interactive(
    program: &Path,
    args: &[String],
    env: &[(String, String)],
) -> Result<()> {
    let program = resolve_program(program)?;
    let mut command = terminal_command(&program, args);
    for (key, value) in env {
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
        bail!("interactive command exited with {status}");
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

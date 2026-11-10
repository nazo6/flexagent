//! `fxg internal bootstrap-workspace`: zero-touch initialization inside an ephemeral VM.
//!
//! Design: `docs/01-architecture-and-sync.md` §6.3, `docs/05-cli-and-pwa-ui.md` §2.3.
//!
//! Clones the repository using the short-lived token injected by the hub
//! (`FXG_GIT_TOKEN`), auto-detects the toolchain from the repository's manifest files,
//! and installs it with `mise` / `uv`. **All output of this function is restricted to
//! `stderr`** (`stdout` is protected as JSON Lines dedicated to the subsequent
//! `fxg internal stdio`).
//!
//! On exit it writes the environment variables (PATH additions + the project profile's
//! `bootstrap_env`) to `$FXG_HOME/bootstrap.env`. The generated provisioner script sources
//! this right before `exec fxg internal stdio`, so the installed tools are inherited by
//! the agent process.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use std::collections::BTreeMap;

use fxg_protocol::config::{BOOTSTRAP_ENV_FILE_NAME, DEFAULT_GIT_USERNAME};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::error::NodeError;
use crate::git;

/// Execution options for `bootstrap-workspace`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapOptions {
    /// Git URL to clone from (required)
    pub repo: String,
    /// Target branch (the remote default branch if omitted)
    pub branch: Option<String>,
    /// Destination directory of the clone
    pub dir: PathBuf,
    /// Data directory (`~/.flexagent`)
    pub fxg_home: PathBuf,
    /// Whether to install the toolchain (`mise` / `uv`) and the tools
    /// (tests set this to `false` to avoid network access)
    pub install_toolchain: bool,
    /// Tools added to the detected ones (for tests and debugging)
    pub extra_tools: Vec<String>,
    /// Setup commands from the project profile (`project_profiles` in `hub.db`)
    pub setup: Vec<String>,
    /// Non-secret environment variables from the project profile
    pub env: BTreeMap<String, String>,
}

/// Standard required CLIs installed automatically (design: docs/01 §6.3).
pub const STANDARD_CLI_TOOLS: &[&str] = &["ripgrep", "fd", "jq", "gh"];

/// Execution result of `bootstrap-workspace`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapOutcome {
    /// Cloned workspace
    pub dir: PathBuf,
    /// Tools that were installed (or attempted)
    pub tools: Vec<String>,
    /// Generated environment variable file
    pub env_file: PathBuf,
}

/// Bootstrap progress log (all to `stderr`).
fn log(message: impl std::fmt::Display) {
    eprintln!("[fxg-bootstrap] {message}");
}

/// Initializes the workspace inside the ephemeral VM.
pub async fn bootstrap_workspace(options: BootstrapOptions) -> Result<BootstrapOutcome, NodeError> {
    std::fs::create_dir_all(&options.fxg_home).map_err(|source| NodeError::Io {
        path: options.fxg_home.clone(),
        source,
    })?;

    if !crate::git::git_available().await {
        return Err(NodeError::GitUnavailable(
            "git is required for bootstrap-workspace".to_owned(),
        ));
    }

    clone_repository(&options).await?;

    // Tool detection from the repository's manifest files
    let mut tools = detect_tools(&options.dir);
    for tool in &options.extra_tools {
        if !tools.contains(tool) {
            tools.push(tool.clone());
        }
    }
    log(format!("detected tools: {}", tools.join(", ")));

    let bin_dir = default_bin_dir();
    if options.install_toolchain {
        std::fs::create_dir_all(&bin_dir).map_err(|source| NodeError::Io {
            path: bin_dir.clone(),
            source,
        })?;
        install_mise(&bin_dir).await;
        if wants_python(&options.dir) {
            install_uv(&bin_dir).await;
        }
        install_tools(&bin_dir, &tools).await;
        if wants_python(&options.dir) {
            prepare_python_environment(&options.dir).await;
        }
    } else {
        log("toolchain installation skipped (install_toolchain = false)");
    }

    // `git` is used with the highest priority, so only try to install it when missing
    if options.install_toolchain && !crate::git::git_available().await {
        log("git is missing after setup; setup commands may fail");
    }

    // Run the project profile's setup commands (output goes to stderr)
    for command in &options.setup {
        log(format!("setup: {command}"));
        let status = run_streaming(&options.dir, command, &[]).await?;
        if !status {
            return Err(NodeError::Hub(format!(
                "bootstrap setup command failed: {command}"
            )));
        }
    }

    let env_file = write_env_file(&options, &bin_dir)?;
    log(format!("environment file written: {}", env_file.display()));
    log("bootstrap completed");

    Ok(BootstrapOutcome {
        dir: options.dir,
        tools,
        env_file,
    })
}

/// Clones the repository (reuses an existing clone).
async fn clone_repository(options: &BootstrapOptions) -> Result<(), NodeError> {
    // git cannot run in a directory that does not exist
    if options.dir.is_dir() && git::toplevel(&options.dir).await?.is_some() {
        log(format!(
            "workspace already cloned: {}",
            options.dir.display()
        ));
        return Ok(());
    }

    if options.dir.exists() {
        let is_empty = std::fs::read_dir(&options.dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true);
        if !is_empty {
            return Err(NodeError::Hub(format!(
                "target directory is not empty and not a git repository: {}",
                options.dir.display()
            )));
        }
    }
    if let Some(parent) = options.dir.parent() {
        std::fs::create_dir_all(parent).map_err(|source| NodeError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    // Pass the short-lived token (FXG_GIT_TOKEN) through GIT_ASKPASS.
    // The token is never embedded in the URL; environment variables are kept in memory
    // only.
    let askpass = write_bootstrap_askpass(&options.fxg_home)?;
    let mut git_env: Vec<(String, String)> = vec![
        (
            "GIT_ASKPASS".to_owned(),
            askpass.to_string_lossy().into_owned(),
        ),
        ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
    ];
    if let Ok(username) = std::env::var("FXG_GIT_USERNAME") {
        git_env.push(("FXG_GIT_USERNAME".to_owned(), username));
    }
    if let Ok(token) = std::env::var("FXG_GIT_TOKEN") {
        git_env.push(("FXG_GIT_TOKEN".to_owned(), token));
    }

    log(format!(
        "cloning {} into {}",
        options.repo,
        options.dir.display()
    ));
    let mut args: Vec<&str> = vec!["clone", "--depth", "50"];
    if let Some(branch) = options.branch.as_deref().filter(|b| !b.trim().is_empty()) {
        args.push("--branch");
        args.push(branch);
    }
    args.push(&options.repo);
    let dir_arg = options.dir.to_string_lossy().into_owned();
    args.push(&dir_arg);

    let cwd = options
        .dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let env_refs: Vec<(&str, &str)> = git_env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let output = git::capture(&cwd, &args, &env_refs).await?;
    if !output.success {
        return Err(NodeError::Git {
            cwd,
            args: args.join(" "),
            message: output.stderr,
        });
    }
    Ok(())
}

/// Detects the tools to install from the repository's manifest files.
pub fn detect_tools(dir: &Path) -> Vec<String> {
    let mut tools: Vec<String> = Vec::new();
    let mut push = |tool: &str| {
        if !tools.iter().any(|existing| existing == tool) {
            tools.push(tool.to_owned());
        }
    };

    if dir.join("Cargo.toml").exists() || dir.join("rust-toolchain.toml").exists() {
        push("rust");
    }
    if dir.join("package.json").exists() || dir.join(".node-version").exists() {
        push("node");
        if dir.join("pnpm-lock.yaml").exists() {
            push("pnpm");
        } else if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
            push("bun");
        } else if dir.join("yarn.lock").exists() {
            push("yarn");
        }
    }
    if dir.join("pyproject.toml").exists() || dir.join(".python-version").exists() {
        push("python");
    }
    // Every tool listed in `mise.toml` / `.tool-versions`
    for tool in read_mise_tools(dir) {
        push(&tool);
    }
    tools
}

/// Reads tool names from `[tools]` in `mise.toml` / `.tool-versions`.
fn read_mise_tools(dir: &Path) -> Vec<String> {
    let mut tools = Vec::new();
    if let Ok(text) = std::fs::read_to_string(dir.join("mise.toml"))
        && let Ok(value) = text.parse::<toml::Value>()
        && let Some(table) = value.get("tools").and_then(|tools| tools.as_table())
    {
        tools.extend(table.keys().cloned());
    }
    if let Ok(text) = std::fs::read_to_string(dir.join(".tool-versions")) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((tool, _version)) = line.split_once(char::is_whitespace) {
                tools.push(tool.to_owned());
            }
        }
    }
    tools
}

/// Whether this is a Python project (whether to build a virtual environment with uv).
fn wants_python(dir: &Path) -> bool {
    dir.join("pyproject.toml").exists() || dir.join(".python-version").exists()
}

/// `~/.local/bin` (tool installation destination).
fn default_bin_dir() -> PathBuf {
    if let Ok(fxg_home) = std::env::var("FXG_HOME") {
        // Tests isolate into a directory under the data directory
        return PathBuf::from(fxg_home).join("bin");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("bin")
}

/// mise's shim directory.
fn mise_shims_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("mise")
        .join("shims")
}

/// Installs `mise` as a single binary (no root privileges or apt required).
async fn install_mise(bin_dir: &Path) {
    let mise = bin_dir.join(if cfg!(windows) { "mise.exe" } else { "mise" });
    if mise.exists() {
        log("mise is already installed");
        return;
    }
    log("installing mise ...");
    let installer = format!(
        "curl -fsSL https://mise.run | MISE_INSTALL_PATH='{}' sh",
        mise.display()
    );
    match run_shell(bin_dir, &installer).await {
        Ok(true) => log("mise installed"),
        Ok(false) => log("failed to install mise (continuing without tool installs)"),
        Err(err) => log(format!("failed to install mise: {err}")),
    }
}

/// Installs `uv` from a prebuilt binary.
async fn install_uv(bin_dir: &Path) {
    let uv = bin_dir.join(if cfg!(windows) { "uv.exe" } else { "uv" });
    if uv.exists() {
        log("uv is already installed");
        return;
    }
    if cfg!(windows) {
        log("uv installation on windows is not automated; install uv manually");
        return;
    }
    let Some(target) = uv_target() else {
        log("unsupported platform for uv; skipping");
        return;
    };
    let url =
        format!("https://github.com/astral-sh/uv/releases/latest/download/uv-{target}.tar.gz");
    let archive = bin_dir.join("uv-download.tar.gz");
    log(format!("installing uv from {url} ..."));
    if !download_file(&url, &archive).await {
        log("failed to download uv");
        return;
    }
    let script = format!(
        "tar -xzf '{}' -C '{}' && find '{}' -name uv -type f -exec cp {{}} '{}' \\; && rm -f '{}'",
        archive.display(),
        bin_dir.display(),
        bin_dir.display(),
        uv.display(),
        archive.display()
    );
    match run_shell(bin_dir, &script).await {
        Ok(true) => log("uv installed"),
        Ok(false) => log("failed to extract uv archive"),
        Err(err) => log(format!("failed to extract uv: {err}")),
    }
}

/// uv distribution target name (OS / architecture).
fn uv_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

/// Downloads a file with `curl` (or `wget` if unavailable).
async fn download_file(url: &str, dest: &Path) -> bool {
    let dest_arg = dest.to_string_lossy().into_owned();
    let curl = format!("curl -fsSL '{url}' -o '{dest_arg}'");
    if matches!(run_shell(Path::new("."), &curl).await, Ok(true)) {
        return true;
    }
    let wget = format!("wget -q '{url}' -O '{dest_arg}'");
    matches!(run_shell(Path::new("."), &wget).await, Ok(true))
}

/// Installs tools with `mise install --yes <tools>` (failures are warnings only).
async fn install_tools(bin_dir: &Path, tools: &[String]) {
    let mut all: Vec<String> = tools
        .iter()
        .filter(|tool| tool.as_str() != "python")
        .cloned()
        .collect();
    for tool in STANDARD_CLI_TOOLS {
        if !all.iter().any(|existing| existing == tool) {
            all.push((*tool).to_owned());
        }
    }
    if all.is_empty() {
        return;
    }
    let mise = bin_dir.join(if cfg!(windows) { "mise.exe" } else { "mise" });
    if !mise.exists() {
        log("mise is unavailable; skipping tool installs");
        return;
    }
    let shims = mise_shims_dir();
    let path_env = format!("PATH={}:{}:$PATH", bin_dir.display(), shims.display());
    log(format!("installing tools via mise: {}", all.join(" ")));
    let script = format!(
        "MISE_YES=1 {} '{}' install --yes {}",
        path_env,
        mise.display(),
        all.join(" ")
    );
    match run_shell(bin_dir, &script).await {
        Ok(true) => log("tools installed"),
        Ok(false) => log("some tools failed to install (continuing)"),
        Err(err) => log(format!("tool installation failed: {err}")),
    }
}

/// Best-effort Python environment setup via `uv venv` / `uv sync`.
async fn prepare_python_environment(dir: &Path) {
    let uv = default_bin_dir().join(if cfg!(windows) { "uv.exe" } else { "uv" });
    if !uv.exists() {
        log("uv is unavailable; skipping python environment setup");
        return;
    }
    log("preparing python environment with uv ...");
    let script = format!("'{}' venv", uv.display());
    match run_streaming(dir, &script, &[]).await {
        Ok(true) => log("python virtual environment ready (.venv)"),
        Ok(false) => log("uv venv failed (continuing)"),
        Err(err) => log(format!("uv venv failed: {err}")),
    }
}

/// Writes `$FXG_HOME/bootstrap.env`.
///
/// The generated format is `export KEY='value'` lines that POSIX `sh` can `source`.
/// `PATH` is prefixed with the installed tools (`~/.local/bin` / mise shims / uv).
fn write_env_file(options: &BootstrapOptions, bin_dir: &Path) -> Result<PathBuf, NodeError> {
    let mut lines = vec![
        "# Generated by `fxg internal bootstrap-workspace`. Do not edit.".to_owned(),
        format!(
            "export PATH={}",
            sh_quote(&format!(
                "{}:{}:$PATH",
                bin_dir.display(),
                mise_shims_dir().display()
            ))
        ),
        format!(
            "export FXG_WORKSPACE={}",
            sh_quote(&options.dir.to_string_lossy())
        ),
    ];
    for (key, value) in &options.env {
        lines.push(format!("export {}={}", key, sh_quote(value)));
    }
    let path = options.fxg_home.join(BOOTSTRAP_ENV_FILE_NAME);
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).map_err(|source| NodeError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Safely wraps in single quotes for a POSIX shell.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Generates the `GIT_ASKPASS` helper for Git authentication during bootstrap
/// (short-lived token).
///
/// Because `bootstrap-workspace` runs before the daemon starts, it uses the lightest
/// helper that reads the environment variable `FXG_GIT_TOKEN` directly instead of going
/// through local IPC (the token lives only in the environment and is never written to
/// disk).
pub fn write_bootstrap_askpass(fxg_home: &Path) -> Result<PathBuf, NodeError> {
    std::fs::create_dir_all(fxg_home).map_err(|source| NodeError::Io {
        path: fxg_home.to_path_buf(),
        source,
    })?;
    let path = if cfg!(windows) {
        fxg_home.join("git-askpass-bootstrap.cmd")
    } else {
        fxg_home.join("git-askpass-bootstrap.sh")
    };
    let content = if cfg!(windows) {
        "@echo off\r\n\
         setlocal\r\n\
         set \"p=%~1\"\r\n\
         if not \"x%p:Username=%\"==\"x%p%\" (\r\n\
         \x20 echo %FXG_GIT_USERNAME%\r\n\
         ) else (\r\n\
         \x20 echo %FXG_GIT_TOKEN%\r\n\
         )\r\n"
            .to_owned()
    } else {
        format!(
            "#!/bin/sh\n\
             # fxg bootstrap GIT_ASKPASS helper (short-lived token). Auto-generated.\n\
             case \"$1\" in\n\
             \x20 *sername*) echo \"${{FXG_GIT_USERNAME:-{DEFAULT_GIT_USERNAME}}}\" ;;\n\
             \x20 *) echo \"$FXG_GIT_TOKEN\" ;;\n\
             esac\n"
        )
    };
    std::fs::write(&path, content).map_err(|source| NodeError::Io {
        path: path.clone(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(&path).map_err(|source| NodeError::Io {
            path: path.clone(),
            source,
        })?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).map_err(|source| NodeError::Io {
            path: path.clone(),
            source,
        })?;
    }
    Ok(path)
}

/// Runs a command through the shell and streams stdout / stderr **to stderr**.
async fn run_streaming(cwd: &Path, command: &str, env: &[(&str, &str)]) -> Result<bool, NodeError> {
    let mut process = shell_command(command);
    process
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        process.env(key, value);
    }
    let mut child = process.spawn().map_err(NodeError::Plain)?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(async move {
        if let Some(stdout) = stdout {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("{line}");
            }
        }
    });
    let stderr_task = tokio::spawn(async move {
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("{line}");
            }
        }
    });
    let status = child.wait().await.map_err(NodeError::Plain)?;
    if let Err(err) = stdout_task.await {
        tracing::warn!("bootstrap stdout task terminated unexpectedly: {err}");
    }
    if let Err(err) = stderr_task.await {
        tracing::warn!("bootstrap stderr task terminated unexpectedly: {err}");
    }
    Ok(status.success())
}

/// Builds a shell command (Windows `cmd /C`, otherwise `sh -c`).
fn shell_command(command: &str) -> Command {
    if cfg!(windows) {
        let mut process = Command::new("cmd");
        process.arg("/C").arg(command);
        process
    } else {
        let mut process = Command::new("sh");
        process.arg("-c").arg(command);
        process
    }
}

/// Runs a shell command (result only; output goes to stderr).
async fn run_shell(cwd: &Path, command: &str) -> Result<bool, NodeError> {
    run_streaming(cwd, command, &[]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_toolchains_from_repository_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").expect("write");
        std::fs::write(dir.path().join("package.json"), "{}").expect("write");
        std::fs::write(dir.path().join("pnpm-lock.yaml"), "").expect("write");
        std::fs::write(
            dir.path().join("mise.toml"),
            "[tools]\nrust = \"stable\"\nnode = \"22\"\n",
        )
        .expect("write");

        let tools = detect_tools(dir.path());
        assert!(tools.contains(&"rust".to_owned()));
        assert!(tools.contains(&"node".to_owned()));
        assert!(tools.contains(&"pnpm".to_owned()));
        // No duplicates
        assert_eq!(tools.iter().filter(|tool| *tool == "rust").count(), 1);
    }

    #[test]
    fn detects_python_and_fxg_bootstrap_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("pyproject.toml"), "[project]\n").expect("write");
        let tools = detect_tools(dir.path());
        assert!(tools.contains(&"python".to_owned()));
        assert!(wants_python(dir.path()));
    }

    #[test]
    fn reads_tool_versions_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join(".tool-versions"),
            "node 22.1.0\n# comment\ngolang 1.22\n",
        )
        .expect("write");
        let tools = detect_tools(dir.path());
        assert!(tools.contains(&"node".to_owned()));
        assert!(tools.contains(&"golang".to_owned()));
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
    }

    #[tokio::test]
    async fn bootstrap_clones_local_repository_and_writes_env() {
        let repo_dir = tempfile::tempdir().expect("tempdir");
        let home = tempfile::tempdir().expect("tempdir");
        let workspace = home.path().join("workspace");

        // Local Git repository (cloned via a file:// URL)
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "fxg test"],
        ] {
            git::run_git(repo_dir.path(), &args).await.expect("git");
        }
        std::fs::write(
            repo_dir.path().join("pyproject.toml"),
            "[project]\nname = \"demo\"\n",
        )
        .expect("write");
        git::run_git(repo_dir.path(), &["add", "-A"])
            .await
            .expect("add");
        git::run_git(repo_dir.path(), &["commit", "-q", "-m", "init"])
            .await
            .expect("commit");

        let repo_url = repo_dir.path().display().to_string();
        let outcome = bootstrap_workspace(BootstrapOptions {
            repo: repo_url,
            branch: Some("main".to_owned()),
            dir: workspace.clone(),
            fxg_home: home.path().join(".flexagent"),
            install_toolchain: false,
            extra_tools: Vec::new(),
            setup: Vec::new(),
            env: Default::default(),
        })
        .await
        .expect("bootstrap");

        assert!(workspace.join("pyproject.toml").exists());
        assert!(outcome.tools.contains(&"python".to_owned()));
        let env = std::fs::read_to_string(&outcome.env_file).expect("env file");
        assert!(env.contains("export FXG_WORKSPACE="));
        assert!(env.contains("export PATH="));

        // Re-running is idempotent (reuses the existing clone)
        let again = bootstrap_workspace(BootstrapOptions {
            repo: repo_dir.path().display().to_string(),
            branch: Some("main".to_owned()),
            dir: workspace.clone(),
            fxg_home: home.path().join(".flexagent"),
            install_toolchain: false,
            extra_tools: Vec::new(),
            setup: Vec::new(),
            env: Default::default(),
        })
        .await
        .expect("bootstrap again");
        assert_eq!(again.dir, workspace);
    }

    #[test]
    fn bootstrap_askpass_script_is_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_bootstrap_askpass(dir.path()).expect("write");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).expect("read");
        assert!(content.contains("FXG_GIT_TOKEN"));
    }
}

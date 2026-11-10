//! Daemon IPC client and CLI output helpers.
//!
//! The `fxg` CLI drives the node through local IPC (`fxg daemon`)
//! (design: `docs/03-protocol-and-api.md` §4). When the daemon is not running,
//! `DaemonClient::connect` starts it in the background once (unless
//! `FXG_NO_AUTO_DAEMON=1` is set).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use fxg_node::IpcClient;
use fxg_protocol::CommandId;
use fxg_protocol::client_api::{DiagnosticsResponse, SupportBundle};
use fxg_protocol::common::{
    ClientSource, IncidentListResponse, PromptInput, SessionControlAction, SessionSummary,
};
use fxg_protocol::config::{env_keys, fxg_home, process_env};
use fxg_protocol::ipc::{
    IPC_PROTOCOL_VERSION, IpcOutcome, IpcRequest, IpcRequestKind, IpcResultKind, IpcServerMessage,
    LocalRequest, LocalResult, SessionDetail,
};
use fxg_protocol::node_server::{NodeRpcRequest, NodeRpcResponse};

/// How long to wait for a freshly spawned daemon to accept IPC connections.
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(15);
/// A startup lock older than this is considered stale (crashed starter).
const DAEMON_START_LOCK_STALE: Duration = Duration::from_secs(30);

/// IPC client for the daemon (`fxg daemon`).
pub struct DaemonClient {
    client: IpcClient,
    request_seq: u64,
}

/// Whether automatic daemon startup is enabled (`FXG_NO_AUTO_DAEMON=1` disables it).
fn auto_daemon_enabled() -> bool {
    !matches!(
        process_env(env_keys::NO_AUTO_DAEMON)
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Starts `fxg daemon` in the background (once) and waits for its IPC endpoint.
///
/// Returns the daemon log path so the caller can include it in an error message.
async fn ensure_daemon_started(endpoint: &str) -> Result<PathBuf> {
    let home = fxg_home(&process_env);
    let log_path = home.join("logs").join("daemon.log");
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let lock_path = home.join("daemon.start.lock");
    for attempt in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut lock) => {
                fxg_protocol::discard_with!(
                    writeln!(lock, "{}", std::process::id()),
                    "best-effort pid note in the daemon startup lock"
                );
                let spawn_result = spawn_daemon(&log_path);
                let ready = match &spawn_result {
                    Ok(()) => wait_for_ipc(endpoint, DAEMON_START_TIMEOUT).await,
                    Err(_) => false,
                };
                remove_start_lock(&lock_path);
                spawn_result?;
                if !ready {
                    tracing::debug!(
                        endpoint,
                        log = %log_path.display(),
                        "daemon did not become ready before the startup timeout"
                    );
                }
                return Ok(log_path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another CLI is already starting the daemon; wait for it.
                if wait_for_ipc(endpoint, Duration::from_secs(5)).await {
                    return Ok(log_path);
                }
                if attempt == 0 && lock_is_stale(&lock_path) {
                    remove_start_lock(&lock_path);
                    continue;
                }
                return Ok(log_path);
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to create {}", lock_path.display()));
            }
        }
    }
    Ok(log_path)
}

/// Spawns `fxg daemon` detached with its stdio redirected to the daemon log.
fn spawn_daemon(log_path: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("failed to resolve the fxg executable")?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;
    let stderr = log
        .try_clone()
        .with_context(|| format!("failed to clone {}", log_path.display()))?;

    let mut command = std::process::Command::new(exe);
    command
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // New session so the daemon survives terminal close (SIGHUP).
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, DETACHED_PROCESS};
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }

    let child = command.spawn().context("failed to spawn `fxg daemon`")?;
    // Detached: the daemon is reparented (Unix) / detached (Windows).
    drop(child);
    Ok(())
}

/// Waits until the IPC endpoint accepts a connection.
async fn wait_for_ipc(endpoint: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if IpcClient::connect(endpoint).await.is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Whether a startup lock is old enough to be considered abandoned.
fn lock_is_stale(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        tracing::debug!(path = %path.display(), "startup lock has no modification time");
        return false;
    };
    match modified.elapsed() {
        Ok(age) => age > DAEMON_START_LOCK_STALE,
        Err(err) => {
            tracing::debug!(path = %path.display(), "startup lock mtime is in the future: {err}");
            false
        }
    }
}

/// Removes the startup lock (best effort).
fn remove_start_lock(path: &Path) {
    fxg_protocol::discard_with!(std::fs::remove_file(path), "remove the daemon startup lock");
}

impl DaemonClient {
    /// Connect to the local IPC endpoint, starting `fxg daemon` if needed.
    pub async fn connect() -> Result<Self> {
        let endpoint = fxg_node::ipc_endpoint(&process_env);
        match IpcClient::connect(&endpoint).await {
            Ok(client) => Ok(Self {
                client,
                request_seq: 0,
            }),
            Err(connect_error) => {
                if !auto_daemon_enabled() {
                    return Err(connect_error).with_context(|| {
                        format!(
                            "cannot connect to the daemon ({endpoint}); start `fxg daemon` first"
                        )
                    });
                }
                let log_path = ensure_daemon_started(&endpoint).await?;
                match IpcClient::connect(&endpoint).await {
                    Ok(client) => Ok(Self {
                        client,
                        request_seq: 0,
                    }),
                    Err(err) => Err(anyhow!(
                        "cannot connect to the daemon ({endpoint}) after auto-start: {err}; see {} (set FXG_NO_AUTO_DAEMON=1 to disable auto-start)",
                        log_path.display()
                    )),
                }
            }
        }
    }

    fn command_id(&mut self) -> CommandId {
        self.request_seq += 1;
        CommandId::from(format!("cli-{}-{}", std::process::id(), self.request_seq))
    }

    async fn request(&mut self, message: LocalRequest) -> Result<LocalResult> {
        let request_id = message.command_id().clone();
        let request = IpcRequest {
            protocol_version: IPC_PROTOCOL_VERSION,
            request_id,
            request: IpcRequestKind::Local { request: message },
        };
        let response = self.client.request(&request).await?;
        match response {
            IpcServerMessage::Response {
                outcome:
                    IpcOutcome::Ok {
                        result: IpcResultKind::Local { result },
                    },
                ..
            } => Ok(result),
            IpcServerMessage::Response {
                outcome: IpcOutcome::Err { error },
                ..
            } => bail!("{}: {}", error.code, error.message),
            other => bail!("unexpected ipc response: {other:?}"),
        }
    }

    /// Sends a shared node RPC over the local IPC transport.
    async fn request_node(
        &mut self,
        command_id: CommandId,
        request: NodeRpcRequest,
    ) -> Result<NodeRpcResponse> {
        let message = IpcRequest {
            protocol_version: IPC_PROTOCOL_VERSION,
            request_id: command_id,
            request: IpcRequestKind::Node { request },
        };
        let response = self.client.request(&message).await?;
        match response {
            IpcServerMessage::Response {
                outcome:
                    IpcOutcome::Ok {
                        result: IpcResultKind::Node { response },
                    },
                ..
            } => Ok(response),
            IpcServerMessage::Response {
                outcome: IpcOutcome::Err { error },
                ..
            } => bail!("{}: {}", error.code, error.message),
            other => bail!("unexpected ipc response: {other:?}"),
        }
    }

    /// Create or reuse a worktree through daemon IPC (`fxg run -w` / `fxg session fork -w`).
    pub async fn add_worktree(
        &mut self,
        cwd: &Path,
        project_id: Option<String>,
        branch: &str,
        base_branch: Option<String>,
    ) -> Result<(String, String, bool)> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::WorktreeAdd {
                command_id,
                cwd: path_to_string(cwd),
                project_id: project_id.map(fxg_protocol::ProjectId::from),
                branch: branch.to_owned(),
                base_branch,
            })
            .await?;
        match result {
            LocalResult::WorktreeAdded {
                path,
                branch,
                created,
            } => Ok((path, branch, created)),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg ps` / `fxg session list`
    pub async fn list_sessions(
        &mut self,
        include_stopped: bool,
        include_archived: bool,
    ) -> Result<Vec<SessionSummary>> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::ListSessions {
                command_id,
                include_stopped,
                include_archived,
            })
            .await?;
        match result {
            LocalResult::Sessions { sessions } => Ok(sessions),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg session archive` / `unarchive` — archive or restore a session.
    ///
    /// Returns the archive timestamp (Unix epoch ms; `None` when restored).
    pub async fn archive_session(
        &mut self,
        session_id: &str,
        archived: bool,
    ) -> Result<Option<i64>> {
        let command_id = self.command_id();
        let response = self
            .request_node(
                command_id,
                NodeRpcRequest::ArchiveSession {
                    session_id: session_id.into(),
                    archived,
                },
            )
            .await?;
        match response {
            NodeRpcResponse::Archive { archived_at } => Ok(archived_at),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg session delete` — delete a session (purges event bodies; irreversible).
    pub async fn delete_session(&mut self, session_id: &str) -> Result<()> {
        let command_id = self.command_id();
        let response = self
            .request_node(
                command_id,
                NodeRpcRequest::DeleteSession {
                    session_id: session_id.into(),
                },
            )
            .await?;
        match response {
            NodeRpcResponse::Delete => Ok(()),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg kill-all`
    pub async fn kill_all(&mut self) -> Result<(Vec<String>, Vec<String>)> {
        let command_id = self.command_id();
        let result = self.request(LocalRequest::KillAll { command_id }).await?;
        match result {
            LocalResult::KillAll {
                killed_sessions,
                killed_ptys,
            } => Ok((killed_sessions, killed_ptys)),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg auth token` / `fxg auth rotate-token`
    pub async fn auth_token(&mut self, rotate: bool) -> Result<String> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::AuthToken { command_id, rotate })
            .await?;
        match result {
            LocalResult::AuthToken { token } => Ok(token),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg internal git-askpass` (GIT_ASKPASS helper; Git credential proxy)
    ///
    /// Returns `(username, token, error)`. In an ephemeral VM the token is kept
    /// in memory only and never written to disk (design: docs/01 §6.4).
    pub async fn git_credential(
        &mut self,
        prompt: &str,
    ) -> Result<(String, Option<String>, Option<String>)> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::GitCredential {
                command_id,
                prompt: prompt.to_owned(),
            })
            .await?;
        match result {
            LocalResult::GitCredential {
                username,
                token,
                error,
            } => Ok((username, token, error)),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// Start a new session for `fxg run <agent>`.
    pub async fn ensure_session(
        &mut self,
        cwd: &Path,
        agent_id: &str,
        extra_args: &[String],
        initial_mode: Option<&str>,
    ) -> Result<String> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::EnsureSession {
                command_id,
                cwd: path_to_string(cwd),
                agent_id: agent_id.to_owned(),
                extra_args: extra_args.to_vec(),
                initial_mode: initial_mode.map(str::to_owned),
            })
            .await?;
        match result {
            LocalResult::EnsureSession { session_id } => Ok(session_id.to_string()),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg session import <agent-session-id>`.
    pub async fn import_session(
        &mut self,
        agent_id: &str,
        agent_session_id: &str,
        cwd: &str,
        title: Option<&str>,
    ) -> Result<String> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::SessionImport {
                command_id,
                agent_id: agent_id.to_owned(),
                agent_session_id: agent_session_id.to_owned(),
                cwd: cwd.to_owned(),
                title: title.map(str::to_owned),
            })
            .await?;
        match result {
            LocalResult::SessionImported { session_id } => Ok(session_id.to_string()),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg doctor` / `fxg session diagnose` (local diagnostics).
    pub async fn diagnostics(&mut self) -> Result<DiagnosticsResponse> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::Diagnostics { command_id })
            .await?;
        match result {
            LocalResult::Diagnostics { diagnostics } => Ok(diagnostics),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg diagnostics export` (redacted support bundle).
    pub async fn support_bundle(&mut self) -> Result<SupportBundle> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::SupportBundle { command_id })
            .await?;
        match result {
            LocalResult::SupportBundle { bundle } => Ok(*bundle),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg diagnostics incidents` (persisted incidents).
    pub async fn incidents(
        &mut self,
        limit: u32,
        include_resolved: bool,
    ) -> Result<IncidentListResponse> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::Incidents {
                command_id,
                limit,
                include_resolved,
            })
            .await?;
        match result {
            LocalResult::Incidents { incidents } => Ok(incidents),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg session prompt` (headless send).
    pub async fn send_prompt(
        &mut self,
        session_id: &str,
        input: &PromptInput,
        client_source: ClientSource,
    ) -> Result<()> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::SendPrompt {
                command_id,
                session_id: session_id.into(),
                input: input.clone(),
                client_source,
            })
            .await?;
        ack_message(result)?;
        Ok(())
    }

    /// Mode switch / config change / cancel / kill (`fxg session stop|kill`).
    pub async fn control_session(
        &mut self,
        session_id: &str,
        action: SessionControlAction,
    ) -> Result<()> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::ControlSession {
                command_id,
                session_id: session_id.into(),
                action,
            })
            .await?;
        ack_message(result)?;
        Ok(())
    }

    /// `fxg session show`
    pub async fn session_show(
        &mut self,
        session_id: &str,
        recent_events: u32,
    ) -> Result<SessionDetail> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::SessionShow {
                command_id,
                session_id: session_id.into(),
                recent_events,
            })
            .await?;
        match result {
            LocalResult::SessionDetail(detail) => Ok(*detail),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg session fork`
    pub async fn fork_session(&mut self, session_id: &str, cwd: Option<&Path>) -> Result<String> {
        let command_id = self.command_id();
        let result = self
            .request(LocalRequest::SessionFork {
                command_id,
                session_id: session_id.into(),
                cwd: cwd.map(path_to_string),
            })
            .await?;
        match result {
            LocalResult::SessionForked { session_id } => Ok(session_id.to_string()),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg session resume` — resume a stopped session.
    ///
    /// Returns whether native context was restored. When `context_restored =
    /// false` the session continues by replaying its history.
    pub async fn resume_session(&mut self, session_id: &str, force_replay: bool) -> Result<bool> {
        let command_id = self.command_id();
        let response = self
            .request_node(
                command_id,
                NodeRpcRequest::ResumeSession {
                    session_id: session_id.into(),
                    force_replay,
                },
            )
            .await?;
        match response {
            NodeRpcResponse::Resume { context_restored } => Ok(context_restored),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }
}

/// Normalize a simple success response (`Ack` / acceptance of an async
/// command) into an optional message.
fn ack_message(result: LocalResult) -> Result<Option<String>> {
    match result {
        LocalResult::Ack { message } => Ok(message),
        LocalResult::CommandAccepted { .. } => Ok(None),
        other => bail!("unexpected ipc result: {other:?}"),
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Print a simple table made of a header and rows to stdout.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| header.chars().count())
        .collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
    }

    let print_row = |cells: Vec<String>| {
        let line = cells
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let width = widths.get(index).copied().unwrap_or(0);
                let padding = width.saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(padding))
            })
            .collect::<Vec<_>>()
            .join("  ");
        println!("{}", line.trim_end());
    };

    print_row(headers.iter().map(|header| (*header).to_owned()).collect());
    for row in rows {
        print_row(row.clone());
    }
}

/// Format Unix epoch milliseconds as `YYYY-MM-DD HH:MM:SSZ` (UTC).
///
/// Minimal implementation for CLI listings (avoids adding a date-time crate
/// such as chrono).
pub fn format_unix_ms_utc(ms: i64) -> String {
    let total_seconds = ms.div_euclid(1000);
    let seconds_of_day = total_seconds.rem_euclid(86_400);
    let days = total_seconds.div_euclid(86_400);

    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z")
}

/// Compute (year, month, day) from days since 1970-01-01 (Howard Hinnant's
/// civil_from_days).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

/// Truncate a string to a maximum number of characters, marking the cut with
/// an ellipsis.
pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Render a session ID in short form (`0195f0ab-1234-...` → `0195f0ab`).
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_millis_as_utc() {
        assert_eq!(format_unix_ms_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(
            format_unix_ms_utc(1_700_000_000_000),
            "2023-11-14 22:13:20Z"
        );
        assert_eq!(
            format_unix_ms_utc(1_752_000_000_000),
            "2025-07-08 18:40:00Z"
        );
        // Negative epochs (before 1970) must not break either.
        assert_eq!(format_unix_ms_utc(-86_400_000), "1969-12-31 00:00:00Z");
    }

    #[test]
    fn truncates_long_text_on_char_boundary() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
        assert_eq!(truncate("日本語テキスト", 4), "日本語…"); // allow-japanese: multibyte truncation
    }

    #[test]
    fn shortens_session_ids() {
        assert_eq!(short_id("0195f0ab-1234-7abc-8def-0123456789ab"), "0195f0ab");
        assert_eq!(short_id("abc"), "abc");
    }

    #[test]
    fn treats_ack_and_command_accepted_as_success() {
        assert_eq!(
            ack_message(LocalResult::Ack {
                message: Some("done".to_owned()),
            })
            .expect("ack"),
            Some("done".to_owned())
        );
        // Acceptance of an async command (`SendPrompt` / `ControlSession`)
        assert_eq!(
            ack_message(LocalResult::CommandAccepted {
                session_id: Some("s1".into()),
            })
            .expect("command accepted"),
            None
        );
        // Any other response is unexpected and must fail.
        assert!(
            ack_message(LocalResult::Sessions {
                sessions: Vec::new(),
            })
            .is_err()
        );
    }
}

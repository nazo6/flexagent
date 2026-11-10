//! CLI subcommands (argument definitions and execution).
//!
//! The `usage` derives are the single source of truth; the KDL spec, shell
//! completions, manpage and Markdown reference are generated from
//! `__usage_spec__` (design: `docs/05-cli-and-pwa-ui.md` §1).

use anyhow::{Context, Result};
use fxg_protocol::common::{ClientSource, SessionControlAction, SessionStatus};
use fxg_protocol::events::UnifiedEventPayload;
use usage::Args;

use crate::client::{NodeClient, format_unix_ms_utc, print_table, short_id, truncate};

use super::*;

// ----------------------------------------------------------------------
// fxg session list / fxg session list
// ----------------------------------------------------------------------

/// Display options for the session list (shared by `fxg session list` and
/// `fxg session list`).
#[derive(Debug, Args)]
pub struct SessionListArgs {
    /// Include stopped sessions
    #[usage(short = 'a', long)]
    all: bool,
    /// Include archived sessions
    #[usage(long)]
    archived: bool,
    /// Filter by logical project ID
    #[usage(long)]
    project: Option<String>,
    /// Filter by node ID
    #[usage(long)]
    node: Option<String>,
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for SessionListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        let sessions = client.list_sessions(self.all, self.archived).await?;
        let sessions: Vec<_> = sessions
            .into_iter()
            .filter(|session| {
                self.project
                    .as_deref()
                    .is_none_or(|project| session.project_id.as_str() == project)
            })
            .filter(|session| {
                self.node
                    .as_deref()
                    .is_none_or(|node| session.node_id.as_str() == node)
            })
            .collect();

        if self.json {
            println!("{}", serde_json::to_string_pretty(&sessions)?);
            return Ok(());
        }

        if sessions.is_empty() {
            println!("(no sessions)");
            return Ok(());
        }

        let rows: Vec<Vec<String>> = sessions
            .iter()
            .map(|session| {
                vec![
                    short_id(session.session_id.as_str()),
                    truncate(session.node_id.as_str(), 16),
                    truncate(&session.agent_id, 16),
                    session.status.to_string(),
                    format_unix_ms_utc(session.updated_at),
                    truncate(&session.title, 32),
                ]
            })
            .collect();
        print_table(
            &["SESSION", "NODE", "AGENT", "STATUS", "UPDATED", "TITLE"],
            &rows,
        );
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg session <command>
// ----------------------------------------------------------------------

/// `fxg session <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct SessionArgs {
    #[usage(subcommand)]
    command: SessionCommands,
}

/// Session management subcommands.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum SessionCommands {
    /// Start a new session
    New(SessionNewArgs),
    /// List sessions
    List(SessionListArgs),
    /// Show session details, state and event statistics
    Show(SessionShowArgs),
    /// Send a prompt to an existing session non-interactively (headless)
    Prompt(SessionPromptArgs),
    /// Cancel the running turn
    Cancel(SessionCancelArgs),
    /// Stop the session gracefully
    Stop(SessionStopArgs),
    /// Force-terminate the session's child process tree
    Kill(SessionKillArgs),
    /// Resume a stopped session while keeping its conversation context
    Resume(SessionResumeArgs),
    /// Create a new session by forking the current conversation (ACP-native fork)
    Fork(SessionForkArgs),
    /// Import an existing agent session (native resume)
    Import(SessionImportArgs),
    /// Archive a session from the list (restorable)
    Archive(SessionArchiveArgs),
    /// Move an archived session back into the list
    Unarchive(SessionUnarchiveArgs),
    /// Delete a session (purges the conversation log; irreversible)
    Delete(SessionDeleteArgs),
}

/// `fxg session show <id>`
#[derive(Debug, Args)]
pub struct SessionShowArgs {
    /// Session ID
    session_id: String,
    /// Number of recent events to show
    #[usage(long, default = "20")]
    events: u32,
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for SessionShowArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        let detail = client.session_show(&self.session_id, self.events).await?;
        if self.json {
            let value = serde_json::json!({
                "session": detail.session,
                "event_count": detail.event_count,
                "recent_events": detail.recent_events,
                "pending_permissions": detail.pending_permissions,
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }

        let session = &detail.session;
        let usage_display = session
            .usage
            .as_ref()
            .map(|usage| {
                let percent = if usage.context_size > 0 {
                    format!(
                        " ({:.1}%)",
                        usage.used_tokens as f64 / usage.context_size as f64 * 100.0
                    )
                } else {
                    String::new()
                };
                let tokens = format!(
                    "{}/{} tokens{percent}",
                    usage.used_tokens, usage.context_size
                );
                match &usage.cost {
                    Some(cost) => format!("{tokens}, cost {:.4} {}", cost.amount, cost.currency),
                    None => tokens,
                }
            })
            .unwrap_or_else(|| "-".to_owned());
        let fields = [
            ("SESSION", session.session_id.to_string()),
            ("TITLE", session.title.clone()),
            ("PROJECT", session.project_id.to_string()),
            ("NODE", session.node_id.to_string()),
            ("AGENT", session.agent_id.clone()),
            ("STATUS", session.status.to_string()),
            (
                "MODE",
                session
                    .current_mode
                    .clone()
                    .unwrap_or_else(|| "-".to_owned()),
            ),
            ("USAGE", usage_display),
            ("PATH", session.local_path.clone()),
            (
                "BRANCH",
                session.git_branch.clone().unwrap_or_else(|| "-".to_owned()),
            ),
            (
                "WORKTREE",
                if session.is_worktree { "yes" } else { "no" }.to_owned(),
            ),
            ("EVENTS", detail.event_count.to_string()),
            ("CREATED", format_unix_ms_utc(session.created_at)),
            ("UPDATED", format_unix_ms_utc(session.updated_at)),
        ];
        for (label, value) in fields {
            println!("{label:<9}{value}");
        }

        if !detail.pending_permissions.is_empty() {
            println!("\nPending approvals:");
            let rows: Vec<Vec<String>> = detail
                .pending_permissions
                .iter()
                .map(|entry| {
                    vec![
                        truncate(&entry.request_id, 24),
                        truncate(&entry.tool_name, 20),
                        truncate(&entry.summary, 48),
                    ]
                })
                .collect();
            print_table(&["REQUEST", "TOOL", "SUMMARY"], &rows);
        }

        if !detail.recent_events.is_empty() {
            println!("\nRecent events:");
            for event in &detail.recent_events {
                println!(
                    "  {:>5}  {}  {}",
                    event.node_seq,
                    format_unix_ms_utc(event.created_at),
                    describe_event(&event.payload)
                );
            }
        }
        Ok(())
    }
}

/// Summarize an event into a single line for CLI display.
fn describe_event(payload: &UnifiedEventPayload) -> String {
    match payload {
        UnifiedEventPayload::SessionCreated {
            title, agent_id, ..
        } => format!("session_created agent={agent_id} title={title}"),
        UnifiedEventPayload::SessionTitleChanged { title } => {
            format!("title_changed {}", truncate(title, 40))
        }
        UnifiedEventPayload::SessionAgentBound { agent_session_id } => {
            format!("agent_bound {agent_session_id}")
        }
        UnifiedEventPayload::UserMessage {
            text,
            client_source,
            ..
        } => format!("user_message ({client_source}) {}", truncate(text, 60)),
        UnifiedEventPayload::AgentMessage { text, .. } => {
            format!("agent_message {}", truncate(text, 60))
        }
        UnifiedEventPayload::AgentThought { text, .. } => {
            format!("agent_thought {}", truncate(text, 60))
        }
        UnifiedEventPayload::ToolCall { title, status, .. } => {
            format!("tool_call [{status}] {}", truncate(title, 60))
        }
        UnifiedEventPayload::PlanUpdate { entries } => {
            format!("plan_update entries={}", entries.len())
        }
        UnifiedEventPayload::PermissionRequest {
            tool_name, summary, ..
        } => format!("permission_request [{tool_name}] {}", truncate(summary, 60)),
        UnifiedEventPayload::PermissionResolved {
            request_id,
            selected_option_id,
            ..
        } => format!("permission_resolved {request_id} → {selected_option_id}"),
        UnifiedEventPayload::ElicitationRequest { message, .. } => {
            format!("elicitation_request {}", truncate(message, 60))
        }
        UnifiedEventPayload::ElicitationResolved {
            elicitation_id,
            action,
            ..
        } => format!("elicitation_resolved {elicitation_id} → {action}"),
        UnifiedEventPayload::SessionArchived { archived } => {
            format!("session_archived archived={archived}")
        }
        UnifiedEventPayload::SessionPermissionPolicyChanged { policy } => {
            format!("session_permission_policy_changed policy={policy}")
        }
        UnifiedEventPayload::SessionDeleted {} => "session_deleted".to_owned(),
        UnifiedEventPayload::TerminalOutput {
            terminal_id,
            command,
            ..
        } => format!("terminal_output {terminal_id} {}", truncate(command, 40)),
        UnifiedEventPayload::TerminalInput { terminal_id, .. } => {
            format!("terminal_input {terminal_id}")
        }
        UnifiedEventPayload::CapabilitiesUpdated { .. } => "capabilities_updated".to_owned(),
        UnifiedEventPayload::StatusChanged {
            status,
            error_message,
        } => match error_message {
            Some(message) => format!("status_changed {status} ({message})"),
            None => format!("status_changed {status}"),
        },
        UnifiedEventPayload::BootstrapLog { line } => {
            format!("bootstrap {}", truncate(line, 60))
        }
        UnifiedEventPayload::UsageUpdated {
            used_tokens,
            context_size,
            cost,
        } => {
            let tokens = format!("usage {used_tokens}/{context_size}");
            match cost {
                Some(cost) => format!("{tokens} cost={:.4} {}", cost.amount, cost.currency),
                None => tokens,
            }
        }
        UnifiedEventPayload::TurnEnded { reason } => {
            format!("turn_ended {reason}")
        }
        UnifiedEventPayload::CompactionUpdated { status, .. } => {
            format!("compaction_updated {status}")
        }
    }
}

/// `fxg session prompt <id> <text>`
#[derive(Debug, Args)]
pub struct SessionPromptArgs {
    /// Session ID
    session_id: String,
    /// Prompt text to send
    text: String,
    /// Attach a file or image to the prompt (repeatable)
    #[usage(long = "attach")]
    attach: Vec<String>,
    /// Wait for the turn to finish and print the agent output
    #[usage(long)]
    wait: bool,
}

impl usage::RunAsync for SessionPromptArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        let input = build_prompt_input(&self.text, &self.attach)?;
        client
            .send_prompt(&self.session_id, &input, ClientSource::Cli)
            .await?;
        if !self.wait {
            println!("accepted ({})", short_id(&self.session_id));
            return Ok(());
        }
        wait_for_turn(&mut client, &self.session_id, &self.text).await
    }
}

/// Poll events until the turn finishes (`idle`) and print them
/// (`fxg session prompt --wait`).
async fn wait_for_turn(client: &mut NodeClient, session_id: &str, prompt: &str) -> Result<()> {
    let mut seen_prompt = false;
    let mut last_seq = 0u64;
    loop {
        let detail = client.session_show(session_id, 100).await?;
        for event in detail.recent_events {
            if event.node_seq > last_seq {
                last_seq = event.node_seq;
                if print_and_check_done(&event.payload, prompt, &mut seen_prompt) {
                    return Ok(());
                }
            }
        }
        if seen_prompt
            && matches!(
                detail.session.status,
                SessionStatus::Idle | SessionStatus::Error | SessionStatus::Stopped
            )
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Print an event and return `true` when the turn is done.
fn print_and_check_done(
    payload: &UnifiedEventPayload,
    prompt: &str,
    seen_prompt: &mut bool,
) -> bool {
    match payload {
        UnifiedEventPayload::UserMessage { text, .. } if text == prompt => {
            *seen_prompt = true;
        }
        UnifiedEventPayload::AgentMessage { text, .. } if !text.is_empty() => {
            println!("{text}");
        }
        UnifiedEventPayload::AgentThought { text, .. } if !text.is_empty() => {
            println!("Think ▸ {text}");
        }
        UnifiedEventPayload::ToolCall { title, status, .. } => {
            println!("• tool [{status}] {title}");
        }
        UnifiedEventPayload::PermissionRequest {
            tool_name, summary, ..
        } => {
            println!("! pending approval [{tool_name}] {summary} (review it in the Web UI)");
        }
        UnifiedEventPayload::StatusChanged {
            status,
            error_message,
        } => {
            if let Some(message) = error_message {
                println!("! error: {message}");
            }
            if *seen_prompt {
                return matches!(
                    status,
                    SessionStatus::Idle | SessionStatus::Error | SessionStatus::Stopped
                );
            }
        }
        _ => {}
    }
    false
}

/// `fxg session stop <id>`
#[derive(Debug, Args)]
pub struct SessionStopArgs {
    /// Session ID
    session_id: String,
}

impl usage::RunAsync for SessionStopArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        client
            .control_session(&self.session_id, SessionControlAction::Kill)
            .await?;
        println!("stopped session ({})", short_id(&self.session_id));
        Ok(())
    }
}

/// `fxg session cancel <id>`
#[derive(Debug, Args)]
pub struct SessionCancelArgs {
    /// Session ID
    session_id: String,
}

impl usage::RunAsync for SessionCancelArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        client
            .control_session(&self.session_id, SessionControlAction::Cancel)
            .await?;
        println!("cancelled current turn ({})", short_id(&self.session_id));
        Ok(())
    }
}

/// `fxg session kill <id>`
#[derive(Debug, Args)]
pub struct SessionKillArgs {
    /// Session ID
    session_id: String,
}

impl usage::RunAsync for SessionKillArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        client
            .control_session(&self.session_id, SessionControlAction::Kill)
            .await?;
        println!(
            "killed session process tree ({})",
            short_id(&self.session_id)
        );
        Ok(())
    }
}

/// `fxg session import <agent-session-id>`
#[derive(Debug, Args)]
pub struct SessionImportArgs {
    /// Agent-side session ID to import
    agent_session_id: String,
    /// Agent ID (aliases allowed)
    #[usage(long)]
    agent: String,
    /// Working directory (defaults to the current directory)
    #[usage(long)]
    cwd: Option<String>,
    /// Session title
    #[usage(long)]
    title: Option<String>,
}

impl usage::RunAsync for SessionImportArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = match self.cwd {
            Some(cwd) => cwd,
            None => std::env::current_dir()
                .context("failed to resolve the current directory")?
                .to_string_lossy()
                .into_owned(),
        };
        let mut client = NodeClient::connect().await?;
        let session_id = client
            .import_session(
                &self.agent,
                &self.agent_session_id,
                &cwd,
                self.title.as_deref(),
            )
            .await?;
        println!("imported session: {session_id}");
        Ok(())
    }
}

/// `fxg session archive <id>`
#[derive(Debug, Args)]
pub struct SessionArchiveArgs {
    /// Session ID
    session_id: String,
}

impl usage::RunAsync for SessionArchiveArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        let archived_at = client.archive_session(&self.session_id, true).await?;
        println!("archived session ({})", short_id(&self.session_id));
        if let Some(archived_at) = archived_at {
            println!("  archived_at: {}", format_unix_ms_utc(archived_at));
        }
        Ok(())
    }
}

/// `fxg session unarchive <id>`
#[derive(Debug, Args)]
pub struct SessionUnarchiveArgs {
    /// Session ID
    session_id: String,
}

impl usage::RunAsync for SessionUnarchiveArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        client.archive_session(&self.session_id, false).await?;
        println!("unarchived session ({})", short_id(&self.session_id));
        Ok(())
    }
}

/// `fxg session delete <id>`
#[derive(Debug, Args)]
pub struct SessionDeleteArgs {
    /// Session ID
    session_id: String,
}

impl usage::RunAsync for SessionDeleteArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        client.delete_session(&self.session_id).await?;
        println!(
            "deleted session ({}) — the conversation log is purged and cannot be restored",
            short_id(&self.session_id)
        );
        Ok(())
    }
}

/// `fxg session fork <id>`
#[derive(Debug, Args)]
pub struct SessionForkArgs {
    /// Session ID to fork from
    session_id: String,
    /// Fork into a new worktree
    #[usage(short = 'w', long)]
    worktree: Option<String>,
    /// Base branch used when creating a new worktree
    #[usage(long)]
    base: Option<String>,
    /// Print only the session ID and do not open a browser
    #[usage(short = 'q', long)]
    quiet: bool,
    /// Do not open the Web UI in the default browser after the fork
    #[usage(long)]
    no_open: bool,
}

impl usage::RunAsync for SessionForkArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        let cwd = match &self.worktree {
            Some(branch) => {
                let dir = std::env::current_dir().context("failed to resolve current directory")?;
                Some(create_worktree(&mut client, &dir, branch, self.base.clone()).await?)
            }
            None => None,
        };

        let session_id = client
            .fork_session(&self.session_id, cwd.as_deref())
            .await?;
        if self.quiet {
            println!("{session_id}");
            return Ok(());
        }
        open_session_url(&session_id, self.no_open)
    }
}

/// `fxg session resume <id>`
#[derive(Debug, Args)]
pub struct SessionResumeArgs {
    /// Session ID to resume
    session_id: String,
    /// Print only the session ID and do not open a browser
    #[usage(short = 'q', long)]
    quiet: bool,
    /// Do not open the Web UI in the default browser after resuming
    #[usage(long)]
    no_open: bool,
    /// Skip native restoration (session/resume, session/load, existing-session
    /// bind) and continue by replaying history
    #[usage(long)]
    replay: bool,
}

impl usage::RunAsync for SessionResumeArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        let context_restored = client.resume_session(&self.session_id, self.replay).await?;
        if context_restored {
            eprintln!("resumed (agent context restored): {}", self.session_id);
        } else {
            eprintln!("resumed with replayed history: {}", self.session_id);
        }
        if self.quiet {
            println!("{}", self.session_id);
            return Ok(());
        }
        open_session_url(&self.session_id, self.no_open)
    }
}

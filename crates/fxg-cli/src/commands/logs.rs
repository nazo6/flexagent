//! `fxg logs` command.

use std::io::{Read, Seek, SeekFrom, Write};

use anyhow::{Context, Result, bail};
use usage::Args;

use crate::client::NodeClient;

/// `fxg logs [node|hub]`
#[derive(Debug, Args)]
pub struct LogsArgs {
    /// Target: node (default) or hub
    target: Option<String>,
    /// Only show logs related to this session (node only; agent stderr)
    #[usage(long)]
    session: Option<String>,
    /// Follow appended log lines
    #[usage(short = 'f', long)]
    follow: bool,
    /// Number of lines to show
    #[usage(long, default = "100")]
    lines: usize,
}

impl usage::RunAsync for LogsArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let target = match self.target.as_deref() {
            None | Some("node") => "node",
            Some("hub") => "hub",
            Some(other) => bail!("unknown target: {other} (node | hub)"),
        };

        let env = fxg_protocol::config::process_env;
        let role = if target == "hub" {
            fxg_protocol::paths::Role::Hub
        } else {
            fxg_protocol::paths::Role::Node
        };
        let paths = fxg_protocol::paths::Paths::resolve(&env, role);

        if let Some(session_id) = self.session.as_deref() {
            if target != "node" {
                bail!("--session is only available for the node log");
            }
            let mut client = NodeClient::connect().await?;
            let detail = client.session_show(session_id, 1).await?;
            let agent_id = detail.session.agent_id;
            let diagnostics = client.diagnostics().await?;
            let Some(agent) = diagnostics
                .agents
                .iter()
                .find(|agent| agent.agent_id == agent_id)
            else {
                println!("(no agent stderr captured for {agent_id})");
                return Ok(());
            };
            let start = agent.stderr_tail.len().saturating_sub(self.lines);
            for line in &agent.stderr_tail[start..] {
                println!("{line}");
            }
            return Ok(());
        }

        let path = paths.log_path();
        if !path.exists() {
            println!("({target} log not found: {})", path.display());
            return Ok(());
        }

        if !self.follow {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(self.lines);
            for line in &lines[start..] {
                println!("{line}");
            }
            return Ok(());
        }

        // Follow mode: print the initial tail, then poll for appended bytes.
        let mut file = std::fs::File::open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        let initial = std::fs::read_to_string(&path)?;
        let lines: Vec<&str> = initial.lines().collect();
        let start = lines.len().saturating_sub(self.lines);
        for line in &lines[start..] {
            println!("{line}");
        }
        let mut offset = match file.metadata() {
            Ok(metadata) => metadata.len(),
            Err(err) => {
                tracing::warn!(path = %path.display(), "failed to read log metadata: {err}");
                0
            }
        };
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let len = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(offset);
            if len < offset {
                offset = 0;
            }
            if len > offset {
                file.seek(SeekFrom::Start(offset))?;
                let mut appended = String::new();
                file.read_to_string(&mut appended)?;
                print!("{appended}");
                std::io::stdout().flush()?;
                offset = len;
            }
        }
    }
}

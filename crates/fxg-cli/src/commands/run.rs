//! CLI subcommands (argument definitions and execution).
//!
//! The `usage` derives are the single source of truth; the KDL spec, shell
//! completions, manpage and Markdown reference are generated from
//! `__usage_spec__` (design: `docs/05-cli-and-pwa-ui.md` §1).
use std::path::Path;

use anyhow::{Context, Result, bail};
use fxg_protocol::common::{AttachmentMeta, ClientSource, PromptInput};
use usage::Args;

use crate::client::DaemonClient;

use super::*;

// ----------------------------------------------------------------------
// fxg run
// ----------------------------------------------------------------------

/// Builds a prompt from text and `--attach` paths.
pub(super) fn build_prompt_input(text: &str, attachments: &[String]) -> Result<PromptInput> {
    let mut input = PromptInput::text(text);
    for path in attachments {
        let canonical = std::fs::canonicalize(path)
            .with_context(|| format!("failed to resolve attachment {path}"))?;
        let metadata = std::fs::metadata(&canonical)
            .with_context(|| format!("failed to read attachment {}", canonical.display()))?;
        if !metadata.is_file() {
            bail!("attachment is not a file: {}", canonical.display());
        }
        let file_name = canonical
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".to_owned());
        input.attachments.push(AttachmentMeta {
            file_name,
            mime_type: guess_mime(&canonical, metadata.len()).to_owned(),
            size_bytes: metadata.len(),
            local_path: Some(canonical.to_string_lossy().into_owned()),
            data_b64: None,
        });
    }
    Ok(input)
}

/// Best-effort MIME type for a local attachment.
fn guess_mime(path: &Path, size: u64) -> &'static str {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "md" | "markdown" => "text/markdown",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "rs" => "text/rust",
        "ts" | "tsx" => "text/typescript",
        "js" | "jsx" => "text/javascript",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "sh" => "text/x-shellscript",
        "txt" | "log" => "text/plain",
        _ if size <= 256 * 1024 => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Arguments of `fxg run <agent>`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Agent ID (`opencode` / `claude`, etc.)
    agent: String,
    /// Send an initial prompt
    #[usage(short = 'p', long)]
    prompt: Option<String>,
    /// Attach a file or image to the initial prompt (repeatable)
    #[usage(long = "attach")]
    attach: Vec<String>,
    /// Print only the session ID and do not open a browser
    #[usage(short = 'd', long)]
    detach: bool,
    /// Do not open the Web UI in the default browser after start
    #[usage(long)]
    no_open: bool,
    /// Initial mode (`code` / `plan`, etc.)
    #[usage(long)]
    mode: Option<String>,
    /// Create or reuse a worktree for the session
    #[usage(short = 'w', long)]
    worktree: Option<String>,
    /// Base branch used when creating a new worktree
    #[usage(long)]
    base: Option<String>,
    /// Start through an ephemeral VM provisioner (runs on the central server
    /// host; requires the central server)
    #[usage(long)]
    provisioner: Option<String>,
    /// Arguments passed through to the agent process
    #[usage(value_name = "EXTRA_ARGS", double_dash = "required")]
    extra_args: Vec<String>,
}

impl usage::RunAsync for RunArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;

        // Ephemeral VM provisioner: start through the central server API
        // (the provisioner runs as a child process on the server host)
        if let Some(provisioner) = &self.provisioner {
            return run_with_provisioner(&self, provisioner, &cwd, &env).await;
        }

        let mut client = DaemonClient::connect().await?;
        let cwd = match &self.worktree {
            Some(branch) => create_worktree(&mut client, &cwd, branch, self.base.clone()).await?,
            None => cwd,
        };

        let session_id = client
            .ensure_session(&cwd, &self.agent, &self.extra_args, self.mode.as_deref())
            .await?;

        let input = build_prompt_input(self.prompt.as_deref().unwrap_or(""), &self.attach)?;
        if !input.is_empty() {
            client
                .send_prompt(&session_id, &input, ClientSource::Cli)
                .await?;
        }

        if self.detach {
            println!("{session_id}");
            return Ok(());
        }
        open_session_url(&session_id, self.no_open)
    }
}

/// `fxg run <agent> --provisioner <name>` (through the central server API).
async fn run_with_provisioner(
    args: &RunArgs,
    provisioner: &str,
    cwd: &Path,
    env: fxg_protocol::config::EnvLookup<'_>,
) -> Result<()> {
    let server = crate::server_api::ServerClient::from_config(env)?;

    // Resolve the logical project of the current directory (the Git remote is
    // the source of truth)
    let env = fxg_protocol::config::process_env;
    let db = node_db(&env).await?;
    let project = fxg_node::resolve_project(cwd, "cli", &db)
        .await
        .context("failed to resolve logical project (run inside a Git repository)")?;

    // `-w/--worktree`: in an ephemeral VM this selects the branch to clone
    // instead of creating a new worktree
    let worktree = match (&args.worktree, &args.base) {
        (Some(branch), base) => Some(fxg_protocol::client_api::WorktreeSpec {
            branch: branch.clone(),
            base_branch: base.clone(),
        }),
        (None, _) => None,
    };

    let request = fxg_protocol::client_api::CreateSessionRequest {
        command_id: format!("cli-{}", fxg_protocol::util::uuid_v7()).into(),
        project_id: project.project_id.clone().into(),
        node_id: None,
        provisioner: Some(provisioner.to_owned()),
        local_path: None,
        worktree,
        agent_id: args.agent.clone(),
        initial_prompt: {
            let input = build_prompt_input(args.prompt.as_deref().unwrap_or(""), &args.attach)?;
            if input.is_empty() { None } else { Some(input) }
        },
        initial_config: Vec::new(),
        mode: args.mode.clone(),
        extra_args: if args.extra_args.is_empty() {
            None
        } else {
            Some(args.extra_args.clone())
        },
        fork: None,
    };
    let response = server.create_session(&request).await?;
    eprintln!(
        "provisioning session {} via '{provisioner}' (bootstrap logs are available in the Web UI / `fxg provisioners test`)",
        response.session_id
    );
    if args.detach {
        println!("{}", response.session_id);
    } else {
        open_session_url(response.session_id.as_str(), args.no_open)?;
    }
    Ok(())
}

//! CLI subcommands (argument definitions and execution).
//!
//! The `usage` derives are the single source of truth; the KDL spec, shell
//! completions, manpage and Markdown reference are generated from
//! `__usage_spec__` (design: `docs/05-cli-and-pwa-ui.md` §1).
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use usage::Args;

use crate::client::{DaemonClient, print_table};

// ----------------------------------------------------------------------
// fxg project <command>
// ----------------------------------------------------------------------

/// `fxg project <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct ProjectArgs {
    #[usage(subcommand)]
    command: ProjectCommands,
}

/// Logical project management subcommands.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum ProjectCommands {
    /// Show the logical project info for the current directory
    Info(ProjectInfoArgs),
    /// List registered logical projects
    List(ProjectListArgs),
    /// Link the current directory to the given logical project ID
    Link(ProjectLinkArgs),
    /// Scan and register the Git repositories under a directory
    Scan(ProjectScanArgs),
}

/// `fxg project info`
#[derive(Debug, Args)]
pub struct ProjectInfoArgs {
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for ProjectInfoArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let project = client.project_info(&cwd).await?;

        if self.json {
            println!("{}", serde_json::to_string_pretty(&project)?);
            return Ok(());
        }

        println!("project_id:      {}", project.project_id);
        println!("name:            {}", project.name);
        println!(
            "canonical_git_url: {}",
            project.canonical_git_url.as_deref().unwrap_or("-")
        );
        println!(
            "git_root:        {}",
            project.git_root.as_deref().unwrap_or("-")
        );
        println!(
            "subpath:         {}",
            project.relative_subpath.as_deref().unwrap_or("-")
        );
        println!("local_path:      {}", project.local_path);
        println!(
            "identity:        {}{}",
            project.identity.kind,
            project
                .identity
                .key
                .as_deref()
                .map(|key| format!(" ({key})"))
                .unwrap_or_default()
        );
        Ok(())
    }
}

/// `fxg project list`
#[derive(Debug, Args)]
pub struct ProjectListArgs {
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for ProjectListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let projects = client.list_projects().await?;

        if self.json {
            println!("{}", serde_json::to_string_pretty(&projects)?);
            return Ok(());
        }

        if projects.is_empty() {
            println!("(no projects; register one with `fxg project scan` or `fxg project link`)");
            return Ok(());
        }

        for project in &projects {
            println!("{}  ({})", project.project_id, project.name);
            for binding in &project.bindings {
                let branch = binding.git_branch.as_deref().unwrap_or("-");
                let kind = if binding.is_worktree {
                    "worktree"
                } else {
                    "main"
                };
                println!(
                    "  - {} [{}] {} ({})",
                    binding.local_path, branch, kind, binding.node_id
                );
            }
        }
        Ok(())
    }
}

/// `fxg project link <project-id>`
#[derive(Debug, Args)]
pub struct ProjectLinkArgs {
    /// Logical project ID to link to (e.g. github.com/nazo6/flexagent)
    project_id: String,
}

impl usage::RunAsync for ProjectLinkArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let message = client.link_project(&cwd, &self.project_id).await?;
        println!(
            "{}",
            message.unwrap_or_else(|| format!("linked to {}", self.project_id))
        );
        Ok(())
    }
}

/// `fxg project scan [dir]`
#[derive(Debug, Args)]
pub struct ProjectScanArgs {
    /// Directory to scan (defaults to project_scan_dirs in config.toml)
    dir: Option<String>,
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for ProjectScanArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = DaemonClient::connect().await?;
        let (projects, scanned_dirs) = client.scan_projects(self.dir).await?;

        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "scanned_dirs": scanned_dirs,
                    "projects": projects,
                }))?
            );
            return Ok(());
        }

        println!("scanned: {}", scanned_dirs.join(", "));
        for project in &projects {
            println!(
                "  {}  ({} bindings)",
                project.project_id,
                project.bindings.len()
            );
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------
// fxg worktree <command>
// ----------------------------------------------------------------------

/// `fxg worktree <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct WorktreeArgs {
    #[usage(subcommand)]
    command: WorktreeCommands,
}

/// Git worktree management subcommands.
#[derive(Debug, usage::Subcommands)]
#[usage(run_async)]
pub enum WorktreeCommands {
    /// List worktrees
    List(WorktreeListArgs),
    /// Create a new worktree
    Add(WorktreeAddArgs),
    /// Remove a worktree
    Remove(WorktreeRemoveArgs),
    /// Clean up metadata of worktrees that were deleted
    Prune(WorktreePruneArgs),
}

/// `fxg worktree list`
#[derive(Debug, Args)]
pub struct WorktreeListArgs {
    /// Filter by logical project ID
    #[usage(long)]
    project: Option<String>,
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for WorktreeListArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let worktrees = client
            .list_worktrees(Some(cwd.to_string_lossy().into_owned()), self.project)
            .await?;

        if self.json {
            println!("{}", serde_json::to_string_pretty(&worktrees)?);
            return Ok(());
        }

        if worktrees.is_empty() {
            println!("(no worktrees)");
            return Ok(());
        }

        let rows: Vec<Vec<String>> = worktrees
            .iter()
            .map(|worktree| {
                vec![
                    if worktree.is_main { "main" } else { "worktree" }.to_owned(),
                    worktree
                        .branch
                        .clone()
                        .unwrap_or_else(|| "(detached)".to_owned()),
                    worktree
                        .head_commit
                        .as_deref()
                        .map(|commit| commit.chars().take(8).collect())
                        .unwrap_or_else(|| "-".to_owned()),
                    worktree.path.clone(),
                ]
            })
            .collect();
        print_table(&["KIND", "BRANCH", "HEAD", "PATH"], &rows);
        Ok(())
    }
}

/// `fxg worktree add <branch>`
#[derive(Debug, Args)]
pub struct WorktreeAddArgs {
    /// Branch name to create (e.g. feat/auth)
    branch: String,
    /// Base branch (defaults to the current HEAD)
    #[usage(long)]
    base: Option<String>,
    /// Logical project ID (defaults to resolving from the current directory)
    #[usage(long)]
    project: Option<String>,
}

impl usage::RunAsync for WorktreeAddArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let (path, branch, created) = client
            .add_worktree(&cwd, self.project, &self.branch, self.base)
            .await?;

        if created {
            println!("created worktree {branch} at {path}");
        } else {
            println!("reusing existing worktree {branch} at {path}");
        }
        Ok(())
    }
}

/// `fxg worktree remove <target>`
#[derive(Debug, Args)]
pub struct WorktreeRemoveArgs {
    /// Branch name or path to remove
    target: String,
    /// Force removal even with uncommitted changes
    #[usage(short = 'f', long)]
    force: bool,
}

impl usage::RunAsync for WorktreeRemoveArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let message = client
            .remove_worktree(&cwd, &self.target, self.force)
            .await?;
        println!("{}", message.unwrap_or_else(|| "removed".to_owned()));
        Ok(())
    }
}

/// `fxg worktree prune`
#[derive(Debug, Args)]
pub struct WorktreePruneArgs {}

impl usage::RunAsync for WorktreePruneArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let cwd = std::env::current_dir().context("failed to resolve current directory")?;
        let mut client = DaemonClient::connect().await?;
        let message = client.prune_worktrees(&cwd).await?;
        println!("{}", message.unwrap_or_else(|| "pruned".to_owned()));
        Ok(())
    }
}

/// Create or reuse a worktree and return its path (`fxg run -w` /
/// `fxg session fork -w`).
pub(super) async fn create_worktree(
    client: &mut DaemonClient,
    cwd: &Path,
    branch: &str,
    base_branch: Option<String>,
) -> Result<PathBuf> {
    let (path, branch, created) = client.add_worktree(cwd, None, branch, base_branch).await?;
    if created {
        eprintln!("created worktree {branch} at {path}");
    } else {
        eprintln!("reusing existing worktree {branch} at {path}");
    }
    Ok(PathBuf::from(path))
}

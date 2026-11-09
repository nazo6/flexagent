//! Resolving, creating, and removing Git worktrees.
//!
//! Design: `docs/01-architecture-and-sync.md` §5, `docs/05-cli-and-pwa-ui.md`
//! §2.2/§2.3.
//!
//! - Worktrees created by `fxg` are aggregated under
//!   `~/.flexagent/worktrees/<project>/<branch>` by default so that the parent folder
//!   is not cluttered (`worktree_dir_template` can change this).
//! - The destination is fixed to `~/.flexagent/worktrees/{project}/{branch}`.
//! - Detection uses `git worktree list --porcelain` and lists every worktree including
//!   the main repository.

use std::path::{Path, PathBuf};

use crate::error::NodeError;
use crate::git;

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    /// Absolute path of the worktree
    pub path: PathBuf,
    /// HEAD commit
    pub head: Option<String>,
    /// Name of the checked-out branch (short name without `refs/heads/`)
    pub branch: Option<String>,
    /// Whether this is a bare repository
    pub is_bare: bool,
    /// Whether HEAD is detached
    pub is_detached: bool,
    /// Whether this is the main repository (the first entry)
    pub is_main: bool,
}

/// Retrieves the worktree list (the main repository comes first).
pub async fn list_worktrees(repo: &Path) -> Result<Vec<WorktreeEntry>, NodeError> {
    ensure_repository(repo).await?;
    let output = git::run_git(repo, &["worktree", "list", "--porcelain"]).await?;
    Ok(parse_worktree_list(&output))
}

/// Parses the output of `git worktree list --porcelain`.
pub fn parse_worktree_list(output: &str) -> Vec<WorktreeEntry> {
    let mut entries: Vec<WorktreeEntry> = Vec::new();
    let mut current = WorktreeEntry {
        path: PathBuf::new(),
        head: None,
        branch: None,
        is_bare: false,
        is_detached: false,
        is_main: false,
    };
    let mut has_current = false;

    let flush =
        |current: &mut WorktreeEntry, has_current: &mut bool, entries: &mut Vec<WorktreeEntry>| {
            if *has_current {
                current.is_main = entries.is_empty();
                entries.push(current.clone());
                *has_current = false;
            }
        };

    for line in output.lines() {
        if line.trim().is_empty() {
            flush(&mut current, &mut has_current, &mut entries);
            current = WorktreeEntry {
                path: PathBuf::new(),
                head: None,
                branch: None,
                is_bare: false,
                is_detached: false,
                is_main: false,
            };
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            current.path = PathBuf::from(rest.trim());
            has_current = true;
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            current.head = Some(rest.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            let branch = rest.trim();
            current.branch = Some(
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_owned(),
            );
        } else if line.trim() == "bare" {
            current.is_bare = true;
        } else if line.trim() == "detached" {
            current.is_detached = true;
        }
    }
    flush(&mut current, &mut has_current, &mut entries);
    entries
}

/// Input used to resolve placeholders in the worktree location template.
#[derive(Debug, Clone)]
pub struct WorktreePathContext {
    /// `~/.flexagent`
    pub fxg_home: PathBuf,
    /// Logical project ID (`github.com/nazo6/flexagent`, etc.)
    pub project_id: String,
    /// Path of the main repository
    pub repo_path: PathBuf,
    /// Branch name to create (`feat/auth`, etc.)
    pub branch: String,
}

/// Normalizes slashes and similar characters to hyphens for `{project}` / `{branch}`
/// in templates.
pub fn slugify_project(project_id: &str) -> String {
    project_id
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '#' => '-',
            other => other,
        })
        .collect()
}

/// Normalizes a branch name for use as a directory name (`feat/auth` -> `feat-auth`).
pub fn slugify_branch(branch: &str) -> String {
    branch
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' => '-',
            other => other,
        })
        .collect()
}

/// Resolves `worktree_dir_template`.
///
/// Available placeholders: `{fxg_home}` / `{project}` / `{repo}` / `{repo_parent}` /
/// `{branch}`.
pub fn resolve_worktree_dir(template: &str, ctx: &WorktreePathContext) -> PathBuf {
    let repo_name = ctx
        .repo_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let repo_parent = ctx
        .repo_path
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default();

    let resolved = template
        .replace("{fxg_home}", &ctx.fxg_home.to_string_lossy())
        .replace("{project}", &slugify_project(&ctx.project_id))
        .replace("{repo_parent}", &repo_parent)
        .replace("{repo}", &repo_name)
        .replace("{branch}", &slugify_branch(&ctx.branch));
    PathBuf::from(resolved)
}

/// Request to create a worktree.
#[derive(Debug, Clone)]
pub struct WorktreeAddRequest<'a> {
    /// Path of the main repository (Git root)
    pub repo: &'a Path,
    /// Logical project ID
    pub project_id: &'a str,
    /// Branch name to create
    pub branch: &'a str,
    /// Base branch (current HEAD if omitted)
    pub base_branch: Option<&'a str>,
    /// `~/.flexagent`
    pub fxg_home: &'a Path,
}

/// Result of creating a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeAddOutcome {
    /// Path of the worktree
    pub path: PathBuf,
    /// Branch name
    pub branch: String,
    /// Whether it was newly created (`false` means an existing worktree was reused)
    pub created: bool,
}

/// Creates a worktree (reuses an existing worktree on the same branch if present).
pub async fn ensure_worktree(
    request: &WorktreeAddRequest<'_>,
) -> Result<WorktreeAddOutcome, NodeError> {
    ensure_repository(request.repo).await?;

    // Reuse an existing worktree (same branch)
    let existing = list_worktrees(request.repo).await?;
    if let Some(entry) = existing
        .iter()
        .find(|entry| entry.branch.as_deref() == Some(request.branch) && entry.path != request.repo)
    {
        // `git worktree list` returns symlink-resolved paths (e.g. /var ->
        // /private/var on macOS), so normalize the returned path to match
        let path = fxg_pty::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone());
        return Ok(WorktreeAddOutcome {
            path,
            branch: request.branch.to_owned(),
            created: false,
        });
    }

    // Fixed destination: ~/.flexagent/worktrees/{project}/{branch}
    let path = resolve_worktree_dir(
        "{fxg_home}/worktrees/{project}/{branch}",
        &WorktreePathContext {
            fxg_home: request.fxg_home.to_path_buf(),
            project_id: request.project_id.to_owned(),
            repo_path: request.repo.to_path_buf(),
            branch: request.branch.to_owned(),
        },
    );
    if path.exists()
        && path
            .read_dir()
            .map(|mut dir| dir.next().is_some())
            .unwrap_or(false)
    {
        return Err(NodeError::InvalidWorktree(format!(
            "worktree directory already exists and is not empty: {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| NodeError::io(parent, source))?;
    }

    // If the branch already exists, check it out; otherwise create it with -b
    let branch_exists = git::try_git(
        request.repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{}", request.branch),
        ],
    )
    .await?
    .is_some();

    let path_str = path.to_string_lossy().into_owned();
    if branch_exists {
        git::run_git(
            request.repo,
            &["worktree", "add", &path_str, request.branch],
        )
        .await?;
    } else {
        let mut args = vec!["worktree", "add", "-b", request.branch, path_str.as_str()];
        if let Some(base) = request.base_branch {
            args.push(base);
        }
        git::run_git(request.repo, &args).await?;
    }

    // Normalize to the symlink-resolved path (to match what git records)
    let path = fxg_pty::canonicalize(&path).unwrap_or(path);

    Ok(WorktreeAddOutcome {
        path,
        branch: request.branch.to_owned(),
        created: true,
    })
}

/// Removes a worktree (`git worktree remove`).
pub async fn remove_worktree(repo: &Path, path: &Path, force: bool) -> Result<(), NodeError> {
    ensure_repository(repo).await?;
    let path_str = path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path_str.as_str());
    git::run_git(repo, &args).await?;
    Ok(())
}

/// Cleans up worktree bookkeeping for deleted directories.
pub async fn prune_worktrees(repo: &Path) -> Result<String, NodeError> {
    ensure_repository(repo).await?;
    git::run_git(repo, &["worktree", "prune"]).await
}

async fn ensure_repository(repo: &Path) -> Result<(), NodeError> {
    if !git::is_inside_work_tree(repo).await? {
        return Err(NodeError::NotARepository(repo.to_path_buf()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    fn add_request<'a>(
        repo: &'a Path,
        fxg_home: &'a Path,
        branch: &'a str,
    ) -> WorktreeAddRequest<'a> {
        WorktreeAddRequest {
            repo,
            project_id: "github.com/nazo6/flexagent",
            branch,
            base_branch: None,
            fxg_home,
        }
    }

    #[test]
    fn parses_worktree_porcelain() {
        let output = "\
worktree /home/nazo/src/flexagent
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /home/nazo/.flexagent/worktrees/github.com-nazo6-flexagent/feat-auth
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feat/auth
";
        let entries = parse_worktree_list(output);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_main);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert!(!entries[1].is_main);
        assert_eq!(entries[1].branch.as_deref(), Some("feat/auth"));
        assert_eq!(
            entries[1].path,
            PathBuf::from("/home/nazo/.flexagent/worktrees/github.com-nazo6-flexagent/feat-auth")
        );
    }

    #[test]
    fn parses_detached_and_bare_entries() {
        let output = "\
worktree /srv/bare.git
bare

worktree /home/nazo/scratch
HEAD 3333333333333333333333333333333333333333
detached
";
        let entries = parse_worktree_list(output);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_bare);
        assert!(entries[1].is_detached);
        assert!(entries[1].branch.is_none());
    }

    #[test]
    fn resolves_template_placeholders() {
        let ctx = WorktreePathContext {
            fxg_home: PathBuf::from("/home/nazo/.flexagent"),
            project_id: "github.com/nazo6/flexagent".to_owned(),
            repo_path: PathBuf::from("/home/nazo/src/flexagent"),
            branch: "feat/auth".to_owned(),
        };
        assert_eq!(
            resolve_worktree_dir("{fxg_home}/worktrees/{project}/{branch}", &ctx),
            PathBuf::from("/home/nazo/.flexagent/worktrees/github.com-nazo6-flexagent/feat-auth")
        );
        assert_eq!(
            resolve_worktree_dir("{repo_parent}/{repo}-{branch}", &ctx),
            PathBuf::from("/home/nazo/src/flexagent-feat-auth")
        );
        assert_eq!(slugify_project("local:node:abc"), "local-node-abc");
        assert_eq!(slugify_branch("fix/bug/1"), "fix-bug-1");
    }

    #[tokio::test]
    async fn creates_and_removes_worktree_at_fixed_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        let home = dir.path().join("fxg-home");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        testutil::init_test_repo(&repo).await;

        let request = add_request(&repo, &home, "feat/auth");
        let outcome = ensure_worktree(&request).await.expect("ensure");

        assert!(outcome.created);
        assert_eq!(
            outcome.path,
            fxg_pty::canonicalize(
                home.join("worktrees")
                    .join("github.com-nazo6-flexagent")
                    .join("feat-auth")
            )
            .expect("canonicalize expected path")
        );
        assert!(outcome.path.join("README.md").is_file());

        // Appears in the list
        let entries = list_worktrees(&repo).await.expect("list");
        assert!(
            entries
                .iter()
                .any(|entry| entry.branch.as_deref() == Some("feat/auth"))
        );

        // The second call reuses it (does not create)
        let again = ensure_worktree(&request).await.expect("ensure again");
        assert!(!again.created);
        assert_eq!(again.path, outcome.path);

        // An untracked file exists, so it cannot be removed without force
        std::fs::write(outcome.path.join(".env"), "SECRET=1\n").expect("write .env");
        assert!(
            remove_worktree(&repo, &outcome.path, false).await.is_err(),
            "an untracked file exists, so force is required"
        );
        remove_worktree(&repo, &outcome.path, true)
            .await
            .expect("remove");
        let entries = list_worktrees(&repo).await.expect("list");
        assert_eq!(entries.len(), 1);
        prune_worktrees(&repo).await.expect("prune");
    }

    #[tokio::test]
    async fn base_branch_is_used_for_new_branch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        let home = dir.path().join("fxg-home");
        std::fs::create_dir_all(&repo).expect("mkdir");
        testutil::init_test_repo(&repo).await;
        // Create a base branch derived from main
        git::run_git(&repo, &["checkout", "-q", "-b", "develop"])
            .await
            .expect("checkout");
        std::fs::write(repo.join("dev.txt"), "dev\n").expect("write");
        git::run_git(&repo, &["add", "-A"]).await.expect("add");
        git::run_git(&repo, &["commit", "-q", "-m", "dev"])
            .await
            .expect("commit");
        git::run_git(&repo, &["checkout", "-q", "main"])
            .await
            .expect("checkout main");

        let request = WorktreeAddRequest {
            repo: &repo,
            project_id: "github.com/nazo6/flexagent",
            branch: "feat/from-develop",
            base_branch: Some("develop"),
            fxg_home: &home,
        };
        let outcome = ensure_worktree(&request).await.expect("ensure");
        assert!(
            outcome.path.join("dev.txt").is_file(),
            "the contents of base_branch are checked out"
        );
    }

    #[tokio::test]
    async fn rejects_non_repository() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = list_worktrees(dir.path()).await.expect_err("must fail");
        assert!(matches!(err, NodeError::NotARepository(_)));
    }
}

//! Workspace-scoped file browsing and Git inspection.
//!
//! Every request is resolved against a directory registered on this node
//! (`project_node_bindings`). The access root is the workspace's Git toplevel,
//! or the registered directory when it is not inside a work tree, so a
//! worktree can never reach the main repository and vice versa.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use fxg_protocol::client_api::{
    WorkspaceEntryKind, WorkspaceFileContent, WorkspaceFileView, WorkspaceFsEntry,
    WorkspaceFsListResponse, WorkspaceFsMatch, WorkspaceFsSearchResponse,
    WorkspaceGitCommitResponse, WorkspaceGitEntry, WorkspaceGitFileStatus,
    WorkspaceGitStatusResponse, WorkspaceGitSyncResponse,
};
use fxg_protocol::common::{DiffScope, WorkspaceDiffResponse};
use sha2::{Digest, Sha256};

use crate::daemon::DaemonState;
use crate::error::NodeError;
use crate::git;

/// Maximum text file size returned to the UI (2 MiB).
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Maximum entries in one directory listing.
const MAX_DIR_ENTRIES: usize = 2000;
/// Maximum changed files in one Git status response.
const MAX_STATUS_ENTRIES: usize = 5000;
/// Maximum matches returned by the workspace quick-open search.
const MAX_SEARCH_MATCHES: u32 = 200;
/// Maximum files inspected by the non-Git search fallback.
const MAX_SEARCH_SCAN: usize = 20_000;
/// Heavy or generated directories skipped by the non-Git search fallback.
const SEARCH_SKIP_DIRS: &[&str] = &[
    ".git",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".venv",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "target",
    "vendor",
    "venv",
];

struct ResolvedWorkspace {
    /// Access root (Git toplevel or the registered workspace directory).
    root: PathBuf,
    /// Canonical requested path.
    path: PathBuf,
}

/// `GET /api/v1/nodes/:id/workspace/fs/list`
pub async fn list_workspace(
    state: &DaemonState,
    requested: &str,
) -> Result<WorkspaceFsListResponse, NodeError> {
    let resolved = resolve_existing(state, requested).await?;
    let metadata =
        std::fs::metadata(&resolved.path).map_err(|err| NodeError::io(&resolved.path, err))?;
    if !metadata.is_dir() {
        return Err(NodeError::InvalidState(format!(
            "not a directory: {}",
            resolved.path.display()
        )));
    }

    let (entries, truncated) = list_dir(&resolved.path)?;
    let parent_path = resolved
        .path
        .parent()
        .filter(|parent| parent.starts_with(&resolved.root))
        .map(|parent| parent.to_string_lossy().into_owned());

    Ok(WorkspaceFsListResponse {
        root_path: resolved.root.to_string_lossy().into_owned(),
        path: resolved.path.to_string_lossy().into_owned(),
        parent_path,
        entries,
        truncated,
    })
}

/// `GET /api/v1/nodes/:id/workspace/fs/search`
///
/// Quick-open search over the workspace's file names (the relative path).
/// Tracked and untracked-but-not-ignored files come from `git ls-files`, so the
/// result matches what the repository considers source; a non-Git workspace
/// falls back to a bounded directory walk.
pub async fn search_workspace_files(
    state: &DaemonState,
    requested: &str,
    query: &str,
    limit: u32,
) -> Result<WorkspaceFsSearchResponse, NodeError> {
    let resolved = resolve_existing(state, requested).await?;
    let root = resolved.root;
    let limit = usize::try_from(limit.clamp(1, MAX_SEARCH_MATCHES)).unwrap_or(50);
    let needle = query.trim().to_lowercase();
    let root_path = root.to_string_lossy().into_owned();
    let mut response = WorkspaceFsSearchResponse {
        root_path,
        query: query.to_owned(),
        matches: Vec::new(),
        truncated: false,
    };
    if needle.is_empty() {
        return Ok(response);
    }

    let mut ranked: Vec<(u8, String)> = Vec::new();
    for relative in workspace_file_paths(&root).await? {
        let Some(score) = match_rank(&relative, &needle) else {
            continue;
        };
        ranked.push((score, relative));
    }
    // Best name matches first, then the shortest path, then alphabetical.
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.len().cmp(&b.1.len()))
            .then_with(|| a.1.cmp(&b.1))
    });
    response.truncated = ranked.len() > limit;
    response.matches = ranked
        .into_iter()
        .take(limit)
        .map(|(_, relative_path)| WorkspaceFsMatch {
            path: root.join(&relative_path).to_string_lossy().into_owned(),
            relative_path,
        })
        .collect();
    Ok(response)
}

/// Whether a relative path matches the needle and how well (lower is better).
fn match_rank(relative: &str, needle: &str) -> Option<u8> {
    let lower = relative.to_lowercase();
    if !lower.contains(needle) {
        return None;
    }
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    Some(if name == needle {
        0
    } else if name.starts_with(needle) {
        1
    } else if name.contains(needle) {
        2
    } else {
        3
    })
}

/// File paths relative to the workspace root (`/` separated).
async fn workspace_file_paths(root: &Path) -> Result<Vec<String>, NodeError> {
    let args = [
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    ];
    match git::capture(root, &args, &[]).await {
        Ok(output) if output.success => {
            let mut files = Vec::new();
            for raw in output.stdout.split(|byte| *byte == 0) {
                if raw.is_empty() {
                    continue;
                }
                let relative = String::from_utf8_lossy(raw).replace('\\', "/");
                if is_safe_relative(&relative) {
                    files.push(relative);
                }
            }
            Ok(files)
        }
        // Not a Git repository (or git is unavailable): walk the tree instead.
        _ => walk_workspace_files(root),
    }
}

fn is_safe_relative(relative: &str) -> bool {
    !relative.starts_with('/')
        && relative
            .split('/')
            .all(|component| component != ".." && component != ".")
}

/// Bounded directory walk used when the workspace is not a Git repository.
fn walk_workspace_files(root: &Path) -> Result<Vec<String>, NodeError> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    let mut scanned = 0usize;
    while let Some(dir) = pending.pop() {
        let read_dir = match std::fs::read_dir(&dir) {
            Ok(read_dir) => read_dir,
            Err(_) => continue,
        };
        for item in read_dir {
            let entry = match item {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                if SEARCH_SKIP_DIRS
                    .iter()
                    .any(|skip| name.eq_ignore_ascii_case(skip))
                {
                    continue;
                }
                pending.push(entry.path());
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if let Ok(relative) = entry.path().strip_prefix(root) {
                let relative = relative
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                if is_safe_relative(&relative) {
                    files.push(relative);
                }
            }
            scanned += 1;
            if scanned >= MAX_SEARCH_SCAN {
                return Ok(files);
            }
        }
    }
    Ok(files)
}

/// `GET /api/v1/nodes/:id/workspace/fs/file`
pub async fn read_workspace_file(
    state: &DaemonState,
    requested: &str,
) -> Result<WorkspaceFileView, NodeError> {
    let resolved = resolve_existing(state, requested).await?;
    let metadata =
        std::fs::metadata(&resolved.path).map_err(|err| NodeError::io(&resolved.path, err))?;
    if !metadata.is_file() {
        return Err(NodeError::InvalidState(format!(
            "not a file: {}",
            resolved.path.display()
        )));
    }

    let path = resolved.path.to_string_lossy().into_owned();
    let size = metadata.len();
    let modified_at = metadata_modified_ms(&metadata);

    if size > MAX_FILE_BYTES {
        return Ok(WorkspaceFileView {
            path,
            size,
            modified_at,
            content: WorkspaceFileContent::TooLarge { size },
        });
    }

    let bytes = tokio::fs::read(&resolved.path)
        .await
        .map_err(|err| NodeError::io(&resolved.path, err))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Ok(WorkspaceFileView {
            path,
            size: bytes.len() as u64,
            modified_at,
            content: WorkspaceFileContent::TooLarge {
                size: bytes.len() as u64,
            },
        });
    }
    if let Some(mime) = image_mime(&resolved.path) {
        return Ok(WorkspaceFileView {
            path,
            size: bytes.len() as u64,
            modified_at,
            content: WorkspaceFileContent::Image {
                mime: mime.to_owned(),
                data_b64: BASE64.encode(&bytes),
            },
        });
    }
    if bytes.contains(&0) {
        return Ok(WorkspaceFileView {
            path,
            size: bytes.len() as u64,
            modified_at,
            content: WorkspaceFileContent::Binary,
        });
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => {
            return Ok(WorkspaceFileView {
                path,
                size,
                modified_at,
                content: WorkspaceFileContent::Binary,
            });
        }
    };

    Ok(WorkspaceFileView {
        path,
        size,
        modified_at,
        content: WorkspaceFileContent::Text {
            content_hash: sha256_hex(text.as_bytes()),
            text,
            truncated: false,
        },
    })
}

/// `GET /api/v1/nodes/:id/workspace/git/status`
pub async fn workspace_git_status(
    state: &DaemonState,
    requested: &str,
) -> Result<WorkspaceGitStatusResponse, NodeError> {
    let resolved = resolve_existing(state, requested).await?;
    let root = resolved.root;
    let root_path = root.to_string_lossy().into_owned();

    if !git::is_inside_work_tree(&root).await? {
        return Ok(WorkspaceGitStatusResponse {
            root_path,
            is_repository: false,
            branch: None,
            ahead: 0,
            behind: 0,
            entries: Vec::new(),
            truncated: false,
        });
    }

    let bytes = git::run_git_bytes(&root, &["status", "--porcelain=v2", "--branch", "-z"]).await?;
    let parsed = parse_status_z(&bytes, &root);
    Ok(WorkspaceGitStatusResponse {
        root_path,
        is_repository: true,
        branch: parsed.branch,
        ahead: parsed.ahead,
        behind: parsed.behind,
        entries: parsed.entries,
        truncated: parsed.truncated,
    })
}

/// `GET /api/v1/nodes/:id/workspace/git/diff`
pub async fn workspace_git_diff(
    state: &DaemonState,
    requested: &str,
    scope: DiffScope,
    base_branch: Option<&str>,
    file: Option<&str>,
) -> Result<WorkspaceDiffResponse, NodeError> {
    let resolved = resolve_existing(state, requested).await?;
    let relative = match file {
        None => None,
        Some(file) => {
            let file_path = PathBuf::from(file);
            if !file_path.is_absolute() {
                return Err(NodeError::InvalidState(
                    "diff file path must be absolute".to_owned(),
                ));
            }
            let canonical = resolve_for_git(&file_path)?;
            reject_git_components(&canonical)?;
            if !canonical.starts_with(&resolved.root) {
                return Err(NodeError::Forbidden(
                    "file is outside the workspace".to_owned(),
                ));
            }
            let relative = match canonical.strip_prefix(&resolved.root) {
                Ok(relative) => relative,
                Err(_) => {
                    return Err(NodeError::Forbidden(
                        "file is outside the workspace".to_owned(),
                    ));
                }
            };
            Some(relative.to_string_lossy().replace('\\', "/"))
        }
    };

    git::workspace_diff_file(&resolved.root, scope, base_branch, relative.as_deref()).await
}

/// Maximum commit message length accepted by the workspace commit endpoint (64 KiB).
const MAX_COMMIT_MESSAGE_BYTES: usize = 64 * 1024;

/// `POST /api/v1/nodes/:id/workspace/git/commit`
///
/// Stages every change in the worktree (`git add -A`) and creates one commit.
/// Partial staging and pushing are intentionally out of scope.
pub async fn workspace_git_commit(
    state: &DaemonState,
    requested: &str,
    message: &str,
) -> Result<WorkspaceGitCommitResponse, NodeError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(NodeError::InvalidState(
            "a commit message is required".to_owned(),
        ));
    }
    if message.len() > MAX_COMMIT_MESSAGE_BYTES {
        return Err(NodeError::InvalidState(format!(
            "the commit message is too long (max {MAX_COMMIT_MESSAGE_BYTES} bytes)"
        )));
    }
    if message.contains('\0') {
        return Err(NodeError::InvalidState(
            "the commit message contains a NUL byte".to_owned(),
        ));
    }

    let root = resolve_git_root(state, requested).await?;

    let bytes = git::run_git_bytes(&root, &["status", "--porcelain=v2", "-z"]).await?;
    let parsed = parse_status_z(&bytes, &root);
    if parsed.entries.is_empty() {
        return Err(NodeError::InvalidState("nothing to commit".to_owned()));
    }
    let files = parsed.entries.len() as u32;

    git::run_git(&root, &["add", "-A"]).await?;
    let output = git::capture(&root, &["commit", "-m", message], &[]).await?;
    if !output.success {
        return Err(NodeError::Git {
            cwd: root.clone(),
            args: "commit -m <message>".to_owned(),
            message: output.stderr,
        });
    }

    let commit = git::head_commit(&root).await?.unwrap_or_default();
    let branch = git::current_branch(&root).await?;
    Ok(WorkspaceGitCommitResponse {
        commit,
        branch,
        files,
    })
}

/// Timeout for Git operations that talk to a remote (fetch / push).
const GIT_NETWORK_TIMEOUT: Duration = Duration::from_secs(120);

/// `POST /api/v1/nodes/:id/workspace/git/pull`
///
/// Fetches the upstream and fast-forwards the current branch. The operation
/// never creates a merge commit or conflict markers; a diverged branch is an
/// error that the user resolves in a terminal.
pub async fn workspace_git_pull(
    state: &DaemonState,
    requested: &str,
) -> Result<WorkspaceGitSyncResponse, NodeError> {
    let root = resolve_git_root(state, requested).await?;
    let upstream = git::upstream_ref(&root).await?.ok_or_else(|| {
        NodeError::InvalidState(
            "the current branch has no upstream; set one in a terminal before pulling".to_owned(),
        )
    })?;
    let before = git::head_commit(&root).await?.unwrap_or_default();

    let fetch = git::capture_with_timeout(
        &root,
        &["fetch"],
        &[("GIT_TERMINAL_PROMPT", "0")],
        GIT_NETWORK_TIMEOUT,
    )
    .await?;
    if !fetch.success {
        return Err(NodeError::Git {
            cwd: root.clone(),
            args: "fetch".to_owned(),
            message: fetch.stderr,
        });
    }

    let merge = git::capture(&root, &["merge", "--ff-only", &upstream], &[]).await?;
    if !merge.success {
        return Err(NodeError::Git {
            cwd: root.clone(),
            args: "merge --ff-only <upstream>".to_owned(),
            message: merge.stderr,
        });
    }

    let commit = git::head_commit(&root).await?.unwrap_or_default();
    Ok(WorkspaceGitSyncResponse {
        branch: git::current_branch(&root).await?,
        updated: before != commit,
        commit,
        upstream: Some(upstream),
    })
}

/// `POST /api/v1/nodes/:id/workspace/git/push`
///
/// Pushes the current branch without force. When the branch has no upstream,
/// `origin` is used and the upstream is set on the first push.
pub async fn workspace_git_push(
    state: &DaemonState,
    requested: &str,
) -> Result<WorkspaceGitSyncResponse, NodeError> {
    let root = resolve_git_root(state, requested).await?;
    let (args, upstream) = match git::upstream_ref(&root).await? {
        Some(upstream) => (vec!["push".to_owned()], upstream),
        None => {
            let branch = git::current_branch(&root)
                .await?
                .ok_or_else(|| NodeError::InvalidState("cannot push a detached HEAD".to_owned()))?;
            if git::remote_origin_url(&root).await?.is_none() {
                return Err(NodeError::InvalidState(
                    "the current branch has no upstream and no 'origin' remote".to_owned(),
                ));
            }
            let upstream = format!("origin/{branch}");
            (
                vec![
                    "push".to_owned(),
                    "--set-upstream".to_owned(),
                    "origin".to_owned(),
                    branch,
                ],
                upstream,
            )
        }
    };
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = git::capture_with_timeout(
        &root,
        &arg_refs,
        &[("GIT_TERMINAL_PROMPT", "0")],
        GIT_NETWORK_TIMEOUT,
    )
    .await?;
    if !output.success {
        return Err(NodeError::Git {
            cwd: root.clone(),
            args: "push".to_owned(),
            message: output.stderr,
        });
    }

    let commit = git::head_commit(&root).await?.unwrap_or_default();
    Ok(WorkspaceGitSyncResponse {
        branch: git::current_branch(&root).await?,
        commit,
        upstream: Some(upstream),
        updated: false,
    })
}

/// Resolves a registered workspace directory to its Git worktree root.
async fn resolve_git_root(state: &DaemonState, requested: &str) -> Result<PathBuf, NodeError> {
    let resolved = resolve_existing(state, requested).await?;
    let metadata =
        std::fs::metadata(&resolved.path).map_err(|err| NodeError::io(&resolved.path, err))?;
    if !metadata.is_dir() {
        return Err(NodeError::InvalidState(format!(
            "not a directory: {}",
            resolved.path.display()
        )));
    }
    if !git::is_inside_work_tree(&resolved.root).await? {
        return Err(NodeError::InvalidState(
            "the workspace is not inside a Git work tree".to_owned(),
        ));
    }
    Ok(resolved.root)
}

/// Resolves an existing path and its workspace access root.
async fn resolve_existing(
    state: &DaemonState,
    requested: &str,
) -> Result<ResolvedWorkspace, NodeError> {
    let requested = PathBuf::from(requested);
    let canonical = dunce::canonicalize(&requested).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            NodeError::NotFound(requested.display().to_string())
        } else {
            NodeError::io(&requested, err)
        }
    })?;
    reject_git_components(&canonical)?;
    let root = access_root(state, &canonical).await?;
    if !canonical.starts_with(&root) {
        return Err(NodeError::Forbidden(
            "path is outside the workspace".to_owned(),
        ));
    }
    Ok(ResolvedWorkspace {
        root,
        path: canonical,
    })
}

/// Resolves a path that may not exist on disk (deleted files in a diff).
fn resolve_for_git(path: &Path) -> Result<PathBuf, NodeError> {
    match dunce::canonicalize(path) {
        Ok(canonical) => Ok(canonical),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| {
                NodeError::InvalidState(format!("path has no parent: {}", path.display()))
            })?;
            let name = path.file_name().ok_or_else(|| {
                NodeError::InvalidState(format!("path has no file name: {}", path.display()))
            })?;
            let parent = dunce::canonicalize(parent)
                .map_err(|parent_err| NodeError::io(parent, parent_err))?;
            Ok(parent.join(name))
        }
        Err(err) => Err(NodeError::io(path, err)),
    }
}

/// Finds the access root of a canonical path among the node's registered workspaces.
async fn access_root(state: &DaemonState, canonical: &Path) -> Result<PathBuf, NodeError> {
    let projects = state.db().list_projects().await?;
    let node_id = state.node_id();
    let mut candidates = Vec::new();

    for project in projects {
        for binding in project.bindings {
            if binding.node_id.as_str() != node_id {
                continue;
            }
            let binding_path = PathBuf::from(&binding.local_path);
            let Ok(canonical_binding) = dunce::canonicalize(&binding_path) else {
                continue;
            };
            if canonical.starts_with(&canonical_binding) {
                candidates.push(canonical_binding);
            }
        }
    }
    for (local_path, _) in state.db().project_path_links().await? {
        let path = PathBuf::from(local_path);
        let Ok(canonical_path) = dunce::canonicalize(&path) else {
            continue;
        };
        if canonical.starts_with(&canonical_path) {
            candidates.push(canonical_path);
        }
    }
    let session_paths = state
        .db()
        .list_session_local_paths(&fxg_db::SessionFilter {
            node_id: Some(node_id.into()),
            include_archived: true,
            ..fxg_db::SessionFilter::default()
        })
        .await?;
    for local_path in session_paths {
        let session_path = PathBuf::from(local_path);
        let Ok(canonical_session) = dunce::canonicalize(&session_path) else {
            continue;
        };
        if canonical.starts_with(&canonical_session) {
            candidates.push(canonical_session);
        }
    }

    candidates.sort();
    candidates.dedup();
    // Prefer the most specific registered workspace (nested repositories).
    candidates.sort_by_key(|path| std::cmp::Reverse(path.components().count()));

    for binding in candidates {
        let root = match git::toplevel(&binding).await {
            Ok(Some(root)) => root,
            Ok(None) | Err(NodeError::GitUnavailable(_)) => binding,
            Err(err) => return Err(err),
        };
        if canonical.starts_with(&root) {
            return Ok(root);
        }
    }

    Err(NodeError::Forbidden(
        "path is outside a registered workspace".to_owned(),
    ))
}

fn reject_git_components(path: &Path) -> Result<(), NodeError> {
    if path.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(".git")
    }) {
        return Err(NodeError::Forbidden(
            "the .git directory is not accessible".to_owned(),
        ));
    }
    Ok(())
}

fn list_dir(dir: &Path) -> Result<(Vec<WorkspaceFsEntry>, bool), NodeError> {
    let read_dir = std::fs::read_dir(dir).map_err(|err| NodeError::io(dir, err))?;
    let mut entries = Vec::new();
    let mut truncated = false;

    for item in read_dir {
        let entry = match item {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case(".git") {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let kind = if file_type.is_dir() {
            WorkspaceEntryKind::Directory
        } else if file_type.is_file() {
            WorkspaceEntryKind::File
        } else if file_type.is_symlink() {
            WorkspaceEntryKind::Symlink
        } else {
            WorkspaceEntryKind::Other
        };
        let metadata = entry_metadata(&entry);
        let size = metadata.as_ref().map_or(0, std::fs::Metadata::len);
        let modified_at = metadata.as_ref().and_then(metadata_modified_ms);
        let is_hidden = name.starts_with('.') || is_hidden_on_windows(metadata.as_ref());

        entries.push(WorkspaceFsEntry {
            name,
            path: entry.path().to_string_lossy().into_owned(),
            kind,
            is_hidden,
            size,
            modified_at,
        });
        if entries.len() >= MAX_DIR_ENTRIES {
            truncated = true;
            break;
        }
    }

    entries.sort_by(|a, b| {
        kind_rank(a.kind)
            .cmp(&kind_rank(b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok((entries, truncated))
}

fn kind_rank(kind: WorkspaceEntryKind) -> u8 {
    match kind {
        WorkspaceEntryKind::Directory => 0,
        WorkspaceEntryKind::File => 1,
        WorkspaceEntryKind::Symlink => 2,
        WorkspaceEntryKind::Other => 3,
    }
}

#[cfg(windows)]
fn is_hidden_on_windows(metadata: Option<&std::fs::Metadata>) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    metadata.is_some_and(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
}

#[cfg(not(windows))]
fn is_hidden_on_windows(_metadata: Option<&std::fs::Metadata>) -> bool {
    false
}

#[allow(
    clippy::manual_ok_err,
    reason = "Result::ok is disallowed by the workspace lint"
)]
fn entry_metadata(entry: &std::fs::DirEntry) -> Option<std::fs::Metadata> {
    match entry.metadata() {
        Ok(metadata) => Some(metadata),
        Err(_) => None,
    }
}

#[allow(
    clippy::manual_ok_err,
    reason = "Result::ok is disallowed by the workspace lint"
)]
fn metadata_modified_ms(metadata: &std::fs::Metadata) -> Option<i64> {
    match metadata.modified() {
        Ok(time) => system_time_ms(time),
        Err(_) => None,
    }
}

#[allow(
    clippy::manual_ok_err,
    reason = "Result::ok is disallowed by the workspace lint"
)]
fn system_time_ms(time: SystemTime) -> Option<i64> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => match i64::try_from(duration.as_millis()) {
            Ok(value) => Some(value),
            Err(_) => None,
        },
        Err(_) => None,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

struct ParsedStatus {
    branch: Option<String>,
    ahead: u32,
    behind: u32,
    entries: Vec<WorkspaceGitEntry>,
    truncated: bool,
}

/// Parses `git status --porcelain=v2 --branch -z`.
fn parse_status_z(bytes: &[u8], root: &Path) -> ParsedStatus {
    let mut parsed = ParsedStatus {
        branch: None,
        ahead: 0,
        behind: 0,
        entries: Vec::new(),
        truncated: false,
    };
    let mut pending_old_path = false;

    for chunk in bytes.split(|byte| *byte == 0) {
        if chunk.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(chunk);

        // The original path of a rename / copy follows its record as a separate
        // NUL-separated field.
        if pending_old_path {
            if let Some(entry) = parsed.entries.last_mut() {
                entry.old_path = Some(line.into_owned());
            }
            pending_old_path = false;
            continue;
        }

        if let Some(head) = line.strip_prefix("# branch.head ") {
            if !head.starts_with('(') {
                parsed.branch = Some(head.to_owned());
            }
            continue;
        }
        if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut parts = ab.split_whitespace();
            parsed.ahead = parse_count(parts.next(), '+');
            parsed.behind = parse_count(parts.next(), '-');
            continue;
        }
        if let Some(path) = line.strip_prefix("? ") {
            push_entry(&mut parsed, '.', '.', path, None, true, false, root);
            continue;
        }
        if let Some(rest) = line.strip_prefix("1 ") {
            let parts: Vec<&str> = rest.splitn(8, ' ').collect();
            if let (Some(xy), Some(path)) = (parts.first(), parts.get(7)) {
                push_xy(&mut parsed, xy, path, None, false, root);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("2 ") {
            let parts: Vec<&str> = rest.splitn(9, ' ').collect();
            if let (Some(xy), Some(path)) = (parts.first(), parts.get(8)) {
                push_xy(&mut parsed, xy, path, None, false, root);
                pending_old_path = true;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("u ") {
            let parts: Vec<&str> = rest.splitn(10, ' ').collect();
            if let (Some(xy), Some(path)) = (parts.first(), parts.get(9)) {
                push_xy(&mut parsed, xy, path, None, true, root);
            }
            continue;
        }
        // `!` (ignored) entries are not requested.
    }

    parsed
}

fn push_xy(
    parsed: &mut ParsedStatus,
    xy: &str,
    path: &str,
    old_path: Option<&str>,
    conflicted: bool,
    root: &Path,
) {
    let mut chars = xy.chars();
    let x = chars.next().unwrap_or('.');
    let y = chars.next().unwrap_or('.');
    push_entry(parsed, x, y, path, old_path, false, conflicted, root);
}

#[allow(
    clippy::too_many_arguments,
    reason = "parser helper keeps the entry construction in one place"
)]
fn push_entry(
    parsed: &mut ParsedStatus,
    x: char,
    y: char,
    path: &str,
    old_path: Option<&str>,
    untracked: bool,
    conflicted: bool,
    root: &Path,
) {
    if parsed.entries.len() >= MAX_STATUS_ENTRIES {
        parsed.truncated = true;
        return;
    }
    parsed.entries.push(WorkspaceGitEntry {
        path: path.to_owned(),
        old_path: old_path.map(str::to_owned),
        absolute_path: root.join(path).to_string_lossy().into_owned(),
        status: classify_status(x, y, untracked, conflicted),
        staged: x != '.',
        unstaged: y != '.' || untracked || conflicted,
    });
}

fn parse_count(value: Option<&str>, sign: char) -> u32 {
    let Some(value) = value else {
        return 0;
    };
    let Some(value) = value.strip_prefix(sign) else {
        return 0;
    };
    value.parse().unwrap_or(0)
}

fn classify_status(x: char, y: char, untracked: bool, conflicted: bool) -> WorkspaceGitFileStatus {
    if conflicted || x == 'U' || y == 'U' {
        WorkspaceGitFileStatus::Conflicted
    } else if untracked {
        WorkspaceGitFileStatus::Untracked
    } else if x == 'R' || y == 'R' {
        WorkspaceGitFileStatus::Renamed
    } else if x == 'C' || y == 'C' {
        WorkspaceGitFileStatus::Copied
    } else if x == 'A' || y == 'A' {
        WorkspaceGitFileStatus::Added
    } else if x == 'D' || y == 'D' {
        WorkspaceGitFileStatus::Deleted
    } else if x == 'T' || y == 'T' {
        WorkspaceGitFileStatus::TypeChanged
    } else if x == 'M' || y == 'M' {
        WorkspaceGitFileStatus::Modified
    } else {
        WorkspaceGitFileStatus::Unknown {
            raw: format!("{x}{y}"),
        }
    }
}

/// MIME type for a raster image extension the browser can render in an `<img>`.
///
/// SVG is intentionally excluded: it is served as text so embedded scripts can
/// never become an image document.
fn image_mime(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "avif" => Some("image/avif"),
        "ico" => Some("image/x-icon"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn image_extensions_map_to_renderable_mime_types() {
        assert_eq!(image_mime(Path::new("a.PNG")), Some("image/png"));
        assert_eq!(image_mime(Path::new("a.jpeg")), Some("image/jpeg"));
        assert_eq!(image_mime(Path::new("a.webp")), Some("image/webp"));
        assert_eq!(image_mime(Path::new("a.svg")), None);
        assert_eq!(image_mime(Path::new("a.txt")), None);
    }

    #[test]
    fn status_parser_handles_ordinary_untracked_and_renamed_entries() {
        let root = Path::new("/repo");
        let output = b"# branch.head main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa bbb src/main.rs\0\
? new file with spaces.txt\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new.rs\0old.rs\0\
u UU N... 100644 100644 100644 100644 aaa bbb ccc conflict.rs\0";

        let parsed = parse_status_z(output, root);

        assert_eq!(parsed.branch.as_deref(), Some("main"));
        assert_eq!(parsed.ahead, 2);
        assert_eq!(parsed.behind, 1);
        assert_eq!(parsed.entries.len(), 4);
        assert_eq!(parsed.entries[0].path, "src/main.rs");
        assert_eq!(parsed.entries[0].status, WorkspaceGitFileStatus::Modified);
        assert_eq!(parsed.entries[1].path, "new file with spaces.txt");
        assert_eq!(parsed.entries[1].status, WorkspaceGitFileStatus::Untracked);
        assert_eq!(parsed.entries[2].path, "new.rs");
        assert_eq!(parsed.entries[2].old_path.as_deref(), Some("old.rs"));
        assert_eq!(parsed.entries[2].status, WorkspaceGitFileStatus::Renamed);
        assert_eq!(parsed.entries[3].status, WorkspaceGitFileStatus::Conflicted);
        assert_eq!(
            parsed.entries[0].absolute_path,
            root.join("src/main.rs").to_string_lossy()
        );
    }

    #[test]
    fn status_parser_detects_detached_head() {
        let parsed = parse_status_z(b"# branch.head (detached)\0", Path::new("/repo"));
        assert_eq!(parsed.branch, None);
    }

    #[test]
    fn git_components_are_rejected() {
        assert!(reject_git_components(Path::new("/repo/.git/config")).is_err());
        assert!(reject_git_components(Path::new("/repo/.GIT/config")).is_err());
        assert!(reject_git_components(Path::new("/repo/src/main.rs")).is_ok());
    }

    #[test]
    fn directory_listing_hides_git_and_sorts_directories_first() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir(dir.path().join("b-dir")).expect("create dir");
        fs::create_dir(dir.path().join(".git")).expect("create .git");
        fs::write(dir.path().join("a.txt"), b"hello").expect("write file");
        fs::write(dir.path().join(".env"), b"secret").expect("write hidden");

        let (entries, truncated) = list_dir(dir.path()).expect("list");

        assert!(!truncated);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b-dir", ".env", "a.txt"]
        );
        assert!(entries[1].is_hidden);
        assert!(!entries.iter().any(|entry| entry.name == ".git"));
    }

    #[test]
    fn quick_open_ranking_prefers_file_name_matches() {
        assert_eq!(match_rank("src/main.rs", "main"), Some(1));
        assert_eq!(match_rank("docs/domain.md", "main"), Some(2));
        assert_eq!(match_rank("main/readme.md", "main"), Some(3));
        assert_eq!(match_rank("src/other.rs", "main"), None);
        assert!(is_safe_relative("src/main.rs"));
        assert!(!is_safe_relative("../secrets.txt"));
        assert!(!is_safe_relative("/etc/passwd"));
        assert!(!is_safe_relative("src/./main.rs"));
    }

    #[test]
    fn fallback_walk_skips_heavy_directories() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::create_dir_all(dir.path().join("src")).expect("create src");
        fs::create_dir_all(dir.path().join("node_modules/pkg")).expect("create node_modules");
        fs::write(dir.path().join("src/lib.rs"), b"").expect("write lib");
        fs::write(dir.path().join("node_modules/pkg/index.js"), b"").expect("write ignored");

        let files = walk_workspace_files(dir.path()).expect("walk");

        assert_eq!(files, vec!["src/lib.rs".to_owned()]);
    }

    #[tokio::test]
    async fn workspace_file_paths_follows_gitignore() {
        let dir = tempfile::tempdir().expect("temp dir");
        crate::testutil::init_test_repo(dir.path()).await;
        fs::write(dir.path().join(".gitignore"), b"ignored.log\n").expect("write gitignore");
        fs::write(dir.path().join("src.rs"), b"fn main() {}\n").expect("write source");
        fs::write(dir.path().join("ignored.log"), b"x\n").expect("write ignored");

        let files = workspace_file_paths(dir.path()).await.expect("paths");

        assert!(files.contains(&"README.md".to_owned()));
        assert!(files.contains(&"src.rs".to_owned()));
        assert!(!files.contains(&"ignored.log".to_owned()));
    }
}

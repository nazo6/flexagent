//! Workspace-scoped file browsing and Git inspection.
//!
//! Every request is resolved against a directory registered on this node
//! (`project_node_bindings`). The access root is the workspace's Git toplevel,
//! or the registered directory when it is not inside a work tree, so a
//! worktree can never reach the main repository and vice versa.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fxg_protocol::client_api::{
    WorkspaceEntryKind, WorkspaceFileContent, WorkspaceFileView, WorkspaceFsEntry,
    WorkspaceFsListResponse, WorkspaceGitEntry, WorkspaceGitFileStatus, WorkspaceGitStatusResponse,
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
    let sessions = state
        .db()
        .list_sessions(&fxg_db::SessionFilter {
            include_archived: true,
            ..fxg_db::SessionFilter::default()
        })
        .await?;
    for session in sessions {
        if session.node_id.as_str() != node_id {
            continue;
        }
        let session_path = PathBuf::from(&session.local_path);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
}

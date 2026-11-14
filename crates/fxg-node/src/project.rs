//! Identity resolution for logical projects (`Logical Project`).
//!
//! Design: `docs/01-architecture-and-sync.md` §4 and
//! `docs/proposals/2026-11-09-settings-consolidation-and-project-identity.md` §3.1.
//!
//! Resolution order:
//! 1. Explicit `project_path_links` entry (exact path, then ancestors up to the
//!    Git root; a Worktree falls back to the main Worktree's links through
//!    `git rev-parse --git-common-dir`)
//! 2. Normalized `git remote.origin.url`
//! 3. Machine-local `local:<node_id>:<path-hash>` fallback
//!
//! Display names are always derived from the identity; they are never configured.

use std::path::{Path, PathBuf};

use fxg_db::Db;
use fxg_protocol::client_api::ProjectPreview;
use fxg_protocol::settings::{ProjectIdentity, ProjectIdentityKind};

use crate::error::NodeError;
use crate::git;

/// Normalizes a `git remote` URL into a logical project key.
///
/// ```text
/// "git@github.com:nazo6/flexagent.git"          -> "github.com/nazo6/flexagent"
/// "https://github.com/nazo6/flexagent.git"      -> "github.com/nazo6/flexagent"
/// "ssh://git@gitlab.example.com:2222/team/app.git" -> "gitlab.example.com/team/app"
/// ```
///
/// When started in a subdirectory of the Git root (as in a monorepo), appends the
/// relative path (POSIX slash normalized) as in
/// `github.com/nazo6/flexagent#packages/sub`.
pub fn normalize_git_url(raw_url: &str, subpath_from_root: &str) -> String {
    let trimmed = raw_url.trim();
    // 1. Strip the scheme (`https://`, `ssh://`, `git://`, etc.)
    let without_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    // 2. Strip the userinfo (`git@`, etc.)
    let without_auth = without_scheme
        .rsplit_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(without_scheme);
    // 3. Separate scp form (`host:path`) / URL form (`host/path`)
    let (host, path) = match without_auth.split_once(':') {
        Some((host, path)) => (host, path),
        None => match without_auth.split_once('/') {
            Some((host, path)) => (host, path),
            None => (without_auth, ""),
        },
    };
    // 4. Strip the port number (`gitlab.example.com:2222/team/app`)
    let path = match path.split_once('/') {
        Some((maybe_port, rest))
            if !maybe_port.is_empty() && maybe_port.chars().all(|ch| ch.is_ascii_digit()) =>
        {
            rest
        }
        _ => path,
    };
    // 5. Strip the `.git` suffix and leading/trailing slashes
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.trim_matches('/');
    let host = host.trim_matches('/').to_ascii_lowercase();

    let mut key = if path.is_empty() {
        host
    } else {
        format!("{host}/{path}")
    };

    let subpath = normalize_subpath(subpath_from_root);
    if !subpath.is_empty() {
        key.push('#');
        key.push_str(&subpath);
    }
    key
}

/// Normalizes a subpath to POSIX slashes (stripping `\` and `./`).
fn normalize_subpath(subpath: &str) -> String {
    subpath
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Fallback project ID for non-Git projects without an explicit link.
///
/// The format is `local:<node_id>:<hash>` (design:
/// `docs/01-architecture-and-sync.md` §4.1). The hash is FNV-1a (64-bit) of the
/// **normalized absolute path** so that same-named folders on the same node do not
/// collide.
pub fn fallback_project_id(node_id: &str, path: &Path) -> String {
    let canonical = fxg_pty::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let normalized = canonical
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    format!("local:{node_id}:{:016x}", fnv1a64(normalized.as_bytes()))
}

/// FNV-1a (64-bit) hash.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Maximum length of an explicit project key / project ID.
const MAX_PROJECT_KEY_LEN: usize = 255;

/// Whether the key is a canonical logical project ID derived from a Git remote
/// (`github.com/owner/repo` / `github.com/owner/repo#packages/sub`).
///
/// Keys containing `/` are used verbatim as the project ID; other keys live under the
/// `user:` namespace.
pub fn is_canonical_project_id(key: &str) -> bool {
    key.contains('/')
}

/// Validates an explicit project key / project ID.
///
/// Accepted forms are a simple user key (`[a-zA-Z0-9._-]`) or a canonical project ID
/// (`host/owner/repo`, optionally with a `#subpath`). `:` is rejected so the reserved
/// namespaces (`user:` / `local:`) cannot be forged.
pub fn validate_project_key(key: &str) -> Result<(), NodeError> {
    let valid = !key.is_empty()
        && key.len() <= MAX_PROJECT_KEY_LEN
        && !key.starts_with('/')
        && !key.ends_with('/')
        && !key.starts_with('#')
        && !key.ends_with('#')
        && !key.contains("//")
        && key.matches('#').count() <= 1
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/' | '#'));
    if valid {
        Ok(())
    } else {
        Err(NodeError::InvalidState(format!(
            "invalid project key {key:?}: use 1-{MAX_PROJECT_KEY_LEN} characters of \
             [a-zA-Z0-9._-] or a canonical project ID (host/owner/repo[#subpath])"
        )))
    }
}

/// A resolved logical project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProject {
    /// Logical project ID (`user:<key>` / linked canonical project ID / normalized Git URL /
    /// `local:...`)
    pub project_id: String,
    /// Display name derived from the identity
    pub name: String,
    /// Git URL before normalization
    pub canonical_git_url: Option<String>,
    /// Identity details
    pub identity: ProjectIdentity,
    /// Git root (`None` for non-Git projects)
    pub git_root: Option<PathBuf>,
    /// Relative subpath from the Git root (monorepo; `None` directly under the root)
    pub relative_subpath: Option<String>,
    /// Directory that was resolved (normalized absolute path)
    pub local_path: PathBuf,
    /// Whether this is a Git repository
    pub is_git_repo: bool,
    /// Explicit link that matched, when any
    pub matched_link_path: Option<PathBuf>,
}

impl ResolvedProject {
    /// Record to register in the `projects` table.
    pub fn to_project_record(&self) -> fxg_db::ProjectRecord {
        fxg_db::ProjectRecord {
            project_id: self.project_id.clone(),
            name: self.name.clone(),
            canonical_git_url: self.canonical_git_url.clone(),
            identity: self.identity.clone(),
        }
    }

    /// Record to register in `project_node_bindings`.
    pub fn to_binding_record(
        &self,
        node_id: &str,
        is_worktree: bool,
        git_branch: Option<&str>,
    ) -> fxg_db::ProjectBindingRecord {
        fxg_db::ProjectBindingRecord {
            project_id: self.project_id.clone(),
            node_id: node_id.to_owned(),
            local_path: self.local_path.to_string_lossy().into_owned(),
            is_worktree,
            // The directory exists once resolution has succeeded
            path_exists: true,
            git_branch: git_branch.map(str::to_owned),
        }
    }

    /// Client-facing preview used by the "Add project folder" flow.
    pub fn to_preview(&self, is_worktree: bool) -> ProjectPreview {
        ProjectPreview {
            project_id: self.project_id.clone().into(),
            name: self.name.clone(),
            canonical_git_url: self.canonical_git_url.clone(),
            identity: self.identity.clone(),
            local_path: self.local_path.to_string_lossy().into_owned(),
            is_git_repo: self.is_git_repo,
            is_worktree,
            relative_subpath: self.relative_subpath.clone(),
        }
    }
}

/// Finds the explicit link matching a path (or one of its ancestors).
fn match_link(
    links: &[(String, String)],
    path: &Path,
    stop_at: Option<&Path>,
) -> Option<(PathBuf, String)> {
    let mut current = Some(path.to_path_buf());
    while let Some(candidate) = current {
        let candidate_str = candidate.to_string_lossy();
        if let Some((_, key)) = links
            .iter()
            .find(|(linked, _)| linked == candidate_str.as_ref())
        {
            return Some((candidate, key.clone()));
        }
        if stop_at.is_some_and(|root| root == candidate) {
            break;
        }
        current = candidate.parent().map(Path::to_path_buf);
    }
    None
}

/// Resolves the explicit `project_key` for a path.
async fn explicit_key(
    db: &Db,
    local_path: &Path,
    git_root: Option<&Path>,
) -> Result<Option<(PathBuf, String)>, NodeError> {
    let links = db.project_path_links().await?;
    if let Some(found) = match_link(&links, local_path, git_root) {
        return Ok(Some(found));
    }
    if let Some(root) = git_root {
        // A Worktree in another directory inherits the main Worktree's link.
        if let Some(main_root) = git::main_worktree_root(root).await?
            && let Some(found) = match_link(&links, &main_root, Some(&main_root))
        {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// Derives the display name from the identity.
fn derive_name(
    identity: &ProjectIdentity,
    project_id: &str,
    git_root: Option<&Path>,
    local_path: &Path,
) -> String {
    match identity.kind {
        ProjectIdentityKind::UserKey => {
            identity.key.clone().unwrap_or_else(|| "project".to_owned())
        }
        ProjectIdentityKind::GitRemote => {
            let without_subpath = project_id.split('#').next().unwrap_or(project_id);
            without_subpath
                .rsplit('/')
                .next()
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    git_root
                        .and_then(|root| root.file_name())
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| project_id.to_owned())
        }
        ProjectIdentityKind::Local => local_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| local_path.display().to_string()),
        ProjectIdentityKind::Unknown(_) => local_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| local_path.display().to_string()),
    }
}

/// Resolves the logical project of the given directory.
pub async fn resolve_project(
    cwd: &Path,
    node_id: &str,
    db: &Db,
) -> Result<ResolvedProject, NodeError> {
    let local_path = fxg_pty::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let git_root = git::toplevel(&local_path).await?;
    let link = explicit_key(db, &local_path, git_root.as_deref()).await?;

    let canonical_git_url = match &git_root {
        Some(root) => git::remote_origin_url(root).await?,
        None => None,
    };

    let relative_subpath = match git_root.as_ref() {
        Some(root) => match local_path.strip_prefix(root) {
            Ok(subpath) if !subpath.as_os_str().is_empty() => {
                Some(subpath.to_string_lossy().replace('\\', "/"))
            }
            Ok(_) => None,
            Err(err) => {
                tracing::debug!(
                    path = %local_path.display(),
                    root = %root.display(),
                    "failed to compute the path relative to the git root: {err}"
                );
                None
            }
        },
        None => None,
    };

    let (project_id, identity) = if let Some((_, key)) = &link {
        if is_canonical_project_id(key) {
            // A full project ID (`github.com/owner/repo[#subpath]`) links the folder directly
            // to the Git-derived project, so another node's checkout merges with it.
            (key.clone(), ProjectIdentity::git_remote())
        } else {
            (format!("user:{key}"), ProjectIdentity::user_key(key))
        }
    } else if let Some(url) = &canonical_git_url {
        (
            normalize_git_url(url, relative_subpath.as_deref().unwrap_or("")),
            ProjectIdentity::git_remote(),
        )
    } else {
        (
            fallback_project_id(node_id, &local_path),
            ProjectIdentity::local(),
        )
    };

    let name = derive_name(&identity, &project_id, git_root.as_deref(), &local_path);

    Ok(ResolvedProject {
        project_id,
        name,
        canonical_git_url,
        identity,
        is_git_repo: git_root.is_some(),
        git_root,
        relative_subpath,
        local_path,
        matched_link_path: link.map(|(path, _)| path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;
    use fxg_db::Db;
    use fxg_protocol::paths::Role;

    fn canonical(path: &Path) -> PathBuf {
        fxg_pty::canonicalize(path).expect("canonicalize")
    }

    async fn test_db() -> Db {
        Db::open_in_memory(Role::Node).await.expect("db")
    }

    #[test]
    fn normalizes_documented_examples() {
        assert_eq!(
            normalize_git_url("git@github.com:nazo6/flexagent.git", ""),
            "github.com/nazo6/flexagent"
        );
        assert_eq!(
            normalize_git_url("https://github.com/nazo6/flexagent.git", ""),
            "github.com/nazo6/flexagent"
        );
        assert_eq!(
            normalize_git_url("ssh://git@gitlab.example.com:2222/team/app.git", ""),
            "gitlab.example.com/team/app"
        );
    }

    #[test]
    fn normalizes_variants() {
        assert_eq!(
            normalize_git_url("github.com/nazo6/flexagent", ""),
            "github.com/nazo6/flexagent"
        );
        assert_eq!(
            normalize_git_url(" HTTPS://GitHub.com/nazo6/flexagent/ ", ""),
            "github.com/nazo6/flexagent"
        );
        assert_eq!(
            normalize_git_url("http://192.168.1.5:8080/team/app.git", ""),
            "192.168.1.5/team/app"
        );
        assert_eq!(
            normalize_git_url("git@github.com:nazo6/flexagent.git", "./packages/sub/"),
            "github.com/nazo6/flexagent#packages/sub"
        );
        assert_eq!(
            normalize_git_url("git@github.com:nazo6/flexagent.git", r"packages\sub"),
            "github.com/nazo6/flexagent#packages/sub"
        );
        assert_eq!(
            normalize_git_url("git@github.com:org/team/repo.git", ""),
            "github.com/org/team/repo"
        );
    }

    #[test]
    fn fallback_id_is_stable_and_path_specific() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir_all(&a).expect("mkdir a");
        std::fs::create_dir_all(&b).expect("mkdir b");

        let id_a1 = fallback_project_id("win-desktop", &a);
        let id_a2 = fallback_project_id("win-desktop", &a);
        let id_b = fallback_project_id("win-desktop", &b);
        assert_eq!(id_a1, id_a2, "same path must give same ID");
        assert_ne!(id_a1, id_b, "different paths must give different IDs");
        assert!(id_a1.starts_with("local:win-desktop:"));
    }

    #[tokio::test]
    async fn resolves_project_from_git_remote() {
        let db = test_db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        testutil::init_test_repo_with_remote(dir.path(), "git@github.com:nazo6/flexagent.git")
            .await;

        let resolved = resolve_project(dir.path(), "test-node", &db)
            .await
            .expect("resolve");
        assert_eq!(resolved.project_id, "github.com/nazo6/flexagent");
        assert_eq!(resolved.name, "flexagent");
        assert_eq!(
            resolved.canonical_git_url.as_deref(),
            Some("git@github.com:nazo6/flexagent.git")
        );
        assert!(resolved.is_git_repo);
        assert!(resolved.relative_subpath.is_none());
        assert_eq!(resolved.identity.kind, ProjectIdentityKind::GitRemote);
    }

    #[tokio::test]
    async fn resolves_monorepo_subpath() {
        let db = test_db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        testutil::init_test_repo_with_remote(dir.path(), "git@github.com:nazo6/flexagent.git")
            .await;
        let sub = dir.path().join("packages/sub");
        std::fs::create_dir_all(&sub).expect("mkdir");

        let resolved = resolve_project(&sub, "test-node", &db)
            .await
            .expect("resolve");
        assert_eq!(
            resolved.project_id,
            "github.com/nazo6/flexagent#packages/sub"
        );
        assert_eq!(resolved.relative_subpath.as_deref(), Some("packages/sub"));
        assert_eq!(
            resolved.git_root.as_deref(),
            Some(canonical(dir.path()).as_path())
        );
        assert_eq!(resolved.name, "flexagent");
    }

    #[test]
    fn validates_user_keys_and_canonical_project_ids() {
        for key in [
            "internal.agent-tools",
            "flexagent-local",
            "github.com/nazo6/flexagent",
            "github.com/nazo6/flexagent#packages/sub",
        ] {
            assert!(validate_project_key(key).is_ok(), "rejected {key:?}");
        }
        for key in [
            "",
            "a b",
            "/leading",
            "trailing/",
            "a//b",
            "a#b#c",
            "#subpath",
            "subpath#",
            "local:test-node:0123",
            "user:foo",
        ] {
            assert!(validate_project_key(key).is_err(), "accepted {key:?}");
        }
        assert!(validate_project_key(&"x".repeat(MAX_PROJECT_KEY_LEN)).is_ok());
        assert!(validate_project_key(&"x".repeat(MAX_PROJECT_KEY_LEN + 1)).is_err());
    }

    #[tokio::test]
    async fn explicit_link_wins_over_git_remote() {
        let db = test_db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        testutil::init_test_repo_with_remote(dir.path(), "git@github.com:nazo6/flexagent.git")
            .await;
        let path = canonical(dir.path()).to_string_lossy().into_owned();
        db.upsert_project_path_link(&path, "internal.agent-tools")
            .await
            .expect("link");

        let resolved = resolve_project(dir.path(), "test-node", &db)
            .await
            .expect("resolve");
        assert_eq!(resolved.project_id, "user:internal.agent-tools");
        assert_eq!(resolved.name, "internal.agent-tools");
        assert_eq!(resolved.identity.kind, ProjectIdentityKind::UserKey);
        assert_eq!(
            resolved.matched_link_path.as_deref(),
            Some(canonical(dir.path()).as_path())
        );
    }

    #[tokio::test]
    async fn explicit_canonical_link_uses_project_id_verbatim() {
        let db = test_db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        testutil::init_test_repo_with_remote(dir.path(), "git@github.com:nazo6/flexagent.git")
            .await;
        let path = canonical(dir.path()).to_string_lossy().into_owned();
        db.upsert_project_path_link(&path, "github.com/nazo6/flexagent")
            .await
            .expect("link");

        let resolved = resolve_project(dir.path(), "test-node", &db)
            .await
            .expect("resolve");
        assert_eq!(resolved.project_id, "github.com/nazo6/flexagent");
        assert_eq!(resolved.name, "flexagent");
        assert_eq!(resolved.identity.kind, ProjectIdentityKind::GitRemote);
    }

    #[tokio::test]
    async fn link_on_parent_directory_applies_to_subdirectory() {
        let db = test_db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        testutil::init_test_repo_with_remote(dir.path(), "git@github.com:nazo6/flexagent.git")
            .await;
        let sub = dir.path().join("packages/sub");
        std::fs::create_dir_all(&sub).expect("mkdir");
        let path = canonical(dir.path()).to_string_lossy().into_owned();
        db.upsert_project_path_link(&path, "mono")
            .await
            .expect("link");

        let resolved = resolve_project(&sub, "test-node", &db)
            .await
            .expect("resolve");
        assert_eq!(resolved.project_id, "user:mono");
    }

    #[tokio::test]
    async fn non_git_directory_falls_back_to_local_project() {
        let db = test_db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let resolved = resolve_project(dir.path(), "home-mac", &db)
            .await
            .expect("resolve");
        assert!(!resolved.is_git_repo);
        assert_eq!(resolved.identity.kind, ProjectIdentityKind::Local);
        assert!(resolved.project_id.starts_with("local:home-mac:"));
        assert!(resolved.canonical_git_url.is_none());
        assert_eq!(
            resolved.name,
            dir.path().file_name().unwrap().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn git_repo_without_remote_falls_back() {
        let db = test_db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        testutil::init_test_repo(dir.path()).await;
        let resolved = resolve_project(dir.path(), "linux-vps", &db)
            .await
            .expect("resolve");
        assert!(resolved.is_git_repo);
        assert_eq!(resolved.identity.kind, ProjectIdentityKind::Local);
        assert!(resolved.project_id.starts_with("local:linux-vps:"));
    }

    #[tokio::test]
    async fn worktree_inherits_main_worktree_link() {
        let db = test_db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        testutil::init_test_repo(&repo).await;
        // Create a commit so a worktree can be added.
        std::fs::write(repo.join("README.md"), "hello\n").expect("write");
        git::run_git(&repo, &["add", "."]).await.expect("add");
        git::run_git(
            &repo,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "init",
            ],
        )
        .await
        .expect("commit");
        let worktree = dir.path().join("wt");
        git::run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "feat",
                worktree.to_str().expect("utf8"),
            ],
        )
        .await
        .expect("worktree add");

        let path = canonical(&repo).to_string_lossy().into_owned();
        db.upsert_project_path_link(&path, "shared-key")
            .await
            .expect("link");

        let resolved = resolve_project(&worktree, "test-node", &db)
            .await
            .expect("resolve");
        assert_eq!(resolved.project_id, "user:shared-key");
        assert!(resolved.is_git_repo);
    }
}

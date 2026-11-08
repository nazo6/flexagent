//! Identity resolution for logical projects (`Logical Project`).
//!
//! Design: `docs/01-architecture-and-sync.md` §4.
//! Determines the `project_key` that ties together different paths on different
//! nodes (Windows `D:\ghq\github.com\nazo6\flexagent`, a Linux VPS, WSL, and so on)
//! as the "same project".
//!
//! Resolution order:
//! 1. `project_key` in `.fxg.toml` (explicit, highest priority)
//! 2. Normalized `git remote.origin.url` ([`normalize_git_url`])
//! 3. Fallback `local:<node_id>:<path-hash>` (not Git and no `.fxg.toml`)

use std::path::{Path, PathBuf};

use fxg_protocol::config::ProjectConfig;

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
    // 4. Strip the port number (`gitlab.example.com:2222/team/app.git`)
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

/// Fallback project ID for non-Git projects (and projects without `.fxg.toml`).
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

/// Source of the `project_key` resolution (re-export of
/// [`fxg_protocol::common::ProjectResolutionSource`]).
pub use fxg_protocol::common::ProjectResolutionSource;

/// A resolved logical project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProject {
    /// Logical project ID (`project_key`)
    pub project_id: String,
    /// Display name
    pub name: String,
    /// Git URL before normalization
    pub canonical_git_url: Option<String>,
    /// Git root (`None` for non-Git projects)
    pub git_root: Option<PathBuf>,
    /// Relative subpath from the Git root (monorepo; `None` directly under the root)
    pub relative_subpath: Option<String>,
    /// Directory being resolved (normalized absolute path)
    pub local_path: PathBuf,
    /// Whether this is a Git repository
    pub is_git_repo: bool,
    /// Resolution source
    pub source: ProjectResolutionSource,
}

impl ResolvedProject {
    /// Record to register in the `projects` table.
    pub fn to_project_record(&self) -> fxg_db::ProjectRecord {
        fxg_db::ProjectRecord {
            project_id: self.project_id.clone(),
            name: self.name.clone(),
            canonical_git_url: self.canonical_git_url.clone(),
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
}

/// Loads `.fxg.toml` in the order "target directory -> Git root".
///
/// Used to look up worktree creation hooks (`copy_files` / `post_create`) and
/// per-project default settings.
pub async fn load_project_config(cwd: &Path) -> Result<Option<ProjectConfig>, NodeError> {
    if let Some(config) = ProjectConfig::load_from_dir(cwd)? {
        return Ok(Some(config));
    }
    if let Some(root) = git::toplevel(cwd).await?
        && root != cwd
    {
        return Ok(ProjectConfig::load_from_dir(&root)?);
    }
    Ok(None)
}

/// Writes `project_key` into `.fxg.toml` (preserving the existing settings).
///
/// Shared by `fxg project link` and the client API (`POST .../projects/link`).
pub fn patch_project_key(dir: &Path, project_key: &str) -> Result<(), NodeError> {
    let path = dir.join(fxg_protocol::config::PROJECT_CONFIG_FILE_NAME);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(NodeError::io(&path, err)),
    };
    let mut lines: Vec<String> = existing.lines().map(str::to_owned).collect();

    // Embed safely as a TOML string literal
    let literal = format!(
        "\"{}\"",
        project_key.replace('\\', "\\\\").replace('"', "\\\"")
    );

    let mut replaced = false;
    for line in lines.iter_mut() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("project_key") && line.contains('=') {
            *line = format!("project_key = {literal}");
            replaced = true;
            break;
        }
    }
    if !replaced {
        lines.insert(0, format!("project_key = {literal}"));
    }

    let mut output = lines.join("\n");
    output.push('\n');
    std::fs::write(&path, output).map_err(|err| NodeError::io(&path, err))?;

    // Verify that it can actually be read back (never write broken TOML)
    ProjectConfig::load_from_path(&path)?;
    Ok(())
}

/// Resolves the logical project of the given directory.
///
/// `.fxg.toml` is searched in the order starting directory -> Git root.
pub async fn resolve_project(cwd: &Path, node_id: &str) -> Result<ResolvedProject, NodeError> {
    let local_path = fxg_pty::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let git_root = git::toplevel(&local_path).await?;
    let project_config = load_project_config(&local_path).await?;

    let canonical_git_url = match &git_root {
        Some(root) => git::remote_origin_url(root).await?,
        None => None,
    };

    let relative_subpath = git_root.as_ref().and_then(|root| {
        local_path
            .strip_prefix(root)
            .ok()
            .filter(|subpath| !subpath.as_os_str().is_empty())
            .map(|subpath| subpath.to_string_lossy().replace('\\', "/"))
    });

    let folder_name = local_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| local_path.display().to_string());
    let repo_name = git_root
        .as_ref()
        .and_then(|root| root.file_name())
        .map(|name| name.to_string_lossy().into_owned());

    let name = project_config
        .as_ref()
        .and_then(|config| config.name.clone())
        .or(repo_name)
        .unwrap_or(folder_name);

    let (project_id, source) = if let Some(key) = project_config
        .as_ref()
        .and_then(|config| config.project_key.clone())
        .filter(|key| !key.trim().is_empty())
    {
        (key, ProjectResolutionSource::FxgToml)
    } else if let Some(url) = &canonical_git_url {
        (
            normalize_git_url(url, relative_subpath.as_deref().unwrap_or("")),
            ProjectResolutionSource::GitRemote,
        )
    } else {
        (
            fallback_project_id(node_id, &local_path),
            ProjectResolutionSource::Fallback,
        )
    };

    Ok(ResolvedProject {
        project_id,
        name,
        canonical_git_url,
        is_git_repo: git_root.is_some(),
        git_root,
        relative_subpath,
        local_path,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

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
        // scheme / no userinfo
        assert_eq!(
            normalize_git_url("github.com/nazo6/flexagent", ""),
            "github.com/nazo6/flexagent"
        );
        // Uppercase host / trailing slash / no .git
        assert_eq!(
            normalize_git_url(" HTTPS://GitHub.com/nazo6/flexagent/ ", ""),
            "github.com/nazo6/flexagent"
        );
        // https with a port
        assert_eq!(
            normalize_git_url("http://192.168.1.5:8080/team/app.git", ""),
            "192.168.1.5/team/app"
        );
        // Subpath (monorepo)
        assert_eq!(
            normalize_git_url("git@github.com:nazo6/flexagent.git", "./packages/sub/"),
            "github.com/nazo6/flexagent#packages/sub"
        );
        // Backslash subpath (Windows input)
        assert_eq!(
            normalize_git_url("git@github.com:nazo6/flexagent.git", r"packages\sub"),
            "github.com/nazo6/flexagent#packages/sub"
        );
        // Group hierarchy
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
        let dir = tempfile::tempdir().expect("tempdir");
        testutil::init_test_repo_with_remote(dir.path(), "git@github.com:nazo6/flexagent.git")
            .await;

        let resolved = resolve_project(dir.path(), "test-node")
            .await
            .expect("resolve");
        assert_eq!(resolved.project_id, "github.com/nazo6/flexagent");
        assert_eq!(
            resolved.name,
            dir.path().file_name().unwrap().to_string_lossy()
        );
        assert_eq!(
            resolved.canonical_git_url.as_deref(),
            Some("git@github.com:nazo6/flexagent.git")
        );
        assert!(resolved.is_git_repo);
        assert!(resolved.relative_subpath.is_none());
        assert_eq!(resolved.source, ProjectResolutionSource::GitRemote);
    }

    #[tokio::test]
    async fn resolves_monorepo_subpath() {
        let dir = tempfile::tempdir().expect("tempdir");
        testutil::init_test_repo_with_remote(dir.path(), "git@github.com:nazo6/flexagent.git")
            .await;
        let sub = dir.path().join("packages/sub");
        std::fs::create_dir_all(&sub).expect("mkdir");

        let resolved = resolve_project(&sub, "test-node").await.expect("resolve");
        assert_eq!(
            resolved.project_id,
            "github.com/nazo6/flexagent#packages/sub"
        );
        assert_eq!(resolved.relative_subpath.as_deref(), Some("packages/sub"));
        // The Git root is resolved as well
        assert_eq!(
            resolved.git_root.as_deref(),
            Some(canonical(dir.path()).as_path())
        );
    }

    #[tokio::test]
    async fn fxg_toml_project_key_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        testutil::init_test_repo_with_remote(dir.path(), "git@github.com:nazo6/flexagent.git")
            .await;
        std::fs::write(
            dir.path().join(".fxg.toml"),
            "project_key = \"internal/agent-tools\"\nname = \"Agent Tools\"\n",
        )
        .expect("write .fxg.toml");

        let resolved = resolve_project(dir.path(), "test-node")
            .await
            .expect("resolve");
        assert_eq!(resolved.project_id, "internal/agent-tools");
        assert_eq!(resolved.name, "Agent Tools");
        assert_eq!(resolved.source, ProjectResolutionSource::FxgToml);
    }

    #[tokio::test]
    async fn non_git_directory_falls_back_to_local_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolved = resolve_project(dir.path(), "home-mac")
            .await
            .expect("resolve");
        assert!(!resolved.is_git_repo);
        assert_eq!(resolved.source, ProjectResolutionSource::Fallback);
        assert!(resolved.project_id.starts_with("local:home-mac:"));
        assert!(resolved.canonical_git_url.is_none());
    }

    #[tokio::test]
    async fn git_repo_without_remote_falls_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        testutil::init_test_repo(dir.path()).await;
        let resolved = resolve_project(dir.path(), "linux-vps")
            .await
            .expect("resolve");
        assert!(resolved.is_git_repo);
        assert_eq!(resolved.source, ProjectResolutionSource::Fallback);
        assert!(resolved.project_id.starts_with("local:linux-vps:"));
    }

    #[test]
    fn patch_project_key_preserves_other_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path()
                .join(fxg_protocol::config::PROJECT_CONFIG_FILE_NAME),
            "name = \"flexagent\"\n\n[worktree]\nbase_branch = \"main\"\n",
        )
        .expect("write");
        patch_project_key(dir.path(), "github.com/nazo6/flexagent").expect("patch");
        let config = ProjectConfig::load_from_dir(dir.path())
            .expect("load")
            .expect("exists");
        assert_eq!(
            config.project_key.as_deref(),
            Some("github.com/nazo6/flexagent")
        );
        assert_eq!(config.name.as_deref(), Some("flexagent"));
        assert_eq!(config.worktree.base_branch.as_deref(), Some("main"));

        // Re-running must not create duplicate lines
        patch_project_key(dir.path(), "other/key").expect("patch again");
        let config = ProjectConfig::load_from_dir(dir.path())
            .expect("load")
            .expect("exists");
        assert_eq!(config.project_key.as_deref(), Some("other/key"));
        let text = std::fs::read_to_string(
            dir.path()
                .join(fxg_protocol::config::PROJECT_CONFIG_FILE_NAME),
        )
        .expect("read");
        assert_eq!(text.matches("project_key").count(), 1);
    }

    #[test]
    fn patch_project_key_does_not_overwrite_unreadable_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir
            .path()
            .join(fxg_protocol::config::PROJECT_CONFIG_FILE_NAME);
        // A directory at the config path makes read_to_string fail with a
        // non-NotFound error. The existing path must not be replaced by a file.
        std::fs::create_dir(&path).expect("create directory");
        let err = patch_project_key(dir.path(), "github.com/nazo6/flexagent")
            .expect_err("patch must fail");
        assert!(matches!(err, NodeError::Io { .. }));
        assert!(path.is_dir(), "existing path must be left untouched");
    }

    fn canonical(path: &Path) -> PathBuf {
        fxg_pty::canonicalize(path).expect("canonicalize")
    }
}

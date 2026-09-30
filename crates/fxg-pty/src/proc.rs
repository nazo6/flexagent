//! コマンド解決 (`which` + `PATHEXT`)・パス正規化 (`dunce`)・
//! Windows Job Object (`WinJobGuard`) によるプロセスツリー管理。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §5.1〜§5.2。
//!
//! - **コマンド解決**: `npx` / `uvx` / `opencode2` などを起動する際は必ず
//!   [`resolve_command`] を通し、Windows では `opencode2.cmd` や `npx.cmd` の
//!   拡張子 (`PATHEXT`) を解決してからプロセスを起動する。
//! - **UNC パス回避**: Windows の `std::fs::canonicalize` は `\\?\D:\ghq\...`
//!   という UNC プレフィックスを付けてしまい、Node.js 製エージェント等がパス解釈に
//!   失敗することがあるため、必ず [`canonicalize`] (`dunce`) を使用する。
//! - **プロセスツリー確実終了**: Windows では子プロセスを **Job Object** に
//!   バインドし、デーモン終了時に孫プロセスまで確実に終了させる
//!   ([`ProcessTreeGuard`])。

use std::path::{Path, PathBuf};

use crate::error::PtyError;

/// 指定ディレクトリ・`PATH` (`PATHEXT` 対応) からコマンドを解決する。
///
/// Windows では `.cmd` / `.exe` 等の拡張子を自動解決する
/// (`which::which_in` が `PATHEXT` を考慮する)。
pub fn resolve_command(command: &str, cwd: impl AsRef<Path>) -> Result<PathBuf, PtyError> {
    let cwd = cwd.as_ref();
    which::which_in(command, std::env::var_os("PATH"), cwd).map_err(|_| PtyError::CommandNotFound {
        command: command.to_owned(),
        cwd: cwd.to_path_buf(),
    })
}

/// パスを正規化する (UNC プレフィックス `\\?\` を付けない `dunce::canonicalize`)。
pub fn canonicalize(path: impl AsRef<Path>) -> Result<PathBuf, PtyError> {
    let path = path.as_ref();
    dunce::canonicalize(path).map_err(|source| PtyError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// プロセスツリーをプロセスグループ/Job Object 単位で確実に終了させるガード。
///
/// - **Windows**: 子プロセスを **Job Object** (`kill on job close`) にバインドする。
///   ガード (およびデーモン) が終了すると、Job Object のハンドルが閉じられ
///   **孫プロセスを含むツリー全体が確実に終了**する。
/// - **Unix**: PTY セッション (セッションリーダー) の終了で子プロセスグループが
///   終了するため、ここでは何もしない (追加の状態は持たない)。
pub struct ProcessTreeGuard {
    #[cfg(windows)]
    job: WinJobGuard,
}

impl std::fmt::Debug for ProcessTreeGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessTreeGuard")
            .field(
                "backend",
                &if cfg!(windows) { "job-object" } else { "none" },
            )
            .finish()
    }
}

impl ProcessTreeGuard {
    /// 新しいガードを作成する (Windows では kill-on-close の Job Object を生成)。
    pub fn new() -> Result<Self, PtyError> {
        #[cfg(windows)]
        {
            Ok(Self {
                job: WinJobGuard::new_kill_on_close()?,
            })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }

    /// 子プロセスをガードへ参加させる (Windows: Job Object へ割当 / Unix: no-op)。
    ///
    /// 参加済みのプロセスは、ガードが Drop された際に**プロセスツリーごと**終了する。
    pub fn attach(&self, child: &dyn portable_pty::Child) -> Result<(), PtyError> {
        #[cfg(windows)]
        {
            if let Some(handle) = child.as_raw_handle() {
                self.job.assign_process(handle)?;
            }
        }
        #[cfg(not(windows))]
        {
            // Unix では PTY セッションの終了に任せる (プロセスは session leader として
            // PTY に紐づき、master 側のクローズで SIGHUP を受ける)。
            let _ = child;
        }
        Ok(())
    }

    /// Job Object によるプロセスツリー管理が有効か (診断用)。
    pub fn is_job_object_backend(&self) -> bool {
        cfg!(windows)
    }
}

/// Windows Job Object の薄いラッパー (`kill on job close`)。
///
/// Drop 時に Job ハンドルが閉じられ、割当済みプロセス (孫プロセス含む) が
/// すべて終了する。エージェントの子プロセスが `Child::kill()` をすり抜けても
/// ファイルロック等が残らないようにするための必須機構
/// (設計: `docs/04-agent-drivers-and-windows.md` §5.1)。
#[cfg(windows)]
pub struct WinJobGuard {
    job: win32job::Job,
}

#[cfg(windows)]
impl WinJobGuard {
    /// kill-on-close が設定された Job Object を作成する。
    pub fn new_kill_on_close() -> Result<Self, PtyError> {
        let job = win32job::Job::create()?;
        let mut info = job.query_extended_limit_info()?;
        info.limit_kill_on_job_close();
        job.set_extended_limit_info(&mut info)?;
        Ok(Self { job })
    }

    /// プロセスハンドルを Job Object に割り当てる。
    pub fn assign_process(
        &self,
        process_handle: std::os::windows::io::RawHandle,
    ) -> Result<(), PtyError> {
        self.job.assign_process(process_handle as isize)?;
        Ok(())
    }

    /// ガード保持プロセス (デーモン自身) を Job Object に割り当てる。
    pub fn assign_current_process(&self) -> Result<(), PtyError> {
        self.job.assign_current_process()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_command_finds_shell_tools() {
        let cwd = std::env::current_dir().expect("cwd");
        let resolved = resolve_command("git", &cwd).expect("git must be resolvable");
        assert!(resolved.is_absolute());
    }

    #[test]
    fn resolve_command_reports_missing_command() {
        let cwd = std::env::current_dir().expect("cwd");
        let err = resolve_command("fxg-definitely-missing-binary", &cwd).expect_err("missing");
        match err {
            PtyError::CommandNotFound { command, .. } => {
                assert_eq!(command, "fxg-definitely-missing-binary");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn canonicalize_avoids_windows_unc_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolved = canonicalize(dir.path()).expect("canonicalize");
        assert!(
            !resolved.to_string_lossy().starts_with(r"\\?\"),
            "dunce は UNC プレフィックスを付けない: {}",
            resolved.display()
        );
    }

    #[test]
    fn process_tree_guard_can_be_created() {
        let guard = ProcessTreeGuard::new().expect("guard");
        assert_eq!(guard.is_job_object_backend(), cfg!(windows));
    }
}

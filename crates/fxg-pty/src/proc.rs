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

    /// Windows の RawHandle を Job Object へ割り当てる。
    #[cfg(windows)]
    pub fn attach_raw_handle(
        &self,
        handle: std::os::windows::io::RawHandle,
    ) -> Result<(), PtyError> {
        self.job.assign_process(handle)
    }

    /// `std::process::Child` をガードへ参加させる (Windows: Job Object へ割当 / Unix: no-op)。
    pub fn attach_std_child(&self, child: &std::process::Child) -> Result<(), PtyError> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            self.job.assign_process(child.as_raw_handle())?;
        }
        #[cfg(not(windows))]
        {
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
        job.set_extended_limit_info(&info)?;
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

    /// Windows Job Object: ルートプロセスが起動した**孫プロセス**が、
    /// ガード (Job) の Drop による kill-on-close で確実に終了すること
    /// (Phase 2 完了条件 / CI: windows-latest で自動検証)。
    #[cfg(windows)]
    #[test]
    fn job_object_kills_grandchildren_on_guard_drop() {
        use std::os::windows::io::AsRawHandle;
        use std::process::Command as StdCommand;

        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("grandchild.pid");

        let guard = ProcessTreeGuard::new().expect("guard");

        // ルート (PowerShell) を Job へ割り当ててから、孫 (ping) を起動させる。
        // Job メンバーが作成したプロセスは自動的に同じ Job に所属するため、
        // ルートが終了しても孫は Job に残る。猶予 (Start-Sleep) は、孫を生む前に
        // ルートの Job 割当が完了していることを保証するためのもの。
        let script = format!(
            "Start-Sleep -Milliseconds 1500; \
             $p = Start-Process -PassThru -FilePath ping -ArgumentList '-n','30','127.0.0.1'; \
             Set-Content -Path '{}' -Value $p.Id",
            pid_file.display()
        );
        let mut root = StdCommand::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn root");
        guard
            .job
            .assign_process(root.as_raw_handle())
            .expect("assign root process to job");

        let grandchild_pid = wait_for_pid_file(&pid_file);
        assert!(
            windows_process_is_running(grandchild_pid),
            "grandchild ({grandchild_pid}) must be running before guard drop"
        );
        let _ = root.wait();

        // ガード Drop → Job ハンドルクローズ → kill-on-close で孫が終了する
        drop(guard);

        let mut killed = false;
        for _ in 0..100 {
            if !windows_process_is_running(grandchild_pid) {
                killed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !killed {
            // テスト失敗時に ping を残さない (ベストエフォート)
            let _ = StdCommand::new("taskkill")
                .args(["/PID", &grandchild_pid.to_string(), "/T", "/F"])
                .status();
        }
        assert!(killed, "grandchild process survived job object close");
    }

    /// 孫プロセスの PID がファイルに書き出されるまで待つ。
    #[cfg(windows)]
    fn wait_for_pid_file(path: &Path) -> u32 {
        for _ in 0..150 {
            if let Ok(text) = std::fs::read_to_string(path)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                return pid;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("grandchild pid file was not written: {}", path.display());
    }

    /// `tasklist` で PID のプロセスが稼働中か確認する。
    #[cfg(windows)]
    fn windows_process_is_running(pid: u32) -> bool {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .expect("tasklist");
        let text = String::from_utf8_lossy(&output.stdout);
        text.split_whitespace()
            .any(|field| field == pid.to_string())
    }
}

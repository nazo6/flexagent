//! コマンド解決 (`which` + `PATHEXT`)・パス正規化 (`dunce`)・
//! Windows Job Object (`WinJobGuard`) によるプロセスツリー管理。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §5.1〜§5.2。
//!
//! - **コマンド解決**: `npx` / `uvx` / `opencode` などを起動する際は必ず
//!   [`resolve_command`] を通し、Windows では `opencode.cmd` や `npx.cmd` の
//!   拡張子 (`PATHEXT`) を解決してからプロセスを起動する。
//! - **UNC パス回避**: Windows の `std::fs::canonicalize` は `\\?\D:\ghq\...`
//!   という UNC プレフィックスを付けてしまい、Node.js 製エージェント等がパス解釈に
//!   失敗することがあるため、必ず [`canonicalize`] (`dunce`) を使用する。
//! - **プロセスツリー確実終了**: Windows では子プロセスを **Job Object** に
//!   バインドし、デーモン終了時に孫プロセスまで確実に終了させる
//!   ([`ProcessTreeGuard`])。
//! - **ラッパー型ランチャーの正常終了**: `npx` → node / `uvx` → python /
//!   PyInstaller onefile のようなラッパーは「内側のプロセスが終了すると、自身で
//!   後始末をしてから自然終了する」。このため終了時は内側から順に終了し、ルートの
//!   自然終了を待ってから強制終了する ([`ProcessTreeGuard::shutdown_tree`])。
//!   公式 ACP SDK も Unix では同じ理由でプロセスグループ単位の終了を行う。

use std::path::{Path, PathBuf};

#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
#[cfg(windows)]
use windows::Win32::System::JobObjects::{
    JOBOBJECT_BASIC_PROCESS_ID_LIST, JobObjectBasicProcessIdList, QueryInformationJobObject,
};
#[cfg(windows)]
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
    WaitForSingleObject,
};

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

/// [`ProcessTreeGuard::shutdown_tree`] の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeShutdownOutcome {
    /// 終了させたルート以外のプロセス数。
    ///
    /// 子孫を列挙できない Unix では常に 0 (既知の制約)。
    pub terminated_descendants: usize,
    /// ルートが猶予内に自然終了したか。
    ///
    /// 待機を行わない Unix では常に `false`。
    pub root_exited_naturally: bool,
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

    /// 指定した PID 以外の Job メンバーをすべて終了する (Windows のみ。ベストエフォート)。
    ///
    /// 戻り値は終了できたプロセス数 (既に終了済みのものは含まない)。
    #[cfg(windows)]
    pub fn terminate_all_except(&self, keep_pid: u32) -> Result<usize, PtyError> {
        self.job.terminate_all_except(keep_pid)
    }

    /// Job Object によるプロセスツリー管理が有効か (診断用)。
    pub fn is_job_object_backend(&self) -> bool {
        cfg!(windows)
    }
}

/// プロセスツリーの終了ポリシー (Windows 実装)。
///
/// ラッパー型ランチャー (`npx` → node / `uvx` → python / PyInstaller onefile 等) は
/// 「内側のプロセスが終了すると、自身で後始末をしてから自然終了する」。
#[cfg(windows)]
impl ProcessTreeGuard {
    /// 指定 PID のプロセスを Job Object へ割り当てる (終了時のツリー kill 対象にする)。
    ///
    /// ハンドルを開き直すため、呼び出し側は生ハンドルを保持する必要がない
    /// (`child.id()` だけで済み、プラットフォーム分岐が呼び出し側に漏れない)。
    pub fn attach_pid(&self, pid: u32) -> Result<(), PtyError> {
        unsafe {
            let handle = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid)?;
            let result = self.job.assign_process(handle.0);
            let _ = CloseHandle(handle);
            result
        }
    }

    /// プロセスツリーを「内側から」終了させ、ルートの自然終了を待つ。
    ///
    /// 1. ルート以外の Job メンバーを終了し、
    /// 2. ルートが自然終了するのを `grace` まで待ち (`WaitForSingleObject`、
    ///    ポーリングなし)、
    /// 3. 猶予を超えた場合のみルートを強制終了する。
    ///
    /// ルートに子孫が居ない場合は待機せず即時終了する (通常のプロセスと同じ)。
    /// ルートが既に終了している場合も成功として扱う。
    pub async fn shutdown_tree(
        &self,
        root_pid: u32,
        grace: std::time::Duration,
    ) -> Result<TreeShutdownOutcome, PtyError> {
        // 「ラッパー型か」は終了要求の成否ではなくメンバー構成で判定する
        // (直前に内側のプロセスが自然終了していても後始末待ちを省かないため)。
        let has_descendants = self
            .job
            .process_ids()?
            .into_iter()
            .any(|pid| pid != 0 && pid != root_pid);
        let terminated_descendants = self.job.terminate_all_except(root_pid)?;
        if !has_descendants {
            terminate_process(root_pid);
            return Ok(TreeShutdownOutcome {
                terminated_descendants,
                root_exited_naturally: false,
            });
        }
        // ルート (ラッパー) が後始末を終えて自ら終了するのを待つ。
        let timeout_ms = u32::try_from(grace.as_millis()).unwrap_or(u32::MAX);
        let exited =
            tokio::task::spawn_blocking(move || wait_for_process_exit(root_pid, timeout_ms))
                .await
                .unwrap_or(false);
        if !exited {
            terminate_process(root_pid);
        }
        Ok(TreeShutdownOutcome {
            terminated_descendants,
            root_exited_naturally: exited,
        })
    }
}

/// プロセスツリーの終了ポリシー (Unix 実装、既知の制約あり)。
///
/// Job Object に相当する仕組みが無いため、ルート以外のプロセスを列挙・終了
/// できない。ルート (直接の子) を `SIGKILL` するのみで、後始末の待機は
/// 呼び出し側の `Child::status()` に任せる。
///
/// このためラッパー型ランチャーの内側のプロセスは孤児になり得る (公式 ACP SDK
/// はプロセスグループ単位の kill で対処するが、ルートごと終了するためラッパーの
/// 後始末は走らない)。詳細: `docs/changelog/2026-10-01-antigravity-startup.md`。
#[cfg(not(windows))]
impl ProcessTreeGuard {
    /// Unix では Job Object に相当する仕組みが無いため何もしない (API 互換用)。
    pub fn attach_pid(&self, pid: u32) -> Result<(), PtyError> {
        let _ = pid;
        Ok(())
    }

    /// プロセスツリーを終了させる (ルートの `SIGKILL` のみ)。
    ///
    /// Windows 実装と異なり待機・子孫列挙を行わない。`grace` は API 互換のため
    /// 受け取るが使用しない。
    pub async fn shutdown_tree(
        &self,
        root_pid: u32,
        grace: std::time::Duration,
    ) -> Result<TreeShutdownOutcome, PtyError> {
        // 待機しない理由: ルートの reap (`waitpid`) は呼び出し側の
        // `Child::status()` と競合するため、fxg-pty 側では行わない。
        let _ = grace;
        unsafe {
            if libc::kill(root_pid as libc::pid_t, libc::SIGKILL) != 0 {
                let err = std::io::Error::last_os_error();
                // ESRCH (既に終了済み) は正常系
                if err.raw_os_error() != Some(libc::ESRCH) {
                    tracing::debug!(root_pid, "failed to kill process: {err}");
                }
            }
        }
        Ok(TreeShutdownOutcome {
            terminated_descendants: 0,
            root_exited_naturally: false,
        })
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

    /// Job Object に現在割り当てられているプロセス ID の一覧を返す。
    ///
    /// ジョブのメンバーが実用上あり得ない数 (`MAX_JOB_MEMBERS`) を超えている
    /// 場合は取得を諦めてエラーを返す (呼び出し側は従来の終了処理へ
    /// フォールバックする)。
    pub fn process_ids(&self) -> Result<Vec<u32>, PtyError> {
        /// 一度に取得できる Job メンバーの上限 (エージェントツリーは通常 10 未満)。
        const MAX_JOB_MEMBERS: usize = 1024;
        /// `JOBOBJECT_BASIC_PROCESS_ID_LIST.ProcessIdList` までのバイトオフセット。
        const HEADER_BYTES: usize =
            std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList);

        // `JOBOBJECT_BASIC_PROCESS_ID_LIST` は可変長 (ULONG_PTR 配列) のため、
        // `usize` 単位で確保して整列を保証する。
        let mut buffer = vec![
            0usize;
            (HEADER_BYTES + MAX_JOB_MEMBERS * std::mem::size_of::<usize>())
                .div_ceil(std::mem::size_of::<usize>())
        ];
        let buffer_bytes =
            u32::try_from(buffer.len() * std::mem::size_of::<usize>()).unwrap_or(u32::MAX);
        unsafe {
            QueryInformationJobObject(
                Some(HANDLE(self.job.handle() as *mut core::ffi::c_void)),
                JobObjectBasicProcessIdList,
                buffer.as_mut_ptr().cast(),
                buffer_bytes,
                None,
            )?;
            let count = (*buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>())
                .NumberOfProcessIdsInList as usize;
            // ULONG_PTR 配列の先頭は `HEADER_BYTES / size_of::<usize>()` 番目のスロット。
            let list = buffer
                .as_ptr()
                .add(HEADER_BYTES / std::mem::size_of::<usize>());
            Ok((0..count.min(MAX_JOB_MEMBERS))
                .map(|index| *list.add(index) as u32)
                .collect())
        }
    }

    /// 指定した PID 以外の Job メンバーをすべて終了する (ベストエフォート)。
    ///
    /// 戻り値は終了できたプロセス数 (既に終了済みのものは含まない)。
    pub fn terminate_all_except(&self, keep_pid: u32) -> Result<usize, PtyError> {
        let mut terminated = 0usize;
        for pid in self.process_ids()? {
            if pid == 0 || pid == keep_pid {
                continue;
            }
            if terminate_process(pid) {
                terminated += 1;
            }
        }
        Ok(terminated)
    }
}

/// プロセスを強制終了する (ベストエフォート。既に終了していれば何もしない)。
///
/// 戻り値は `TerminateProcess` が成功したか。
#[cfg(windows)]
fn terminate_process(pid: u32) -> bool {
    unsafe {
        match OpenProcess(PROCESS_TERMINATE, false, pid) {
            Ok(handle) => {
                let terminated = TerminateProcess(handle, 1).is_ok();
                let _ = CloseHandle(handle);
                terminated
            }
            Err(err) => {
                tracing::debug!(pid, "failed to open process for termination: {err}");
                false
            }
        }
    }
}

/// プロセスの終了を最大 `timeout_ms` ミリ秒まで待つ (ポーリングなし)。
///
/// プロセスが既に存在しない場合も `true` を返す。プロセスハンドルのシグナル
/// 状態を `WaitForSingleObject` で待つ。
#[cfg(windows)]
fn wait_for_process_exit(pid: u32, timeout_ms: u32) -> bool {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else {
            // プロセスが既に終了している
            return true;
        };
        let result = WaitForSingleObject(handle, timeout_ms);
        let _ = CloseHandle(handle);
        result == WAIT_OBJECT_0
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
        let ping_out = dir.path().join("grandchild.out");
        let ping_err = dir.path().join("grandchild.err");

        let guard = ProcessTreeGuard::new().expect("guard");

        // ルート (PowerShell) を Job へ割り当ててから、孫 (ping) を起動させる。
        // Job メンバーが作成したプロセスは自動的に同じ Job に所属するため、
        // ルートが終了しても孫は Job に残る。猶予 (Start-Sleep) は、孫を生む前に
        // ルートの Job 割当が完了していることを保証するためのもの。
        //
        // `-NoNewWindow` + 出力リダイレクトで**新しいコンソールを作らない**:
        // 既定の `Start-Process` は孫用に新しいコンソールウィンドウを作るため、
        // テスト実行時に ping の窓が開いてしまい、さらに Job の kill-on-close と
        // コンソール初期化が競合すると conhost が
        // 「起動時にエラー 0x800700e8 (ERROR_NO_DATA: パイプが閉じられています)」
        // をその窓へ出力する (テスト自体は成功するがノイズになる)。
        let script = format!(
            "Start-Sleep -Milliseconds 1500; \
             $p = Start-Process -PassThru -NoNewWindow -FilePath ping \
                  -ArgumentList '-n','30','127.0.0.1' \
                  -RedirectStandardOutput '{}' -RedirectStandardError '{}'; \
             Set-Content -Path '{}' -Value $p.Id",
            ping_out.display(),
            ping_err.display(),
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

    /// Windows Job Object: [`ProcessTreeGuard::terminate_all_except`] がルートを
    /// 残して他のメンバーだけを終了すること (PyInstaller onefile 等の一時ファイル
    /// 自動削除を妨げない終了処理の検証)。
    #[cfg(windows)]
    #[test]
    fn terminate_all_except_spares_root_but_kills_the_rest() {
        use std::os::windows::io::AsRawHandle;
        use std::process::Command as StdCommand;

        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("child.pid");
        let ping_out = dir.path().join("child.out");
        let ping_err = dir.path().join("child.err");

        let guard = ProcessTreeGuard::new().expect("guard");

        // ルート (powershell) は孫 (ping) を起動して 30 秒待機し続ける
        let script = format!(
            "$p = Start-Process -PassThru -NoNewWindow -FilePath ping \
                  -ArgumentList '-n','30','127.0.0.1' \
                  -RedirectStandardOutput '{}' -RedirectStandardError '{}'; \
             Set-Content -Path '{}' -Value $p.Id; \
             Start-Sleep -Seconds 30",
            ping_out.display(),
            ping_err.display(),
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

        let child_pid = wait_for_pid_file(&pid_file);
        assert!(
            windows_process_is_running(child_pid),
            "child ({child_pid}) must be running before termination"
        );

        // ルート以外 (孫) だけを終了する
        let terminated = guard.terminate_all_except(root.id()).expect("terminate");
        assert!(terminated >= 1, "expected at least one terminated member");

        let mut killed = false;
        for _ in 0..100 {
            if !windows_process_is_running(child_pid) {
                killed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !killed {
            // テスト失敗時に ping を残さない (ベストエフォート)
            let _ = StdCommand::new("taskkill")
                .args(["/PID", &child_pid.to_string(), "/T", "/F"])
                .status();
        }
        assert!(killed, "child process survived terminate_all_except");
        assert!(
            windows_process_is_running(root.id()),
            "root process must be spared by terminate_all_except"
        );

        // 後始末 (ガード Drop の kill-on-close でルートも終了する)
        drop(guard);
        let _ = root.kill();
        let _ = root.wait();
    }

    /// Windows Job Object: [`ProcessTreeGuard::shutdown_tree`] がラッパー型
    /// ランチャーの後始末完了 (自然終了) を待つこと。ルートは孫の終了を検知して
    /// 自らマーカーファイルを書き込んでから終了する。
    #[cfg(windows)]
    #[tokio::test]
    async fn shutdown_tree_waits_for_wrapper_cleanup() {
        use std::os::windows::io::AsRawHandle;
        use std::process::Command as StdCommand;

        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("child.pid");
        let cleanup_marker = dir.path().join("cleanup.done");
        let ping_out = dir.path().join("child.out");
        let ping_err = dir.path().join("child.err");

        let guard = ProcessTreeGuard::new().expect("guard");

        // ルート (powershell) は孫 (ping) を起動し、孫の終了を待ってから
        // 自ら後始末 (マーカーファイル) をして終了するラッパーを模す。
        let script = format!(
            "$p = Start-Process -PassThru -NoNewWindow -FilePath ping \
                  -ArgumentList '-n','60','127.0.0.1' \
                  -RedirectStandardOutput '{}' -RedirectStandardError '{}'; \
             Set-Content -Path '{}' -Value $p.Id; \
             Wait-Process -Id $p.Id; \
             Set-Content -Path '{}' -Value 'cleaned'",
            ping_out.display(),
            ping_err.display(),
            pid_file.display(),
            cleanup_marker.display()
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

        let child_pid = wait_for_pid_file(&pid_file);
        assert!(
            windows_process_is_running(child_pid),
            "child must be running"
        );

        let outcome = guard
            .shutdown_tree(root.id(), std::time::Duration::from_secs(15))
            .await
            .expect("shutdown_tree");
        assert!(
            outcome.terminated_descendants >= 1,
            "wrapper child must be terminated"
        );
        assert!(
            outcome.root_exited_naturally,
            "wrapper must exit naturally after its child exits"
        );
        assert!(
            cleanup_marker.is_file(),
            "wrapper cleanup must run before exit"
        );
        assert!(!windows_process_is_running(root.id()));
        let _ = root.wait();
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

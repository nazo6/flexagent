//! `portable-pty` による ConPTY (Windows) / Unix PTY の双方向ストリーム管理。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §5.3。
//!
//! - **クロスプラットフォーム PTY**: Windows では ConPTY、macOS/Linux では
//!   Unix PTY を `portable-pty` が自動選択する。
//! - **非同期ストリーミング**: Master 側の `Read` を専用スレッドで読み取り、
//!   [`PtyEvent`] として `tokio::sync::broadcast` で購読者 (WebSocket / IPC) へ
//!   配信する。入力は専用ライタースレッドが Master の `Write` へ即時フラッシュする。
//!   終了検知は `wait()` 専用スレッドが担う (Windows の ConPTY は子プロセス終了
//!   だけでは EOF にならないため、リーダーの EOF を終了の起点にはできない)。
//! - **動的リサイズ**: [`PtySessionManager::resize`] で ConPTY / Unix PTY の
//!   ウィンドウサイズを即時変更する。
//! - **プロセスツリー確実終了**: 子プロセスは [`ProcessTreeGuard`]
//!   (Windows: Job Object) にバインドする。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{broadcast, mpsc};

use crate::error::PtyError;
use crate::proc::{ProcessTreeGuard, resolve_command};

/// 購読者へ配信される PTY イベント。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    /// 出力チャンク (ANSI エスケープを含む生バイト列)
    Output {
        /// 対象 PTY ID
        pty_id: String,
        /// 出力データ
        data: Vec<u8>,
    },
    /// プロセス終了 (以降この PTY はセッションマップから削除される)
    Exit {
        /// 対象 PTY ID
        pty_id: String,
        /// 終了コード (取得できない場合は `None`)
        exit_code: Option<i32>,
    },
}

/// 稼働中 PTY の情報。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtySessionInfo {
    /// PTY ID
    pub pty_id: String,
    /// 紐づくエージェントセッション ID (任意)
    pub session_id: Option<String>,
    /// 子プロセスの OS プロセスID
    pub pid: Option<u32>,
    /// 現在の列数
    pub cols: u16,
    /// 現在の行数
    pub rows: u16,
}

/// PTY 起動要求。
#[derive(Debug, Clone)]
pub struct PtySpawnRequest {
    /// PTY ID (呼び出し側が採番)
    pub pty_id: String,
    /// 紐づくエージェントセッション ID (任意)
    pub session_id: Option<String>,
    /// 作業ディレクトリ (省略時は現在のディレクトリ)
    pub cwd: Option<PathBuf>,
    /// 起動するシェル/コマンド (省略時は OS 既定シェル)。
    /// 指定時は `PATH` (`PATHEXT`) から解決する。
    pub shell_cmd: Option<String>,
    /// `shell_cmd` へ渡す引数
    pub args: Vec<String>,
    /// 列数
    pub cols: u16,
    /// 行数
    pub rows: u16,
    /// 追加で設定する環境変数 (親プロセスの環境を継承する)
    pub env: Vec<(String, String)>,
}

impl PtySpawnRequest {
    /// 既定サイズ (80x24) の起動要求を作る。
    pub fn new(pty_id: impl Into<String>) -> Self {
        Self {
            pty_id: pty_id.into(),
            session_id: None,
            cwd: None,
            shell_cmd: None,
            args: Vec::new(),
            cols: 80,
            rows: 24,
            env: Vec::new(),
        }
    }
}

struct PtySession {
    info: PtySessionInfo,
    master: Box<dyn MasterPty + Send>,
    input_tx: mpsc::UnboundedSender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Drop でプロセスツリーを確実に終了させる (Windows Job Object)
    _guard: ProcessTreeGuard,
}

struct Inner {
    sessions: Mutex<HashMap<String, PtySession>>,
    events: broadcast::Sender<PtyEvent>,
}

/// ConPTY / Unix PTY のセッション管理マネージャ。
///
/// クローン可能なハンドル (`Arc<Inner>`) で、WebSocket ハンドラや IPC サーバーから
/// 共有して使用する。
#[derive(Clone)]
pub struct PtySessionManager {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for PtySessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtySessionManager")
            .field("sessions", &self.list().len())
            .finish()
    }
}

impl Default for PtySessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PtySessionManager {
    /// 空のマネージャを作成する。
    pub fn new() -> Self {
        let (events, _receiver) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Inner {
                sessions: Mutex::new(HashMap::new()),
                events,
            }),
        }
    }

    /// PTY を起動し、非同期読み書きループを開始する。
    pub fn spawn(&self, request: PtySpawnRequest) -> Result<PtySessionInfo, PtyError> {
        {
            let sessions = self.lock_sessions();
            if sessions.contains_key(&request.pty_id) {
                return Err(PtyError::AlreadyExists(request.pty_id.clone()));
            }
        }

        let size = PtySize {
            rows: request.rows,
            cols: request.cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(size).map_err(PtyError::from_display)?;

        let cwd = request
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let mut command = match &request.shell_cmd {
            Some(shell_cmd) => {
                let resolved = resolve_command(shell_cmd, &cwd)?;
                let mut cmd = CommandBuilder::new(resolved);
                cmd.args(request.args.iter());
                cmd
            }
            None => CommandBuilder::new_default_prog(),
        };
        command.cwd(&cwd);
        for (key, value) in &request.env {
            command.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(PtyError::from_display)?;

        // Windows: Job Object へバインド (デーモン終了時に孫プロセスまで確実に終了)
        let guard = ProcessTreeGuard::new()?;
        guard.attach(child.as_ref())?;

        let pid = child.process_id();
        let killer = child.clone_killer();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(PtyError::from_display)?;
        let writer = pair.master.take_writer().map_err(PtyError::from_display)?;

        // slave 側ハンドルを閉じる: 子プロセス終了時に master 側で EOF を検出できる
        drop(pair.slave);

        let info = PtySessionInfo {
            pty_id: request.pty_id.clone(),
            session_id: request.session_id.clone(),
            pid,
            cols: request.cols,
            rows: request.rows,
        };

        // 入力ライタースレッド
        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut writer = writer;
            while let Some(data) = input_rx.blocking_recv() {
                if writer.write_all(&data).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
            // writer の Drop で EOF が送られる
        });

        // 出力リーダースレッド: master 側を読み続けて Output を配信する。
        //
        // EOF は「子プロセスの終了」ではなく「セッション破棄 (master の Drop)」
        // で発生する点に注意する。Windows の ConPTY は子プロセスが終了しても
        // 出力パイプが閉じず、ConPTY 自体を閉じて初めて EOF になる。
        let pty_id = request.pty_id.clone();
        let inner = Arc::clone(&self.inner);
        let (reader_done_tx, reader_done_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buffer = vec![0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        let _ = inner.events.send(PtyEvent::Output {
                            pty_id: pty_id.clone(),
                            data: buffer[..read].to_vec(),
                        });
                    }
                }
            }
            let _ = reader_done_tx.send(());
        });

        // 終了検知スレッド: 子プロセスの終了を `wait()` で検知し、セッションを
        // 破棄して Exit を配信する。破棄で master が Drop され、ConPTY / PTY が
        // クローズされてリーダーにも EOF が届く (Windows で `Exit` が永遠に
        // 発火しないバグを回避する。CI: windows-latest で検出)。
        // セッション登録後に起動する: 超高速で終了するコマンドでも
        // 「登録前に remove してしまう」競合を避ける。
        // リーダーの EOF を待ってから Exit を送ることで、購読者が最終出力を
        // 取りこぼさないことを保証する。
        self.lock_sessions().insert(
            info.pty_id.clone(),
            PtySession {
                info: info.clone(),
                master: pair.master,
                input_tx,
                killer,
                _guard: guard,
            },
        );

        let exit_pty_id = info.pty_id.clone();
        let exit_inner = Arc::clone(&self.inner);
        std::thread::spawn(move || {
            let mut child = child;
            let exit_code = child
                .wait()
                .ok()
                .map(|status| i32::try_from(status.exit_code()).unwrap_or(i32::MAX));
            // セッションの Drop (master → ClosePseudoConsole) はロック外で行う。
            // ClosePseudoConsole は出力パイプの drain 状況によって待たされる
            // ことがあるため、ロックを保持したまま実行しない。
            let removed = exit_inner
                .sessions
                .lock()
                .expect("pty sessions mutex poisoned")
                .remove(&exit_pty_id);
            drop(removed);
            // リーダーの EOF を待つ。万一 EOF が得られない場合
            // (孫プロセスが slave 側を保持し続ける等) でも Exit を保証するため
            // フェイルセーフのタイムアウトを設ける。
            let _ = reader_done_rx.recv_timeout(Duration::from_secs(5));
            let _ = exit_inner.events.send(PtyEvent::Exit {
                pty_id: exit_pty_id,
                exit_code,
            });
        });

        Ok(info)
    }

    /// 出力イベントを購読する。
    pub fn subscribe(&self, pty_id: &str) -> Result<broadcast::Receiver<PtyEvent>, PtyError> {
        if !self.lock_sessions().contains_key(pty_id) {
            return Err(PtyError::NotFound(pty_id.to_owned()));
        }
        Ok(self.inner.events.subscribe())
    }

    /// キー入力 / コマンド入力を PTY の標準入力へ送る。
    pub fn write(&self, pty_id: &str, data: &[u8]) -> Result<usize, PtyError> {
        let sessions = self.lock_sessions();
        let session = sessions
            .get(pty_id)
            .ok_or_else(|| PtyError::NotFound(pty_id.to_owned()))?;
        session
            .input_tx
            .send(data.to_vec())
            .map_err(|_| PtyError::NotFound(pty_id.to_owned()))?;
        Ok(data.len())
    }

    /// ウィンドウサイズを動的に変更する。
    pub fn resize(&self, pty_id: &str, cols: u16, rows: u16) -> Result<(), PtyError> {
        let mut sessions = self.lock_sessions();
        let session = sessions
            .get_mut(pty_id)
            .ok_or_else(|| PtyError::NotFound(pty_id.to_owned()))?;
        session
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(PtyError::from_display)?;
        session.info.cols = cols;
        session.info.rows = rows;
        Ok(())
    }

    /// PTY の子プロセスを終了する (Windows では Job Object 経由でツリーごと)。
    pub fn kill(&self, pty_id: &str) -> Result<(), PtyError> {
        let mut sessions = self.lock_sessions();
        let session = sessions
            .get_mut(pty_id)
            .ok_or_else(|| PtyError::NotFound(pty_id.to_owned()))?;
        session.killer.kill().map_err(PtyError::from_display)?;
        Ok(())
    }

    /// 稼働中の全 PTY を終了する (緊急キルスイッチ用)。
    ///
    /// 終了要求を送った PTY ID の一覧を返す。実際の削除は各リーダースレッドが
    /// EOF を検出したタイミングで行われる。
    pub fn kill_all(&self) -> Vec<String> {
        let mut sessions = self.lock_sessions();
        let mut killed = Vec::with_capacity(sessions.len());
        for (pty_id, session) in sessions.iter_mut() {
            if session.killer.kill().is_ok() {
                killed.push(pty_id.clone());
            }
        }
        killed
    }

    /// 稼働中 PTY の一覧。
    pub fn list(&self) -> Vec<PtySessionInfo> {
        self.lock_sessions()
            .values()
            .map(|session| session.info.clone())
            .collect()
    }

    /// 指定 PTY が稼働中か。
    pub fn contains(&self, pty_id: &str) -> bool {
        self.lock_sessions().contains_key(pty_id)
    }

    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, PtySession>> {
        self.inner
            .sessions
            .lock()
            .expect("pty sessions mutex poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 出力を Exit まで収集する (タイムアウト付き)。
    ///
    /// Unix PTY はプロセス終了で EOF になるため、リーダー起点の Exit 検知を
    /// 検証する。Windows (ConPTY) は EOF にならないため使わない。
    #[cfg(unix)]
    fn collect_until_exit(
        receiver: broadcast::Receiver<PtyEvent>,
        timeout: Duration,
    ) -> (Vec<u8>, Option<i32>) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut receiver = receiver;
            let mut output = Vec::new();
            let mut exit_code = None;
            loop {
                match receiver.blocking_recv() {
                    Ok(PtyEvent::Output { data, .. }) => output.extend_from_slice(&data),
                    Ok(PtyEvent::Exit {
                        exit_code: code, ..
                    }) => {
                        exit_code = code;
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            let _ = tx.send((output, exit_code));
        });
        rx.recv_timeout(timeout)
            .expect("pty did not exit before the timeout")
    }

    #[cfg(unix)]
    #[test]
    fn spawns_command_and_streams_output() {
        let manager = PtySessionManager::new();
        let mut request = PtySpawnRequest::new("pty-1");
        request.shell_cmd = Some("sh".to_owned());
        request.args = vec!["-c".to_owned(), "printf hello-pty".to_owned()];
        let info = manager.spawn(request).expect("spawn");
        assert_eq!(info.pty_id, "pty-1");
        assert!(info.pid.is_some());

        let receiver = manager.subscribe("pty-1").expect("subscribe");
        let (output, exit_code) = collect_until_exit(receiver, Duration::from_secs(10));
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("hello-pty"), "unexpected output: {text:?}");
        assert_eq!(exit_code, Some(0));

        // 終了後はセッションマップから削除される
        for _ in 0..50 {
            if !manager.contains("pty-1") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !manager.contains("pty-1"),
            "session must be removed on exit"
        );
        assert!(manager.list().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn interactive_shell_accepts_input_and_resize() {
        let manager = PtySessionManager::new();
        let mut request = PtySpawnRequest::new("pty-2");
        request.shell_cmd = Some("sh".to_owned());
        let info = manager.spawn(request).expect("spawn");
        assert_eq!((info.cols, info.rows), (80, 24));

        manager.resize("pty-2", 100, 40).expect("resize");
        assert_eq!(manager.list()[0].cols, 100);

        manager
            .write("pty-2", b"echo interactive-ok\n")
            .expect("write");
        manager.write("pty-2", b"exit\n").expect("write exit");

        let receiver = manager.subscribe("pty-2").expect("subscribe");
        let (output, _) = collect_until_exit(receiver, Duration::from_secs(10));
        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("interactive-ok"),
            "unexpected output: {text:?}"
        );
    }

    #[test]
    fn unknown_pty_ids_are_rejected() {
        let manager = PtySessionManager::new();
        assert!(matches!(
            manager.write("missing", b"x").expect_err("write"),
            PtyError::NotFound(_)
        ));
        assert!(matches!(
            manager.resize("missing", 80, 24).expect_err("resize"),
            PtyError::NotFound(_)
        ));
        assert!(matches!(
            manager.kill("missing").expect_err("kill"),
            PtyError::NotFound(_)
        ));
        assert!(manager.subscribe("missing").is_err());
    }

    /// Windows の ConPTY で cmd を実行し、出力・Exit 配信・セッション破棄を検証する。
    ///
    /// ConPTY は EOF セマンティクスが Unix PTY と異なるため、待機は
    /// [`PtyEvent`] のポーリングで行い、失敗時は診断情報を出力する。
    #[cfg(windows)]
    #[test]
    fn spawns_cmd_on_windows() {
        let manager = PtySessionManager::new();
        let mut request = PtySpawnRequest::new("pty-win");
        request.shell_cmd = Some("cmd".to_owned());
        request.args = vec!["/C".to_owned(), "echo hello-pty".to_owned()];
        let info = manager.spawn(request).expect("spawn");

        let mut receiver = manager.subscribe("pty-win").expect("subscribe");
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut output = Vec::new();
        let mut exit_code = None;
        while exit_code.is_none() {
            match receiver.try_recv() {
                Ok(PtyEvent::Output { data, .. }) => output.extend_from_slice(&data),
                Ok(PtyEvent::Exit {
                    exit_code: code, ..
                }) => exit_code = Some(code),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Empty) => {
                    if std::time::Instant::now() > deadline {
                        eprintln!(
                            "[diag] timeout: pid={:?} output={:?} session_present={}",
                            info.pid,
                            String::from_utf8_lossy(&output),
                            manager.contains("pty-win"),
                        );
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }

        let text = String::from_utf8_lossy(&output).to_lowercase();
        assert!(
            text.contains("hello-pty"),
            "unexpected output: {text:?} (exit={exit_code:?}, session_present={})",
            manager.contains("pty-win")
        );
        assert_eq!(exit_code, Some(Some(0)));

        // 終了後はセッションマップから削除される
        for _ in 0..50 {
            if !manager.contains("pty-win") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !manager.contains("pty-win"),
            "session must be removed on exit"
        );
    }
}

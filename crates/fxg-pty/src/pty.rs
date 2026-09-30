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
//! - **ConPTY 起動ハンドシェイク** (Windows): ConPTY は起動時にカーソル位置照会
//!   (`ESC[6n`) を送り、応答が届くまで子プロセスのコンソール操作をブロックする。
//!   マネージャが代わりに応答し、照会は出力から除去する
//!   ([`ConPtyStartupHandshake`])。これによりヘッドレス実行でも子プロセスが
//!   停止しない。
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

/// ConPTY が起動時に送るカーソル位置照会 (Device Status Report)。
#[cfg(any(windows, test))]
const CPR_QUERY: &[u8] = b"\x1b[6n";

/// 照会への応答 (カーソル位置 = 行1・列1)。
#[cfg(windows)]
const CPR_RESPONSE: &[u8] = b"\x1b[1;1R";

/// Windows ConPTY の起動ハンドシェイク (カーソル位置照会) を処理するフィルタ。
///
/// ConPTY は `PSEUDOCONSOLE_INHERIT_CURSOR` により起動直後に
/// カーソル位置照会 (`ESC[6n`) を送り、応答 (`ESC[1;1R`) が届くまで
/// **子プロセスのコンソール操作をブロックする**
/// (CI: windows-latest で検出。応答が無いとヘッドレス実行で子プロセスが
/// 永久に停止する)。
///
/// マネージャ (ターミナル側 IO 層) が起動照会へ応答し、照会シーケンスは
/// 出力から除去する:
/// - ヘッドレス (購読者なし) でも子プロセスが正常に起動する
/// - フロントエンド (xterm.js 等) の自動応答と二重応答にならない
///
/// 起動照会はプロセス起動前に送られるため、**最初に見つかった照会のみ**を
/// 対象とする (アプリ自身が発行する照会はフロントエンドが応答する)。
#[cfg(any(windows, test))]
#[derive(Debug, Default)]
struct ConPtyStartupHandshake {
    /// 照会を処理済みか (以降はすべて透過)
    done: bool,
    /// チャンク境界で分断された照会の持ち越し (最大 `CPR_QUERY.len() - 1` バイト)
    carry: Vec<u8>,
}

#[cfg(any(windows, test))]
impl ConPtyStartupHandshake {
    /// 出力チャンクを処理し、転送すべきデータと照会への応答要否を返す。
    fn process(&mut self, chunk: &[u8]) -> (Vec<u8>, bool) {
        if self.done {
            return (chunk.to_vec(), false);
        }
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(chunk);

        if let Some(pos) = find_subslice(&data, CPR_QUERY) {
            self.done = true;
            let mut out = data[..pos].to_vec();
            out.extend_from_slice(&data[pos + CPR_QUERY.len()..]);
            return (out, true);
        }

        // 照会がチャンク境界で分断され得るため、pattern の prefix になり得る
        // 末尾のみを持ち越す (通常は 0〜3 バイト)
        let keep = pattern_prefix_suffix_len(&data, CPR_QUERY);
        let split = data.len() - keep;
        let out = data[..split].to_vec();
        self.carry = data[split..].to_vec();
        (out, false)
    }

    /// 未処理の持ち越し (部分シーケンス) を取り出す (セッション終了時のフラッシュ用)。
    #[cfg(windows)]
    fn take_carry(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.carry)
    }
}

/// スライス内で `needle` を検索する (最初の出現位置)。
#[cfg(any(windows, test))]
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// `data` の末尾が `pattern` の prefix になっている最長長 (≤ `pattern.len() - 1`)。
#[cfg(any(windows, test))]
fn pattern_prefix_suffix_len(data: &[u8], pattern: &[u8]) -> usize {
    let max = (pattern.len() - 1).min(data.len());
    (1..=max)
        .rev()
        .find(|&len| data[data.len() - len..] == pattern[..len])
        .unwrap_or(0)
}

/// 出力チャンクを購読者へ配信するフォワーダ。
struct OutputForwarder {
    inner: Arc<Inner>,
    pty_id: String,
    /// Windows ConPTY の起動ハンドシェイク処理 (照会への応答送信を含む)
    #[cfg(windows)]
    handshake: ConPtyStartupHandshake,
    /// 照会への応答を PTY 入力へ送るための送信チャネル
    #[cfg(windows)]
    input_tx: mpsc::UnboundedSender<Vec<u8>>,
}

impl OutputForwarder {
    /// チャンクを配信する (Windows: ConPTY 起動照会の検出・除去・応答)。
    fn forward(&mut self, chunk: &[u8]) {
        #[cfg(windows)]
        let (data, respond) = self.handshake.process(chunk);
        #[cfg(windows)]
        if respond {
            // ConPTY の起動照会へ応答する (ヘッドレス実行でのブロック回避)
            let _ = self.input_tx.send(CPR_RESPONSE.to_vec());
        }
        #[cfg(not(windows))]
        let data = chunk.to_vec();

        if !data.is_empty() {
            let _ = self.inner.events.send(PtyEvent::Output {
                pty_id: self.pty_id.clone(),
                data,
            });
        }
    }

    /// セッション終了時に持ち越しをフラッシュする。
    fn finish(&mut self) {
        #[cfg(windows)]
        let tail = self.handshake.take_carry();
        #[cfg(not(windows))]
        let tail: Vec<u8> = Vec::new();
        if !tail.is_empty() {
            let _ = self.inner.events.send(PtyEvent::Output {
                pty_id: self.pty_id.clone(),
                data: tail,
            });
        }
    }
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
        let mut forwarder = OutputForwarder {
            inner: Arc::clone(&self.inner),
            pty_id: request.pty_id.clone(),
            #[cfg(windows)]
            handshake: ConPtyStartupHandshake::default(),
            #[cfg(windows)]
            input_tx: input_tx.clone(),
        };
        let (reader_done_tx, reader_done_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buffer = vec![0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => forwarder.forward(&buffer[..read]),
                }
            }
            forwarder.finish();
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

    /// ConPTY 起動ハンドシェイク: 起動照会 (`ESC[6n`) を除去して応答を要求し、
    /// 2 回目以降の照会 (アプリ由来) はそのまま通す。
    #[test]
    fn conpty_handshake_strips_query_and_requests_response() {
        let mut handshake = ConPtyStartupHandshake::default();
        let (data, respond) = handshake.process(b"\x1b[6nhello");
        assert!(respond);
        assert_eq!(data, b"hello");

        let (data, respond) = handshake.process(b"world\x1b[6n");
        assert!(!respond);
        assert_eq!(data, b"world\x1b[6n");
    }

    /// 起動照会がチャンク境界で分断されても検出できること。
    #[test]
    fn conpty_handshake_handles_split_query_across_chunks() {
        let mut handshake = ConPtyStartupHandshake::default();

        // pattern の prefix でないデータはそのまま配信される
        let (data, respond) = handshake.process(b"hel");
        assert!(!respond);
        assert_eq!(data, b"hel");

        // "\x1b[" は照会の prefix のため持ち越される
        let (data, respond) = handshake.process(b"\x1b[");
        assert!(!respond);
        assert!(data.is_empty());

        // "6n" で完成 → 照会は除去され応答要求が出る
        let (data, respond) = handshake.process(b"6nX");
        assert!(respond);
        assert_eq!(data, b"X");
    }

    /// 照会が来ない場合も (最大3バイトの持ち越し以外は) すべて透過すること。
    #[test]
    fn conpty_handshake_passes_data_when_no_query_arrives() {
        let mut handshake = ConPtyStartupHandshake::default();
        let (data, respond) = handshake.process(b"plain output\n");
        assert!(!respond);
        assert_eq!(data, b"plain output\n");

        // 末尾の部分 prefix は次チャンクへ持ち越される
        let (data, respond) = handshake.process(b"tail\x1b");
        assert!(!respond);
        assert_eq!(data, b"tail");
        let (data, respond) = handshake.process(b"X");
        assert!(!respond);
        assert_eq!(data, b"\x1bX");
    }

    /// Windows の ConPTY で cmd を実行し、出力・Exit 配信・セッション破棄を検証する。
    ///
    /// ConPTY は EOF セマンティクスが Unix PTY と異なるため、待機は
    /// [`PtyEvent`] のポーリングで行う。起動時のカーソル位置照会 (`ESC[6n`) は
    /// [`ConPtyStartupHandshake`] がマネージャ側で応答するため、
    /// ヘッドレス (購読者なし) でも子プロセスが起動・終了する。
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
        while exit_code.is_none() && std::time::Instant::now() < deadline {
            match receiver.try_recv() {
                Ok(PtyEvent::Output { data, .. }) => output.extend_from_slice(&data),
                Ok(PtyEvent::Exit {
                    exit_code: code, ..
                }) => exit_code = Some(code),
                Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        if exit_code.is_none() {
            let tasklist = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {}", info.pid.unwrap_or(0)), "/NH"])
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned());
            eprintln!(
                "[diag] timeout: output={:?} tasklist={tasklist:?}",
                String::from_utf8_lossy(&output),
            );
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

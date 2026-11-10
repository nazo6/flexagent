//! Bidirectional ConPTY (Windows) / Unix PTY stream management via
//! `portable-pty`.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §5.3.
//!
//! - **Cross-platform PTY**: `portable-pty` selects ConPTY on Windows and Unix
//!   PTY on macOS/Linux automatically.
//! - **Asynchronous streaming**: a dedicated thread reads the Master side's
//!   `Read` and publishes [`PtyEvent`]s to subscribers (WebSocket) via
//!   `tokio::sync::broadcast`. Input is flushed immediately to the Master's
//!   `Write` by a dedicated writer thread. Exit detection is handled by a
//!   dedicated `wait()` thread (Windows ConPTY does not reach EOF just because
//!   the child process exits, so the reader's EOF cannot be used as the exit
//!   trigger).
//! - **Dynamic resize**: [`PtySessionManager::resize`] changes the ConPTY / Unix
//!   PTY window size immediately.
//! - **ConPTY startup handshake** (Windows): on startup ConPTY sends a cursor
//!   position query (`ESC[6n`) and blocks the child process's console operations
//!   until a response arrives. The manager responds on its behalf and strips the
//!   query from the output ([`ConPtyStartupHandshake`]). This keeps child
//!   processes from stalling in headless runs.
//! - **Guaranteed process tree termination**: child processes are bound to a
//!   [`ProcessTreeGuard`] (Windows: Job Object).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{broadcast, mpsc};

use crate::error::PtyError;
use crate::proc::{ProcessTreeGuard, resolve_command};

/// PTY events delivered to subscribers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    /// Output chunk (raw bytes, including ANSI escapes)
    Output {
        /// Target PTY ID
        pty_id: String,
        /// Output data
        data: Vec<u8>,
    },
    /// Process exit (the PTY is removed from the session map afterwards)
    Exit {
        /// Target PTY ID
        pty_id: String,
        /// Exit code (`None` if unavailable)
        exit_code: Option<i32>,
    },
}

/// Information about a running PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtySessionInfo {
    /// PTY ID
    pub pty_id: String,
    /// Agent session ID this PTY belongs to (optional)
    pub session_id: Option<String>,
    /// OS process ID of the child process
    pub pid: Option<u32>,
    /// Current number of columns
    pub cols: u16,
    /// Current number of rows
    pub rows: u16,
}

/// Request to spawn a PTY.
#[derive(Debug, Clone)]
pub struct PtySpawnRequest {
    /// PTY ID (assigned by the caller)
    pub pty_id: String,
    /// Agent session ID this PTY belongs to (optional)
    pub session_id: Option<String>,
    /// Working directory (defaults to the current directory)
    pub cwd: Option<PathBuf>,
    /// Shell / command to launch (defaults to the OS default shell).
    /// When specified, it is resolved from `PATH` (`PATHEXT`).
    pub shell_cmd: Option<String>,
    /// Arguments passed to `shell_cmd`
    pub args: Vec<String>,
    /// Number of columns
    pub cols: u16,
    /// Number of rows
    pub rows: u16,
    /// Extra environment variables to set (the parent process environment is
    /// inherited)
    pub env: Vec<(String, String)>,
}

impl PtySpawnRequest {
    /// Creates a spawn request with the default size (80x24).
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
    /// Drop reliably terminates the process tree (Windows Job Object)
    _guard: ProcessTreeGuard,
}

struct Inner {
    sessions: Mutex<HashMap<String, PtySession>>,
    events: broadcast::Sender<PtyEvent>,
}

/// Cursor position query (Device Status Report) that ConPTY sends at startup.
#[cfg(any(windows, test))]
const CPR_QUERY: &[u8] = b"\x1b[6n";

/// Response to the query (cursor position = row 1, column 1).
#[cfg(windows)]
const CPR_RESPONSE: &[u8] = b"\x1b[1;1R";

/// Filter that handles the Windows ConPTY startup handshake (cursor position
/// query).
///
/// With `PSEUDOCONSOLE_INHERIT_CURSOR`, ConPTY sends a cursor position query
/// (`ESC[6n`) right after startup and **blocks the child process's console
/// operations** until a response (`ESC[1;1R`) arrives
/// (CI: detected on windows-latest. Without a response, child processes hang
/// forever in headless runs).
///
/// The manager (the terminal-side IO layer) answers the startup query and strips
/// the query sequence from the output:
/// - child processes start normally even when headless (no subscribers)
/// - no double response with the frontend's own automatic response (xterm.js etc.)
///
/// The startup query is sent before the process starts, so **only the first
/// query found** is handled (queries issued by the application itself are
/// answered by the frontend).
#[cfg(any(windows, test))]
#[derive(Debug, Default)]
struct ConPtyStartupHandshake {
    /// Whether the query has been handled (everything is passed through
    /// afterwards)
    done: bool,
    /// Query bytes carried over when split across chunk boundaries (at most
    /// `CPR_QUERY.len() - 1` bytes)
    carry: Vec<u8>,
}

#[cfg(any(windows, test))]
impl ConPtyStartupHandshake {
    /// Processes an output chunk and returns the data to forward plus whether a
    /// response to the query is required.
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

        // Because the query can be split across chunk boundaries, carry over
        // only the trailing bytes that could be a prefix of the pattern (usually
        // 0-3 bytes)
        let keep = pattern_prefix_suffix_len(&data, CPR_QUERY);
        let split = data.len() - keep;
        let out = data[..split].to_vec();
        self.carry = data[split..].to_vec();
        (out, false)
    }

    /// Extracts the unprocessed carry-over (partial sequence) for flushing at
    /// session exit.
    #[cfg(windows)]
    fn take_carry(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.carry)
    }
}

/// Searches a slice for `needle` and returns the first occurrence.
#[cfg(any(windows, test))]
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Longest length (≤ `pattern.len() - 1`) such that the tail of `data` is a
/// prefix of `pattern`.
#[cfg(any(windows, test))]
fn pattern_prefix_suffix_len(data: &[u8], pattern: &[u8]) -> usize {
    let max = (pattern.len() - 1).min(data.len());
    (1..=max)
        .rev()
        .find(|&len| data[data.len() - len..] == pattern[..len])
        .unwrap_or(0)
}

/// Forwarder that publishes output chunks to subscribers.
struct OutputForwarder {
    inner: Arc<Inner>,
    pty_id: String,
    /// Windows ConPTY startup handshake handling (including sending the query
    /// response)
    #[cfg(windows)]
    handshake: ConPtyStartupHandshake,
    /// Channel for sending the query response to the PTY input
    #[cfg(windows)]
    input_tx: mpsc::UnboundedSender<Vec<u8>>,
}

impl OutputForwarder {
    /// Delivers a chunk (Windows: detect, strip, and respond to the ConPTY
    /// startup query).
    fn forward(&mut self, chunk: &[u8]) {
        #[cfg(windows)]
        let (data, respond) = self.handshake.process(chunk);
        #[cfg(windows)]
        if respond {
            // Respond to ConPTY's startup query (avoids blocking in headless runs)
            fxg_protocol::discard_with!(
                self.input_tx.send(CPR_RESPONSE.to_vec()),
                "respond to the ConPTY startup query"
            );
        }
        #[cfg(not(windows))]
        let data = chunk.to_vec();

        if !data.is_empty() {
            fxg_protocol::discard_with!(
                self.inner.events.send(PtyEvent::Output {
                    pty_id: self.pty_id.clone(),
                    data,
                }),
                "broadcast pty output"
            );
        }
    }

    /// Flushes the carry-over when the session ends.
    fn finish(&mut self) {
        #[cfg(windows)]
        let tail = self.handshake.take_carry();
        #[cfg(not(windows))]
        let tail: Vec<u8> = Vec::new();
        if !tail.is_empty() {
            fxg_protocol::discard_with!(
                self.inner.events.send(PtyEvent::Output {
                    pty_id: self.pty_id.clone(),
                    data: tail,
                }),
                "broadcast trailing pty output"
            );
        }
    }
}

/// Session manager for ConPTY / Unix PTY.
///
/// A cloneable handle (`Arc<Inner>`) shared by WebSocket handlers.
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
    /// Creates an empty manager.
    pub fn new() -> Self {
        let (events, _receiver) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Inner {
                sessions: Mutex::new(HashMap::new()),
                events,
            }),
        }
    }

    /// Spawns a PTY and starts the asynchronous read/write loops.
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

        // Windows: bind to a Job Object (ensure even grandchildren are terminated
        // when the daemon exits)
        let guard = ProcessTreeGuard::new()?;
        guard.attach(child.as_ref())?;

        let pid = child.process_id();
        let killer = child.clone_killer();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(PtyError::from_display)?;
        let writer = pair.master.take_writer().map_err(PtyError::from_display)?;

        // Close the slave-side handle: lets the master side detect EOF when the
        // child process exits
        drop(pair.slave);

        let info = PtySessionInfo {
            pty_id: request.pty_id.clone(),
            session_id: request.session_id.clone(),
            pid,
            cols: request.cols,
            rows: request.rows,
        };

        // Input writer thread
        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut writer = writer;
            while let Some(data) = input_rx.blocking_recv() {
                if writer.write_all(&data).is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }
            }
            // Dropping the writer sends EOF
        });

        // Output reader thread: keeps reading the master side and publishes
        // Output events.
        //
        // Note that EOF happens on "session teardown (master Drop)", not on
        // "child process exit". Windows ConPTY does not close the output pipe
        // when the child exits; EOF only occurs once ConPTY itself is closed.
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
            fxg_protocol::discard_with!(
                reader_done_tx.send(()),
                "signal that the pty output reader finished"
            );
        });

        // Exit detection thread: detects child exit via `wait()`, tears down the
        // session, and publishes Exit. Teardown drops the master, closes
        // ConPTY / PTY, and delivers EOF to the reader as well (this works around
        // a bug where `Exit` never fires on Windows. CI: detected on
        // windows-latest). Started after the session is registered, so that even
        // a command that exits extremely quickly cannot race with "remove before
        // register". Waiting for the reader's EOF before sending Exit guarantees
        // that subscribers do not miss the final output.
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
            let exit_code = match child.wait() {
                Ok(status) => Some(i32::try_from(status.exit_code()).unwrap_or(i32::MAX)),
                Err(err) => {
                    tracing::warn!(
                        pty_id = %exit_pty_id,
                        "failed to wait for the pty child process (exit code unknown): {err}"
                    );
                    None
                }
            };
            // The session Drop (master → ClosePseudoConsole) is done outside the
            // lock. ClosePseudoConsole can block depending on the drain state of
            // the output pipe, so it must not run while holding the lock.
            let removed = exit_inner
                .sessions
                .lock()
                .expect("pty sessions mutex poisoned")
                .remove(&exit_pty_id);
            drop(removed);
            // Wait for the reader's EOF. A failsafe timeout is used so that Exit
            // is still guaranteed if EOF never arrives (e.g. a grandchild keeps
            // the slave side open).
            fxg_protocol::discard_with!(
                reader_done_rx.recv_timeout(Duration::from_secs(5)),
                "wait for the pty output reader before publishing exit"
            );
            fxg_protocol::discard_with!(
                exit_inner.events.send(PtyEvent::Exit {
                    pty_id: exit_pty_id,
                    exit_code,
                }),
                "broadcast pty exit"
            );
        });

        Ok(info)
    }

    /// Subscribes to output events.
    pub fn subscribe(&self, pty_id: &str) -> Result<broadcast::Receiver<PtyEvent>, PtyError> {
        if !self.lock_sessions().contains_key(pty_id) {
            return Err(PtyError::NotFound(pty_id.to_owned()));
        }
        Ok(self.inner.events.subscribe())
    }

    /// Sends keyboard / command input to the PTY's standard input.
    pub fn write(&self, pty_id: &str, data: &[u8]) -> Result<usize, PtyError> {
        let sessions = self.lock_sessions();
        let session = sessions
            .get(pty_id)
            .ok_or_else(|| PtyError::NotFound(pty_id.to_owned()))?;
        if session.input_tx.send(data.to_vec()).is_err() {
            return Err(PtyError::InputClosed(pty_id.to_owned()));
        }
        Ok(data.len())
    }

    /// Changes the window size dynamically.
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

    /// Terminates the PTY's child process (on Windows, the whole tree via the
    /// Job Object).
    pub fn kill(&self, pty_id: &str) -> Result<(), PtyError> {
        let mut sessions = self.lock_sessions();
        let session = sessions
            .get_mut(pty_id)
            .ok_or_else(|| PtyError::NotFound(pty_id.to_owned()))?;
        session.killer.kill().map_err(PtyError::from_display)?;
        Ok(())
    }

    /// Terminates all running PTYs (emergency kill switch).
    ///
    /// Returns the list of PTY IDs to which a termination request was sent.
    /// Actual removal happens when each reader thread detects EOF.
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

    /// List of running PTYs.
    pub fn list(&self) -> Vec<PtySessionInfo> {
        self.lock_sessions()
            .values()
            .map(|session| session.info.clone())
            .collect()
    }

    /// Whether the given PTY is running.
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

    /// Collects output until Exit (with a timeout).
    ///
    /// Unix PTY reaches EOF when the process exits, so this verifies
    /// reader-driven Exit detection. Not used on Windows (ConPTY), which does not
    /// reach EOF.
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
            fxg_protocol::discard_with!(
                tx.send((output, exit_code)),
                "report the pty exit to the test waiter"
            );
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

        // The session is removed from the session map after exit
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

    /// ConPTY startup handshake: strips the startup query (`ESC[6n`) and requests
    /// a response, while passing through later queries (issued by the
    /// application).
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

    /// The startup query must be detected even when split across chunk
    /// boundaries.
    #[test]
    fn conpty_handshake_handles_split_query_across_chunks() {
        let mut handshake = ConPtyStartupHandshake::default();

        // Data that is not a prefix of the pattern is delivered as is
        let (data, respond) = handshake.process(b"hel");
        assert!(!respond);
        assert_eq!(data, b"hel");

        // "\x1b[" is a prefix of the query, so it is carried over
        let (data, respond) = handshake.process(b"\x1b[");
        assert!(!respond);
        assert!(data.is_empty());

        // Completed with "6n" → the query is stripped and a response is requested
        let (data, respond) = handshake.process(b"6nX");
        assert!(respond);
        assert_eq!(data, b"X");
    }

    /// All data must pass through even when no query arrives (except for the
    /// carry-over of at most 3 bytes).
    #[test]
    fn conpty_handshake_passes_data_when_no_query_arrives() {
        let mut handshake = ConPtyStartupHandshake::default();
        let (data, respond) = handshake.process(b"plain output\n");
        assert!(!respond);
        assert_eq!(data, b"plain output\n");

        // The trailing partial prefix is carried over to the next chunk
        let (data, respond) = handshake.process(b"tail\x1b");
        assert!(!respond);
        assert_eq!(data, b"tail");
        let (data, respond) = handshake.process(b"X");
        assert!(!respond);
        assert_eq!(data, b"\x1bX");
    }

    /// Runs cmd in Windows ConPTY and verifies output, Exit delivery, and session
    /// teardown.
    ///
    /// ConPTY has different EOF semantics from Unix PTY, so waiting is done by
    /// polling [`PtyEvent`]. The startup cursor position query (`ESC[6n`) is
    /// answered by [`ConPtyStartupHandshake`] on the manager side, so the child
    /// process starts and exits even when headless (no subscribers).
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

        // The session is removed from the session map after exit
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

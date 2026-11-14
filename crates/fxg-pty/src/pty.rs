//! Bidirectional ConPTY (Windows) / Unix PTY stream management via
//! `portable-pty`.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §5.3 and
//! `docs/proposals/2026-11-14-terminal-workspace-scope.md`.
//!
//! - **Cross-platform PTY**: `portable-pty` selects ConPTY on Windows and Unix
//!   PTY on macOS/Linux automatically.
//! - **Workspace terminals**: a PTY opened with a `workspace` key is unique per
//!   workspace path. Opening it again reuses the running shell instead of
//!   starting another one.
//! - **Detached lifetime**: closing a client socket only detaches. A terminal
//!   with no clients is killed after [`DEFAULT_IDLE_TIMEOUT`] (or the timeout
//!   given to [`PtySessionManager::with_idle_timeout`]).
//! - **Output log**: each PTY keeps a bounded [`PtyOutputLog`] with sequence
//!   numbers so an attaching client receives the retained output first.
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
//!   query from the output ([`ConPtyStartupHandshake`]).
//! - **Guaranteed process tree termination**: child processes are bound to a
//!   [`ProcessTreeGuard`] (Windows: Job Object).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use fxg_protocol::util::{PtyOutputLog, now_ms};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{broadcast, mpsc};

use crate::error::PtyError;
use crate::proc::{ProcessTreeGuard, resolve_command};

/// Idle timeout applied when a terminal has no attached clients.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Capacity of the broadcast channel that carries PTY events.
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// PTY events delivered to subscribers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    /// Output chunk (raw bytes, including ANSI escapes)
    Output {
        /// Target PTY ID
        pty_id: String,
        /// Monotonic chunk sequence number (0-based per PTY)
        seq: u64,
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
    /// Agent session ID this PTY belongs to (origin, optional)
    pub session_id: Option<String>,
    /// Working directory (canonical workspace path for web terminals)
    pub path: PathBuf,
    /// OS process ID of the child process
    pub pid: Option<u32>,
    /// Current number of columns
    pub cols: u16,
    /// Current number of rows
    pub rows: u16,
    /// Number of attached clients
    pub attached: u32,
    /// Creation time (Unix epoch ms)
    pub created_at: i64,
}

/// Request to spawn a PTY.
#[derive(Debug, Clone)]
pub struct PtySpawnRequest {
    /// PTY ID (assigned by the caller)
    pub pty_id: String,
    /// Agent session ID this PTY belongs to (origin, optional)
    pub session_id: Option<String>,
    /// Workspace key that makes the terminal unique (web terminals only).
    ///
    /// Agent-owned terminals (ACP `terminal/*`) leave this `None`, so they never
    /// share a workspace with the user's web terminal.
    pub workspace: Option<PathBuf>,
    /// Working directory (defaults to the workspace or the current directory)
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
            workspace: None,
            cwd: None,
            shell_cmd: None,
            args: Vec::new(),
            cols: 80,
            rows: 24,
            env: Vec::new(),
        }
    }
}

/// Result of opening a workspace terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyOpenOutcome {
    /// Information about the running (new or reused) terminal
    pub info: PtySessionInfo,
    /// Whether an existing workspace terminal was reused
    pub attached: bool,
}

/// Retained output delivered before the live stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PtyReplay {
    /// Retained bytes (may start mid-escape sequence when the log wrapped)
    pub data: Vec<u8>,
    /// Sequence number of the next live chunk (drop `seq < next_seq`)
    pub next_seq: u64,
}

struct PtySession {
    info: PtySessionInfo,
    master: Box<dyn MasterPty + Send>,
    input_tx: mpsc::UnboundedSender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Drop reliably terminates the process tree (Windows Job Object)
    _guard: ProcessTreeGuard,
    /// Retained output for attach-time replay
    log: Arc<Mutex<PtyOutputLog>>,
    /// When the last client detached (base of the idle timeout)
    detached_at: Option<Instant>,
}

#[derive(Default)]
struct State {
    sessions: HashMap<String, PtySession>,
    /// Workspace path -> PTY ID (web terminals only)
    workspaces: HashMap<PathBuf, String>,
}

struct Inner {
    state: Mutex<State>,
    events: broadcast::Sender<PtyEvent>,
    idle_timeout: Duration,
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

/// Forwarder that appends output to the log and publishes it to subscribers.
struct OutputForwarder {
    inner: Arc<Inner>,
    pty_id: String,
    log: Arc<Mutex<PtyOutputLog>>,
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
            self.publish(data);
        }
    }

    /// Flushes the carry-over when the session ends.
    fn finish(&mut self) {
        #[cfg(windows)]
        let tail = self.handshake.take_carry();
        #[cfg(not(windows))]
        let tail: Vec<u8> = Vec::new();
        if !tail.is_empty() {
            self.publish(tail);
        }
    }

    fn publish(&self, data: Vec<u8>) {
        let seq = self
            .log
            .lock()
            .expect("pty output log poisoned")
            .push(&data);
        fxg_protocol::discard_with!(
            self.inner.events.send(PtyEvent::Output {
                pty_id: self.pty_id.clone(),
                seq,
                data,
            }),
            "broadcast pty output"
        );
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
    /// Creates an empty manager with [`DEFAULT_IDLE_TIMEOUT`].
    pub fn new() -> Self {
        Self::with_idle_timeout(DEFAULT_IDLE_TIMEOUT)
    }

    /// Creates an empty manager with an explicit detached-terminal timeout.
    ///
    /// The timeout also bounds the reaper interval (at most 30 seconds).
    pub fn with_idle_timeout(idle_timeout: Duration) -> Self {
        let (events, _receiver) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let manager = Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                events,
                idle_timeout,
            }),
        };
        manager.start_reaper();
        manager
    }

    /// Reaps terminals whose last client detached longer than the idle timeout
    /// ago. The thread holds a weak reference and exits with the manager.
    fn start_reaper(&self) {
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        let tick =
            (self.inner.idle_timeout / 4).clamp(Duration::from_millis(50), Duration::from_secs(30));
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(tick);
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let expired: Vec<String> = {
                    let state = inner.state.lock().expect("pty sessions mutex poisoned");
                    state
                        .sessions
                        .iter()
                        .filter(|(_, session)| session.info.attached == 0)
                        .filter(|(_, session)| {
                            session
                                .detached_at
                                .is_some_and(|at| at.elapsed() >= inner.idle_timeout)
                        })
                        .map(|(pty_id, _)| pty_id.clone())
                        .collect()
                };
                for pty_id in expired {
                    let mut state = inner.state.lock().expect("pty sessions mutex poisoned");
                    let Some(session) = state.sessions.get_mut(&pty_id) else {
                        continue;
                    };
                    // A client may have attached between the scan and this point.
                    if session.info.attached > 0 {
                        continue;
                    }
                    tracing::info!(pty_id = %pty_id, "killing idle workspace terminal");
                    if let Err(err) = session.killer.kill() {
                        tracing::warn!(pty_id = %pty_id, "failed to kill idle terminal: {err}");
                    }
                }
            }
        });
    }

    /// Spawns a PTY unconditionally (agent-owned terminals).
    ///
    /// Web terminals must use [`open`](Self::open) so the workspace stays unique.
    pub fn spawn(&self, request: PtySpawnRequest) -> Result<PtySessionInfo, PtyError> {
        let mut state = self.lock_state();
        self.spawn_new_locked(&mut state, request, 0)
    }

    /// Opens the workspace terminal, reusing the running shell when it exists.
    ///
    /// The caller is counted as an attached client. When `new` is true, a
    /// running terminal of the same workspace is killed and replaced.
    pub fn open(&self, request: PtySpawnRequest, new: bool) -> Result<PtyOpenOutcome, PtyError> {
        let Some(workspace) = request.workspace.clone() else {
            let info = self.spawn(request)?;
            return Ok(PtyOpenOutcome {
                info,
                attached: false,
            });
        };

        let mut state = self.lock_state();
        if let Some(existing_id) = state.workspaces.get(&workspace).cloned() {
            if let Some(session) = state.sessions.get_mut(&existing_id) {
                if !new {
                    session.info.attached = session.info.attached.saturating_add(1);
                    session.detached_at = None;
                    return Ok(PtyOpenOutcome {
                        info: session.info.clone(),
                        attached: true,
                    });
                }
                // Replace the shell: the workspace mapping is released now,
                // the old session is removed by its exit thread.
                if let Err(err) = session.killer.kill() {
                    tracing::warn!(pty_id = %existing_id, "failed to replace terminal: {err}");
                }
            }
            state.workspaces.remove(&workspace);
        }

        let info = self.spawn_new_locked(&mut state, request, 1)?;
        Ok(PtyOpenOutcome {
            info,
            attached: false,
        })
    }

    /// Marks one client as attached to the PTY.
    pub fn attach(&self, pty_id: &str) -> Result<PtySessionInfo, PtyError> {
        let mut state = self.lock_state();
        let session = state
            .sessions
            .get_mut(pty_id)
            .ok_or_else(|| PtyError::NotFound(pty_id.to_owned()))?;
        session.info.attached = session.info.attached.saturating_add(1);
        session.detached_at = None;
        Ok(session.info.clone())
    }

    /// Marks one client as detached. The terminal stays alive until the idle
    /// timeout expires.
    pub fn detach(&self, pty_id: &str) {
        let mut state = self.lock_state();
        let Some(session) = state.sessions.get_mut(pty_id) else {
            return;
        };
        session.info.attached = session.info.attached.saturating_sub(1);
        if session.info.attached == 0 {
            session.detached_at = Some(Instant::now());
        }
    }

    /// Subscribes to output events (no replay).
    pub fn subscribe(&self, pty_id: &str) -> Result<broadcast::Receiver<PtyEvent>, PtyError> {
        if !self.contains(pty_id) {
            return Err(PtyError::NotFound(pty_id.to_owned()));
        }
        Ok(self.inner.events.subscribe())
    }

    /// Subscribes to output events and returns the retained output to send first.
    ///
    /// The receiver is created before the snapshot is read, so no chunk between
    /// the snapshot and the live stream is lost. Chunks with `seq <
    /// replay.next_seq` are already included in the snapshot.
    pub fn subscribe_with_replay(
        &self,
        pty_id: &str,
    ) -> Result<(broadcast::Receiver<PtyEvent>, PtyReplay), PtyError> {
        let state = self.lock_state();
        let session = state
            .sessions
            .get(pty_id)
            .ok_or_else(|| PtyError::NotFound(pty_id.to_owned()))?;
        let receiver = self.inner.events.subscribe();
        let (data, next_seq) = session
            .log
            .lock()
            .expect("pty output log poisoned")
            .snapshot();
        Ok((receiver, PtyReplay { data, next_seq }))
    }

    /// Sends keyboard / command input to the PTY's standard input.
    pub fn write(&self, pty_id: &str, data: &[u8]) -> Result<usize, PtyError> {
        let state = self.lock_state();
        let session = state
            .sessions
            .get(pty_id)
            .ok_or_else(|| PtyError::NotFound(pty_id.to_owned()))?;
        if session.input_tx.send(data.to_vec()).is_err() {
            return Err(PtyError::InputClosed(pty_id.to_owned()));
        }
        Ok(data.len())
    }

    /// Changes the window size dynamically.
    pub fn resize(&self, pty_id: &str, cols: u16, rows: u16) -> Result<(), PtyError> {
        let mut state = self.lock_state();
        let session = state
            .sessions
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
        let mut state = self.lock_state();
        let session = state
            .sessions
            .get_mut(pty_id)
            .ok_or_else(|| PtyError::NotFound(pty_id.to_owned()))?;
        session.killer.kill().map_err(PtyError::from_display)?;
        remove_workspace_mapping(&mut state, pty_id);
        Ok(())
    }

    /// Terminates all running PTYs (emergency kill switch).
    ///
    /// Returns the list of PTY IDs to which a termination request was sent.
    /// Actual removal happens when each wait thread detects the exit.
    pub fn kill_all(&self) -> Vec<String> {
        let mut state = self.lock_state();
        let mut killed = Vec::with_capacity(state.sessions.len());
        for (pty_id, session) in state.sessions.iter_mut() {
            if session.killer.kill().is_ok() {
                killed.push(pty_id.clone());
            }
        }
        for pty_id in &killed {
            remove_workspace_mapping(&mut state, pty_id);
        }
        killed
    }

    /// List of running PTYs.
    pub fn list(&self) -> Vec<PtySessionInfo> {
        self.lock_state()
            .sessions
            .values()
            .map(|session| session.info.clone())
            .collect()
    }

    /// Whether the given PTY is running.
    pub fn contains(&self, pty_id: &str) -> bool {
        self.lock_state().sessions.contains_key(pty_id)
    }

    /// Spawns a PTY and starts the asynchronous read/write loops.
    ///
    /// The caller must hold the state lock. `initial_attached` seeds the
    /// attached-client count (opening a workspace terminal counts the caller).
    fn spawn_new_locked(
        &self,
        state: &mut State,
        request: PtySpawnRequest,
        initial_attached: u32,
    ) -> Result<PtySessionInfo, PtyError> {
        if state.sessions.contains_key(&request.pty_id) {
            return Err(PtyError::AlreadyExists(request.pty_id.clone()));
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
            .workspace
            .clone()
            .or_else(|| request.cwd.clone())
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

        let log = Arc::new(Mutex::new(PtyOutputLog::with_default_capacity()));
        let info = PtySessionInfo {
            pty_id: request.pty_id.clone(),
            session_id: request.session_id.clone(),
            path: cwd.clone(),
            pid,
            cols: request.cols,
            rows: request.rows,
            attached: initial_attached,
            created_at: now_ms(),
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
            log: Arc::clone(&log),
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
        let workspace_key = request.workspace.clone();
        if let Some(path) = &workspace_key {
            state.workspaces.insert(path.clone(), info.pty_id.clone());
        }
        state.sessions.insert(
            info.pty_id.clone(),
            PtySession {
                info: info.clone(),
                master: pair.master,
                input_tx,
                killer,
                _guard: guard,
                log,
                detached_at: None,
            },
        );

        let exit_pty_id = info.pty_id.clone();
        let exit_workspace = workspace_key.clone();
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
            let removed = {
                let mut state = exit_inner
                    .state
                    .lock()
                    .expect("pty sessions mutex poisoned");
                if let Some(path) = &exit_workspace
                    && state
                        .workspaces
                        .get(path)
                        .is_some_and(|id| id == &exit_pty_id)
                {
                    state.workspaces.remove(path);
                }
                state.sessions.remove(&exit_pty_id)
            };
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

    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .expect("pty sessions mutex poisoned")
    }
}

/// Removes the workspace mapping that points at `pty_id`.
fn remove_workspace_mapping(state: &mut State, pty_id: &str) {
    state.workspaces.retain(|_, id| id != pty_id);
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
        initial_output: Vec<u8>,
        timeout: Duration,
    ) -> (Vec<u8>, Option<i32>) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut receiver = receiver;
            let mut output = initial_output;
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

    /// Polls until the condition holds (or panics after the timeout).
    fn wait_until(mut condition: impl FnMut() -> bool, reason: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("condition was not met before the timeout: {reason}");
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
        assert_eq!(info.attached, 0);

        // The replay log makes the test independent of the race between a
        // short-lived process and the subscription.
        let (receiver, replay) = manager.subscribe_with_replay("pty-1").expect("subscribe");
        let (output, exit_code) =
            collect_until_exit(receiver, replay.data, Duration::from_secs(10));
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("hello-pty"), "unexpected output: {text:?}");
        assert_eq!(exit_code, Some(0));

        // The session is removed from the session map after exit
        wait_until(|| !manager.contains("pty-1"), "session removal on exit");
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

        let (receiver, replay) = manager.subscribe_with_replay("pty-2").expect("subscribe");
        let (output, _) = collect_until_exit(receiver, replay.data, Duration::from_secs(10));
        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("interactive-ok"),
            "unexpected output: {text:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn opening_the_same_workspace_reuses_the_terminal() {
        let manager = PtySessionManager::new();
        let workspace = std::env::temp_dir();
        let mut first = PtySpawnRequest::new("pty-open-1");
        first.shell_cmd = Some("sh".to_owned());
        first.workspace = Some(workspace.clone());
        let opened = manager.open(first, false).expect("open");
        assert!(!opened.attached);
        assert_eq!(opened.info.path, workspace);
        assert_eq!(opened.info.attached, 1);

        let mut second = PtySpawnRequest::new("pty-open-2");
        second.shell_cmd = Some("sh".to_owned());
        second.workspace = Some(workspace.clone());
        let reused = manager.open(second, false).expect("reopen");
        assert!(reused.attached);
        assert_eq!(reused.info.pty_id, opened.info.pty_id);
        assert_eq!(reused.info.attached, 2);
        assert_eq!(manager.list().len(), 1);

        manager.kill(&opened.info.pty_id).expect("kill");
    }

    #[cfg(unix)]
    #[test]
    fn replacing_a_workspace_terminal_starts_a_new_shell() {
        let manager = PtySessionManager::new();
        let workspace = std::env::temp_dir();
        let mut first = PtySpawnRequest::new("pty-replace-1");
        first.shell_cmd = Some("sh".to_owned());
        first.workspace = Some(workspace.clone());
        let opened = manager.open(first, false).expect("open");

        let mut second = PtySpawnRequest::new("pty-replace-2");
        second.shell_cmd = Some("sh".to_owned());
        second.workspace = Some(workspace);
        let replaced = manager.open(second, true).expect("replace");
        assert!(!replaced.attached);
        assert_ne!(replaced.info.pty_id, opened.info.pty_id);
        assert!(manager.contains(&replaced.info.pty_id));
        wait_until(
            || !manager.contains(&opened.info.pty_id),
            "old terminal removal",
        );

        manager.kill(&replaced.info.pty_id).expect("kill");
    }

    #[cfg(unix)]
    #[test]
    fn replay_returns_retained_output_and_the_next_sequence() {
        let manager = PtySessionManager::new();
        let mut request = PtySpawnRequest::new("pty-replay");
        request.shell_cmd = Some("sh".to_owned());
        manager.spawn(request).expect("spawn");

        manager
            .write("pty-replay", b"printf replay-ok\n")
            .expect("write");
        wait_until(
            || {
                let (_, replay) = manager
                    .subscribe_with_replay("pty-replay")
                    .expect("snapshot");
                String::from_utf8_lossy(&replay.data).contains("replay-ok")
            },
            "output was retained",
        );

        let (receiver, replay) = manager
            .subscribe_with_replay("pty-replay")
            .expect("subscribe with replay");
        assert!(replay.next_seq > 0);
        assert!(replay.data.len() <= fxg_protocol::util::PTY_OUTPUT_LOG_CAPACITY);
        drop(receiver);
        manager.kill("pty-replay").expect("kill");
    }

    #[cfg(unix)]
    #[test]
    fn detached_terminals_expire_after_the_idle_timeout() {
        let manager = PtySessionManager::with_idle_timeout(Duration::from_millis(200));
        let mut request = PtySpawnRequest::new("pty-idle");
        request.shell_cmd = Some("sh".to_owned());
        manager.spawn(request).expect("spawn");
        manager.attach("pty-idle").expect("attach");
        manager.detach("pty-idle");

        wait_until(|| !manager.contains("pty-idle"), "idle expiry");
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
        assert!(manager.attach("missing").is_err());
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

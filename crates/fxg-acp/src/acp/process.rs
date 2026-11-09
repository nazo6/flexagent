use super::client::*;
use super::*;

/// Handle to the resident process (shared by the pool / leases).
pub(super) struct ProcessHandle {
    /// Command sender to the process
    control_tx: mpsc::UnboundedSender<ProcessCommand>,
    /// JoinHandle of the resident task (`dispose_process` waits for teardown to finish)
    join: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ProcessHandle {
    /// Creates a new session and returns the handle plus native restore status.
    pub(super) async fn new_session(
        &self,
        params: SessionParams,
        events: mpsc::UnboundedSender<DriverEvent>,
        lease: Arc<ProcessLease>,
    ) -> anyhow::Result<(AcpSessionHandle, bool)> {
        let commands = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        match self.control_tx.send(ProcessCommand::NewSession {
            params,
            events,
            lease,
            commands,
            ready: ready_tx,
        }) {
            Ok(()) => {}
            Err(_) => return Err(anyhow!("acp agent process is no longer running")),
        }
        ready_rx
            .await
            .context("acp agent process ended before the session became ready")?
    }

    /// Terminates the process and waits for teardown to finish (immediate if already done).
    pub(super) async fn dispose_process(&self) {
        fxg_protocol::discard_with!(
            self.control_tx.send(ProcessCommand::Dispose),
            "request process disposal"
        );
        let join = self.join.lock().expect("process join poisoned").take();
        if let Some(join) = join
            && let Err(err) = join.await
        {
            tracing::warn!("acp process task terminated unexpectedly: {err}");
        }
    }
}

impl WarmProcess for ProcessHandle {
    fn dispose(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(self.dispose_process())
    }
}

/// Process lease held by a session. Returned to the pool on end, or disposed.
pub(super) struct ProcessLease {
    /// Reuse key
    pub(super) key: LaunchKey,
    /// Resident process
    pub(super) process: Arc<ProcessHandle>,
    /// Destination pool (`None` = pool disabled → dispose)
    pub(super) pool: Option<Arc<WarmPool<ProcessHandle>>>,
    /// [`AcpSessionHandle::dispose`] requested (dispose the process instead of
    /// returning it to the pool)
    pub(super) dispose_requested: AtomicBool,
    /// Prevents double return / dispose
    pub(super) finished: AtomicBool,
    /// Sender for turn-loop end notification (taken by the process side when serve starts)
    pub(super) session_end_tx: Mutex<Option<oneshot::Sender<()>>>,
    /// Receiver for turn-loop end notification (`shutdown` waits on it)
    pub(super) session_end_rx: Mutex<Option<oneshot::Receiver<()>>>,
}

impl std::fmt::Debug for ProcessLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessLease")
            .field("key", &self.key)
            .field("dispose_requested", &self.dispose_requested)
            .field("finished", &self.finished)
            .finish()
    }
}

impl ProcessLease {
    /// Creates a new lease (including the turn-loop end notification channel).
    pub(super) fn new(
        key: LaunchKey,
        process: Arc<ProcessHandle>,
        pool: Option<Arc<WarmPool<ProcessHandle>>>,
    ) -> Self {
        let (session_end_tx, session_end_rx) = oneshot::channel();
        Self {
            key,
            process,
            pool,
            dispose_requested: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            session_end_tx: Mutex::new(Some(session_end_tx)),
            session_end_rx: Mutex::new(Some(session_end_rx)),
        }
    }

    /// Takes the turn-loop end notifier (process side, once).
    pub(super) fn take_session_end_sender(&self) -> Option<oneshot::Sender<()>> {
        self.session_end_tx
            .lock()
            .expect("process lease poisoned")
            .take()
    }

    /// Takes the receiver for waiting on turn-loop end (`shutdown`, once).
    pub(super) fn take_session_end_receiver(&self) -> Option<oneshot::Receiver<()>> {
        self.session_end_rx
            .lock()
            .expect("process lease poisoned")
            .take()
    }

    /// Marks the process to be disposed instead of returned to the pool (e.g.
    /// abnormal session end).
    pub(super) fn mark_dispose(&self) {
        self.dispose_requested.store(true, Ordering::SeqCst);
    }

    /// Post-session cleanup (return to the pool or dispose). Duplicate calls are ignored.
    pub(super) async fn finish(&self) {
        if self.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.dispose_requested.load(Ordering::SeqCst) || self.pool.is_none() {
            self.process.dispose_process().await;
        } else if let Some(pool) = &self.pool {
            pool.release(self.key.clone(), Arc::clone(&self.process));
        }
    }
}

/// Spawns the agent process and returns the resident task and the process ready notification.
///
/// ready is notified when `initialize` completes (sessions are created by
/// [`ProcessHandle::new_session`]).
pub(super) fn spawn_agent_process(
    agent: AcpAgent,
    agent_id: String,
    stderr: StderrRegistry,
) -> (Arc<ProcessHandle>, oneshot::Receiver<anyhow::Result<()>>) {
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        if let Err(err) = run_acp_process(AcpProcessTaskParams {
            agent,
            agent_id,
            stderr,
            ready_tx,
            control_rx,
        })
        .await
        {
            tracing::warn!("acp agent process ended: {err:#}");
        }
    });
    (
        Arc::new(ProcessHandle {
            control_tx,
            join: Mutex::new(Some(join)),
        }),
        ready_rx,
    )
}

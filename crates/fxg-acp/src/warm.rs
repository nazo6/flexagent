//! Reuse pool for idle agent processes (warm pool).
//!
//! Agents that are slow to start (e.g. PyInstaller onefile self-extraction taking
//! a dozen seconds) pay the full cost again on the next session if the process is
//! discarded at every session end. This module keeps post-session processes idle
//! per launch spec and reuses them at the next session start (just issuing
//! `session/new` on an initialized connection starts the session in seconds).
//!
//! - At most **one process per launch spec ([`LaunchKey`])** is kept.
//! - Idle processes older than the TTL ([`DEFAULT_WARM_IDLE`]) are disposed automatically.
//! - When a process is released while an idle process with the same key exists, the
//!   older one is disposed.
//! - If the pool itself is dropped, idle processes are disposed on a best-effort
//!   basis (if even that fails, the process tree's Job Object reclaims them at the
//!   OS level).

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::driver::AgentLaunchSpec;

/// Default idle retention time for processes.
pub(crate) const DEFAULT_WARM_IDLE: Duration = Duration::from_secs(10 * 60);

/// Process held by the pool (implemented by the caller).
pub(crate) trait WarmProcess: Send + Sync + 'static {
    /// Terminates the process and waits for teardown to finish (immediate if already done).
    fn dispose(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Process reuse key. Only processes whose key matches are reused.
///
/// Per-session information (cwd / initial mode / resume selection) is not included
/// (those are `session/new` arguments and do not affect whether a process can be reused).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct LaunchKey {
    agent_id: String,
    program: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

impl LaunchKey {
    /// Builds a key from the launch spec and the resolved executable.
    pub(crate) fn new(spec: &AgentLaunchSpec, program: PathBuf) -> Self {
        let mut env = spec.env.clone();
        // Normalize so that environment variable ordering does not produce a different key
        env.sort();
        Self {
            agent_id: spec.agent_id.clone(),
            program,
            args: spec.args.clone(),
            env,
        }
    }

    /// Agent ID (for logging).
    pub(crate) fn agent_id(&self) -> &str {
        &self.agent_id
    }
}

/// Reuse pool for idle processes.
pub(crate) struct WarmPool<P: WarmProcess> {
    /// Idle retention time
    ttl: Duration,
    /// Pool state (`std::sync::Mutex` because it is never held across an await)
    inner: Mutex<PoolState<P>>,
}

struct PoolState<P: WarmProcess> {
    /// Idle process per key
    idle: HashMap<LaunchKey, IdleEntry<P>>,
    /// Monotonic token for TTL expiry checks
    next_token: u64,
}

struct IdleEntry<P: WarmProcess> {
    process: Arc<P>,
    token: u64,
}

impl<P: WarmProcess> WarmPool<P> {
    /// Creates a pool with a TTL.
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(PoolState {
                idle: HashMap::new(),
                next_token: 0,
            }),
        }
    }

    /// Takes the idle process matching the key (`None` if absent).
    pub(crate) fn checkout(&self, key: &LaunchKey) -> Option<Arc<P>> {
        self.inner
            .lock()
            .expect("warm pool poisoned")
            .idle
            .remove(key)
            .map(|entry| entry.process)
    }

    /// Returns a process after session end (disposed automatically after `ttl`).
    pub(crate) fn release(self: &Arc<Self>, key: LaunchKey, process: Arc<P>) {
        let (token, replaced) = {
            let mut state = self.inner.lock().expect("warm pool poisoned");
            state.next_token += 1;
            let token = state.next_token;
            let entry = IdleEntry { process, token };
            let replaced = state.idle.insert(key.clone(), entry);
            (token, replaced)
        };
        // If an idle process with the same key already exists, dispose the older one (max 1 kept)
        if let Some(old) = replaced {
            spawn_dispose(old.process);
        }
        // TTL expiry timer. Holds a `Weak` reference so it does not prevent the pool
        // itself from being dropped (so Drop can dispose all remaining idle processes at once).
        let ttl = self.ttl;
        let pool = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(ttl).await;
            let Some(pool) = pool.upgrade() else {
                return;
            };
            let expired = {
                let mut state = pool.inner.lock().expect("warm pool poisoned");
                match state.idle.get(&key) {
                    Some(entry) if entry.token == token => {
                        state.idle.remove(&key).map(|entry| entry.process)
                    }
                    _ => None,
                }
            };
            if let Some(process) = expired {
                process.dispose().await;
            }
        });
    }

    /// Disposes all idle processes (`fxg kill-all` / daemon shutdown).
    ///
    /// The return value is the number of processes disposed (waited on).
    pub(crate) async fn shutdown_idle(&self) -> usize {
        let drained: Vec<Arc<P>> = {
            let mut state = self.inner.lock().expect("warm pool poisoned");
            state.idle.drain().map(|(_, entry)| entry.process).collect()
        };
        let count = drained.len();
        for process in drained {
            process.dispose().await;
        }
        count
    }
}

impl<P: WarmProcess> Drop for WarmPool<P> {
    fn drop(&mut self) {
        let drained: Vec<Arc<P>> = {
            let mut state = self.inner.lock().expect("warm pool poisoned");
            state.idle.drain().map(|(_, entry)| entry.process).collect()
        };
        // dispose cannot be driven while the runtime is shutting down, so leave it to
        // the process tree's Job Object (kill-on-close).
        for process in drained {
            spawn_dispose(process);
        }
    }
}

/// Runs disposal detached (without waiting).
fn spawn_dispose<P: WarmProcess>(process: Arc<P>) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            process.dispose().await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Test process: records dispose calls in a shared counter.
    struct MockProcess {
        dispose_count: Arc<AtomicUsize>,
    }

    impl WarmProcess for MockProcess {
        fn dispose(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(async move {
                self.dispose_count.fetch_add(1, Ordering::SeqCst);
            })
        }
    }

    fn key(agent: &str) -> LaunchKey {
        LaunchKey {
            agent_id: agent.to_owned(),
            program: PathBuf::from("agent.exe"),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    fn mock(counter: &Arc<AtomicUsize>) -> Arc<MockProcess> {
        Arc::new(MockProcess {
            dispose_count: Arc::clone(counter),
        })
    }

    /// Waits until the counter reaches `expected` (best effort).
    async fn wait_for(counter: &Arc<AtomicUsize>, expected: usize) {
        for _ in 0..200 {
            if counter.load(Ordering::SeqCst) >= expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn checkout_returns_released_process_once() {
        let counter = Arc::new(AtomicUsize::new(0));
        let pool = Arc::new(WarmPool::new(Duration::from_secs(60)));
        let process = mock(&counter);
        pool.release(key("a"), Arc::clone(&process));

        let checked_out = pool.checkout(&key("a")).expect("idle process");
        assert!(Arc::ptr_eq(&checked_out, &process));
        // Cannot be reused after checkout
        assert!(pool.checkout(&key("a")).is_none());
        // Checkout does not dispose it
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn keys_are_isolated() {
        let counter = Arc::new(AtomicUsize::new(0));
        let pool = Arc::new(WarmPool::new(Duration::from_secs(60)));
        pool.release(key("a"), mock(&counter));
        assert!(pool.checkout(&key("b")).is_none());
        assert!(pool.checkout(&key("a")).is_some());
    }

    #[tokio::test]
    async fn release_replaces_existing_idle_process() {
        let counter = Arc::new(AtomicUsize::new(0));
        let pool = Arc::new(WarmPool::new(Duration::from_secs(60)));
        pool.release(key("a"), mock(&counter));
        let second = mock(&counter);
        pool.release(key("a"), Arc::clone(&second));

        // The older idle process is disposed
        wait_for(&counter, 1).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        // The newer one remains
        let checked_out = pool.checkout(&key("a")).expect("idle process");
        assert!(Arc::ptr_eq(&checked_out, &second));
    }

    #[tokio::test]
    async fn ttl_expiry_disposes_idle_process() {
        let counter = Arc::new(AtomicUsize::new(0));
        let pool = Arc::new(WarmPool::new(Duration::from_millis(30)));
        pool.release(key("a"), mock(&counter));

        wait_for(&counter, 1).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert!(pool.checkout(&key("a")).is_none());
    }

    #[tokio::test]
    async fn shutdown_idle_disposes_all() {
        let counter = Arc::new(AtomicUsize::new(0));
        let pool = Arc::new(WarmPool::new(Duration::from_secs(60)));
        pool.release(key("a"), mock(&counter));
        pool.release(key("b"), mock(&counter));

        let disposed = pool.shutdown_idle().await;
        assert_eq!(disposed, 2);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert!(pool.checkout(&key("a")).is_none());
    }

    #[tokio::test]
    async fn dropped_pool_disposes_idle_processes() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let pool = Arc::new(WarmPool::new(Duration::from_secs(60)));
            pool.release(key("a"), mock(&counter));
        }
        wait_for(&counter, 1).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}

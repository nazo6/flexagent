//! アイドル状態のエージェントプロセスの再利用プール (warm pool)。
//!
//! 起動に時間がかかるエージェント (例: PyInstaller onefile の自己展開に十数秒)
//! は、セッション終了のたびにプロセスを破棄すると次のセッションで再びフル
//! コストがかかる。本モジュールは「セッション終了後のプロセス」を起動スペック
//! 単位でアイドル保持し、次のセッション開始時に再利用する (initialize 済みの
//! 接続へ `session/new` を発行するだけ = 数秒で開始できる)。
//!
//! - 保持は **起動スペック ([`LaunchKey`]) 単位で最大 1 プロセス**。
//! - TTL ([`DEFAULT_WARM_IDLE`]) を超えたアイドルは自動破棄する。
//! - 返却時に同一キーのアイドルが既にある場合は、古い方を破棄する。
//! - プール自体が Drop された場合も、ベストエフォートでアイドルを破棄する
//!   (それも叶わない場合はプロセスツリーの Job Object が OS 側で回収する)。

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::driver::AgentLaunchSpec;

/// アイドルプロセスの既定保持時間。
pub(crate) const DEFAULT_WARM_IDLE: Duration = Duration::from_secs(10 * 60);

/// プールが保持するプロセス (呼び出し側が実装する)。
pub(crate) trait WarmProcess: Send + Sync + 'static {
    /// プロセスを終了し、teardown の完了を待つ (完了済みなら即時)。
    fn dispose(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// プロセスの再利用キー。これが一致するプロセスのみ再利用する。
///
/// セッション単位の情報 (cwd / 初期モード / resume 指定) は含めない
/// (これらは `session/new` の引数であり、プロセス再利用の可否に影響しない)。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct LaunchKey {
    agent_id: String,
    program: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

impl LaunchKey {
    /// 起動スペックと解決済み実行ファイルからキーを作る。
    pub(crate) fn new(spec: &AgentLaunchSpec, program: PathBuf) -> Self {
        let mut env = spec.env.clone();
        // 環境変数の順序差で別キー扱いにならないよう正規化する
        env.sort();
        Self {
            agent_id: spec.agent_id.clone(),
            program,
            args: spec.args.clone(),
            env,
        }
    }

    /// エージェントID (ログ用)。
    pub(crate) fn agent_id(&self) -> &str {
        &self.agent_id
    }
}

/// アイドルプロセスの再利用プール。
pub(crate) struct WarmPool<P: WarmProcess> {
    /// アイドル保持時間
    ttl: Duration,
    /// プール状態 (await を跨がないため `std::sync::Mutex`)
    inner: Mutex<PoolState<P>>,
}

struct PoolState<P: WarmProcess> {
    /// キーごとのアイドルプロセス
    idle: HashMap<LaunchKey, IdleEntry<P>>,
    /// TTL 失効判定用の単調トークン
    next_token: u64,
}

struct IdleEntry<P: WarmProcess> {
    process: Arc<P>,
    token: u64,
}

impl<P: WarmProcess> WarmPool<P> {
    /// TTL 付きでプールを作成する。
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(PoolState {
                idle: HashMap::new(),
                next_token: 0,
            }),
        }
    }

    /// キーに一致するアイドルプロセスを取り出す (なければ `None`)。
    pub(crate) fn checkout(&self, key: &LaunchKey) -> Option<Arc<P>> {
        self.inner
            .lock()
            .expect("warm pool poisoned")
            .idle
            .remove(key)
            .map(|entry| entry.process)
    }

    /// セッション終了後のプロセスを返却する (`ttl` 経過後に自動破棄)。
    pub(crate) fn release(self: &Arc<Self>, key: LaunchKey, process: Arc<P>) {
        let (token, replaced) = {
            let mut state = self.inner.lock().expect("warm pool poisoned");
            state.next_token += 1;
            let token = state.next_token;
            let entry = IdleEntry { process, token };
            let replaced = state.idle.insert(key.clone(), entry);
            (token, replaced)
        };
        // 同一キーのアイドルが既にあった場合は古い方を破棄する (保持は最大 1)
        if let Some(old) = replaced {
            spawn_dispose(old.process);
        }
        // TTL 失効タイマー。プール自体の Drop を妨げないよう `Weak` で参照する
        // (Drop 時に残りのアイドルをまとめて破棄できるようにするため)。
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

    /// アイドルプロセスをすべて破棄する (`fxg kill-all` / デーモン終了時)。
    ///
    /// 戻り値は破棄した (dispose を待った) プロセス数。
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
        // ランタイム停止中は dispose を駆動できないため、その場合は
        // プロセスツリーの Job Object (kill-on-close) に任せる。
        for process in drained {
            spawn_dispose(process);
        }
    }
}

/// 破棄処理を切り離して実行する (待たない)。
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

    /// テスト用プロセス: dispose 回数を共有カウンタへ記録する。
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

    /// カウンタが `expected` になるまで待つ (ベストエフォート)。
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
        // 取り出した後は再利用できない
        assert!(pool.checkout(&key("a")).is_none());
        // 取り出しでは破棄されない
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

        // 古いアイドルが破棄される
        wait_for(&counter, 1).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        // 残っているのは新しい方
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

//! `fxg daemon --stdio`: 一時VM・サンドボックスノード用の標準入出力トランスポート。
//!
//! 設計: `docs/01-architecture-and-sync.md` §6.1、`docs/03-protocol-and-api.md` §2。
//!
//! 中央サーバー (`fxg server`) が子プロセスとして起動したプロビジョナーコマンドの
//! **`stdin` (`ServerToNodeMsg`) / `stdout` (`NodeToServerMsg`)** 上で
//! JSON Lines 通信を行う (ネットワーク・VPN 完全非依存)。ブートストラップや
//! `mise` / `uv` によるツール導入ログはすべて `stderr` に出力され、
//! `stdout` はプロトコル通信専用として保護する。
//!
//! - 常駐ノードと同じ [`SessionManager`](crate::session_manager::SessionManager) /
//!   イベントバス / Outbox を使用し、メッセージ型も 100% 共通
//! - `--workspace <DIR>`: クローン済みワークスペースをプロジェクトとして登録し、
//!   `StartSession` の `local_path` をこのディレクトリに固定する
//! - `--ephemeral`: `DrainAndShutdown` 受信時に Git バンドルを退避して終了する
//! - ローカルIPCサーバーも起動する (`fxg git-askpass` → Git Credential Proxy 用)

use std::path::PathBuf;
use std::time::Duration;

use fxg_protocol::node_server::NodeToServerMsg;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{mpsc, watch};

use super::{DaemonConfig, DaemonState};
use crate::error::NodeError;

/// `fxg daemon --stdio` の起動設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioConfig {
    /// 基本設定 (ノードID / データディレクトリ等)
    pub daemon: DaemonConfig,
    /// `--workspace`: 初期対象ディレクトリ (クローン済みワークスペース)
    pub workspace: Option<PathBuf>,
    /// `--ephemeral`: 一時VMモード (Drain 時の Git バンドル退避を有効化)
    pub ephemeral: bool,
}

/// 送信キューの容量 (PTY forwarder 含む)。
const OUT_QUEUE_CAPACITY: usize = 1024;

/// Drain 後に送信キューをフラッシュする待ち時間の上限。
const FLUSH_TIMEOUT: Duration = Duration::from_secs(15);

/// stdio トランスポートでデーモンを稼働させる (終了まで戻らない)。
pub async fn run_stdio(config: StdioConfig) -> Result<(), NodeError> {
    let mut daemon_config = config.daemon.clone();
    daemon_config.ephemeral = config.ephemeral;
    let state = DaemonState::new(daemon_config).await?;
    state.register_node().await?;

    // `--workspace` を論理プロジェクトとして登録する
    // (NodeHello のプロジェクト報告に含まれ、サーバー側でセッション起動に使われる)
    if let Some(workspace) = config.workspace.as_deref() {
        if !workspace.is_dir() {
            return Err(NodeError::Server(format!(
                "--workspace directory does not exist: {}",
                workspace.display()
            )));
        }
        match state.resolve_and_register_project(workspace).await {
            Ok(resolved) => tracing::info!(
                project_id = %resolved.project_id,
                path = %workspace.display(),
                "workspace registered"
            ),
            Err(err) => {
                tracing::warn!(
                    path = %workspace.display(),
                    "failed to register workspace project: {err}"
                );
            }
        }
    }

    // ローカルIPCサーバー (GIT_ASKPASS ヘルパー / ローカル CLI 用)
    let (ipc_shutdown_tx, ipc_shutdown_rx) = watch::channel(false);
    let ipc_task = tokio::spawn({
        let state = state.clone();
        let endpoint = state.config().ipc_endpoint.clone();
        async move {
            if let Err(err) = super::ipc::serve(state, endpoint, ipc_shutdown_rx).await {
                tracing::error!("local ipc server stopped: {err}");
            }
        }
    });

    // stdout はプロトコル通信専用 (JSON Lines)。書き込みは専用タスクに直列化する。
    let (out_tx, mut out_rx) = mpsc::channel::<NodeToServerMsg>(OUT_QUEUE_CAPACITY);
    let writer = tokio::spawn(async move {
        let mut writer = BufWriter::new(tokio::io::stdout());
        while let Some(message) = out_rx.recv().await {
            let mut line = match serde_json::to_vec(&message) {
                Ok(line) => line,
                Err(err) => {
                    tracing::warn!("failed to serialize message for stdout: {err}");
                    continue;
                }
            };
            line.push(b'\n');
            if writer.write_all(&line).await.is_err() || writer.flush().await.is_err() {
                break;
            }
        }
        let _ = writer.flush().await;
    });

    // Git Credential Proxy をこのトランスポートに紐付ける
    state.credentials().attach(out_tx.clone());

    // 1. ハンドシェイク + 未送信イベントのフラッシュ
    let hello = crate::sync::build_hello(&state).await;
    let _ = out_tx.send(hello).await;
    if let Err(err) = crate::sync::push_outbox(&state, &out_tx).await {
        tracing::warn!("failed to flush outbox at startup: {err}");
    }

    // 2. 双方向ループ (stdin の ServerToNodeMsg / イベントバス → stdout)
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let ctx = crate::sync::CommandContext {
        workspace: config.workspace.as_deref(),
        ephemeral: config.ephemeral,
        shutdown: Some(&shutdown_tx),
    };
    let mut bus_rx = state.bus().subscribe();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    tracing::info!(
        node_id = state.node_id(),
        ephemeral = config.ephemeral,
        "fxg daemon started (stdio transport)"
    );

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => break,
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    crate::sync::handle_server_message(&state, &line, &out_tx, &ctx).await;
                }
                Ok(None) => {
                    tracing::info!("stdin closed; shutting down");
                    break;
                }
                Err(err) => {
                    tracing::warn!("failed to read stdin: {err}");
                    break;
                }
            },
            bus = bus_rx.recv() => match bus {
                Ok(crate::session::SessionBroadcast::Persisted { event, .. }) => {
                    let _ = out_tx
                        .send(NodeToServerMsg::EventBatchPush { events: vec![event] })
                        .await;
                }
                Ok(crate::session::SessionBroadcast::StreamDelta { session_id, delta }) => {
                    let _ = out_tx
                        .send(NodeToServerMsg::LiveStreamDelta { session_id, delta })
                        .await;
                }
                // エフェメラルイベント (キーストローク等) は同期しない
                Ok(crate::session::SessionBroadcast::Ephemeral(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "session bus lagged; flushing outbox");
                    let _ = crate::sync::push_outbox(&state, &out_tx).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
        }
    }

    // 3. 終了処理 (ベストエフォート): エージェント / PTY を停止し、
    //    未送信イベントをフラッシュしてから stdout を閉じる
    if let Err(err) = state.kill_all_local("stdio transport ending").await {
        tracing::warn!("failed to stop sessions on shutdown: {err}");
    }
    if let Err(err) = crate::sync::push_outbox(&state, &out_tx).await {
        tracing::warn!("failed to flush outbox on shutdown: {err}");
    }
    state.credentials().detach();
    if !state.config().ephemeral {
        // 一時VMはサーバー側でノード行を破棄するため、オフライン記録は通常ノードのみ
        if let Err(err) = state.mark_offline().await {
            tracing::warn!("failed to mark node offline: {err}");
        }
    }
    let _ = ipc_shutdown_tx.send(true);
    drop(out_tx);
    let _ = tokio::time::timeout(FLUSH_TIMEOUT, writer).await;
    let _ = ipc_task.await;
    tracing::info!("fxg daemon stopped (stdio transport)");
    Ok(())
}

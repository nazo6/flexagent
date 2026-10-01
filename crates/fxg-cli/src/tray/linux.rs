//! Linux (ksni バックエンド): D-Bus ワーカースレッドを持つためイベントループ不要。
//!
//! 専用スレッドでトレイを作成し、更新をチャネルで受けてメニューを操作する
//! (`muda` のメニュー項目は `!Send` のため、作成したスレッド上でのみ操作する)。
//! `libappindicator` (GTK3) ではなく純 Rust の `ksni` を利用するため、
//! GTK のビルド依存やデスクトップセッションは不要 (D-Bus セッションのみ必要)。

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc::unbounded_channel;

use super::{Notifier, TrayAction, TrayContext, TrayUi, TrayUpdate, spawn_controller};

/// トレイ常駐ハンドル。`close` でトレイを削除しスレッドの終了を待つ。
pub struct TrayHandle {
    notifier: Notifier,
    join: Option<JoinHandle<()>>,
}

impl TrayHandle {
    /// トレイを削除し、トレイスレッドの終了を待つ。
    pub fn close(mut self) {
        if self.join.is_some() {
            (self.notifier)(TrayUpdate::Close);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        if self.join.is_some() {
            // 明示的な `close` なしで破棄された場合もトレイを消す (join はしない)
            (self.notifier)(TrayUpdate::Close);
        }
    }
}

/// トレイ常駐を開始する (専用スレッド + チャネル駆動ループ)。
pub fn spawn(handle: &tokio::runtime::Handle, ctx: TrayContext) -> Result<TrayHandle> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let (update_tx, mut update_rx) = unbounded_channel::<TrayUpdate>();
    let (action_tx, action_rx) = unbounded_channel::<TrayAction>();

    let loop_ctx = ctx.clone();
    let join = std::thread::Builder::new()
        .name("fxg-tray".to_owned())
        .spawn(move || {
            let mut ui = match TrayUi::new(&loop_ctx, action_tx) {
                Ok(ui) => ui,
                Err(err) => {
                    let _ = ready_tx.send(Err(format!("{err:#}")));
                    return;
                }
            };
            if ready_tx.send(Ok(())).is_err() {
                return;
            }
            // ksni は自前のワーカースレッドで D-Bus を処理するため、
            // ここは更新待ちのブロッキングループでよい (起床処理は不要)。
            while let Some(update) = update_rx.blocking_recv() {
                if !ui.apply(update) {
                    break;
                }
            }
        })
        .context("failed to spawn the tray thread")?;

    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            let _ = join.join();
            bail!("{err}");
        }
        Err(_) => {
            let _ = join.join();
            bail!("the tray thread exited before initializing");
        }
    }

    let notifier: Notifier = Arc::new(move |update: TrayUpdate| {
        let _ = update_tx.send(update);
    });
    spawn_controller(handle, ctx, notifier.clone(), action_rx);
    Ok(TrayHandle {
        notifier,
        join: Some(join),
    })
}

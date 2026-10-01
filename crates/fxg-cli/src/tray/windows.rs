//! Windows: トレイを作成したスレッドで Win32 メッセージループを回すバックエンド。
//!
//! `tray-icon` は `Shell_NotifyIconW` を隠しウィンドウで受けるため、トレイを
//! 作成したスレッドでメッセージループが動作している必要がある。ここでは専用
//! スレッドを立てて `GetMessageW` ループを回し、デーモン本体 (tokio) とは
//! チャネルで連携する。

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc::unbounded_channel;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_APP,
};

use super::{Notifier, TrayAction, TrayContext, TrayUi, TrayUpdate, spawn_controller};

/// ステータス更新をトレイスレッドへ知らせるスレッドメッセージ (`wndproc` を持たない)。
const WM_FXG_TRAY_WAKE: u32 = WM_APP + 1;

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

/// トレイ常駐を開始する (専用スレッド + Win32 メッセージループ)。
pub fn spawn(handle: &tokio::runtime::Handle, ctx: TrayContext) -> Result<TrayHandle> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
    let (update_tx, mut update_rx) = unbounded_channel::<TrayUpdate>();
    let (action_tx, action_rx) = unbounded_channel::<TrayAction>();

    let loop_ctx = ctx.clone();
    let join = std::thread::Builder::new()
        .name("fxg-tray".to_owned())
        .spawn(move || {
            // トレイ作成 (隠しウィンドウ生成) でメッセージキューが作られるため、
            // ready を返した後の `PostThreadMessageW` は取りこぼされない。
            let mut ui = match TrayUi::new(&loop_ctx, action_tx) {
                Ok(ui) => ui,
                Err(err) => {
                    let _ = ready_tx.send(Err(format!("{err:#}")));
                    return;
                }
            };
            let thread_id = unsafe { GetCurrentThreadId() };
            if ready_tx.send(Ok(thread_id)).is_err() {
                return;
            }
            let mut message: MSG = unsafe { std::mem::zeroed() };
            loop {
                if !ui.apply_pending(&mut update_rx) {
                    break;
                }
                // 更新通知は `PostThreadMessageW` で起こされる。WM_QUIT (0) /
                // エラー (-1) で終了する。
                if unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } <= 0 {
                    break;
                }
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        })
        .context("failed to spawn the tray thread")?;

    let thread_id = match ready_rx.recv() {
        Ok(Ok(thread_id)) => thread_id,
        Ok(Err(err)) => {
            let _ = join.join();
            bail!("{err}");
        }
        Err(_) => {
            let _ = join.join();
            bail!("the tray thread exited before initializing");
        }
    };

    let notifier: Notifier = Arc::new(move |update: TrayUpdate| {
        let _ = update_tx.send(update);
        unsafe {
            PostThreadMessageW(thread_id, WM_FXG_TRAY_WAKE, 0, 0);
        }
    });
    spawn_controller(handle, ctx, notifier.clone(), action_rx);
    Ok(TrayHandle {
        notifier,
        join: Some(join),
    })
}

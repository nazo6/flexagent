//! Windows: backend that runs the Win32 message loop on the thread that created
//! the tray.
//!
//! `tray-icon` receives `Shell_NotifyIconW` through a hidden window, so the
//! message loop must run on the thread that created the tray. This backend
//! sets up a dedicated thread that runs a `GetMessageW` loop and cooperates
//! with the daemon body (tokio) over channels.

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

/// Thread message that wakes the tray thread for a status update (no
/// `wndproc`).
const WM_FXG_TRAY_WAKE: u32 = WM_APP + 1;

/// Handle for the tray residency. `close` removes the tray and waits for the
/// thread to exit.
pub struct TrayHandle {
    notifier: Notifier,
    join: Option<JoinHandle<()>>,
}

impl TrayHandle {
    /// Remove the tray and wait for the tray thread to exit.
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
            // Also remove the tray when dropped without an explicit `close`
            // (without joining)
            (self.notifier)(TrayUpdate::Close);
        }
    }
}

/// Start the tray residency (dedicated thread + Win32 message loop).
pub fn spawn(handle: &tokio::runtime::Handle, ctx: TrayContext) -> Result<TrayHandle> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();
    let (update_tx, mut update_rx) = unbounded_channel::<TrayUpdate>();
    let (action_tx, action_rx) = unbounded_channel::<TrayAction>();

    let loop_ctx = ctx.clone();
    let join = std::thread::Builder::new()
        .name("fxg-tray".to_owned())
        .spawn(move || {
            // Creating the tray (spawning the hidden window) creates the message
            // queue, so `PostThreadMessageW` calls after ready is sent are not
            // lost.
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
                // Update notifications are delivered by `PostThreadMessageW`.
                // The loop ends on WM_QUIT (0) or an error (-1).
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

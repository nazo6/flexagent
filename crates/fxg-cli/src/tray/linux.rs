//! Linux (ksni backend): the D-Bus backend has its own worker thread, so no
//! event loop is needed.
//!
//! The tray is created on a dedicated thread, which then receives updates over
//! a channel and drives the menu (`muda` menu items are `!Send`, so they are
//! touched only on the creating thread). Because this uses the pure-Rust
//! `ksni` instead of `libappindicator` (GTK3), no GTK build dependency or
//! desktop session is required (only a D-Bus session).

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc::unbounded_channel;

use super::{Notifier, TrayAction, TrayContext, TrayUi, TrayUpdate, spawn_controller};

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
            if let Some(join) = self.join.take()
                && let Err(err) = join.join()
            {
                tracing::warn!("tray thread panicked: {err:?}");
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

/// Start the tray residency (dedicated thread + channel-driven loop).
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
                    fxg_protocol::discard_with!(
                        ready_tx.send(Err(format!("{err:#}"))),
                        "report the tray initialization failure"
                    );
                    return;
                }
            };
            if ready_tx.send(Ok(())).is_err() {
                return;
            }
            // ksni handles D-Bus on its own worker thread, so a blocking loop
            // waiting for updates is fine here (no wake-up handling needed).
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
            if let Err(join_err) = join.join() {
                tracing::warn!("tray thread panicked: {join_err:?}");
            }
            bail!("{err}");
        }
        Err(_) => {
            if let Err(join_err) = join.join() {
                tracing::warn!("tray thread panicked: {join_err:?}");
            }
            bail!("the tray thread exited before initializing");
        }
    }

    let notifier: Notifier = Arc::new(move |update: TrayUpdate| {
        fxg_protocol::discard_with!(update_tx.send(update), "notify the tray thread");
    });
    spawn_controller(handle, ctx, notifier.clone(), action_rx);
    Ok(TrayHandle {
        notifier,
        join: Some(join),
    })
}

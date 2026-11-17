//! Graceful shutdown helpers shared by the hub and the local node.

use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Spawns a shutdown signal handler (`Ctrl-C` on all platforms, plus `SIGTERM` on Unix).
///
/// The first signal requests graceful shutdown; a subsequent signal while
/// already shutting down forces immediate process exit.
pub fn spawn_ctrl_c_handler(shutdown_tx: watch::Sender<bool>) {
    fxg_protocol::spawn_guarded!("shutdown signal handler", async move {
        #[cfg(unix)]
        let mut sigterm =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => Some(signal),
                Err(err) => {
                    tracing::warn!("failed to install SIGTERM handler: {err}");
                    None
                }
            };

        loop {
            let (signal_name, force_exit_code) = {
                #[cfg(unix)]
                {
                    tokio::select! {
                        res = tokio::signal::ctrl_c() => {
                            if res.is_err() {
                                break;
                            }
                            ("Ctrl-C", 130)
                        }
                        () = wait_sigterm(&mut sigterm) => ("SIGTERM", 143),
                    }
                }
                #[cfg(not(unix))]
                {
                    if tokio::signal::ctrl_c().await.is_err() {
                        break;
                    }
                    ("Ctrl-C", 130)
                }
            };

            if *shutdown_tx.borrow() {
                tracing::warn!(
                    signal = signal_name,
                    "received second shutdown signal; forcing exit"
                );
                std::process::exit(force_exit_code);
            }
            tracing::info!(
                signal = signal_name,
                "received {signal_name}, shutting down (send again to force exit)"
            );
            fxg_protocol::discard_with!(
                shutdown_tx.send(true),
                "request shutdown after shutdown signal"
            );
        }
    });
}

#[cfg(unix)]
async fn wait_sigterm(sigterm: &mut Option<tokio::signal::unix::Signal>) {
    if let Some(stream) = sigterm.as_mut()
        && stream.recv().await.is_some()
    {
        return;
    }
    *sigterm = None;
    std::future::pending::<()>().await;
}

/// Waits for a background task to exit during shutdown, aborting it on timeout.
pub async fn await_shutdown_task(
    task_name: &'static str,
    mut handle: JoinHandle<()>,
    timeout: std::time::Duration,
) {
    match tokio::time::timeout(timeout, &mut handle).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            tracing::error!(
                task = task_name,
                "{task_name} task terminated unexpectedly: {err}"
            );
        }
        Err(_) => {
            tracing::warn!(
                task = task_name,
                timeout = ?timeout,
                "timed out waiting for {task_name} task during shutdown; aborting"
            );
            handle.abort();
        }
    }
}

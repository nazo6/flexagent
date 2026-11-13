//! macOS: backend that runs the tao loop on the main thread.
//!
//! `NSStatusItem` must be created and driven **while the main thread's event
//! loop is running**, so `fxg node start --tray` hands the main thread over to tao
//! (the NSApplication loop) and starts the node body (tokio runtime) on a
//! worker thread (called from the early branch in `main.rs`).
//!
//! In environments where the tray cannot be created (no window server, etc.) the
//! node logs a warning and keeps running without it.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc;

use fxg_node::daemon::{DaemonState, NodeDaemon, ShutdownHandle};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::unbounded_channel;

use super::{Notifier, TrayAction, TrayContext, TrayUi, TrayUpdate, spawn_controller};
use crate::Commands;
use crate::commands::{NodeCommands, NodeStartArgs};

/// Handles of the started node (passed from the worker thread to the main
/// thread).
type ReadyPayload = (DaemonState, ShutdownHandle, tokio::runtime::Handle);

/// User event sent to the main loop.
enum UserEvent {
    /// A tray update arrived
    Tray,
    /// The node stopped (process exit code)
    DaemonStopped(i32),
}

/// If this is `fxg node start --tray` (macOS), start the node with a tray on the
/// main thread.
///
/// When it returns `Some`, the caller (`main`) exits with that exit code. When
/// it returns `None`, the normal path (tokio runtime on the main thread)
/// handles the command.
pub async fn run_node_if_requested(command: &Commands) -> Option<ExitCode> {
    let Commands::Node(node_args) = command else {
        return None;
    };
    let NodeCommands::Start(args) = &node_args.command else {
        return None;
    };
    let env = fxg_protocol::config::process_env;
    // If the config cannot be loaded, leave it to the normal path (which reports
    // the error)
    let global = fxg_protocol::config::NodeConfig::load(&env).ok()?;
    let tray_configured = crate::commands::load_tray_setting(&env).await;
    if !args.tray_requested(tray_configured) {
        return None;
    }
    Some(run_node_with_tray(args, &global))
}

/// Run `fxg node start --tray` (macOS). Does not return until the node stops.
fn run_node_with_tray(args: &NodeStartArgs, global: &fxg_protocol::config::NodeConfig) -> ExitCode {
    let config = args.node_config_from(global);

    // The node body (tokio) starts on a worker thread (tao owns the main
    // thread)
    let (ready_tx, ready_rx) = mpsc::channel::<Result<ReadyPayload, String>>();
    let worker = match std::thread::Builder::new()
        .name("fxg-node".to_owned())
        .spawn(move || -> i32 {
            let runtime = match Runtime::new() {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = ready_tx.send(Err(format!("failed to start the tokio runtime: {err}")));
                    return 1;
                }
            };
            let handle = runtime.handle().clone();
            runtime.block_on(async move {
                match NodeDaemon::start(config).await {
                    Ok(node) => {
                        let _ = ready_tx.send(Ok((
                            node.state().clone(),
                            node.shutdown_handle(),
                            handle,
                        )));
                        node.wait().await;
                        0
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(format!("{err}")));
                        1
                    }
                }
            })
        }) {
        Ok(worker) => worker,
        Err(err) => {
            eprintln!("fxg: failed to spawn the node thread: {err}");
            return ExitCode::FAILURE;
        }
    };

    let (state, shutdown, runtime_handle) = match ready_rx.recv() {
        Ok(Ok(values)) => values,
        Ok(Err(err)) => {
            eprintln!("fxg: {err}");
            let _ = worker.join();
            return ExitCode::FAILURE;
        }
        Err(_) => {
            let _ = worker.join();
            return ExitCode::FAILURE;
        }
    };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    let proxy = event_loop.create_proxy();
    // Notify the main loop when the worker thread (node) exits
    let proxy_done = proxy.clone();
    std::thread::spawn(move || {
        let code = worker.join().unwrap_or(1);
        let _ = proxy_done.send_event(UserEvent::DaemonStopped(code));
    });

    let (update_tx, mut update_rx) = unbounded_channel::<TrayUpdate>();
    let (action_tx, action_rx) = unbounded_channel::<TrayAction>();
    let mut ui: Option<TrayUi> = None;
    let mut action_rx = Some(action_rx);

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                // The tray must be created after the event loop starts running
                let ctx = TrayContext {
                    state: state.clone(),
                    shutdown: shutdown.clone(),
                };
                match TrayUi::new(&ctx, action_tx.clone()) {
                    Ok(created) => {
                        ui = Some(created);
                        let proxy = proxy.clone();
                        let update_tx = update_tx.clone();
                        let notifier: Notifier = Arc::new(move |update: TrayUpdate| {
                            let _ = update_tx.send(update);
                            let _ = proxy.send_event(UserEvent::Tray);
                        });
                        if let Some(actions) = action_rx.take() {
                            spawn_controller(&runtime_handle, ctx, notifier, actions);
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            "failed to create the tray icon (continuing without tray): {err:#}"
                        );
                    }
                }
            }
            Event::UserEvent(UserEvent::Tray) => {
                if let Some(ui) = ui.as_mut()
                    && !ui.apply_pending(&mut update_rx)
                {
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(UserEvent::DaemonStopped(code)) => {
                if let Some(ui) = ui.as_mut() {
                    let _ = ui.apply(TrayUpdate::Close);
                }
                std::process::exit(code);
            }
            _ => {}
        }
    });
}

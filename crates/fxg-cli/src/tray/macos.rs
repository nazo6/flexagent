//! macOS: メインスレッドで tao ループを回すバックエンド。
//!
//! `NSStatusItem` は**メインスレッドのイベントループが動作している状態**で
//! 作成・操作する必要があるため、`fxg daemon --tray` ではメインスレッドを
//! tao (NSApplication ループ) に明け渡し、デーモン本体 (tokio ランタイム) を
//! ワーカースレッドで起動する (`main.rs` の早期分岐から呼ばれる)。
//!
//! トレイが作成できない環境 (ウィンドウサーバーなし等) では警告のみで
//! デーモンはそのまま稼働を続ける。

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
use crate::commands::DaemonArgs;

/// 起動済みデーモンのハンドル群 (ワーカースレッド → メインスレッドへ渡す)。
type ReadyPayload = (DaemonState, ShutdownHandle, tokio::runtime::Handle);

/// メインループへ送るユーザーイベント。
enum UserEvent {
    /// トレイ更新が届いた
    Tray,
    /// デーモンが停止した (プロセス終了コード)
    DaemonStopped(i32),
}

/// `fxg daemon --tray` (macOS) ならメインスレッドでトレイ付きデーモンを起動する。
///
/// `Some` を返した場合は呼び出し元 (`main`) がその終了コードで終了する。
/// `None` なら通常経路 (tokio ランタイムをメインスレッドで実行) に任せる。
pub fn run_daemon_if_requested(command: &Commands) -> Option<ExitCode> {
    let Commands::Daemon(args) = command else {
        return None;
    };
    // 設定が読めない場合は通常経路に任せる (エラー表示はそちらで行う)
    let global =
        fxg_protocol::config::GlobalConfig::load(&fxg_protocol::config::process_env).ok()?;
    if !args.tray_requested(global.node.tray) {
        return None;
    }
    Some(run_daemon_with_tray(args, &global))
}

/// `fxg daemon --tray` (macOS) を実行する。デーモン停止まで戻らない。
fn run_daemon_with_tray(
    args: &DaemonArgs,
    global: &fxg_protocol::config::GlobalConfig,
) -> ExitCode {
    let config = args.daemon_config_from(global);

    // デーモン本体 (tokio) はワーカースレッドで起動する (メインスレッドは tao が占有)
    let (ready_tx, ready_rx) = mpsc::channel::<Result<ReadyPayload, String>>();
    let worker = match std::thread::Builder::new()
        .name("fxg-daemon".to_owned())
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
                    Ok(daemon) => {
                        let _ = ready_tx.send(Ok((
                            daemon.state().clone(),
                            daemon.shutdown_handle(),
                            handle,
                        )));
                        daemon.wait().await;
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
            eprintln!("fxg: failed to spawn the daemon thread: {err}");
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
    // ワーカースレッド (デーモン) の終了をメインループへ通知する
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
                // トレイはイベントループが動作し始めてから作成する必要がある
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

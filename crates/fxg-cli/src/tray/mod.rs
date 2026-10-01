//! デーモンのタスクトレイ常駐 (Windows / Linux / macOS)。
//!
//! 設計: `docs/04-agent-drivers-and-windows.md` §5.4。
//!
//! `tray-icon` (tauri) はトレイのイベントループを自前で持たないため、
//! OS ごとに「ループをどこで動かすか」が異なる点をバックエンドで吸収する:
//!
//! - **Windows** ([`windows`]): トレイを作成したスレッドで Win32
//!   メッセージループが必要なため、専用スレッドで `GetMessageW` ループを回す。
//! - **Linux** ([`linux`]): `ksni` (D-Bus) バックエンドは自前のワーカースレッドを
//!   持つためイベントループ不要。専用スレッド + チャネル駆動でメニューを操作する。
//! - **macOS** ([`macos`]): `NSStatusItem` はメインスレッドのイベントループ上で
//!   作成・操作する必要があるため、`main.rs` の早期分岐からメインスレッドで
//!   tao ループを回す (デーモン本体はワーカースレッドへ退避)。
//!
//! メニュー構築・ステータス整形・アイコン生成・アクション処理は全 OS 共通。
//! `muda` のメニュー項目は `!Send` のため、**作成したスレッド上でのみ**操作する。

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{TrayHandle, spawn};
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::run_daemon_if_requested;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{TrayHandle, spawn};

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use fxg_db::SessionFilter;
use fxg_node::daemon::{DaemonState, ShutdownHandle};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tray_icon::TrayIcon;
use tray_icon::TrayIconBuilder;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};

use crate::service::{ServiceAction, is_installed, run_sync};

/// ステータス表示の更新間隔。
const STATUS_INTERVAL: Duration = Duration::from_secs(3);
/// トレイアイコンのサイズ (px)。元画像 (192px) の約数であること。
const ICON_SIZE: u32 = 32;
/// 自動起動メニュー項目のラベル。
const AUTOSTART_LABEL: &str = "ログイン時に自動起動";
/// ステータス取得前のプレースホルダ。
const STATUS_FETCHING: &str = "状態: 取得中…";
/// セッション数取得前のプレースホルダ。
const SESSIONS_FETCHING: &str = "セッション: 取得中…";
/// 同梱アイコン (UI の PWA アイコンと共通のアセット)。
const ICON_PNG: &[u8] = include_bytes!("../../../../ui/static/icon-192.png");

/// トレイが利用するデーモンの状態。
#[derive(Clone)]
pub struct TrayContext {
    /// ノードデーモンの共有状態 (ステータス表示に使う)
    pub state: DaemonState,
    /// graceful shutdown ハンドル (「終了」メニュー)
    pub shutdown: ShutdownHandle,
}

/// デーモン → トレイスレッド / メインループへ送る更新。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrayUpdate {
    /// ステータス表示の更新
    Status(TrayStatus),
    /// 自動起動の登録状態
    Autostart(bool),
    /// トレイを閉じてループを終了する
    Close,
}

/// メニューから要求されたアクション (トレイ → デーモン)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayAction {
    /// 自動起動の登録/解除をトグルする
    ToggleAutostart,
    /// Web UI をブラウザで開く
    OpenWeb,
    /// デーモンを終了する
    Quit,
}

/// トレイ側へ更新を通知するクロージャ (起床手段は OS 別)。
pub(crate) type Notifier = Arc<dyn Fn(TrayUpdate) + Send + Sync>;

/// メニューに表示するステータス。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrayStatus {
    /// 中央サーバーへ接続中か
    central_connected: bool,
    /// 未同期 Outbox イベント件数
    unsynced: u64,
    /// 稼働中セッション数
    active_sessions: usize,
}

/// メニュー項目の ID (イベントの振り分けに使う)。
#[derive(Clone)]
struct TrayMenuIds {
    autostart: MenuId,
    web: MenuId,
    quit: MenuId,
}

/// トレイ本体とメニュー項目 (トレイを作成したスレッドだけが触れる)。
pub(crate) struct TrayUi {
    /// トレイ本体 (メニューを所有する。Drop でアイコンが消える)
    tray: TrayIcon,
    status: MenuItem,
    sessions: MenuItem,
    autostart: CheckMenuItem,
    node_name: String,
    version: &'static str,
}

impl TrayUi {
    /// メニューとアイコンを生成してトレイを表示する。
    ///
    /// **トレイを作成するスレッドから呼ぶこと** (Windows/Linux: 専用スレッド、
    /// macOS: メインスレッド)。
    pub(crate) fn new(ctx: &TrayContext, actions: UnboundedSender<TrayAction>) -> Result<Self> {
        let node_name = ctx.state.config().node_name.clone();
        let version = ctx.state.version();

        let header = MenuItem::new(format!("FlexAgent v{version} — {node_name}"), false, None);
        let status = MenuItem::new(STATUS_FETCHING, false, None);
        let sessions = MenuItem::new(SESSIONS_FETCHING, false, None);
        let autostart = CheckMenuItem::new(AUTOSTART_LABEL, true, false, None);
        let web = MenuItem::new("Web UI を開く", true, None);
        let quit = MenuItem::new("終了", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &header,
            &status,
            &sessions,
            &PredefinedMenuItem::separator(),
            &autostart,
            &web,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .context("failed to build the tray menu")?;

        let ids = TrayMenuIds {
            autostart: autostart.id().clone(),
            web: web.id().clone(),
            quit: quit.id().clone(),
        };
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == ids.autostart {
                Some(TrayAction::ToggleAutostart)
            } else if event.id == ids.web {
                Some(TrayAction::OpenWeb)
            } else if event.id == ids.quit {
                Some(TrayAction::Quit)
            } else {
                None
            };
            if let Some(action) = action {
                let _ = actions.send(action);
            }
        }));

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(build_icon(false)?)
            .with_tooltip(tooltip(&node_name, version, None))
            .with_menu_on_left_click(true)
            .with_menu_on_right_click(true)
            .build()
            .context("failed to create the tray icon")?;

        Ok(Self {
            tray,
            status,
            sessions,
            autostart,
            node_name,
            version,
        })
    }

    /// 更新を 1 件反映する。`false` を返したらトレイを閉じてループを終了する。
    pub(crate) fn apply(&mut self, update: TrayUpdate) -> bool {
        match update {
            TrayUpdate::Status(status) => {
                self.status.set_text(status_line(&status));
                self.sessions.set_text(session_line(status.active_sessions));
                let _ = self.tray.set_tooltip(Some(tooltip(
                    &self.node_name,
                    self.version,
                    Some(&status),
                )));
                match build_icon(status.central_connected) {
                    Ok(icon) => {
                        if let Err(err) = self.tray.set_icon(Some(icon)) {
                            tracing::debug!("failed to update the tray icon: {err}");
                        }
                    }
                    Err(err) => tracing::debug!("failed to render the tray icon: {err:#}"),
                }
            }
            TrayUpdate::Autostart(installed) => self.autostart.set_checked(installed),
            TrayUpdate::Close => return false,
        }
        true
    }

    /// 受信済みの更新をまとめて反映する (Windows / macOS のイベントループ用)。
    #[cfg(not(target_os = "linux"))]
    pub(crate) fn apply_pending(&mut self, updates: &mut UnboundedReceiver<TrayUpdate>) -> bool {
        while let Ok(update) = updates.try_recv() {
            if !self.apply(update) {
                return false;
            }
        }
        true
    }
}

/// トレイ常駐を有効にするか (`--tray` / `--no-tray` / `[node] tray` の優先順)。
pub(crate) fn resolve_enabled(force: bool, disabled: bool, configured: Option<bool>) -> bool {
    if disabled {
        false
    } else if force {
        true
    } else {
        configured.unwrap_or(false)
    }
}

/// ステータス収集とメニューアクション処理を行うタスクを起動する。
pub(crate) fn spawn_controller(
    handle: &tokio::runtime::Handle,
    ctx: TrayContext,
    notifier: Notifier,
    mut actions: UnboundedReceiver<TrayAction>,
) {
    handle.spawn(async move {
        let mut autostart = tokio::task::spawn_blocking(|| is_installed(false))
            .await
            .unwrap_or(false);
        notifier(TrayUpdate::Autostart(autostart));
        let mut ticker = tokio::time::interval(STATUS_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    notifier(TrayUpdate::Status(collect_status(&ctx.state).await));
                }
                action = actions.recv() => {
                    let Some(action) = action else { break };
                    match action {
                        TrayAction::ToggleAutostart => {
                            let service_action = if autostart {
                                ServiceAction::Uninstall
                            } else {
                                ServiceAction::Install
                            };
                            let result =
                                tokio::task::spawn_blocking(move || run_sync(service_action, false)).await;
                            match result {
                                Ok(Ok(())) => {}
                                Ok(Err(err)) => tracing::warn!("failed to toggle autostart: {err:#}"),
                                Err(err) => tracing::warn!("autostart task failed: {err}"),
                            }
                            autostart = tokio::task::spawn_blocking(|| is_installed(false))
                                .await
                                .unwrap_or(autostart);
                            notifier(TrayUpdate::Autostart(autostart));
                        }
                        TrayAction::OpenWeb => {
                            let url = crate::commands::local_web_url(
                                &ctx.state.config().listen_addr,
                                &ctx.state.auth_token(),
                            );
                            if let Err(err) = opener::open_browser(&url) {
                                tracing::warn!("failed to open the web UI: {err}");
                            }
                        }
                        TrayAction::Quit => {
                            ctx.shutdown.shutdown();
                            return;
                        }
                    }
                }
            }
        }
    });
}

/// デーモンの状態からトレイ表示用のスナップショットを収集する。
async fn collect_status(state: &DaemonState) -> TrayStatus {
    let unsynced = state.db().unsynced_event_count().await.unwrap_or(0);
    let active_sessions = state
        .db()
        .list_sessions(&SessionFilter {
            statuses: DaemonState::active_statuses(),
            ..SessionFilter::default()
        })
        .await
        .map(|sessions| sessions.len())
        .unwrap_or(0);
    TrayStatus {
        central_connected: state.central_connected(),
        unsynced,
        active_sessions,
    }
}

/// ステータス行 (中央サーバー接続状態 + 未同期件数)。
fn status_line(status: &TrayStatus) -> String {
    if status.central_connected {
        format!("状態: ● 中央サーバー接続中 (未同期 {})", status.unsynced)
    } else {
        "状態: ○ スタンドアロン".to_owned()
    }
}

/// 稼働中セッション数。
fn session_line(active_sessions: usize) -> String {
    format!("セッション: 稼働中 {active_sessions} 件")
}

/// ツールチップ (アイコンホバー時の表示)。
fn tooltip(node_name: &str, version: &str, status: Option<&TrayStatus>) -> String {
    match status {
        Some(status) => {
            let mode = if status.central_connected {
                "接続中"
            } else {
                "スタンドアロン"
            };
            format!(
                "FlexAgent v{version} — {node_name} ({mode} / セッション {})",
                status.active_sessions
            )
        }
        None => format!("FlexAgent v{version} — {node_name} (起動中)"),
    }
}

/// 同梱アイコンを縮小し、接続状態ドットを合成したトレイアイコンを作る。
fn build_icon(central_connected: bool) -> Result<tray_icon::Icon> {
    let rgba = render_icon_rgba(ICON_PNG, central_connected)?;
    tray_icon::Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE)
        .map_err(|err| anyhow::anyhow!("invalid tray icon image: {err}"))
}

/// PNG をデコードし `ICON_SIZE` へ縮小して状態ドットを描画する (テスト可能な純関数)。
fn render_icon_rgba(png_bytes: &[u8], central_connected: bool) -> Result<Vec<u8>> {
    let (source, width, height) = decode_png_rgba(png_bytes)?;
    let mut rgba = downsample_rgba(&source, width, height, ICON_SIZE)?;
    draw_status_dot(&mut rgba, ICON_SIZE, central_connected);
    Ok(rgba)
}

/// PNG (8bit RGB / RGBA) を RGBA8 へデコードする。
fn decode_png_rgba(png_bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder
        .read_info()
        .context("failed to decode the tray icon PNG")?;
    let buffer_size = reader
        .output_buffer_size()
        .context("the tray icon PNG is too large")?;
    let mut buffer = vec![0u8; buffer_size];
    let frame = reader
        .next_frame(&mut buffer)
        .context("failed to decode the tray icon PNG frame")?;
    let (color_type, bit_depth) = reader.output_color_type();
    let pixels = frame.width as usize * frame.height as usize;
    let rgba = match (color_type, bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => {
            buffer.truncate(pixels * 4);
            buffer
        }
        (png::ColorType::Rgb, png::BitDepth::Eight) => {
            buffer.truncate(pixels * 3);
            let (chunks, _) = buffer.as_chunks::<3>();
            chunks
                .iter()
                .flat_map(|px| [px[0], px[1], px[2], u8::MAX])
                .collect()
        }
        (color_type, bit_depth) => bail!(
            "unsupported tray icon PNG format: {color_type:?} / {bit_depth:?} \
             (8bit RGB or RGBA required)"
        ),
    };
    Ok((rgba, frame.width, frame.height))
}

/// ブロック平均による縮小 (元画像は `size` の整数倍であること)。
fn downsample_rgba(source: &[u8], width: u32, height: u32, size: u32) -> Result<Vec<u8>> {
    if !width.is_multiple_of(size) || !height.is_multiple_of(size) {
        bail!("tray icon size {width}x{height} is not a multiple of {size}");
    }
    let (block_x, block_y) = (width / size, height / size);
    let mut output = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let mut sums = [0u32; 4];
            for sy in 0..block_y {
                for sx in 0..block_x {
                    let pixel = (((y * block_y + sy) * width + x * block_x + sx) * 4) as usize;
                    for (channel, sum) in sums.iter_mut().enumerate() {
                        *sum += u32::from(source[pixel + channel]);
                    }
                }
            }
            let count = block_x * block_y;
            let target = ((y * size + x) * 4) as usize;
            for (channel, sum) in sums.iter().enumerate() {
                output[target + channel] = (sum / count) as u8;
            }
        }
    }
    Ok(output)
}

/// 右下に中央サーバー接続状態のドット (緑=接続中 / グレー=スタンドアロン) を描く。
fn draw_status_dot(rgba: &mut [u8], size: u32, central_connected: bool) {
    // #22c55e (接続中)
    const CONNECTED: [u8; 4] = [34, 197, 94, u8::MAX];
    // #9ca3af (スタンドアロン)
    const STANDALONE: [u8; 4] = [156, 163, 175, u8::MAX];
    // #0f172a (視認性のための縁取り)
    const OUTLINE: [u8; 4] = [15, 23, 42, u8::MAX];

    let color = if central_connected {
        CONNECTED
    } else {
        STANDALONE
    };
    let center = size as f32 / 2.0 - 0.5 + (size as f32 * 0.22);
    let radius = size as f32 * 0.2;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let pixel = ((y * size + x) * 4) as usize;
            if distance <= radius {
                rgba[pixel..pixel + 4].copy_from_slice(&color);
            } else if distance <= radius + 1.5 {
                rgba[pixel..pixel + 4].copy_from_slice(&OUTLINE);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(central_connected: bool, unsynced: u64, active_sessions: usize) -> TrayStatus {
        TrayStatus {
            central_connected,
            unsynced,
            active_sessions,
        }
    }

    #[test]
    fn resolve_enabled_priority() {
        // --no-tray が最優先
        assert!(!resolve_enabled(true, true, Some(true)));
        // --tray は config より優先
        assert!(resolve_enabled(true, false, Some(false)));
        assert!(resolve_enabled(true, false, None));
        // 指定が無ければ config に従う (未設定は無効)
        assert!(resolve_enabled(false, false, Some(true)));
        assert!(!resolve_enabled(false, false, Some(false)));
        assert!(!resolve_enabled(false, false, None));
    }

    #[test]
    fn status_and_session_lines() {
        assert_eq!(
            status_line(&status(true, 3, 2)),
            "状態: ● 中央サーバー接続中 (未同期 3)"
        );
        assert_eq!(status_line(&status(false, 0, 0)), "状態: ○ スタンドアロン");
        assert_eq!(session_line(0), "セッション: 稼働中 0 件");
        assert_eq!(session_line(12), "セッション: 稼働中 12 件");
    }

    #[test]
    fn tooltip_reflects_status() {
        assert_eq!(
            tooltip("home-pc", "0.1.0", Some(&status(true, 1, 2))),
            "FlexAgent v0.1.0 — home-pc (接続中 / セッション 2)"
        );
        assert_eq!(
            tooltip("home-pc", "0.1.0", Some(&status(false, 0, 0))),
            "FlexAgent v0.1.0 — home-pc (スタンドアロン / セッション 0)"
        );
        assert_eq!(
            tooltip("home-pc", "0.1.0", None),
            "FlexAgent v0.1.0 — home-pc (起動中)"
        );
    }

    #[test]
    fn icon_is_rendered_with_status_dot() {
        let connected = render_icon_rgba(ICON_PNG, true).expect("connected icon");
        let standalone = render_icon_rgba(ICON_PNG, false).expect("standalone icon");
        let expected_len = (ICON_SIZE * ICON_SIZE * 4) as usize;
        assert_eq!(connected.len(), expected_len);
        assert_eq!(standalone.len(), expected_len);
        // 右下のドット中心付近が状態色になっている (緑 / グレー)
        let dot = ((ICON_SIZE - 8) * ICON_SIZE + ICON_SIZE - 8) as usize * 4;
        assert_eq!(&connected[dot..dot + 3], &[34, 197, 94]);
        assert_eq!(&standalone[dot..dot + 3], &[156, 163, 175]);
        // 状態が違えば画像も異なる
        assert_ne!(connected, standalone);
    }

    #[test]
    fn downsample_rejects_non_multiple_size() {
        let source = vec![0u8; 100 * 100 * 4];
        assert!(downsample_rgba(&source, 100, 100, 32).is_err());
    }
}

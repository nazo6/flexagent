//! Daemon task-tray integration (Windows / Linux / macOS).
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §5.4.
//!
//! `tray-icon` (tauri) does not own a tray event loop, so the backend absorbs
//! the platform difference in where the loop runs:
//!
//! - **Windows** ([`windows`]): the thread that created the tray needs a Win32
//!   message loop, so `GetMessageW` runs on a dedicated thread.
//! - **Linux** ([`linux`]): the `ksni` (D-Bus) backend owns its worker thread,
//!   so no event loop is needed. A dedicated thread driven by a channel
//!   manipulates the menu.
//! - **macOS** ([`macos`]): `NSStatusItem` must be created and driven on the
//!   main thread's event loop, so the early branch in `main.rs` runs the tao
//!   loop on the main thread (the daemon body moves to a worker thread).
//!
//! Menu construction, status formatting, icon rendering and action handling
//! are shared by all platforms. `muda` menu items are `!Send`, so they are
//! touched **only on the thread that created them**.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{TrayHandle, spawn};
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::run_node_if_requested;
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

use crate::service::{ServiceAction, ServiceTarget, is_installed, run_sync};

/// Interval at which the status display is refreshed.
const STATUS_INTERVAL: Duration = Duration::from_secs(3);
/// Tray icon size in px. Must divide the source image size (192px).
const ICON_SIZE: u32 = 32;
/// Label of the auto-start menu item.
const AUTOSTART_LABEL: &str = "Start at login";
/// Placeholder shown before the status is fetched.
const STATUS_FETCHING: &str = "Status: fetching…";
/// Placeholder shown before the session count is fetched.
const SESSIONS_FETCHING: &str = "Sessions: fetching…";
/// Bundled icon (the same asset as the UI's PWA icon).
const ICON_PNG: &[u8] = include_bytes!("../../../../ui/static/icon-192.png");

/// Daemon state the tray relies on.
#[derive(Clone)]
pub struct TrayContext {
    /// Shared node daemon state (used for the status display)
    pub state: DaemonState,
    /// Graceful shutdown handle (the "Quit" menu item)
    pub shutdown: ShutdownHandle,
}

/// Update sent from the daemon to the tray thread / main loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrayUpdate {
    /// Status display refresh
    Status(TrayStatus),
    /// Whether auto-start is registered
    Autostart(bool),
    /// Close the tray and stop the loop
    Close,
}

/// Action requested from the menu (tray → daemon).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayAction {
    /// Toggle the auto-start registration
    ToggleAutostart,
    /// Open the Web UI in a browser
    OpenWeb,
    /// Stop the daemon
    Quit,
}

/// Closure that notifies the tray side (the wake-up mechanism is per OS).
pub(crate) type Notifier = Arc<dyn Fn(TrayUpdate) + Send + Sync>;

/// Status shown in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrayStatus {
    /// Whether the hub is connected
    hub_connected: bool,
    /// Number of unsynced Outbox events
    unsynced: u64,
    /// Number of active sessions
    active_sessions: usize,
}

/// Menu item IDs (used to dispatch events).
#[derive(Clone)]
struct TrayMenuIds {
    autostart: MenuId,
    web: MenuId,
    quit: MenuId,
}

/// Tray body and menu items (only the thread that created the tray may touch
/// them).
pub(crate) struct TrayUi {
    /// Tray body (owns the menu; dropping it removes the icon)
    tray: TrayIcon,
    status: MenuItem,
    sessions: MenuItem,
    autostart: CheckMenuItem,
    node_name: String,
    version: &'static str,
}

impl TrayUi {
    /// Create the menu and icon and show the tray.
    ///
    /// **Must be called from the thread that creates the tray** (Windows and
    /// Linux: a dedicated thread; macOS: the main thread).
    pub(crate) fn new(ctx: &TrayContext, actions: UnboundedSender<TrayAction>) -> Result<Self> {
        let node_name = ctx.state.config().node_name.clone();
        let version = ctx.state.version();

        let header = MenuItem::new(format!("FlexAgent v{version} — {node_name}"), false, None);
        let status = MenuItem::new(STATUS_FETCHING, false, None);
        let sessions = MenuItem::new(SESSIONS_FETCHING, false, None);
        let autostart = CheckMenuItem::new(AUTOSTART_LABEL, true, false, None);
        let web = MenuItem::new("Open Web UI", true, None);
        let quit = MenuItem::new("Quit", true, None);
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
                fxg_protocol::discard_with!(actions.send(action), "forward a tray action");
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

    /// Apply one update. Returning `false` closes the tray and stops the loop.
    pub(crate) fn apply(&mut self, update: TrayUpdate) -> bool {
        match update {
            TrayUpdate::Status(status) => {
                self.status.set_text(status_line(&status));
                self.sessions.set_text(session_line(status.active_sessions));
                fxg_protocol::discard_with!(
                    self.tray.set_tooltip(Some(tooltip(
                        &self.node_name,
                        self.version,
                        Some(&status),
                    ))),
                    "update the tray tooltip"
                );
                match build_icon(status.hub_connected) {
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

    /// Apply every pending update (used by the Windows / macOS event loop).
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

/// Whether tray residency is enabled (priority: `--tray` / `--no-tray` /
/// `[node] tray`).
pub(crate) fn resolve_enabled(force: bool, disabled: bool, configured: Option<bool>) -> bool {
    if disabled {
        false
    } else if force {
        true
    } else {
        configured.unwrap_or(false)
    }
}

/// Spawn the task that collects status and handles menu actions.
pub(crate) fn spawn_controller(
    handle: &tokio::runtime::Handle,
    ctx: TrayContext,
    notifier: Notifier,
    mut actions: UnboundedReceiver<TrayAction>,
) {
    handle.spawn(async move {
        let mut autostart = tokio::task::spawn_blocking(|| is_installed(ServiceTarget::Node))
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
                                tokio::task::spawn_blocking(move || run_sync(service_action, ServiceTarget::Node)).await;
                            match result {
                                Ok(Ok(())) => {}
                                Ok(Err(err)) => tracing::warn!("failed to toggle autostart: {err:#}"),
                                Err(err) => tracing::warn!("autostart task failed: {err}"),
                            }
                            autostart = tokio::task::spawn_blocking(|| is_installed(ServiceTarget::Node))
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

/// Collect a snapshot for the tray display from the daemon state.
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
        hub_connected: state.hub_connected(),
        unsynced,
        active_sessions,
    }
}

/// Status line (hub connection state + unsynced count).
fn status_line(status: &TrayStatus) -> String {
    if status.hub_connected {
        format!("Status: ● connected to hub ({} unsynced)", status.unsynced)
    } else {
        "Status: ○ standalone".to_owned()
    }
}

/// Number of active sessions.
fn session_line(active_sessions: usize) -> String {
    format!("Sessions: {active_sessions} active")
}

/// Tooltip (shown on icon hover).
fn tooltip(node_name: &str, version: &str, status: Option<&TrayStatus>) -> String {
    match status {
        Some(status) => {
            let mode = if status.hub_connected {
                "connected"
            } else {
                "standalone"
            };
            format!(
                "FlexAgent v{version} — {node_name} ({mode} / {} sessions)",
                status.active_sessions
            )
        }
        None => format!("FlexAgent v{version} — {node_name} (starting)"),
    }
}

/// Build the tray icon by downscaling the bundled icon and compositing the
/// connection-state dot.
fn build_icon(hub_connected: bool) -> Result<tray_icon::Icon> {
    let rgba = render_icon_rgba(ICON_PNG, hub_connected)?;
    tray_icon::Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE)
        .map_err(|err| anyhow::anyhow!("invalid tray icon image: {err}"))
}

/// Decode the PNG, downscale it to `ICON_SIZE` and draw the state dot (a pure
/// function so it can be tested).
fn render_icon_rgba(png_bytes: &[u8], hub_connected: bool) -> Result<Vec<u8>> {
    let (source, width, height) = decode_png_rgba(png_bytes)?;
    let mut rgba = downsample_rgba(&source, width, height, ICON_SIZE)?;
    draw_status_dot(&mut rgba, ICON_SIZE, hub_connected);
    Ok(rgba)
}

/// Decode a PNG (8-bit RGB / RGBA) into RGBA8.
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

/// Downscale by block averaging (the source dimensions must be a multiple of
/// `size`).
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

/// Draw the hub connection-state dot in the bottom-right corner
/// (green = connected / grey = standalone).
fn draw_status_dot(rgba: &mut [u8], size: u32, hub_connected: bool) {
    // #22c55e (connected)
    const CONNECTED: [u8; 4] = [34, 197, 94, u8::MAX];
    // #9ca3af (standalone)
    const STANDALONE: [u8; 4] = [156, 163, 175, u8::MAX];
    // #0f172a (outline for visibility)
    const OUTLINE: [u8; 4] = [15, 23, 42, u8::MAX];

    let color = if hub_connected { CONNECTED } else { STANDALONE };
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

    fn status(hub_connected: bool, unsynced: u64, active_sessions: usize) -> TrayStatus {
        TrayStatus {
            hub_connected,
            unsynced,
            active_sessions,
        }
    }

    #[test]
    fn resolve_enabled_priority() {
        // --no-tray takes the highest priority
        assert!(!resolve_enabled(true, true, Some(true)));
        // --tray overrides the config
        assert!(resolve_enabled(true, false, Some(false)));
        assert!(resolve_enabled(true, false, None));
        // Without a flag, follow the config (unset means disabled)
        assert!(resolve_enabled(false, false, Some(true)));
        assert!(!resolve_enabled(false, false, Some(false)));
        assert!(!resolve_enabled(false, false, None));
    }

    #[test]
    fn status_and_session_lines() {
        assert_eq!(
            status_line(&status(true, 3, 2)),
            "Status: ● connected to hub (3 unsynced)"
        );
        assert_eq!(status_line(&status(false, 0, 0)), "Status: ○ standalone");
        assert_eq!(session_line(0), "Sessions: 0 active");
        assert_eq!(session_line(12), "Sessions: 12 active");
    }

    #[test]
    fn tooltip_reflects_status() {
        assert_eq!(
            tooltip("home-pc", "0.1.0", Some(&status(true, 1, 2))),
            "FlexAgent v0.1.0 — home-pc (connected / 2 sessions)"
        );
        assert_eq!(
            tooltip("home-pc", "0.1.0", Some(&status(false, 0, 0))),
            "FlexAgent v0.1.0 — home-pc (standalone / 0 sessions)"
        );
        assert_eq!(
            tooltip("home-pc", "0.1.0", None),
            "FlexAgent v0.1.0 — home-pc (starting)"
        );
    }

    #[test]
    fn icon_is_rendered_with_status_dot() {
        let connected = render_icon_rgba(ICON_PNG, true).expect("connected icon");
        let standalone = render_icon_rgba(ICON_PNG, false).expect("standalone icon");
        let expected_len = (ICON_SIZE * ICON_SIZE * 4) as usize;
        assert_eq!(connected.len(), expected_len);
        assert_eq!(standalone.len(), expected_len);
        // The pixels near the center of the bottom-right dot use the state color
        // (green / grey)
        let dot = ((ICON_SIZE - 8) * ICON_SIZE + ICON_SIZE - 8) as usize * 4;
        assert_eq!(&connected[dot..dot + 3], &[34, 197, 94]);
        assert_eq!(&standalone[dot..dot + 3], &[156, 163, 175]);
        // A different state produces a different image
        assert_ne!(connected, standalone);
    }

    #[test]
    fn downsample_rejects_non_multiple_size() {
        let source = vec![0u8; 100 * 100 * 4];
        assert!(downsample_rgba(&source, 100, 100, 32).is_err());
    }
}

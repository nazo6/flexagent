//! 内蔵TUI (`AcpTui` モード) と純正TUI Attach の起動。
//!
//! 設計: `docs/05-cli-and-pwa-ui.md` §1 (`fxg run` / `fxg attach`)。
//!
//! - **内蔵TUI**: `ratatui` + `crossterm` でセッションのイベントストリーム
//!   (`AttachSession` 以降の `EventBatch` / `LiveStreamDelta`) を描画し、
//!   プロンプト送信と承認応答を同じ IPC 接続から行う。
//! - **純正TUI Attach**: [`AttachMode::NativeOpenCodeAttach`] の場合は
//!   デーモン管理下の `opencode2 serve` へ `opencode2 run --server <url>
//!   --session <id>` で接続し、100% 純正の TUI をそのまま表示する。

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fxg_protocol::common::{PermissionOption, SessionControlAction, SessionStatus};
use fxg_protocol::events::UnifiedEventPayload;
use fxg_protocol::ipc::{AttachMode, IpcClientMessage, IpcResult, IpcServerMessage};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block as WidgetBlock, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::client::{DaemonClient, short_id};

/// `fxg attach` / `fxg run` の共通アタッチ処理。
///
/// アタッチモードに応じて内蔵TUI または純正TUI を起動する。
pub async fn attach(client: &mut DaemonClient, session_id: &str) -> Result<()> {
    let attach_mode = client
        .attach_session(session_id, None)
        .await
        .context("セッションへアタッチできません")?;
    match attach_mode {
        AttachMode::AcpTui => run_internal(client, session_id).await,
        AttachMode::NativeOpenCodeAttach {
            server_url,
            session_id,
            env,
        } => run_native(&server_url, &session_id, &env),
    }
}

/// デーモン管理下の `opencode2 serve` へ純正TUIを接続する。
///
/// デーモンがサーバーを保持し続けるため、CLI が終了してもセッションと
/// イベント記録は継続する (`fxg attach` でいつでも再接続できる)。
fn run_native(server_url: &str, session_id: &str, env: &[(String, String)]) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let program = fxg_pty::resolve_command("opencode2", &cwd)
        .context("opencode2 が見つかりません (OpenCode2 純正TUIを起動できません)")?;
    println!("opencode2 純正TUIでアタッチします (server: {server_url}, session: {session_id})");
    let status = std::process::Command::new(program)
        .arg("run")
        .arg("--server")
        .arg(server_url)
        .arg("--session")
        .arg(session_id)
        .envs(env.iter().cloned())
        .status()
        .context("failed to start opencode2")?;
    if !status.success() {
        bail!("opencode2 が異常終了しました: {status}");
    }
    Ok(())
}

/// ターミナルの raw mode / alternate screen を確実に復元するガード。
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new() -> Result<Self> {
        enable_raw_mode().context("failed to enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
        let terminal =
            Terminal::new(CrosstermBackend::new(stdout)).context("failed to create terminal")?;
        Ok(Self { terminal })
    }

    fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

/// 表示ブロック (トランスクリプトの 1 要素)。
#[derive(Debug)]
enum Block {
    /// ユーザーメッセージ
    User(String),
    /// エージェント返答 (`complete = false` はストリーミング中)
    Agent { id: String, text: String },
    /// 思考プロセス
    Thought {
        id: String,
        text: String,
        complete: bool,
    },
    /// ツール呼び出し (`tool_call_id` 単位で状態を更新する)
    Tool { title: String, status: String },
    /// システム・エラー表示
    System(String),
    /// 承認ダイアログの履歴
    Permission(String),
}

/// 内蔵TUIのアプリケーション状態。
struct App {
    session_id: String,
    status: SessionStatus,
    blocks: Vec<Block>,
    input: String,
    /// 承認待ちリクエスト (request_id, summary, options)
    pending_permission: Option<(String, String, Vec<PermissionOption>)>,
    /// 最新へ追従するか (ユーザーがスクロールすると解除)
    follow: bool,
    /// 追従解除中のスクロール位置 (上からの行数)
    scroll: u16,
    quit: bool,
    /// デーモンとの接続が切れたか
    disconnected: bool,
}

impl App {
    fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_owned(),
            status: SessionStatus::Idle,
            blocks: Vec::new(),
            input: String::new(),
            pending_permission: None,
            follow: true,
            scroll: 0,
            quit: false,
            disconnected: false,
        }
    }

    /// セッションイベントを表示ブロックへ反映する。
    fn push_event(&mut self, payload: &UnifiedEventPayload) {
        match payload {
            UnifiedEventPayload::UserMessage { text, .. } => {
                self.blocks.push(Block::User(text.clone()));
            }
            UnifiedEventPayload::AgentMessage {
                message_id, text, ..
            } => {
                self.upsert_agent(message_id, text);
            }
            UnifiedEventPayload::AgentThought {
                thought_id, text, ..
            } => {
                self.upsert_thought(thought_id, text, true);
            }
            UnifiedEventPayload::ToolCall {
                tool_call_id,
                title,
                status,
                ..
            } => {
                self.upsert_tool(tool_call_id, title, status);
            }
            UnifiedEventPayload::StatusChanged { status, .. } => {
                self.status = *status;
            }
            UnifiedEventPayload::SessionReverted {
                target_node_seq,
                restored_files,
                removed_files,
                ..
            } => {
                self.blocks.push(Block::System(format!(
                    "Revert: node_seq={target_node_seq} へ復元 (restored={restored_files}, removed={removed_files})"
                )));
            }
            UnifiedEventPayload::PermissionRequest {
                request_id,
                tool_name,
                summary,
                options,
                ..
            } => {
                self.blocks.push(Block::Permission(format!(
                    "承認待ち: [{tool_name}] {summary}"
                )));
                self.pending_permission =
                    Some((request_id.clone(), summary.clone(), options.clone()));
            }
            UnifiedEventPayload::PermissionResolved {
                request_id,
                selected_option_id,
                ..
            } => {
                if let Some((pending_id, ..)) = &self.pending_permission
                    && pending_id == request_id
                {
                    self.pending_permission = None;
                }
                self.blocks.push(Block::Permission(format!(
                    "承認解決: {request_id} → {selected_option_id}"
                )));
            }
            UnifiedEventPayload::SessionCreated { title, .. } => {
                self.blocks
                    .push(Block::System(format!("セッション開始: {title}")));
            }
            UnifiedEventPayload::SessionAgentBound { agent_session_id } => {
                self.blocks.push(Block::System(format!(
                    "エージェント接続: {agent_session_id}"
                )));
            }
            UnifiedEventPayload::TerminalOutput { .. }
            | UnifiedEventPayload::TerminalInput { .. }
            | UnifiedEventPayload::PlanUpdate { .. }
            | UnifiedEventPayload::SessionTitleChanged { .. }
            | UnifiedEventPayload::CapabilitiesUpdated { .. }
            | UnifiedEventPayload::BootstrapLog { .. } => {}
        }
    }

    /// ストリーミング差分を表示ブロックへ反映する。
    fn push_delta(&mut self, delta: &fxg_protocol::common::StreamDeltaPayload) {
        use fxg_protocol::common::StreamDeltaPayload;
        match delta {
            StreamDeltaPayload::AgentMessageDelta {
                message_id,
                text_delta,
            } => {
                match self.blocks.iter_mut().rev().find_map(|block| match block {
                    Block::Agent { id, text } if id == message_id => Some(text),
                    _ => None,
                }) {
                    Some(text) => text.push_str(text_delta),
                    None => self.blocks.push(Block::Agent {
                        id: message_id.clone(),
                        text: text_delta.clone(),
                    }),
                }
            }
            StreamDeltaPayload::AgentThoughtDelta {
                thought_id,
                text_delta,
            } => {
                let existing = self.blocks.iter_mut().rev().find_map(|block| match block {
                    Block::Thought { id, text, .. } if id == thought_id => Some(text),
                    _ => None,
                });
                match existing {
                    Some(text) => text.push_str(text_delta),
                    None => self.blocks.push(Block::Thought {
                        id: thought_id.clone(),
                        text: text_delta.clone(),
                        complete: false,
                    }),
                }
            }
            StreamDeltaPayload::ToolCallProgress {
                tool_call_id,
                status,
                ..
            } => {
                for block in self.blocks.iter_mut().rev() {
                    if let Block::Tool {
                        title,
                        status: current,
                    } = block
                        && title.contains(tool_call_id)
                    {
                        *current = status.clone();
                        break;
                    }
                }
            }
            StreamDeltaPayload::TerminalOutputDelta { .. } => {}
        }
    }

    /// 完成したエージェントメッセージでブロックを更新する。
    fn upsert_agent(&mut self, message_id: &str, text: &str) {
        for block in self.blocks.iter_mut().rev() {
            if let Block::Agent { id, text: current } = block
                && id == message_id
            {
                *current = text.to_owned();
                return;
            }
        }
        self.blocks.push(Block::Agent {
            id: message_id.to_owned(),
            text: text.to_owned(),
        });
    }

    /// 思考ブロックを更新・追加する。
    fn upsert_thought(&mut self, thought_id: &str, text: &str, complete: bool) {
        for block in self.blocks.iter_mut().rev() {
            if let Block::Thought {
                id,
                text: current,
                complete: current_complete,
            } = block
                && id == thought_id
            {
                *current = text.to_owned();
                *current_complete = complete;
                return;
            }
        }
        self.blocks.push(Block::Thought {
            id: thought_id.to_owned(),
            text: text.to_owned(),
            complete,
        });
    }

    /// ツール呼び出しブロックを更新・追加する。
    fn upsert_tool(&mut self, tool_call_id: &str, title: &str, status: &str) {
        let marker = format!("[{tool_call_id}]");
        for block in self.blocks.iter_mut() {
            if let Block::Tool {
                title: current_title,
                status: current_status,
            } = block
                && current_title.starts_with(&marker)
            {
                *current_title = format!("{marker} {title}");
                *current_status = status.to_owned();
                return;
            }
        }
        self.blocks.push(Block::Tool {
            title: format!("{marker} {title}"),
            status: status.to_owned(),
        });
    }
}

/// 内蔵TUIを起動する (Ctrl+C / Ctrl+Q でデタッチ)。
async fn run_internal(client: &mut DaemonClient, session_id: &str) -> Result<()> {
    let mut app = App::new(session_id);
    let mut terminal = TerminalGuard::new()?;
    let mut input_rx = spawn_input_task();

    // 初回描画 (空の画面を避ける)
    terminal
        .terminal_mut()
        .draw(|frame| draw(frame, &mut app))
        .context("failed to draw")?;

    let mut outgoing: Vec<IpcClientMessage> = Vec::new();
    loop {
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        tokio::select! {
            event = input_rx.recv() => {
                match event {
                    Some(event) => handle_input(&mut app, event, &mut outgoing),
                    None => app.quit = true,
                }
            }
            message = client.next_message() => {
                match message {
                    Ok(Some(message)) => handle_message(&mut app, message),
                    Ok(None) => {
                        app.disconnected = true;
                        app.quit = true;
                    }
                    Err(err) => {
                        app.blocks.push(Block::System(format!("IPC エラー: {err:#}")));
                        app.disconnected = true;
                        app.quit = true;
                    }
                }
            }
            _ = ticker.tick() => {}
        }

        for command in outgoing.drain(..) {
            // 空の `command_id` はデーモン側の重複排除で潰れるため、送信直前に採番する
            let command = with_command_id(command, client.next_command_id());
            if let Err(err) = client.send_stream_command(command).await {
                app.blocks.push(Block::System(format!("送信失敗: {err:#}")));
            }
        }

        terminal
            .terminal_mut()
            .draw(|frame| draw(frame, &mut app))
            .context("failed to draw")?;
        if app.quit {
            break;
        }
    }

    if app.disconnected {
        println!("デーモンとの接続が切れました (セッションは継続しています)");
    }
    Ok(())
}

/// 送信直前に相関IDを採番する (デーモンの冪等性判定に使われる)。
fn with_command_id(message: IpcClientMessage, command_id: String) -> IpcClientMessage {
    match message {
        IpcClientMessage::SendPrompt {
            session_id,
            text,
            client_source,
            ..
        } => IpcClientMessage::SendPrompt {
            command_id,
            session_id,
            text,
            client_source,
        },
        IpcClientMessage::RespondPermission {
            session_id,
            request_id,
            selected_option_id,
            resolved_by,
            ..
        } => IpcClientMessage::RespondPermission {
            command_id,
            session_id,
            request_id,
            selected_option_id,
            resolved_by,
        },
        IpcClientMessage::ControlSession {
            session_id, action, ..
        } => IpcClientMessage::ControlSession {
            command_id,
            session_id,
            action,
        },
        other => other,
    }
}

/// 入力イベントを処理し、必要なら送信コマンドを積む。
fn handle_input(app: &mut App, event: Event, outgoing: &mut Vec<IpcClientMessage>) {
    let Event::Key(key) = event else {
        return;
    };
    // Windows では Press 以外のイベントも届くため無視する
    if key.kind != KeyEventKind::Press {
        return;
    }

    // 承認ダイアログ表示中は y/a/n で応答する
    if let Some((request_id, _, options)) = app.pending_permission.clone() {
        if let Some(option_id) = permission_decision(&key, &options) {
            app.pending_permission = None;
            outgoing.push(IpcClientMessage::RespondPermission {
                command_id: String::new(),
                session_id: app.session_id.clone(),
                request_id,
                selected_option_id: option_id,
                resolved_by: "cli".to_owned(),
            });
            return;
        }
        if key.code == KeyCode::Esc {
            app.pending_permission = None;
            return;
        }
    }

    match key.code {
        KeyCode::Char('c' | 'q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.quit = true;
        }
        KeyCode::Enter => {
            let text = app.input.trim().to_owned();
            if text.is_empty() {
                return;
            }
            app.input.clear();
            app.blocks.push(Block::User(text.clone()));
            app.follow = true;
            outgoing.push(IpcClientMessage::SendPrompt {
                command_id: String::new(),
                session_id: app.session_id.clone(),
                text,
                client_source: "cli".to_owned(),
            });
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Esc => {
            // 実行中ターンの中断
            app.blocks
                .push(Block::System("中断を要求しました".to_owned()));
            outgoing.push(IpcClientMessage::ControlSession {
                command_id: String::new(),
                session_id: app.session_id.clone(),
                action: SessionControlAction::Cancel,
            });
        }
        KeyCode::Char(ch) => {
            if !key.modifiers.contains(KeyModifiers::CONTROL) {
                app.input.push(ch);
            }
        }
        KeyCode::Up => scroll_up(app, 1),
        KeyCode::Down => scroll_down(app, 1),
        KeyCode::PageUp => scroll_up(app, 10),
        KeyCode::PageDown => scroll_down(app, 10),
        KeyCode::End => app.follow = true,
        _ => {}
    }
}

/// 承認キーを `option_id` へ解決する (`y` = 許可 / `a` = 常に許可 / `n` = 却下)。
fn permission_decision(key: &KeyEvent, options: &[PermissionOption]) -> Option<String> {
    let kind = match key.code {
        KeyCode::Char('y') => "allow_once",
        KeyCode::Char('a') => "allow_always",
        KeyCode::Char('n') => "reject",
        _ => return None,
    };
    let option = options
        .iter()
        .find(|option| option.kind.starts_with(kind))
        // `allow_once` が無いドライバ向けフォールバック
        .or_else(|| {
            options
                .iter()
                .find(|option| (kind == "reject") == PermissionOption::is_reject_kind(&option.kind))
        })?;
    Some(option.option_id.clone())
}

/// 上へスクロールする (追従を解除する)。
fn scroll_up(app: &mut App, lines: u16) {
    app.follow = false;
    app.scroll = app.scroll.saturating_sub(lines);
}

/// 下へスクロールする。
fn scroll_down(app: &mut App, lines: u16) {
    app.scroll = app.scroll.saturating_add(lines);
}

/// デーモンからのメッセージを表示状態へ反映する。
fn handle_message(app: &mut App, message: IpcServerMessage) {
    match message {
        IpcServerMessage::EventBatch { events, .. } => {
            for event in events {
                app.push_event(&event.payload);
            }
        }
        IpcServerMessage::LiveStreamDelta { delta, .. } => app.push_delta(&delta),
        IpcServerMessage::Result { result, .. } => match result {
            IpcResult::Ack { .. } | IpcResult::CommandAccepted { .. } => {}
            other => app.blocks.push(Block::System(format!("{other:?}"))),
        },
        IpcServerMessage::Error { code, message, .. } => {
            app.blocks
                .push(Block::System(format!("エラー: {code}: {message}")));
        }
    }
}

/// crossterm の入力イベントを専用スレッドで読み続ける。
fn spawn_input_task() -> tokio::sync::mpsc::UnboundedReceiver<Event> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        loop {
            match crossterm::event::poll(Duration::from_millis(100)) {
                Ok(true) => match crossterm::event::read() {
                    Ok(event) => {
                        if tx.send(event).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                },
                Ok(false) => continue,
                Err(_) => return,
            }
        }
    });
    rx
}

/// 画面全体を描画する。
fn draw(frame: &mut ratatui::Frame<'_>, app: &mut App) {
    let area = frame.area();
    let show_permission = app.pending_permission.is_some();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, app, chunks[0]);
    draw_transcript(frame, app, chunks[1]);
    draw_input(frame, app, chunks[2]);
    draw_footer(frame, chunks[3]);

    if show_permission {
        draw_permission(frame, app, area);
    }
}

/// ヘッダ (セッションID・状態) を描画する。
fn draw_header(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let status_style = match app.status {
        SessionStatus::Running => Style::default().fg(Color::Yellow),
        SessionStatus::WaitingPermission => Style::default().fg(Color::Magenta),
        SessionStatus::Error => Style::default().fg(Color::Red),
        SessionStatus::Idle => Style::default().fg(Color::Green),
        _ => Style::default().fg(Color::DarkGray),
    };
    let line = Line::from(vec![
        Span::styled("fxg ", Style::default().fg(Color::Cyan)),
        Span::raw(short_id(&app.session_id)),
        Span::raw("  "),
        Span::styled(app.status.to_string(), status_style),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// トランスクリプト (会話・ツール・承認) を描画する。
fn draw_transcript(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let width = area.width.saturating_sub(2).max(8) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for block in &app.blocks {
        match block {
            Block::User(text) => push_wrapped(
                &mut lines,
                "You ▸ ",
                text,
                Style::default().fg(Color::Cyan),
                width,
            ),
            Block::Agent { text, .. } => push_wrapped(
                &mut lines,
                "Agent ▸ ",
                text,
                Style::default().fg(Color::White),
                width,
            ),
            Block::Thought { text, complete, .. } => {
                let mut style = Style::default().fg(Color::DarkGray);
                if !complete {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                push_wrapped(&mut lines, "Think ▸ ", text, style, width);
            }
            Block::Tool { title, status } => {
                let color = match status.as_str() {
                    "completed" => Color::Green,
                    "failed" => Color::Red,
                    _ => Color::Yellow,
                };
                push_wrapped(
                    &mut lines,
                    "Tool ▸ ",
                    &format!("{title} [{status}]"),
                    Style::default().fg(color),
                    width,
                );
            }
            Block::System(text) => push_wrapped(
                &mut lines,
                "· ",
                text,
                Style::default().fg(Color::Blue),
                width,
            ),
            Block::Permission(text) => push_wrapped(
                &mut lines,
                "! ",
                text,
                Style::default().fg(Color::Magenta),
                width,
            ),
        }
        lines.push(Line::from(""));
    }

    let total = lines.len() as u16;
    let viewport = area.height.saturating_sub(2);
    let offset = if app.follow {
        total.saturating_sub(viewport)
    } else {
        app.scroll.min(total.saturating_sub(viewport))
    };
    app.scroll = offset;

    let paragraph = Paragraph::new(Text::from(lines))
        .block(WidgetBlock::default().borders(Borders::ALL))
        .scroll((offset, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// 入力欄を描画する。
fn draw_input(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let paragraph = Paragraph::new(app.input.as_str())
        .block(
            WidgetBlock::default()
                .borders(Borders::ALL)
                .title(" プロンプト (Enter 送信 / Esc 中断) "),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// フッター (キー操作) を描画する。
fn draw_footer(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let line = Line::from(Span::styled(
        "Ctrl+C デタッチ  ↑/↓ スクロール  End 最新へ追従",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(Paragraph::new(line), area);
}

/// 承認ダイアログを描画する。
fn draw_permission(frame: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let Some((request_id, summary, options)) = &app.pending_permission else {
        return;
    };
    let popup = centered_rect(70, 7, area);
    frame.render_widget(Clear, popup);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("承認リクエスト ", Style::default().fg(Color::Magenta)),
            Span::raw(request_id.clone()),
        ]),
        Line::from(""),
        Line::from(summary.clone()),
        Line::from(""),
        Line::from(vec![
            Span::styled("y", Style::default().fg(Color::Green)),
            Span::raw(format!(
                ": {}  ",
                option_name(options, "allow_once", "許可")
            )),
            Span::styled("a", Style::default().fg(Color::Green)),
            Span::raw(format!(
                ": {}  ",
                option_name(options, "allow_always", "常に許可")
            )),
            Span::styled("n", Style::default().fg(Color::Red)),
            Span::raw(format!(": {}  ", option_name(options, "reject", "却下"))),
            Span::styled("Esc", Style::default().fg(Color::DarkGray)),
            Span::raw(": 閉じる"),
        ]),
    ];
    lines.truncate(6);
    let paragraph = Paragraph::new(Text::from(lines))
        .block(
            WidgetBlock::default()
                .borders(Borders::ALL)
                .title(" 承認待ち ")
                .style(Style::default().bg(Color::Black)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

/// 指定した種別の選択肢名を返す (該当なしはフォールバック名)。
fn option_name(options: &[PermissionOption], kind: &str, fallback: &str) -> String {
    options
        .iter()
        .find(|option| option.kind.starts_with(kind))
        .map(|option| option.name.clone())
        .unwrap_or_else(|| fallback.to_owned())
}

/// 画面中央の矩形を返す。
fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height.saturating_sub(height) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

/// プレフィックス付きで表示幅を考慮して折り返した行を追加する。
fn push_wrapped(
    lines: &mut Vec<Line<'static>>,
    prefix: &str,
    text: &str,
    style: Style,
    width: usize,
) {
    for line in wrap_with_prefix(prefix, text, width) {
        lines.push(Line::from(Span::styled(line, style)));
    }
}

/// プレフィックス付きで表示幅 (CJK 全角 = 2) を考慮して折り返す。
///
/// 折り返し 2 行目以降はプレフィックスと同じ幅のインデントを付ける。
/// プレフィックスは先頭行にのみ表示する。
fn wrap_with_prefix(prefix: &str, text: &str, width: usize) -> Vec<String> {
    let prefix_width = UnicodeWidthStr::width(prefix);
    let width = width.max(prefix_width + 1);
    let indent = " ".repeat(prefix_width);
    let mut out = Vec::new();
    let mut current = prefix.to_owned();
    let mut current_width = prefix_width;

    for (line_index, raw_line) in text.split('\n').enumerate() {
        if line_index > 0 {
            out.push(std::mem::replace(&mut current, indent.clone()));
            current_width = prefix_width;
        }
        // 現在行に本文が入ったか (折り返し判定用)
        let mut has_content = false;
        for word in raw_line.split_inclusive(' ') {
            let word_width = UnicodeWidthStr::width(word);
            if has_content && current_width + word_width > width {
                out.push(std::mem::replace(&mut current, indent.clone()));
                current_width = prefix_width;
            }
            current.push_str(word);
            current_width += word_width;
            has_content = true;
        }
    }
    out.push(current);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_decision_maps_keys_to_option_ids() {
        let options = vec![
            PermissionOption {
                option_id: "allow_once".to_owned(),
                name: "Allow".to_owned(),
                kind: "allow_once".to_owned(),
            },
            PermissionOption {
                option_id: "allow_always".to_owned(),
                name: "Always".to_owned(),
                kind: "allow_always".to_owned(),
            },
            PermissionOption {
                option_id: "reject_once".to_owned(),
                name: "Reject".to_owned(),
                kind: "reject_once".to_owned(),
            },
        ];
        let key = |ch| KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
        assert_eq!(
            permission_decision(&key('y'), &options).as_deref(),
            Some("allow_once")
        );
        assert_eq!(
            permission_decision(&key('a'), &options).as_deref(),
            Some("allow_always")
        );
        assert_eq!(
            permission_decision(&key('n'), &options).as_deref(),
            Some("reject_once")
        );
        assert!(permission_decision(&key('x'), &options).is_none());
    }

    #[test]
    fn app_applies_events_and_deltas() {
        let mut app = App::new("0195f0ab-0000-7000-8000-000000000000");
        app.push_event(&UnifiedEventPayload::UserMessage {
            text: "hello".to_owned(),
            attachments: vec![],
            client_source: "cli".to_owned(),
            snapshot_tree_hash: None,
        });
        app.push_delta(
            &fxg_protocol::common::StreamDeltaPayload::AgentMessageDelta {
                message_id: "m1".to_owned(),
                text_delta: "he".to_owned(),
            },
        );
        app.push_delta(
            &fxg_protocol::common::StreamDeltaPayload::AgentMessageDelta {
                message_id: "m1".to_owned(),
                text_delta: "llo".to_owned(),
            },
        );
        app.push_event(&UnifiedEventPayload::AgentMessage {
            message_id: "m1".to_owned(),
            text: "hello".to_owned(),
            is_complete: true,
        });
        app.push_event(&UnifiedEventPayload::ToolCall {
            tool_call_id: "call_1".to_owned(),
            title: "echo hi".to_owned(),
            kind: "execute".to_owned(),
            status: "in_progress".to_owned(),
            locations: vec![],
            diff: None,
            raw_output: None,
        });
        app.push_event(&UnifiedEventPayload::StatusChanged {
            status: SessionStatus::Running,
            error_message: None,
        });

        assert_eq!(app.status, SessionStatus::Running);
        assert_eq!(app.blocks.len(), 3);
        match &app.blocks[1] {
            Block::Agent { text, .. } => assert_eq!(text, "hello"),
            other => panic!("unexpected block: {other:?}"),
        }
    }
}

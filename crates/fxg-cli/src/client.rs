//! デーモン IPC クライアントと CLI 出力ヘルパー。
//!
//! `fxg` CLI はローカルIPC (`fxg daemon`) 経由でノードを操作する
//! (設計: `docs/03-protocol-and-api.md` §4)。デーモンが起動していない場合は
//! 起動方法を案内するエラーを返す。

use std::path::Path;

use anyhow::{Context, Result, bail};
use fxg_node::IpcClient;
use fxg_protocol::client_api::WorktreeInfo;
use fxg_protocol::common::{HookLogEntry, ProjectSummary, SessionSummary};
use fxg_protocol::config::process_env;
use fxg_protocol::ipc::{IpcClientMessage, IpcResult, IpcServerMessage, ProjectInfo};

/// デーモン (`fxg daemon`) への IPC クライアント。
pub struct DaemonClient {
    client: IpcClient,
    request_seq: u64,
}

impl DaemonClient {
    /// ローカルIPCエンドポイントへ接続する。
    pub async fn connect() -> Result<Self> {
        let endpoint = fxg_node::ipc_endpoint(&process_env);
        let client = IpcClient::connect(&endpoint).await.with_context(|| {
            format!("デーモンに接続できません ({endpoint})。先に `fxg daemon` を起動してください")
        })?;
        Ok(Self {
            client,
            request_seq: 0,
        })
    }

    fn command_id(&mut self) -> String {
        self.request_seq += 1;
        format!("cli-{}-{}", std::process::id(), self.request_seq)
    }

    async fn request(&mut self, message: IpcClientMessage) -> Result<IpcResult> {
        let response = self.client.request(&message).await?;
        match response {
            IpcServerMessage::Result { result, .. } => Ok(result),
            IpcServerMessage::Error { code, message, .. } => bail!("{code}: {message}"),
            other => bail!("unexpected ipc response: {other:?}"),
        }
    }

    /// `fxg project info`
    pub async fn project_info(&mut self, cwd: &Path) -> Result<ProjectInfo> {
        let command_id = self.command_id();
        let result = self
            .request(IpcClientMessage::ProjectInfo {
                command_id,
                cwd: path_to_string(cwd),
            })
            .await?;
        match result {
            IpcResult::ProjectInfo { project } => Ok(project),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg project list`
    pub async fn list_projects(&mut self) -> Result<Vec<ProjectSummary>> {
        let command_id = self.command_id();
        let result = self
            .request(IpcClientMessage::ProjectList { command_id })
            .await?;
        match result {
            IpcResult::Projects { projects } => Ok(projects),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg project link`
    pub async fn link_project(&mut self, cwd: &Path, project_id: &str) -> Result<Option<String>> {
        let command_id = self.command_id();
        let result = self
            .request(IpcClientMessage::ProjectLink {
                command_id,
                cwd: path_to_string(cwd),
                project_id: project_id.to_owned(),
            })
            .await?;
        ack_message(result)
    }

    /// `fxg project scan`
    pub async fn scan_projects(
        &mut self,
        dir: Option<String>,
    ) -> Result<(Vec<ProjectSummary>, Vec<String>)> {
        let command_id = self.command_id();
        let result = self
            .request(IpcClientMessage::ProjectScan { command_id, dir })
            .await?;
        match result {
            IpcResult::ProjectScan {
                projects,
                scanned_dirs,
            } => Ok((projects, scanned_dirs)),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg worktree list`
    pub async fn list_worktrees(
        &mut self,
        cwd: Option<String>,
        project_id: Option<String>,
    ) -> Result<Vec<WorktreeInfo>> {
        let command_id = self.command_id();
        let result = self
            .request(IpcClientMessage::WorktreeList {
                command_id,
                cwd,
                project_id,
            })
            .await?;
        match result {
            IpcResult::Worktrees { worktrees } => Ok(worktrees),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg worktree add`
    #[allow(clippy::too_many_arguments)]
    pub async fn add_worktree(
        &mut self,
        cwd: &Path,
        project_id: Option<String>,
        branch: &str,
        base_branch: Option<String>,
        path: Option<String>,
    ) -> Result<(String, String, bool, Vec<HookLogEntry>)> {
        let command_id = self.command_id();
        let result = self
            .request(IpcClientMessage::WorktreeAdd {
                command_id,
                cwd: path_to_string(cwd),
                project_id,
                branch: branch.to_owned(),
                base_branch,
                path,
            })
            .await?;
        match result {
            IpcResult::WorktreeAdded {
                path,
                branch,
                created,
                hook_logs,
            } => Ok((path, branch, created, hook_logs)),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg worktree remove`
    pub async fn remove_worktree(
        &mut self,
        cwd: &Path,
        target: &str,
        force: bool,
    ) -> Result<Option<String>> {
        let command_id = self.command_id();
        let result = self
            .request(IpcClientMessage::WorktreeRemove {
                command_id,
                cwd: path_to_string(cwd),
                target: target.to_owned(),
                force,
            })
            .await?;
        ack_message(result)
    }

    /// `fxg worktree prune`
    pub async fn prune_worktrees(&mut self, cwd: &Path) -> Result<Option<String>> {
        let command_id = self.command_id();
        let result = self
            .request(IpcClientMessage::WorktreePrune {
                command_id,
                cwd: path_to_string(cwd),
            })
            .await?;
        ack_message(result)
    }

    /// `fxg ps` / `fxg session list`
    pub async fn list_sessions(&mut self, include_stopped: bool) -> Result<Vec<SessionSummary>> {
        let command_id = self.command_id();
        let result = self
            .request(IpcClientMessage::ListSessions {
                command_id,
                include_stopped,
            })
            .await?;
        match result {
            IpcResult::Sessions { sessions } => Ok(sessions),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg kill-all`
    pub async fn kill_all(&mut self) -> Result<(Vec<String>, Vec<String>)> {
        let command_id = self.command_id();
        let result = self
            .request(IpcClientMessage::KillAll { command_id })
            .await?;
        match result {
            IpcResult::KillAll {
                killed_sessions,
                killed_ptys,
            } => Ok((killed_sessions, killed_ptys)),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }

    /// `fxg auth token` / `fxg auth rotate-token`
    pub async fn auth_token(&mut self, rotate: bool) -> Result<String> {
        let command_id = self.command_id();
        let result = self
            .request(IpcClientMessage::AuthToken { command_id, rotate })
            .await?;
        match result {
            IpcResult::AuthToken { token } => Ok(token),
            other => bail!("unexpected ipc result: {other:?}"),
        }
    }
}

fn ack_message(result: IpcResult) -> Result<Option<String>> {
    match result {
        IpcResult::Ack { message } => Ok(message),
        other => bail!("unexpected ipc result: {other:?}"),
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// ヘッダと行から簡易テーブルを標準出力へ表示する。
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| header.chars().count())
        .collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
    }

    let print_row = |cells: Vec<String>| {
        let line = cells
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let width = widths.get(index).copied().unwrap_or(0);
                let padding = width.saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(padding))
            })
            .collect::<Vec<_>>()
            .join("  ");
        println!("{}", line.trim_end());
    };

    print_row(headers.iter().map(|header| (*header).to_owned()).collect());
    for row in rows {
        print_row(row.clone());
    }
}

/// Unix epoch ミリ秒を `YYYY-MM-DD HH:MM:SSZ` (UTC) へ整形する。
///
/// CLI の一覧表示用の最小実装 (chrono 等の日時クレートを追加しない)。
pub fn format_unix_ms_utc(ms: i64) -> String {
    let total_seconds = ms.div_euclid(1000);
    let seconds_of_day = total_seconds.rem_euclid(86_400);
    let days = total_seconds.div_euclid(86_400);

    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}Z")
}

/// 1970-01-01 からの日数から (年, 月, 日) を求める (Howard Hinnant の civil_from_days)。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

/// 文字数を制限して末尾を省略する。
pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

/// セッションIDを短縮表示する (`0195f0ab-1234-...` → `0195f0ab`)。
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_millis_as_utc() {
        assert_eq!(format_unix_ms_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(
            format_unix_ms_utc(1_700_000_000_000),
            "2023-11-14 22:13:20Z"
        );
        assert_eq!(
            format_unix_ms_utc(1_752_000_000_000),
            "2025-07-08 18:40:00Z"
        );
        // 負の epoch (1970 以前) も破綻しない
        assert_eq!(format_unix_ms_utc(-86_400_000), "1969-12-31 00:00:00Z");
    }

    #[test]
    fn truncates_long_text_on_char_boundary() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
        assert_eq!(truncate("日本語テキスト", 4), "日本語…");
    }

    #[test]
    fn shortens_session_ids() {
        assert_eq!(short_id("0195f0ab-1234-7abc-8def-0123456789ab"), "0195f0ab");
        assert_eq!(short_id("abc"), "abc");
    }
}

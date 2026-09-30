//! `fxg` バイナリのエントリポイント (CLI / TUI)。
//!
//! CLI パースには **`usage-rs`** (`usage` クレート) を使用する (`clap` は使用しない)。
//! `#[derive(Cli)]` / `Args` / `Subcommands` の宣言が単一のソースとなり、
//! `__usage_spec__` から KDL spec・シェル補完・manpage・Markdown リファレンスを
//! 生成できる (設計: `docs/05-cli-and-pwa-ui.md` §1)。
//!
//! Phase 2 では基本サブコマンド (`daemon` / `ps` / `session list` / `project` /
//! `worktree` / `auth` / `kill-all`) を提供する。
//! エージェント実行系 (`run` / `attach` / `inbox` 等) は Phase 3 で追加する。

use std::process::ExitCode;

use usage::{Cli, RunAsync, Subcommands};

mod client;
mod commands;

/// `fxg` ルートコマンド。
#[derive(Debug, Cli)]
#[usage(run_async)]
#[usage(
    bin = "fxg",
    version = env!("CARGO_PKG_VERSION"),
    about = "FlexAgent: ローカルファーストなエージェントマネージャ",
    arg_required_else_help
)]
struct Fxg {
    #[usage(subcommand)]
    command: Commands,
}

/// トップレベルサブコマンド。
#[derive(Debug, Subcommands)]
#[usage(run_async)]
enum Commands {
    /// ノードデーモンをフォアグラウンド起動する
    Daemon(commands::DaemonArgs),
    /// 稼働中・最近のセッション一覧を表示する (`fxg session list` の別名)
    Ps(commands::SessionListArgs),
    /// セッション管理 (list)
    Session(commands::SessionArgs),
    /// 論理プロジェクト管理 (info / list / link / scan)
    Project(commands::ProjectArgs),
    /// Git Worktree 管理 (list / add / remove / prune)
    #[usage(alias = "wt")]
    Worktree(commands::WorktreeArgs),
    /// 認証トークン管理 (token / rotate-token)
    Auth(commands::AuthArgs),
    /// 【緊急停止】ローカルの全セッション・子プロセスツリー・PTYを強制終了する
    KillAll(commands::KillAllArgs),
}

#[tokio::main]
async fn main() -> ExitCode {
    // ログは `RUST_LOG` で制御する (既定: warn)。CLI の出力は stdout に限定する。
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Fxg::parse();
    match cli.run_async().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("fxg: {err:#}");
            ExitCode::FAILURE
        }
    }
}

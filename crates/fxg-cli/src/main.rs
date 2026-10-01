//! `fxg` バイナリのエントリポイント (CLI / TUI)。
//!
//! CLI パースには **`usage-rs`** (`usage` クレート) を使用する (`clap` は使用しない)。
//! `#[derive(Cli)]` / `Args` / `Subcommands` の宣言が単一のソースとなり、
//! `__usage_spec__` から KDL spec・シェル補完・manpage・Markdown リファレンスを
//! 生成できる (設計: `docs/05-cli-and-pwa-ui.md` §1)。
//!
//! Phase 2 では基本サブコマンド (`daemon` / `ps` / `session list` / `project` /
//! `worktree` / `auth` / `kill-all`) を提供する。
//! Phase 3 でエージェント実行系 (`run` / `attach` / `session show|prompt|stop|kill|revert|fork`
//! / `inbox` / `agents`) と内蔵TUI (`tui`) を追加した。

use std::process::ExitCode;

use usage::{Cli, RunAsync, Subcommands};

mod client;
mod commands;
pub mod logging;
mod server_api;
mod service;
mod tray;
mod tui;

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
    /// 新規セッションを起動し、即座にターミナルを Attach する
    Run(commands::RunArgs),
    /// 稼働中セッションにターミナル (内蔵TUI / OpenCode2 純正TUI) を再接続する
    Attach(commands::AttachArgs),
    /// 稼働中・最近のセッション一覧を表示する (`fxg session list` の別名)
    Ps(commands::SessionListArgs),
    /// セッション管理 (list / show / prompt / stop / kill / revert / fork)
    Session(commands::SessionArgs),
    /// 承認待ちリクエストの確認と応答 (list / approve / reject)
    Inbox(commands::InboxArgs),
    /// 論理プロジェクト管理 (info / list / link / scan)
    Project(commands::ProjectArgs),
    /// Git Worktree 管理 (list / add / remove / prune)
    #[usage(alias = "wt")]
    Worktree(commands::WorktreeArgs),
    /// エージェント管理 (list / install / update / remove)
    Agents(commands::AgentsArgs),
    /// 一時VM・サンドボックスプロビジョナーの確認 (list / test)
    Provisioners(commands::ProvisionersArgs),
    /// ノードデーモンをフォアグラウンド起動する
    Daemon(commands::DaemonArgs),
    /// 中央サーバーをフォアグラウンド起動する (:8080)
    Server(commands::ServerArgs),
    /// 一時VM内の Zero-Touch 初期化 (git clone + ツール自動導入。出力は stderr)
    BootstrapWorkspace(commands::BootstrapWorkspaceArgs),
    /// Git Credential Proxy の GIT_ASKPASS ヘルパー (内部用)
    GitAskpass(commands::GitAskpassArgs),
    /// OSログイン時のバックグラウンド常駐サービスを管理する
    Service(commands::ServiceArgs),
    /// トークン付きURLをデフォルトブラウザで開く
    Web(commands::WebArgs),
    /// 認証トークン管理 (token / rotate-token / node-token)
    Auth(commands::AuthArgs),
    /// 【緊急停止】ローカルの全セッション・子プロセスツリー・PTYを強制終了する
    KillAll(commands::KillAllArgs),
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Fxg::parse();

    let env = fxg_protocol::config::process_env;
    let fxg_home = fxg_protocol::config::fxg_home(&env);
    let global = fxg_protocol::config::GlobalConfig::load(&env).unwrap_or_default();

    let logging_options = match &cli.command {
        Commands::Daemon(daemon_args) => daemon_args.logging_options(),
        Commands::Server(server_args) => server_args.logging_options(),
        _ => logging::LoggingOptions {
            kind: logging::CommandLogKind::Cli,
            ..Default::default()
        },
    };
    logging::init_logging(&logging_options, &global, &fxg_home);

    // macOS ではトレイ (NSStatusItem) をメインスレッドのイベントループ上で
    // 作成・操作する必要があるため、`fxg daemon --tray` はここでメインスレッドを
    // tao ループに明け渡す (デーモン本体はワーカースレッドへ退避する)。
    #[cfg(target_os = "macos")]
    if let Some(code) = tray::run_daemon_if_requested(&cli.command) {
        return code;
    }

    match cli.run_async().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("fxg: {err:#}");
            ExitCode::FAILURE
        }
    }
}

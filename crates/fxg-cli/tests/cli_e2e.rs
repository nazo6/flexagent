//! `fxg` CLI のエンドツーエンドテスト。
//!
//! 実際に `fxg daemon` を子プロセスとして起動し、**IPC 経由**で
//! `fxg project info` / `fxg project list` / `fxg worktree add|list|remove` /
//! `fxg ps` / `fxg auth token` が動作することを検証する
//! (Phase 2 完了条件: 「`fxg daemon` 起動後、IPC経由で各コマンドが動作」)。
//!
//! テストは一時的な `FXG_HOME` と IPC エンドポイントを使うため、
//! 実環境のデーモンと衝突しない。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// `fxg` バイナリのパス (cargo がテスト用に提供する)。
fn fxg_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fxg"))
}

/// テスト用の分離された環境。
struct TestEnv {
    dir: tempfile::TempDir,
    home: PathBuf,
    /// IPC エンドポイント決定に使う環境変数 (unix: XDG_RUNTIME_DIR / windows: USERNAME)
    ipc_env: Vec<(String, String)>,
    daemon: Option<Child>,
}

impl TestEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("flexagent-home");
        std::fs::create_dir_all(&home).expect("mkdir home");
        let runtime = dir.path().join("runtime");
        std::fs::create_dir_all(&runtime).expect("mkdir runtime");
        let ipc_env = if cfg!(windows) {
            // テストは同一プロセス内で並列実行されるため、Named Pipe 名を連番で一意化する
            static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            vec![(
                "USERNAME".to_owned(),
                format!("fxg-test-{}-{n}", std::process::id()),
            )]
        } else {
            vec![(
                "XDG_RUNTIME_DIR".to_owned(),
                runtime.to_string_lossy().into_owned(),
            )]
        };
        Self {
            dir,
            home,
            ipc_env,
            daemon: None,
        }
    }

    /// 子プロセス用の環境変数を組み立てる。
    fn command(&self) -> Command {
        let mut command = Command::new(fxg_bin());
        command.env("FXG_HOME", &self.home);
        for (key, value) in &self.ipc_env {
            command.env(key, value);
        }
        command
    }

    /// デーモンを起動し、IPC が応答するまで待機する。
    async fn start_daemon(&mut self) {
        // デーモンのログは失敗時の診断用にファイルへ保存する
        let log_path = self.dir.path().join("daemon.log");
        let log = std::fs::File::create(&log_path).expect("create daemon log");
        let child = self
            .command()
            .args(["daemon", "--listen", "127.0.0.1:0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn fxg daemon");
        self.daemon = Some(child);

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let output = self.run(&["ps"], None);
            if output.status.success() {
                // デーモン起動メッセージが出力され、かつ ANSI カラーコードが含まれていないことを検証
                let log = self.daemon_log();
                assert!(
                    log.contains("fxg daemon started") || log.contains("FlexAgent daemon"),
                    "daemon log should contain startup message, got:\n{log}"
                );
                assert!(
                    !log.contains("\x1b["),
                    "daemon log should not contain ANSI escape sequences, got:\n{log}"
                );
                return;
            }
            if Instant::now() > deadline {
                panic!(
                    "fxg daemon did not become ready: {}\n--- daemon log ---\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    self.daemon_log()
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// デーモンのログ (Stdio 診断用)。
    fn daemon_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("daemon.log")).unwrap_or_default()
    }

    /// デーモンを停止する。
    fn stop_daemon(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// `fxg` を実行する。
    fn run(&self, args: &[&str], cwd: Option<&Path>) -> Output {
        let mut command = self.command();
        command.args(args);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        command
            .stdin(Stdio::null())
            .output()
            .expect("run fxg command")
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        self.stop_daemon();
        let _ = &self.dir;
    }
}

fn stdout_of(output: &Output) -> String {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// 一時ディレクトリに Git リポジトリ + origin remote を作る。
fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("mkdir repo");
    run_git(dir, &["init", "-q", "-b", "main"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "fxg test"]);
    run_git(
        dir,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:nazo6/flexagent.git",
        ],
    );
    std::fs::write(dir.join("README.md"), "# test\n").expect("write");
    run_git(dir, &["add", "-A"]);
    run_git(dir, &["commit", "-q", "-m", "initial"]);
}

#[tokio::test]
async fn cli_commands_work_through_daemon_ipc() {
    let mut env = TestEnv::new();
    env.start_daemon().await;

    // --- fxg project info ---
    let repo = env.dir.path().join("repo");
    init_repo(&repo);
    let output = env.run(&["project", "info"], Some(&repo));
    let text = stdout_of(&output);
    assert!(
        text.contains("github.com/nazo6/flexagent"),
        "unexpected project info output: {text}"
    );
    assert!(
        text.contains("git_remote"),
        "source must be reported: {text}"
    );

    // --- fxg project info --json ---
    let output = env.run(&["project", "info", "--json"], Some(&repo));
    let json: serde_json::Value = serde_json::from_str(&stdout_of(&output)).expect("valid json");
    assert_eq!(json["project_id"], "github.com/nazo6/flexagent");
    assert_eq!(json["is_git_repo"], true);

    // --- fxg project list ---
    let output = env.run(&["project", "list"], Some(&repo));
    let text = stdout_of(&output);
    assert!(
        text.contains("github.com/nazo6/flexagent"),
        "unexpected project list output: {text}"
    );

    // --- fxg worktree add / list / remove ---
    let worktree_dir = env.dir.path().join("worktrees");
    let output = env.run(
        &[
            "worktree",
            "add",
            "feat/cli-e2e",
            "--path",
            &worktree_dir.to_string_lossy(),
        ],
        Some(&repo),
    );
    let text = stdout_of(&output);
    assert!(text.contains("created worktree"), "got: {text}");
    assert!(worktree_dir.join("README.md").is_file());

    // 別名 `fxg wt list` でも一覧できる
    let output = env.run(&["wt", "list"], Some(&repo));
    let text = stdout_of(&output);
    assert!(text.contains("feat/cli-e2e"), "got: {text}");
    assert!(text.contains("main"), "got: {text}");

    let output = env.run(
        &["worktree", "remove", "feat/cli-e2e", "--force"],
        Some(&repo),
    );
    let text = stdout_of(&output);
    assert!(text.contains("removed worktree"), "got: {text}");

    // --- fxg ps (セッションはまだ無い) ---
    let output = env.run(&["ps"], Some(&repo));
    let text = stdout_of(&output);
    assert!(text.contains("セッションなし"), "got: {text}");

    // `fxg session list --json` も同じ経路
    let output = env.run(&["session", "list", "--json"], Some(&repo));
    let json: serde_json::Value = serde_json::from_str(&stdout_of(&output)).expect("valid json");
    assert!(json.as_array().expect("array").is_empty());

    // --- fxg auth token (64文字の hex) ---
    let output = env.run(&["auth", "token"], Some(&repo));
    let token = stdout_of(&output).trim().to_owned();
    assert_eq!(token.len(), 64, "token length: {token}");
    assert!(token.chars().all(|ch| ch.is_ascii_hexdigit()));

    // --- fxg kill-all ---
    let output = env.run(&["kill-all", "--local-only"], Some(&repo));
    let text = stdout_of(&output);
    assert!(text.contains("killed 0 session(s)"), "got: {text}");

    env.stop_daemon();
}

#[tokio::test]
async fn cli_reports_missing_daemon() {
    let env = TestEnv::new();
    let output = env.run(&["ps"], None);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fxg daemon"),
        "stderr must guide to starting the daemon: {stderr}"
    );
}

/// Phase 3 のセッション操作系コマンド (`session show|prompt|stop|kill|revert|fork`
/// / `inbox` / `attach`) が IPC 経由で正しくエラーを返すことを検証する。
///
/// 実エージェントを起動せずに検証できる範囲 (引数パース・IPC 往復・エラーコード)
/// を対象とする。
#[tokio::test]
async fn phase3_session_and_inbox_commands_roundtrip_through_daemon() {
    let mut env = TestEnv::new();
    // 起動に失敗するカスタムエージェント (セッション記録経路の検証用)
    std::fs::write(
        env.home.join(fxg_protocol::config::CONFIG_FILE_NAME),
        "[agents.custom.broken]\nname = \"Broken Agent\"\ncommand = \"fxg-definitely-not-installed\"\n",
    )
    .expect("write config.toml");
    env.start_daemon().await;
    let repo = env.dir.path().join("repo");
    init_repo(&repo);

    // --- fxg inbox list (承認待ちなし) ---
    let output = env.run(&["inbox", "list"], Some(&repo));
    let text = stdout_of(&output);
    assert!(text.contains("承認待ちリクエストなし"), "got: {text}");

    let output = env.run(&["inbox", "list", "--json"], Some(&repo));
    let json: serde_json::Value = serde_json::from_str(&stdout_of(&output)).expect("valid json");
    assert!(
        json["requests"]
            .as_array()
            .expect("requests array")
            .is_empty(),
        "got: {json}"
    );
    assert!(
        json["elicitations"]
            .as_array()
            .expect("elicitations array")
            .is_empty(),
        "got: {json}"
    );

    // 存在しない request id への応答は拒否される
    let output = env.run(&["inbox", "approve", "req-unknown"], Some(&repo));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("承認待ちリクエストが見つかりません"),
        "got: {stderr}"
    );

    // --- fxg session show (未知のセッション) ---
    let unknown = "0195f0ab-0000-7000-8000-000000000000";
    let output = env.run(&["session", "show", unknown], Some(&repo));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("NOT_FOUND"), "got: {stderr}");

    // --- 未知セッションへの操作は INVALID_STATE で失敗する ---
    for args in [
        vec!["session", "prompt", unknown, "hello"],
        vec!["session", "stop", unknown],
        vec!["session", "stop", unknown, "--turn-only"],
        vec!["session", "kill", unknown],
        vec!["session", "revert", unknown, "--to-seq", "3"],
        vec!["session", "resume", unknown, "--detach"],
        vec!["session", "fork", unknown],
    ] {
        let output = env.run(&args, Some(&repo));
        assert!(!output.status.success(), "{args:?} must fail");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("INVALID_STATE"), "{args:?}: {stderr}");
    }

    // --- fxg run --provisioner は中央サーバー必須 (未設定時は設定エラー) ---
    let output = env.run(
        &["run", "opencode", "--provisioner", "colab-pro"],
        Some(&repo),
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("central_server_url"), "got: {stderr}");

    // --- 起動に失敗するエージェントでもセッションは記録される ---
    let output = env.run(&["run", "broken", "--detach"], Some(&repo));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to start agent"),
        "got: {stderr}\n--- daemon log ---\n{}",
        env.daemon_log()
    );

    // `fxg ps -a --json` から記録されたセッションIDを取得する
    let output = env.run(&["ps", "-a", "--json"], Some(&repo));
    let sessions: serde_json::Value = serde_json::from_str(&stdout_of(&output)).expect("json");
    let session_id = sessions
        .as_array()
        .expect("array")
        .iter()
        .find(|session| session["agent_id"] == "broken")
        .map(|session| session["session_id"].as_str().expect("id").to_owned())
        .expect("broken agent session must be recorded");

    // --- fxg session show (詳細 + 直近イベント) ---
    let output = env.run(&["session", "show", &session_id], Some(&repo));
    let text = stdout_of(&output);
    assert!(text.contains(&session_id), "got: {text}");
    assert!(text.contains("error"), "status must be error: {text}");
    assert!(text.contains("session_created"), "got: {text}");

    // `--json` でも同じ情報が取れる
    let output = env.run(&["session", "show", &session_id, "--json"], Some(&repo));
    let json: serde_json::Value = serde_json::from_str(&stdout_of(&output)).expect("json");
    assert_eq!(json["session"]["agent_id"], "broken");
    assert_eq!(json["session"]["status"], "error");

    // --- エージェント起動に失敗するセッションの resume もエラーになる ---
    let output = env.run(&["session", "resume", &session_id, "--detach"], Some(&repo));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to start agent"),
        "got: {stderr}\n--- daemon log ---\n{}",
        env.daemon_log()
    );

    env.stop_daemon();
}

#[tokio::test]
async fn cli_shows_help_for_missing_subcommand() {
    let env = TestEnv::new();
    let output = env.run(&["project"], None);
    // サブコマンド未指定は usage エラー扱いで help を stderr へ出力する (clap 互換)
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("info") && text.contains("scan"),
        "container command must show help: {text}"
    );
}

#[tokio::test]
async fn daemon_logging_to_custom_file_and_no_color() {
    let mut env = TestEnv::new();
    let custom_log_path = env.dir.path().join("custom_daemon.log");
    let mut cmd = env.command();
    cmd.args([
        "daemon",
        "--listen",
        "127.0.0.1:0",
        "--log-file",
        custom_log_path.to_str().unwrap(),
        "--no-color",
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());

    let child = cmd.spawn().expect("spawn daemon");
    env.daemon = Some(child);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let output = env.run(&["ps"], None);
        if output.status.success() {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("daemon did not start in time");
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // custom_daemon.log にログが書き込まれ、かつ ANSI エスケープが含まれないことを検証
    assert!(custom_log_path.exists(), "custom log file must exist");
    let content = std::fs::read_to_string(&custom_log_path).unwrap_or_default();
    assert!(
        content.contains("fxg daemon started"),
        "custom log file should contain startup message, got:\n{content}"
    );
    assert!(
        !content.contains("\x1b["),
        "custom log file must not contain ANSI escape sequences, got:\n{content}"
    );

    env.stop_daemon();
}

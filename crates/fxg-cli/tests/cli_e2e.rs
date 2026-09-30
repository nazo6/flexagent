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
            vec![(
                "USERNAME".to_owned(),
                format!("fxg-test-{}", std::process::id()),
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
        let child = self
            .command()
            .args(["daemon", "--listen", "127.0.0.1:0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn fxg daemon");
        self.daemon = Some(child);

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let output = self.run(&["ps"], None);
            if output.status.success() {
                return;
            }
            if Instant::now() > deadline {
                panic!(
                    "fxg daemon did not become ready: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
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

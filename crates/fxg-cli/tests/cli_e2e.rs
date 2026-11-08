//! End-to-end tests for the `fxg` CLI.
//!
//! The tests actually start `fxg daemon` as a child process and verify that
//! `fxg project info` / `fxg project list` / `fxg worktree add|list|remove` /
//! `fxg ps` / `fxg auth token` work **through IPC** (Phase 2 exit criterion:
//! "after `fxg daemon` starts, each command works through IPC").
//!
//! Each test uses a temporary `FXG_HOME` and IPC endpoint, so it never collides
//! with a real daemon.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Path of the `fxg` binary (provided by cargo for tests).
fn fxg_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fxg"))
}

/// Isolated environment for a test.
struct TestEnv {
    dir: tempfile::TempDir,
    home: PathBuf,
    /// Environment variables that determine the IPC endpoint
    /// (unix: XDG_RUNTIME_DIR / windows: USERNAME)
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
            // Tests run in parallel within the same process, so make the Named
            // Pipe name unique with a counter
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

    /// Build the environment variables for the child process.
    fn command(&self) -> Command {
        let mut command = Command::new(fxg_bin());
        command.env("FXG_HOME", &self.home);
        for (key, value) in &self.ipc_env {
            command.env(key, value);
        }
        command
    }

    /// Start the daemon and wait until IPC responds.
    async fn start_daemon(&mut self) {
        // Save daemon logs to a file for diagnostics on failure
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
                // Verify the daemon startup message was printed and that the log
                // contains no ANSI color codes
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

    /// Daemon log (for Stdio diagnostics).
    fn daemon_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("daemon.log")).unwrap_or_default()
    }

    /// Stop the daemon.
    fn stop_daemon(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Run `fxg`.
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

/// Create a Git repository with an origin remote in a temporary directory.
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

    // The alias `fxg wt list` must list them too
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

    // --- fxg ps (there are no sessions yet) ---
    let output = env.run(&["ps"], Some(&repo));
    let text = stdout_of(&output);
    assert!(text.contains("no sessions"), "got: {text}");

    // `fxg session list --json` uses the same path
    let output = env.run(&["session", "list", "--json"], Some(&repo));
    let json: serde_json::Value = serde_json::from_str(&stdout_of(&output)).expect("valid json");
    assert!(json.as_array().expect("array").is_empty());

    // --- fxg auth token (64 hex characters) ---
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

/// Verify that the Phase 3 session commands (`session
/// show|prompt|stop|kill|revert|fork` / `inbox` / `attach`) return errors
/// correctly through IPC.
///
/// The tests cover what can be verified without launching a real agent
/// (argument parsing, IPC round-trips, error codes).
#[tokio::test]
async fn phase3_session_and_inbox_commands_roundtrip_through_daemon() {
    let mut env = TestEnv::new();
    // A custom agent that fails to start (for verifying the session recording
    // path)
    std::fs::write(
        env.home.join(fxg_protocol::config::CONFIG_FILE_NAME),
        "[agents.custom.broken]\nname = \"Broken Agent\"\ncommand = \"fxg-definitely-not-installed\"\n",
    )
    .expect("write config.toml");
    env.start_daemon().await;
    let repo = env.dir.path().join("repo");
    init_repo(&repo);

    // --- fxg inbox list (nothing pending) ---
    let output = env.run(&["inbox", "list"], Some(&repo));
    let text = stdout_of(&output);
    assert!(text.contains("no pending approval requests"), "got: {text}");

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

    // Responding to a nonexistent request ID is rejected
    let output = env.run(&["inbox", "approve", "req-unknown"], Some(&repo));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pending approval request not found"),
        "got: {stderr}"
    );

    // --- fxg session show (unknown session) ---
    let unknown = "0195f0ab-0000-7000-8000-000000000000";
    let output = env.run(&["session", "show", unknown], Some(&repo));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("NOT_FOUND"), "got: {stderr}");

    // --- Operations on an unknown session fail with INVALID_STATE ---
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

    // --- fxg run --provisioner requires the central server (a config error
    // when unset) ---
    let output = env.run(
        &["run", "opencode", "--provisioner", "colab-pro"],
        Some(&repo),
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("central_server_url"), "got: {stderr}");

    // --- Sessions are recorded even when the agent fails to start ---
    let output = env.run(&["run", "broken", "--detach"], Some(&repo));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to start agent"),
        "got: {stderr}\n--- daemon log ---\n{}",
        env.daemon_log()
    );

    // Get the recorded session ID from `fxg ps -a --json`
    let output = env.run(&["ps", "-a", "--json"], Some(&repo));
    let sessions: serde_json::Value = serde_json::from_str(&stdout_of(&output)).expect("json");
    let session_id = sessions
        .as_array()
        .expect("array")
        .iter()
        .find(|session| session["agent_id"] == "broken")
        .map(|session| session["session_id"].as_str().expect("id").to_owned())
        .expect("broken agent session must be recorded");

    // --- fxg session show (details + recent events) ---
    let output = env.run(&["session", "show", &session_id], Some(&repo));
    let text = stdout_of(&output);
    assert!(text.contains(&session_id), "got: {text}");
    assert!(text.contains("error"), "status must be error: {text}");
    assert!(text.contains("session_created"), "got: {text}");

    // `--json` returns the same information
    let output = env.run(&["session", "show", &session_id, "--json"], Some(&repo));
    let json: serde_json::Value = serde_json::from_str(&stdout_of(&output)).expect("json");
    assert_eq!(json["session"]["agent_id"], "broken");
    assert_eq!(json["session"]["status"], "error");

    // --- Resuming a session whose agent failed to start also errors ---
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
    // A missing subcommand is a usage error and prints help to stderr (clap
    // compatible)
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

    // Verify that the log is written to custom_daemon.log and contains no ANSI
    // escapes
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

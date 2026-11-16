//! End-to-end tests for the `fxg` CLI.
//!
//! The tests actually start `fxg node start` as a child process and verify that
//! `fxg session` / `fxg node access-token` work **through the local HTTP API**.
//!
//! Each test uses a temporary `FXG_HOME`, so it never collides with a real node.

#![allow(
    clippy::disallowed_types,
    reason = "tests assert raw JSON output; see docs/06-type-safety.md"
)]

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
    node: Option<Child>,
}

impl TestEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("flexagent-home");
        std::fs::create_dir_all(&home).expect("mkdir home");
        Self {
            dir,
            home,
            node: None,
        }
    }

    /// Build the environment variables for the child process.
    fn command(&self) -> Command {
        let mut command = Command::new(fxg_bin());
        command.env("FXG_HOME", &self.home);
        // Never auto-start a competing node while waiting for the explicit one.
        command.env("FXG_NO_AUTO_NODE", "1");
        command
    }

    /// Start the node and wait until the HTTP API responds.
    async fn start_node(&mut self) {
        // Save node logs to a file for diagnostics on failure
        let log_path = self.dir.path().join("node.log");
        let log = std::fs::File::create(&log_path).expect("create node log");
        let child = self
            .command()
            .args(["node", "start", "--listen", "127.0.0.1:0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn fxg node start");
        self.node = Some(child);

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let output = self.run(&["session", "list"], None);
            if output.status.success() {
                // Verify the node startup message was printed and that the log
                // contains no ANSI color codes
                let log = self.node_log();
                assert!(
                    log.contains("FlexAgent node"),
                    "node log should contain startup message, got:\n{log}"
                );
                assert!(
                    !log.contains("\x1b["),
                    "node log should not contain ANSI escape sequences, got:\n{log}"
                );
                return;
            }
            if Instant::now() > deadline {
                panic!(
                    "fxg node start did not become ready: {}\n--- node log ---\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    self.node_log()
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Daemon log (for Stdio diagnostics).
    fn node_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("node.log")).expect("read node.log")
    }

    /// Stop the node.
    fn stop_node(&mut self) {
        if let Some(mut child) = self.node.take() {
            fxg_protocol::discard_with!(child.kill(), "kill the test node");
            fxg_protocol::discard_with!(child.wait(), "reap the test node");
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

    /// Run `fxg` with text fed to stdin (for confirmation prompts).
    fn run_with_stdin(&self, args: &[&str], input: &str) -> Output {
        use std::io::Write;

        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn fxg command");
        child
            .stdin
            .take()
            .expect("child stdin")
            .write_all(input.as_bytes())
            .expect("write child stdin");
        child.wait_with_output().expect("run fxg command")
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        self.stop_node();
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
async fn cli_commands_work_through_node_http() {
    let mut env = TestEnv::new();
    env.start_node().await;

    // --- fxg session list (there are no sessions yet) ---
    let output = env.run(&["session", "list"], None);
    let text = stdout_of(&output);
    assert!(text.contains("no sessions"), "got: {text}");

    // `fxg session list --json` uses the same path
    let output = env.run(&["session", "list", "--json"], None);
    let json: serde_json::Value = serde_json::from_str(&stdout_of(&output)).expect("valid json");
    assert!(json.as_array().expect("array").is_empty());

    // --- fxg node access-token show (64 hex characters) ---
    let output = env.run(&["node", "access-token", "show"], None);
    let token = stdout_of(&output).trim().to_owned();
    assert_eq!(token.len(), 64, "token length: {token}");
    assert!(token.chars().all(|ch| ch.is_ascii_hexdigit()));

    env.stop_node();
}

#[tokio::test]
async fn cli_reports_missing_node_when_auto_start_is_disabled() {
    let env = TestEnv::new();
    // Auto-start is on by default; disable it to keep testing the error path.
    let output = env
        .command()
        .env(fxg_protocol::config::env_keys::NO_AUTO_NODE, "1")
        .arg("session")
        .arg("list")
        .stdin(Stdio::null())
        .output()
        .expect("run fxg session list");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fxg node start"),
        "stderr must guide to starting the node: {stderr}"
    );
}

/// Verify that the Phase 3 session commands (`session
/// show|prompt|stop|kill|resume|fork` / `attach`) return errors
/// correctly through HTTP.
///
/// The tests cover what can be verified without launching a real agent
/// (argument parsing, HTTP round-trips, error codes).
#[tokio::test]
async fn phase3_session_commands_roundtrip_through_node() {
    let mut env = TestEnv::new();
    // A custom agent that fails to start (for verifying the session recording
    // path). Custom agents are owned by `node.db`.
    {
        let db = fxg_db::Db::open(
            &fxg_protocol::paths::Paths::portable(&env.home, fxg_protocol::paths::Role::Node)
                .db_path(),
            fxg_protocol::paths::Role::Node,
        )
        .await
        .expect("open node db");
        db.upsert_custom_agent(
            "broken",
            &fxg_protocol::settings::CustomAgentConfig {
                name: Some("Broken Agent".to_owned()),
                command: "fxg-definitely-not-installed".to_owned(),
                ..Default::default()
            },
        )
        .await
        .expect("register broken agent");
    }
    env.start_node().await;
    let repo = env.dir.path().join("repo");
    init_repo(&repo);

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
        vec!["session", "cancel", unknown],
        vec!["session", "kill", unknown],
        vec!["session", "resume", unknown, "--quiet"],
    ] {
        let output = env.run(&args, Some(&repo));
        assert!(!output.status.success(), "{args:?} must fail");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("INVALID_STATE"), "{args:?}: {stderr}");
    }

    // Fork resolves the source session over REST before creating the fork, so
    // an unknown source is a NOT_FOUND error.
    let output = env.run(&["session", "fork", unknown], Some(&repo));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("NOT_FOUND"), "got: {stderr}");

    // --- fxg session new --provisioner needs a reachable hub (the address is
    // resolved from `hub.addr`, the node pairing, then `[hub] listen_addr`) ---
    let output = env.run(
        &["session", "new", "opencode", "--provisioner", "colab-pro"],
        Some(&repo),
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to reach hub"), "got: {stderr}");

    // --- Sessions are recorded even when the agent fails to start ---
    let output = env.run(&["session", "new", "broken", "--quiet"], Some(&repo));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to start agent"),
        "got: {stderr}\n--- node log ---\n{}",
        env.node_log()
    );

    // Get the recorded session ID from `fxg session list -a --json`
    let output = env.run(&["session", "list", "-a", "--json"], Some(&repo));
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
    let output = env.run(&["session", "resume", &session_id, "--quiet"], Some(&repo));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to start agent"),
        "got: {stderr}\n--- node log ---\n{}",
        env.node_log()
    );

    env.stop_node();
}

#[tokio::test]
async fn cli_shows_help_for_missing_subcommand() {
    let env = TestEnv::new();
    let output = env.run(&["session"], None);
    // A missing subcommand is a usage error and prints help to stderr (clap
    // compatible)
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("list") && text.contains("show"),
        "container command must show help: {text}"
    );
}

#[tokio::test]
async fn internal_wipe_removes_all_data_and_optionally_config() {
    let env = TestEnv::new();
    let node = fxg_protocol::paths::Paths::portable(&env.home, fxg_protocol::paths::Role::Node);
    let hub = fxg_protocol::paths::Paths::portable(&env.home, fxg_protocol::paths::Role::Hub);
    node.ensure_dirs().expect("create node dirs");
    hub.ensure_dirs().expect("create hub dirs");
    for path in [
        node.db_path(),
        hub.db_path(),
        node.auth_token_path(),
        node.node_token_path(),
        hub.vapid_path(),
        node.secrets_dir().join("example"),
        node.log_path(),
        node.config_path(),
        hub.config_path(),
    ] {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&path, "x").expect("write test file");
    }

    // A running node is refused and nothing is deleted.
    std::fs::write(node.addr_path(), "127.0.0.1:1").expect("write addr file");
    let output = env.run(&["internal", "wipe"], None);
    assert!(!output.status.success(), "wipe must refuse a running node");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("stop it with `fxg node stop`"),
        "got: {stderr}"
    );
    assert!(
        node.db_path().is_file(),
        "a refused wipe must not delete data"
    );
    std::fs::remove_file(node.addr_path()).expect("remove addr file");

    // Declining the confirmation keeps everything.
    let output = env.run_with_stdin(&["internal", "wipe"], "n\n");
    assert!(output.status.success(), "declining must not fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("aborted"), "got: {stderr}");
    assert!(
        node.db_path().is_file(),
        "a declined wipe must not delete data"
    );

    // Data, state, cache and logs go; the configuration files stay.
    let text = stdout_of(&env.run_with_stdin(&["internal", "wipe"], "y\n"));
    assert!(text.contains("removed"), "got: {text}");
    for path in [
        node.data_dir(),
        hub.data_dir(),
        node.state_dir(),
        hub.state_dir(),
        node.cache_dir(),
        hub.cache_dir(),
        node.log_dir(),
        hub.log_dir(),
    ] {
        assert!(!path.exists(), "{} must be removed", path.display());
    }
    assert!(
        node.config_path().is_file(),
        "node.toml must stay without --config"
    );
    assert!(
        hub.config_path().is_file(),
        "hub.toml must stay without --config"
    );

    // --config also deletes the bootstrap configuration.
    stdout_of(&env.run_with_stdin(&["internal", "wipe", "--config"], "y\n"));
    assert!(!node.config_path().exists(), "node.toml must be removed");
    assert!(!hub.config_path().exists(), "hub.toml must be removed");
}

#[tokio::test]
async fn node_logging_to_custom_file_and_no_color() {
    let mut env = TestEnv::new();
    let custom_log_path = env.dir.path().join("custom_node.log");
    let mut cmd = env.command();
    cmd.args([
        "node",
        "start",
        "--listen",
        "127.0.0.1:0",
        "--log-file",
        custom_log_path.to_str().unwrap(),
        "--no-color",
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());

    let child = cmd.spawn().expect("spawn node");
    env.node = Some(child);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let output = env.run(&["session", "list"], None);
        if output.status.success() {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("node did not start in time");
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Verify that the log is written to custom_node.log (daily rotation appends
    // the date) and contains no ANSI escapes
    let rotated_log_path = fxg_hub::api::resolve_log_file(&custom_log_path);
    assert!(
        rotated_log_path.exists(),
        "custom log file must exist: {}",
        rotated_log_path.display()
    );
    let content = std::fs::read_to_string(&rotated_log_path).expect("read the custom node log");
    assert!(
        content.contains("fxg node start started"),
        "custom log file should contain startup message, got:\n{content}"
    );
    assert!(
        !content.contains("\x1b["),
        "custom log file must not contain ANSI escape sequences, got:\n{content}"
    );

    env.stop_node();
}

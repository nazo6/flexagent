//! Git credential proxy / drain E2E tests for `fxg daemon --stdio`.
//!
//! The tests start the real binary as a child process and the test side acts as
//! the central server, reading and writing JSON Lines:
//!
//! - Receive `NodeHello` (reporting `is_ephemeral = true` as an ephemeral node)
//! - `fxg git-askpass` (local IPC) → `GitCredentialRequest` →
//!   test-side `GitCredentialResponse` → askpass prints the token
//! - `DrainAndShutdown` → `DrainComplete` → the process exits cleanly
//!
//! The tests also verify that `stdout` is reserved for the protocol (every
//! line is JSON).

#![allow(
    clippy::disallowed_types,
    reason = "tests assert raw JSON Lines; see docs/06-type-safety.md"
)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Temporary environment for the tests.
struct TestEnv {
    home: std::path::PathBuf,
    ipc_endpoint: String,
    node_id: String,
}

impl TestEnv {
    fn new(base: &Path) -> Self {
        let ipc_endpoint = if cfg!(windows) {
            format!(r"\\.\pipe\fxg-stdio-test-{}", std::process::id())
        } else {
            base.join("daemon.sock").to_string_lossy().into_owned()
        };
        Self {
            home: base.join("home"),
            ipc_endpoint,
            node_id: "ephemeral-test-node".to_owned(),
        }
    }

    fn base_env(&self) -> Vec<(String, String)> {
        vec![
            (
                "FXG_HOME".to_owned(),
                self.home.to_string_lossy().into_owned(),
            ),
            ("FXG_IPC_ENDPOINT".to_owned(), self.ipc_endpoint.clone()),
            ("FXG_NODE_ID".to_owned(), self.node_id.clone()),
            ("RUST_LOG".to_owned(), "warn".to_owned()),
        ]
    }
}

fn fxg() -> &'static str {
    env!("CARGO_BIN_EXE_fxg")
}

/// Start a thread that reads the child process's stdout line by line and sends
/// the JSON values over a channel.
fn spawn_line_reader(stdout: impl std::io::Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// Wait on the channel for a message matching the predicate (panics on timeout).
fn wait_for_message(
    rx: &Receiver<String>,
    what: &str,
    timeout: Duration,
    mut predicate: impl FnMut(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for: {what}");
        }
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                let value: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|err| panic!("non-JSON line on stdout ({err}): {line}"));
                if predicate(&value) {
                    return value;
                }
            }
            Err(err) => panic!("waiting for {what}: {err}"),
        }
    }
}

fn write_message(stdin: &mut ChildStdin, message: &Value) {
    let mut line = serde_json::to_vec(message).expect("encode");
    line.push(b'\n');
    stdin.write_all(&line).expect("write stdin");
    stdin.flush().expect("flush stdin");
}

/// Run `fxg internal git-askpass` and return the single stdout line.
fn run_askpass(env: &TestEnv, prompt: &str) -> String {
    let mut command = Command::new(fxg());
    command.args(["internal", "git-askpass", prompt]);
    for (key, value) in env.base_env() {
        command.env(key, value);
    }
    let output = command.output().expect("run git-askpass");
    assert!(
        output.status.success(),
        "internal git-askpass failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn stdio_daemon_proxies_git_credentials_and_drains() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = TestEnv::new(tmp.path());
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("mkdir workspace");

    let mut command = Command::new(fxg());
    command.args([
        "daemon",
        "--stdio",
        "--ephemeral",
        "--workspace",
        &workspace.to_string_lossy(),
    ]);
    for (key, value) in env.base_env() {
        command.env(key, value);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child: Child = command.spawn().expect("spawn fxg daemon --stdio");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut stdin = stdin;
    let lines = spawn_line_reader(stdout);

    // 1. NodeHello (handshake as an ephemeral node)
    let hello = wait_for_message(&lines, "node_hello", Duration::from_secs(60), |value| {
        value["op"] == "node_hello"
    });
    assert_eq!(hello["node_id"], env.node_id);
    assert_eq!(hello["is_ephemeral"], true);

    // 2. Password prompt → GitCredentialRequest → response → token output
    let askpass = {
        let env_home = env.home.clone();
        let env_ipc = env.ipc_endpoint.clone();
        let prompt = "Password for 'https://github.com':";
        std::thread::spawn(move || {
            let env = TestEnv {
                home: env_home,
                ipc_endpoint: env_ipc,
                node_id: String::new(),
            };
            run_askpass(&env, prompt)
        })
    };
    let request = wait_for_message(
        &lines,
        "git_credential_request",
        Duration::from_secs(30),
        |value| value["op"] == "git_credential_request",
    );
    assert_eq!(request["repo_url"], "https://github.com");
    let request_id = request["request_id"].as_str().expect("request_id");
    write_message(
        &mut stdin,
        &json!({
            "op": "git_credential_response",
            "request_id": request_id,
            "username": "x-access-token",
            "token": "secret-token",
            "error": Value::Null,
        }),
    );
    assert_eq!(askpass.join().expect("askpass thread"), "secret-token");

    // 3. Username prompt → returns the username
    let askpass = {
        let env_home = env.home.clone();
        let env_ipc = env.ipc_endpoint.clone();
        std::thread::spawn(move || {
            let env = TestEnv {
                home: env_home,
                ipc_endpoint: env_ipc,
                node_id: String::new(),
            };
            run_askpass(&env, "Username for 'https://github.com':")
        })
    };
    let request = wait_for_message(
        &lines,
        "git_credential_request (username)",
        Duration::from_secs(30),
        |value| value["op"] == "git_credential_request",
    );
    write_message(
        &mut stdin,
        &json!({
            "op": "git_credential_response",
            "request_id": request["request_id"],
            "username": "x-access-token",
            "token": Value::Null,
            "error": Value::Null,
        }),
    );
    assert_eq!(askpass.join().expect("askpass thread"), "x-access-token");

    // 4. DrainAndShutdown → DrainComplete → clean exit
    write_message(
        &mut stdin,
        &json!({
            "op": "drain_and_shutdown",
            "reason": "test finished",
        }),
    );
    wait_for_message(&lines, "drain_complete", Duration::from_secs(30), |value| {
        value["op"] == "drain_complete"
    });

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => {
                assert!(status.success(), "daemon exited with {status}");
                break;
            }
            None if Instant::now() >= deadline => panic!("daemon did not exit after drain"),
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

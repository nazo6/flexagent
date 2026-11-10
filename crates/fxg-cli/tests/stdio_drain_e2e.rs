//! Drain E2E tests for `fxg internal stdio`.
//!
//! The tests start the real binary as a child process and the test side acts as
//! the hub, reading and writing JSON Lines:
//!
//! - Receive `NodeHello` (reporting `is_ephemeral = true` as an ephemeral node)
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
    node_id: String,
}

impl TestEnv {
    fn new(base: &Path) -> Self {
        Self {
            home: base.join("home"),
            node_id: "ephemeral-test-node".to_owned(),
        }
    }

    fn base_env(&self) -> Vec<(String, String)> {
        vec![
            (
                "FXG_HOME".to_owned(),
                self.home.to_string_lossy().into_owned(),
            ),
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

#[test]
fn stdio_node_handshakes_and_drains() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = TestEnv::new(tmp.path());
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("mkdir workspace");

    let mut command = Command::new(fxg());
    command.args([
        "internal",
        "stdio",
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
    let mut child: Child = command.spawn().expect("spawn fxg internal stdio");
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

    // 2. DrainAndShutdown → DrainComplete → clean exit
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
                assert!(status.success(), "node exited with {status}");
                break;
            }
            None if Instant::now() >= deadline => panic!("node did not exit after drain"),
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

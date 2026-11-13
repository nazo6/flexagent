//! Minimal mock ACP agent binary used by integration tests.
//!
//! Speaks JSON-RPC 2.0 over stdin/stdout and supports:
//! - `--mode auth-required`: returns advertised terminal auth in initialize, rejects `session/new`
//! - `--mode fork`: returns `fork` capability in initialize, handles `session/fork`, `session/load`, and `session/new`
//! - `--mode no-fork`: returns no `fork` capability in initialize, handles `session/new`
//! - `--mode modes-as-config`: exposes build/plan as `configOptions[category = mode]`
//!   (like OpenCode) and rejects `session/set_mode`
//! - `--mode late-updates`: emits a background report after the prompt response,
//!   and flushes a queued report before the next turn's user-message echo
//! - `--mode thought-before-requests`: emits `agent_thought_chunk` right before
//!   `elicitation/create` and `session/request_permission` without ending the turn

#![allow(
    clippy::disallowed_types,
    reason = "mock agent operates directly on raw JSON-RPC messages; see docs/06-type-safety.md"
)]

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let mode = parse_mode();
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut stdout = io::stdout();
    let mut prompt_count = 0usize;

    while let Some(Ok(line)) = lines.next() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(id) = request.get("id") else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = request.get("params").cloned().unwrap_or(Value::Null);

        if method == "session/prompt" && mode == "thought-before-requests" {
            handle_thought_before_requests(&mut lines, &mut stdout, id);
            continue;
        }

        if method == "session/prompt" && mode == "late-updates" {
            prompt_count += 1;
            send_late_updates(&mut stdout, id, prompt_count);
            continue;
        }

        if method == "session/prompt" && matches!(mode.as_str(), "subagents" | "opencode-subagents")
        {
            send_child_updates(&mut stdout, &mode);
            std::thread::sleep(std::time::Duration::from_millis(150));
            write_line(
                &mut stdout,
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "stopReason": "end_turn" }
                }),
            );
            continue;
        }

        // `session/load` replays the conversation before the response (ACP spec).
        if method == "session/load" && mode == "list" {
            send_list_replay(&mut stdout);
            write_line(
                &mut stdout,
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "sessionId": "external-1" }
                }),
            );
            continue;
        }

        let response = match (mode.as_str(), method) {
            ("auth-required", "initialize") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": 1,
                    "agentCapabilities": {},
                    "authMethods": [
                        {
                            "id": "terminal-login",
                            "name": "Terminal login",
                            "type": "terminal",
                            "args": ["--login"]
                        }
                    ]
                }
            }),
            ("auth-required", "session/new") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32000,
                    "message": "authentication required"
                }
            }),
            ("fork", "initialize") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": 1,
                    "agentCapabilities": {
                        "loadSession": true,
                        "sessionCapabilities": {
                            "fork": {}
                        }
                    },
                    "authMethods": []
                }
            }),
            ("fork", "session/fork") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "sessionId": "forked-session"
                }
            }),
            ("fork", "session/load") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "sessionId": "forked-session"
                }
            }),
            ("fork", "session/new") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "sessionId": "source-session"
                }
            }),
            ("list", "initialize") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": 1,
                    "agentCapabilities": {
                        "loadSession": true,
                        "sessionCapabilities": {
                            "list": {}
                        }
                    },
                    "authMethods": []
                }
            }),
            ("list", "session/list") => {
                if params.get("cursor").and_then(Value::as_str) == Some("page-2") {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "sessions": [
                                {
                                    "sessionId": "external-2",
                                    "cwd": "/tmp",
                                    "title": "Second external session"
                                }
                            ]
                        }
                    })
                } else {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "sessions": [
                                {
                                    "sessionId": "external-1",
                                    "cwd": "/tmp",
                                    "title": "First external session",
                                    "updatedAt": "2026-11-12T10:00:00Z"
                                }
                            ],
                            "nextCursor": "page-2"
                        }
                    })
                }
            }
            ("no-fork", "initialize") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": 1,
                    "agentCapabilities": {
                        "loadSession": true
                    },
                    "authMethods": []
                }
            }),
            ("no-fork", "session/new") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "sessionId": "source-session"
                }
            }),
            ("modes-as-config", "initialize") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": 1,
                    "agentCapabilities": {},
                    "authMethods": []
                }
            }),
            ("modes-as-config", "session/new") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "sessionId": "mode-session",
                    "configOptions": mode_config_options("build")
                }
            }),
            ("modes-as-config", "session/set_config_option") => {
                let value = params
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or("build");
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "configOptions": mode_config_options(value)
                    }
                })
            }
            ("subagents" | "opencode-subagents", "initialize") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": 1,
                    "agentCapabilities": {},
                    "authMethods": []
                }
            }),
            (
                "subagents" | "opencode-subagents" | "late-updates" | "thought-before-requests",
                "session/new",
            ) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "sessionId": "root-session"
                }
            }),
            ("late-updates" | "thought-before-requests", "initialize") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": 1,
                    "agentCapabilities": {},
                    "authMethods": []
                }
            }),
            // OpenCode rejects `session/set_mode` for mode IDs from config options
            ("modes-as-config", "session/set_mode") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32602,
                    "message": "mode not found"
                }
            }),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": "method not found"
                }
            }),
        };

        write_line(&mut stdout, &response);
    }
}

/// Writes one JSON-RPC line.
fn write_line(stdout: &mut io::Stdout, value: &Value) {
    if let Ok(mut out) = serde_json::to_vec(value) {
        out.push(b'\n');
        if stdout.write_all(&out).is_err() {
            return;
        }
        if stdout.flush().is_err() {
            // Best-effort: the process is exiting on the next failed write.
        }
    }
}

/// Sends the history replay used by the external-session import integration test.
fn send_list_replay(stdout: &mut io::Stdout) {
    for (update, session_id) in [
        (
            json!({
                "sessionUpdate": "user_message_chunk",
                "messageId": "u1",
                "content": { "type": "text", "text": "hello from history" }
            }),
            "external-1",
        ),
        (
            json!({
                "sessionUpdate": "agent_message_chunk",
                "messageId": "a1",
                "content": { "type": "text", "text": "reply from history" }
            }),
            "external-1",
        ),
    ] {
        write_line(
            stdout,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": { "sessionId": session_id, "update": update }
            }),
        );
    }
}

/// Sends the child-session notifications used by the subagent integration tests.
fn send_child_updates(stdout: &mut io::Stdout, mode: &str) {
    match mode {
        "subagents" => {
            write_line(
                stdout,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": "root-session",
                        "update": {
                            "sessionUpdate": "subagent_update",
                            "sessionId": "child-rfd",
                            "title": "Explore",
                            "description": "RFD child",
                            "state": { "state": "running" }
                        }
                    }
                }),
            );
            // Give the client a moment to register the child handler.
            std::thread::sleep(std::time::Duration::from_millis(100));
            write_line(
                stdout,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": "child-rfd",
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "messageId": "child-rfd-m1",
                            "content": { "type": "text", "text": "hello from the child" }
                        }
                    }
                }),
            );
            write_line(
                stdout,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": "child-rfd",
                        "update": {
                            "sessionUpdate": "tool_call",
                            "toolCallId": "child-rfd-t1",
                            "title": "Read file",
                            "kind": "read",
                            "status": "pending"
                        }
                    }
                }),
            );
        }
        "opencode-subagents" => {
            write_line(
                stdout,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "opencode/session/child_update",
                    "params": {
                        "rootSessionId": "root-session",
                        "childSessionId": "child-open",
                        "parentSessionId": "root-session",
                        "depth": 1,
                        "title": "Explore",
                        "type": "status",
                        "status": "running"
                    }
                }),
            );
            write_line(
                stdout,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "opencode/session/child_update",
                    "params": {
                        "rootSessionId": "root-session",
                        "childSessionId": "child-open",
                        "parentSessionId": "root-session",
                        "depth": 1,
                        "title": "Explore",
                        "type": "update",
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "messageId": "child-open-m1",
                            "content": { "type": "text", "text": "child says hi" }
                        }
                    }
                }),
            );
        }
        _ => {}
    }
}

/// Sends background updates for the idle-update integration tests.
///
/// The first prompt answers normally and then emits a report after the prompt
/// response (like agy's teamwork follow-up). Later prompts replay a queued
/// report before the user-message echo, mimicking an agent that only forwards
/// idle output once the next prompt starts.
fn send_late_updates(stdout: &mut io::Stdout, id: &Value, prompt_count: usize) {
    if prompt_count == 1 {
        write_line(
            stdout,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "root-session",
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": { "type": "text", "text": "first answer" }
                    }
                }
            }),
        );
        write_line(
            stdout,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "stopReason": "end_turn" }
            }),
        );
        // A background task reports back after the turn has ended.
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let mut out = io::stdout();
            for text in ["late result", " done"] {
                write_line(
                    &mut out,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": "root-session",
                            "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": text }
                            }
                        }
                    }),
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
        return;
    }

    // A queued report flushed only now, before the new turn's echo.
    for update in [
        json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "queued prelude" }
        }),
        json!({
            "sessionUpdate": "user_message_chunk",
            "content": { "type": "text", "text": "second" }
        }),
        json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "next answer" }
        }),
    ] {
        write_line(
            stdout,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": { "sessionId": "root-session", "update": update }
            }),
        );
    }
    write_line(
        stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "stopReason": "end_turn" }
        }),
    );
}

/// Emits thoughts right before `elicitation/create` and `session/request_permission`
/// and waits for the client's responses before completing the turn.
fn handle_thought_before_requests<R: BufRead>(
    lines: &mut io::Lines<R>,
    stdout: &mut io::Stdout,
    prompt_id: &Value,
) {
    write_line(
        stdout,
        &json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "root-session",
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "messageId": "t-question",
                    "content": { "type": "text", "text": "thinking before question" }
                }
            }
        }),
    );
    write_line(
        stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": "req-elicit-1",
            "method": "elicitation/create",
            "params": {
                "sessionId": "root-session",
                "mode": "form",
                "message": "Choose option",
                "requestedSchema": {
                    "type": "object",
                    "properties": {
                        "choice": { "type": "string" }
                    },
                    "required": ["choice"]
                }
            }
        }),
    );
    wait_for_response(lines, "req-elicit-1");

    write_line(
        stdout,
        &json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "root-session",
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "messageId": "t-perm",
                    "content": { "type": "text", "text": "thinking before permission" }
                }
            }
        }),
    );
    write_line(
        stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": "req-perm-1",
            "method": "session/request_permission",
            "params": {
                "sessionId": "root-session",
                "toolCall": {
                    "toolCallId": "tc-perm-1",
                    "title": "Run check"
                },
                "options": [
                    { "optionId": "allow_once", "name": "Allow", "kind": "allow_once" }
                ]
            }
        }),
    );
    wait_for_response(lines, "req-perm-1");

    write_line(
        stdout,
        &json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "root-session",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": "m-done",
                    "content": { "type": "text", "text": "all done" }
                }
            }
        }),
    );
    write_line(
        stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": prompt_id,
            "result": { "stopReason": "end_turn" }
        }),
    );
}

fn wait_for_response<R: BufRead>(lines: &mut io::Lines<R>, expected_id: &str) {
    while let Some(Ok(line)) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if value.get("id").and_then(Value::as_str) == Some(expected_id)
            && (value.get("result").is_some() || value.get("error").is_some())
        {
            return;
        }
    }
}

/// Config options for `--mode modes-as-config` (model + selectable mode).
fn mode_config_options(current_mode: &str) -> Value {
    json!([
        {
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": "m1",
            "options": [{ "value": "m1", "name": "Model 1" }]
        },
        {
            "id": "mode",
            "name": "Session Mode",
            "category": "mode",
            "type": "select",
            "currentValue": current_mode,
            "options": [
                {
                    "value": "build",
                    "name": "Build",
                    "description": "The default agent."
                },
                {
                    "value": "plan",
                    "name": "Plan",
                    "description": "Read-only planning."
                }
            ]
        }
    ])
}

fn parse_mode() -> String {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--mode" {
            if let Some(val) = args.next() {
                return val;
            }
        } else if let Some(val) = arg.strip_prefix("--mode=") {
            return val.to_owned();
        } else if !arg.starts_with('-') {
            return arg;
        }
    }
    String::new()
}

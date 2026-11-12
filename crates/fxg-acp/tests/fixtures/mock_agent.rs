//! Minimal mock ACP agent binary used by integration tests.
//!
//! Speaks JSON-RPC 2.0 over stdin/stdout and supports:
//! - `--mode auth-required`: returns advertised terminal auth in initialize, rejects `session/new`
//! - `--mode fork`: returns `fork` capability in initialize, handles `session/fork`, `session/load`, and `session/new`
//! - `--mode no-fork`: returns no `fork` capability in initialize, handles `session/new`
//! - `--mode modes-as-config`: exposes build/plan as `configOptions[category = mode]`
//!   (like OpenCode) and rejects `session/set_mode`

#![allow(
    clippy::disallowed_types,
    reason = "mock agent operates directly on raw JSON-RPC messages; see docs/06-type-safety.md"
)]

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let mode = parse_mode();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
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

        if let Ok(mut out) = serde_json::to_vec(&response) {
            out.push(b'\n');
            if stdout.write_all(&out).is_err() || stdout.flush().is_err() {
                break;
            }
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

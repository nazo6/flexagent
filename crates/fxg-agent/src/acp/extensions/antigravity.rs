//! Antigravity CLI extension (Tier 2).
//!
//! The CLI owns the Google / Claude / GPT quota grouping, so usage is read from
//! its structured `/usage` command instead of a private Cloud Code endpoint.
//!
//! Design: `docs/04-agent-drivers-and-windows.md` §1.2.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use fxg_protocol::common::{
    AgentUsageLimits, UsageGroup, UsageSource, UsageWindow, UsageWindowKind,
};
use fxg_protocol::util::now_ms;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::acp::extensions::AgentExtension;
use crate::driver::AgentLaunchSpec;
use crate::usage::{rfc3339_to_epoch_ms, used_fraction_from_remaining};

/// Upper bound for one `agy -p /usage` invocation (cold start is ~6s).
const USAGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Reuse one snapshot across concurrent sessions / refreshes.
const CACHE_TTL: Duration = Duration::from_secs(60);
/// Profile flags that must be forwarded so the command sees the same login.
const PROFILE_FLAGS: [&str; 4] = [
    "--app-data-dir",
    "--app_data_dir",
    "--gemini-dir",
    "--gemini_dir",
];

/// Antigravity CLI extension.
#[derive(Debug, Default)]
pub struct AntigravityExtension {
    cache: Mutex<HashMap<String, CachedUsage>>,
    /// Serializes cold fetches so concurrent sessions do not spawn duplicate
    /// `agy` processes; the second waiter reads the cache after the first.
    fetch: Mutex<()>,
}

#[derive(Debug)]
struct CachedUsage {
    at: Instant,
    usage: AgentUsageLimits,
}

#[async_trait]
impl AgentExtension for AntigravityExtension {
    async fn usage_limits(
        &self,
        launch: &AgentLaunchSpec,
        cwd: &Path,
        force: bool,
    ) -> anyhow::Result<Option<AgentUsageLimits>> {
        let Some(program) = launch.program.as_deref() else {
            return Ok(None);
        };
        let key = cache_key(program, launch);
        let _fetch = self.fetch.lock().await;
        if !force && let Some(usage) = self.cached(&key).await {
            return Ok(Some(usage));
        }
        let Some(usage) = run_usage(program, launch, cwd).await? else {
            return Ok(None);
        };
        self.store(key, usage.clone()).await;
        Ok(Some(usage))
    }
}

impl AntigravityExtension {
    async fn cached(&self, key: &str) -> Option<AgentUsageLimits> {
        let cache = self.cache.lock().await;
        cache
            .get(key)
            .filter(|entry| entry.at.elapsed() < CACHE_TTL)
            .map(|entry| entry.usage.clone())
    }

    async fn store(&self, key: String, usage: AgentUsageLimits) {
        let mut cache = self.cache.lock().await;
        cache.retain(|_, entry| entry.at.elapsed() < CACHE_TTL);
        cache.insert(
            key,
            CachedUsage {
                at: Instant::now(),
                usage,
            },
        );
    }
}

fn cache_key(program: &Path, launch: &AgentLaunchSpec) -> String {
    let mut key = program.to_string_lossy().into_owned();
    for arg in &launch.args {
        key.push('\0');
        key.push_str(arg);
    }
    key
}

async fn run_usage(
    program: &Path,
    launch: &AgentLaunchSpec,
    cwd: &Path,
) -> anyhow::Result<Option<AgentUsageLimits>> {
    let mut command = Command::new(program);
    command
        .arg("-p")
        .arg("/usage")
        .arg("--output-format")
        .arg("json")
        .current_dir(cwd)
        .envs(
            launch
                .env
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    forward_profile_args(launch, &mut command);

    let output = tokio::time::timeout(USAGE_TIMEOUT, command.output())
        .await
        .map_err(|err| anyhow::anyhow!("agy usage timed out after {USAGE_TIMEOUT:?}: {err}"))??;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("agy usage failed with {}: {}", output.status, stderr.trim());
    }
    parse_usage(&output.stdout)
}

/// Forwards profile options from the ACP launch so `/usage` sees the same account.
fn forward_profile_args(launch: &AgentLaunchSpec, command: &mut Command) {
    let mut args = launch.args.iter();
    while let Some(arg) = args.next() {
        if PROFILE_FLAGS.contains(&arg.as_str()) {
            command.arg(arg);
            if let Some(value) = args.next() {
                command.arg(value);
            }
        } else if let Some((flag, _)) = arg.split_once('=')
            && PROFILE_FLAGS.contains(&flag)
        {
            command.arg(arg);
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct UsageEnvelope {
    command: Option<UsageCommand>,
}

#[derive(Debug, serde::Deserialize)]
struct UsageCommand {
    data: Option<UsageData>,
}

#[derive(Debug, serde::Deserialize)]
struct UsageData {
    groups: Vec<UsageGroupData>,
}

#[derive(Debug, serde::Deserialize)]
struct UsageGroupData {
    name: String,
    description: Option<String>,
    buckets: Vec<UsageBucket>,
}

#[derive(Debug, serde::Deserialize)]
struct UsageBucket {
    name: String,
    window: String,
    remaining_fraction: f64,
    reset_time: Option<String>,
}

/// Parses `agy -p /usage --output-format json` output.
pub(super) fn parse_usage(bytes: &[u8]) -> anyhow::Result<Option<AgentUsageLimits>> {
    let envelope: UsageEnvelope = serde_json::from_slice(bytes)
        .map_err(|err| anyhow::anyhow!("invalid agy usage JSON: {err}"))?;
    let Some(data) = envelope.command.and_then(|command| command.data) else {
        return Ok(None);
    };
    if data.groups.is_empty() {
        return Ok(None);
    }

    let groups = data
        .groups
        .into_iter()
        .enumerate()
        .map(|(index, group)| UsageGroup {
            id: group_id(&group.name, index),
            label: group.name,
            description: group.description,
            windows: group.buckets.into_iter().map(usage_window).collect(),
        })
        .collect();

    Ok(Some(AgentUsageLimits {
        source: UsageSource {
            id: "antigravity".to_owned(),
            label: "Antigravity".to_owned(),
            plan: None,
            account: None,
            icon: Some("builtin:antigravity".to_owned()),
        },
        groups,
        fetched_at: now_ms(),
    }))
}

fn usage_window(bucket: UsageBucket) -> UsageWindow {
    UsageWindow {
        kind: window_kind(&bucket.window),
        label: Some(bucket.name),
        used_fraction: used_fraction_from_remaining(bucket.remaining_fraction),
        resets_at: bucket.reset_time.as_deref().and_then(rfc3339_to_epoch_ms),
        limited: bucket.remaining_fraction <= 0.0,
    }
}

/// Maps the CLI window names to the protocol vocabulary.
///
/// `agy`'s `5h` is a fixed window, distinct from OpenCode's rolling 5-hour
/// window. Unknown windows are preserved instead of being coerced.
fn window_kind(value: &str) -> UsageWindowKind {
    match value {
        "5h" => UsageWindowKind::FiveHour,
        "rolling_5h" => UsageWindowKind::Rolling5h,
        "weekly" => UsageWindowKind::Weekly,
        "monthly" => UsageWindowKind::Monthly,
        "budget" => UsageWindowKind::Budget,
        other => UsageWindowKind::Unknown(other.to_owned()),
    }
}

fn group_id(name: &str, index: usize) -> String {
    let slug = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        format!("group-{index}")
    } else {
        slug.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{AntigravityExtension, parse_usage};
    use crate::acp::extensions::AgentExtension;
    use crate::driver::{AcpLaunchOptions, AgentLaunchSpec, LaunchBackend};

    #[test]
    fn parses_antigravity_usage_groups() {
        let payload = br#"{
            "conversation_id": "",
            "status": "SUCCESS",
            "num_turns": 0,
            "command": {
                "name": "usage",
                "data": {
                    "groups": [
                        {
                            "name": "Gemini Models",
                            "description": "Models within this group: Gemini 4 Hydrogen",
                            "buckets": [
                                {
                                    "id": "gemini-weekly",
                                    "name": "Weekly Limit Remaining",
                                    "window": "weekly",
                                    "remaining_fraction": 0.58,
                                    "reset_time": "2026-11-20T23:55:29Z"
                                },
                                {
                                    "id": "gemini-5h",
                                    "name": "Five Hour Limit Remaining",
                                    "window": "5h",
                                    "remaining_fraction": 0.99,
                                    "reset_time": "2026-11-15T15:20:50Z"
                                }
                            ]
                        },
                        {
                            "name": "Claude and GPT models",
                            "buckets": [
                                {
                                    "name": "Weekly Limit Remaining",
                                    "window": "weekly",
                                    "remaining_fraction": 0.0,
                                    "reset_time": "2026-11-22T11:53:08Z"
                                }
                            ]
                        }
                    ]
                }
            }
        }"#;

        let usage = parse_usage(payload)
            .expect("valid agy JSON")
            .expect("usage data");
        assert_eq!(usage.source.id, "antigravity");
        assert_eq!(usage.source.label, "Antigravity");
        assert_eq!(usage.groups.len(), 2);
        let gemini = &usage.groups[0];
        assert_eq!(gemini.label, "Gemini Models");
        assert_eq!(gemini.id, "gemini-models");
        assert_eq!(gemini.windows[0].kind.as_str(), "weekly");
        assert!((gemini.windows[0].used_fraction - 0.42).abs() < 1e-9);
        assert!(gemini.windows[0].resets_at.is_some());
        assert_eq!(gemini.windows[1].kind.as_str(), "5h");
        assert!(!gemini.windows[1].limited);
        assert!(usage.groups[1].windows[0].limited);
    }

    #[test]
    fn ignores_output_without_usage_command() {
        assert!(
            parse_usage(br#"{"status":"SUCCESS"}"#)
                .expect("valid JSON")
                .is_none()
        );
    }

    #[tokio::test]
    #[ignore = "requires an installed and signed-in agy CLI"]
    async fn reads_usage_from_the_installed_cli() {
        let Ok(program) = which::which("agy") else {
            return;
        };
        let launch = AgentLaunchSpec {
            agent_id: "agy".to_owned(),
            display_name: "Antigravity".to_owned(),
            backend: LaunchBackend::Acp(AcpLaunchOptions::default()),
            program: Some(program),
            args: vec!["acp".to_owned()],
            env: Vec::new(),
        };
        let usage = AntigravityExtension::default()
            .usage_limits(&launch, Path::new("."), true)
            .await
            .expect("agy usage")
            .expect("usage limits");
        assert_eq!(usage.source.id, "antigravity");
        assert!(!usage.groups.is_empty());
    }
}

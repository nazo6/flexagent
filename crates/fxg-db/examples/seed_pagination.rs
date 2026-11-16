//! Seeds an isolated node DB with sessions and chat history for pagination checks.
//!
//! Usage (the node must be stopped while seeding):
//!
//! ```bash
//! FXG_HOME=/tmp/fxg-paging \
//!   cargo run -p fxg-db --example seed_pagination -- 250 600 1
//! ```
//!
//! Arguments: `<sessions> [chat_events] [projects] [interleaved|blocks]`.
//! `blocks` gives each project a contiguous slice of the timeline (project 0
//! oldest), which shows how projects whose sessions are all older than the
//! first page appear in the sidebar. The node adopts the seeded node ID when
//! `node.toml` has no `node_id` (it reuses the first row of `nodes`).

use fxg_db::{Db, NodeRecord, ProjectBindingRecord, ProjectRecord};
use fxg_protocol::common::{ClientSource, SessionStatus};
use fxg_protocol::events::{SessionEventEnvelope, UnifiedEventPayload};
use fxg_protocol::paths::{Paths, Role};
use fxg_protocol::settings::ProjectIdentity;
use fxg_protocol::util::{now_ms, uuid_v7};

const NODE_ID: &str = "seed-node";

struct ProjectSpec {
    id: String,
    name: String,
    path: String,
}

#[allow(
    clippy::disallowed_methods,
    reason = "EnvLookup treats a missing variable as absent; the example does not need the error"
)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Never seed the real data directory: this tool is for a throwaway home.
    if std::env::var_os("FXG_HOME").is_none() {
        return Err("FXG_HOME must point at a throwaway directory".into());
    }

    let mut args = std::env::args().skip(1);
    let sessions: usize = parse_arg(args.next(), 250)?;
    let events: usize = parse_arg(args.next(), 600)?;
    let projects: usize = parse_arg(args.next(), 1)?.max(1);
    let layout = args.next().unwrap_or_else(|| "interleaved".to_owned());
    if layout != "interleaved" && layout != "blocks" {
        return Err(format!("unknown layout: {layout} (use interleaved or blocks)").into());
    }

    let env = |key: &str| std::env::var(key).ok();
    let paths = Paths::resolve(&env, Role::Node);
    let db_path = paths.db_path();
    println!("seeding {}", db_path.display());

    let db = Db::open(&db_path, Role::Node).await?;
    db.upsert_node(&NodeRecord::new(
        NODE_ID,
        "Seed Node",
        std::env::consts::OS,
        std::env::consts::ARCH,
        env!("CARGO_PKG_VERSION"),
    ))
    .await?;

    let specs: Vec<ProjectSpec> = (0..projects)
        .map(|index| ProjectSpec {
            id: format!("seed.local/pagination-demo-{index}"),
            name: format!("pagination-demo-{index}"),
            path: format!("/tmp/fxg-pagination-demo-{index}"),
        })
        .collect();
    for spec in &specs {
        db.upsert_project(&ProjectRecord {
            project_id: spec.id.clone(),
            name: spec.name.clone(),
            canonical_git_url: None,
            identity: ProjectIdentity::local(),
        })
        .await?;
        db.upsert_project_binding(&ProjectBindingRecord {
            project_id: spec.id.clone(),
            node_id: NODE_ID.to_owned(),
            local_path: spec.path.clone(),
            is_worktree: false,
            path_exists: true,
            git_branch: Some("main".to_owned()),
        })
        .await?;
    }

    // Spread `updated_at` over the past so the keyset order is stable and
    // visible in the sidebar.
    let base = now_ms() - (sessions as i64 + 1) * 60_000;
    let mut chat_session_id = None;

    for index in 0..sessions {
        let session_id = uuid_v7();
        let created_at = base + index as i64 * 60_000;
        let is_chat = index + 1 == sessions;
        if is_chat {
            chat_session_id = Some(session_id.clone());
        }
        let project_index = if layout == "blocks" {
            index * projects / sessions.max(1)
        } else {
            index % projects
        };
        let project = &specs[project_index];

        let mut batch = vec![SessionEventEnvelope {
            event_id: uuid_v7().into(),
            session_id: session_id.clone().into(),
            node_seq: 1,
            created_at,
            payload: UnifiedEventPayload::SessionCreated {
                node_id: NODE_ID.into(),
                project_id: project.id.clone().into(),
                project_name: project.name.clone(),
                local_path: project.path.clone(),
                git_branch: Some("main".to_owned()),
                is_worktree: false,
                agent_id: "seed".to_owned(),
                parent_session_id: None,
                title: if is_chat {
                    format!("Seeded chat ({events} events)")
                } else {
                    format!("Seeded session {:03} ({})", index + 1, project.name)
                },
            },
        }];

        let mut last_at = created_at;
        if is_chat {
            for event_index in 0..events {
                let is_user = event_index % 2 == 0;
                let payload = if is_user {
                    UnifiedEventPayload::UserMessage {
                        text: format!(
                            "Question {} in the seeded pagination session. \
                             This text is long enough to exercise the chat layout.",
                            event_index / 2 + 1
                        ),
                        attachments: vec![],
                        client_source: ClientSource::Web,
                        agent_message_id: None,
                    }
                } else {
                    UnifiedEventPayload::AgentMessage {
                        message_id: format!("seed-msg-{event_index}"),
                        text: format!(
                            "Answer {} in the seeded pagination session. \
                             Load earlier messages to walk back through the pages.",
                            event_index / 2 + 1
                        ),
                        is_complete: true,
                        subagent_id: None,
                    }
                };
                last_at = created_at + event_index as i64 * 1_000;
                batch.push(SessionEventEnvelope {
                    event_id: uuid_v7().into(),
                    session_id: session_id.clone().into(),
                    node_seq: event_index as u64 + 2,
                    created_at: last_at,
                    payload,
                });
            }
        }

        // Seed a terminal status: the daemon rewrites active sessions to
        // `stopped` on restart, which would flatten the `updated_at` order.
        batch.push(SessionEventEnvelope {
            event_id: uuid_v7().into(),
            session_id: session_id.clone().into(),
            node_seq: batch.len() as u64 + 1,
            created_at: last_at + 1,
            payload: UnifiedEventPayload::StatusChanged {
                status: SessionStatus::Stopped,
                error_message: None,
            },
        });

        db.append_events(&batch).await?;
    }

    println!(
        "seeded {sessions} sessions across {projects} project(s), {events} chat events, layout={layout}"
    );
    for spec in &specs {
        println!("project: {} ({})", spec.name, spec.id);
    }
    if let Some(session_id) = chat_session_id {
        println!("chat session: {session_id}");
    }
    db.close().await;
    Ok(())
}

fn parse_arg(value: Option<String>, default: usize) -> Result<usize, Box<dyn std::error::Error>> {
    match value {
        Some(value) => Ok(value.parse()?),
        None => Ok(default),
    }
}

//! Settings, project-link, profile and secret-metadata persistence.

use std::collections::BTreeMap;

use fxg_db::{Db, DbError, DbRole};
use fxg_protocol::settings::{
    AgentSettings, ConfigSource, GitCredentialConfig, NodeSettings, ProjectIdentity,
    ProvisionerDefinition, SourcedValue, UpdateAgentSettingsRequest, UpdateHubSettingsRequest,
    UpdateNodeSettingsRequest, UpdateProjectProfileRequest,
};
use fxg_protocol::{ProjectId, SecretId};

async fn db() -> Db {
    Db::open_in_memory(DbRole::Node).await.expect("db")
}

fn node_settings_request(revision: u64, name: &str) -> UpdateNodeSettingsRequest {
    UpdateNodeSettingsRequest {
        name: name.to_owned(),
        allow_remote_pty: true,
        project_scan_dirs: vec!["/src".to_owned()],
        tray: Some(false),
        expected_revision: revision,
    }
}

#[tokio::test]
async fn node_settings_use_optimistic_revision() {
    let db = db().await;
    let initial: NodeSettings = db.ensure_node_settings("Node").await.expect("ensure");
    assert_eq!(initial.revision, 1);
    assert_eq!(initial.name, "Node");

    let updated = db
        .update_node_settings(&node_settings_request(1, "Renamed"))
        .await
        .expect("update");
    assert_eq!(updated.name, "Renamed");
    assert!(updated.allow_remote_pty);
    assert_eq!(updated.revision, 2);

    // A stale revision must not overwrite the newer value.
    let err = db
        .update_node_settings(&node_settings_request(1, "Stale"))
        .await
        .expect_err("stale revision must fail");
    assert!(matches!(err, DbError::RevisionMismatch));
    assert_eq!(db.node_settings().await.unwrap().unwrap().name, "Renamed");
}

#[tokio::test]
async fn agent_settings_and_aliases_roundtrip() {
    let db = db().await;
    let initial: AgentSettings = db.ensure_agent_settings().await.expect("ensure");
    assert!(initial.aliases.is_empty());

    let updated = db
        .update_agent_settings(&UpdateAgentSettingsRequest {
            default_agent: Some("opencode2".to_owned()),
            registry_url: Some("https://example.test/registry.json".to_owned()),
            registry_cache_ttl_secs: Some(60),
            aliases: BTreeMap::from([("fast".to_owned(), "opencode2".to_owned())]),
            expected_revision: initial.revision,
        })
        .await
        .expect("update");
    assert_eq!(updated.default_agent.as_deref(), Some("opencode2"));
    assert_eq!(
        updated.aliases.get("fast").map(String::as_str),
        Some("opencode2")
    );
    assert_eq!(updated.resolved_registry_cache_ttl_secs(), 60);

    let err = db
        .update_agent_settings(&UpdateAgentSettingsRequest {
            default_agent: None,
            registry_url: None,
            registry_cache_ttl_secs: None,
            aliases: BTreeMap::new(),
            expected_revision: 1,
        })
        .await
        .expect_err("stale revision must fail");
    assert!(matches!(err, DbError::RevisionMismatch));
}

#[tokio::test]
async fn project_path_links_are_upserted_and_deleted() {
    let db = db().await;
    db.upsert_project_path_link("/src/repo", "team/repo")
        .await
        .expect("link");
    assert_eq!(
        db.project_key_for_path("/src/repo")
            .await
            .unwrap()
            .as_deref(),
        Some("team/repo")
    );
    db.upsert_project_path_link("/src/repo", "team/other")
        .await
        .expect("relink");
    assert_eq!(
        db.project_key_for_path("/src/repo")
            .await
            .unwrap()
            .as_deref(),
        Some("team/other")
    );
    assert_eq!(db.delete_project_path_link("/src/repo").await.unwrap(), 1);
    assert!(
        db.project_key_for_path("/src/repo")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn prune_removes_unreported_bindings_and_relinks_sessions() {
    let node = DbRole::Hub;
    let db = Db::open_in_memory(node).await.expect("db");
    db.upsert_node(&fxg_db::NodeRecord::new(
        "node-a", "Node A", "linux", "x86_64", "0.1.0",
    ))
    .await
    .expect("node");

    for (project_id, path) in [
        ("github.com/acme/one", "/src/one"),
        ("github.com/acme/two", "/src/two"),
    ] {
        db.upsert_project(&fxg_db::ProjectRecord {
            project_id: project_id.to_owned(),
            name: project_id.rsplit('/').next().unwrap().to_owned(),
            canonical_git_url: None,
            identity: ProjectIdentity::git_remote(),
        })
        .await
        .expect("project");
        db.upsert_project_binding(&fxg_db::ProjectBindingRecord {
            project_id: project_id.to_owned(),
            node_id: "node-a".to_owned(),
            local_path: path.to_owned(),
            is_worktree: false,
            path_exists: true,
            git_branch: Some("main".to_owned()),
        })
        .await
        .expect("binding");
    }

    db.upsert_provisional_session(&fxg_db::registration::ProvisionalSessionRecord {
        session_id: "s1".to_owned(),
        project_id: "github.com/acme/one".to_owned(),
        project_name: "one".to_owned(),
        node_id: "node-a".to_owned(),
        local_path: "/src/one".to_owned(),
        git_branch: None,
        agent_id: "opencode".to_owned(),
        title: "session".to_owned(),
    })
    .await
    .expect("session");

    // Move the session to the new project and prune the old binding.
    db.reassign_sessions_project(
        "node-a",
        &[("/src/one".to_owned(), "github.com/acme/two".to_owned())],
    )
    .await
    .expect("reassign");
    let session = db.get_session("s1").await.unwrap().expect("session row");
    assert_eq!(session.project_id.as_str(), "github.com/acme/two");

    let pruned = db
        .prune_node_project_bindings("node-a", &["/src/two".to_owned()])
        .await
        .expect("prune");
    assert_eq!(pruned, 1);
    db.delete_orphan_projects().await.expect("orphans");
    let projects = db.list_projects().await.expect("projects");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].project_id.as_str(), "github.com/acme/two");
}

#[tokio::test]
async fn hub_settings_provisioners_and_profiles() {
    let db = Db::open_in_memory(DbRole::Hub).await.expect("db");
    let initial = db.ensure_hub_settings().await.expect("ensure");

    let updated = db
        .update_hub_settings(&UpdateHubSettingsRequest {
            vapid_subject: Some("mailto:admin@example.com".to_owned()),
            git_credentials: BTreeMap::from([(
                "github.com".to_owned(),
                GitCredentialConfig::Secret {
                    username: Some("git".to_owned()),
                    secret_id: SecretId::new("git-token"),
                },
            )]),
            expected_revision: initial.revision,
        })
        .await
        .expect("hub settings");
    assert_eq!(
        updated.vapid_subject.as_deref(),
        Some("mailto:admin@example.com")
    );
    match updated.git_credentials.get("github.com").expect("github") {
        GitCredentialConfig::Secret { secret_id, .. } => {
            assert_eq!(secret_id.as_str(), "git-token");
        }
        other @ (GitCredentialConfig::GhCli { .. } | GitCredentialConfig::Env { .. }) => {
            panic!("unexpected provider: {other:?}")
        }
    }

    db.upsert_provisioner(&ProvisionerDefinition {
        name: "docker".to_owned(),
        description: Some("Local".to_owned()),
        command: "docker".to_owned(),
        args: vec!["run".to_owned()],
        idle_timeout_secs: Some(900),
        env: BTreeMap::from([("KEY".to_owned(), "value".to_owned())]),
    })
    .await
    .expect("provisioner");
    let provisioners = db.provisioners().await.expect("list");
    assert_eq!(provisioners["docker"].args, vec!["run"]);
    assert_eq!(provisioners["docker"].env["KEY"], "value");

    let project_id = ProjectId::from("github.com/acme/one");
    db.upsert_project(&fxg_db::ProjectRecord {
        project_id: project_id.as_str().to_owned(),
        name: "one".to_owned(),
        canonical_git_url: None,
        identity: ProjectIdentity::git_remote(),
    })
    .await
    .expect("project");
    let profile = db
        .upsert_project_profile(
            &project_id,
            &UpdateProjectProfileRequest {
                bootstrap_setup: vec!["cargo fetch".to_owned()],
                bootstrap_env: BTreeMap::from([("RUST_BACKTRACE".to_owned(), "1".to_owned())]),
                expected_revision: 0,
            },
        )
        .await
        .expect("profile");
    assert_eq!(profile.revision, 1);
    let err = db
        .upsert_project_profile(
            &project_id,
            &UpdateProjectProfileRequest {
                bootstrap_setup: Vec::new(),
                bootstrap_env: BTreeMap::new(),
                expected_revision: 0,
            },
        )
        .await
        .expect_err("stale profile revision");
    assert!(matches!(err, DbError::RevisionMismatch));
}

#[tokio::test]
async fn secret_metadata_roundtrip() {
    let db = db().await;
    let id = SecretId::new("secret-1");
    db.upsert_secret_metadata(&id, Some("API key"))
        .await
        .expect("insert");
    let secrets = db.secrets().await.expect("list");
    assert_eq!(secrets.len(), 1);
    assert_eq!(secrets[0].id, id);
    assert_eq!(secrets[0].description.as_deref(), Some("API key"));

    db.upsert_secret_metadata(&id, Some("Renamed"))
        .await
        .expect("update");
    assert_eq!(
        db.secret(&id)
            .await
            .unwrap()
            .unwrap()
            .description
            .as_deref(),
        Some("Renamed")
    );
    assert_eq!(db.delete_secret_metadata(&id).await.unwrap(), 1);
    assert!(db.secret(&id).await.unwrap().is_none());
}

#[test]
fn sourced_value_serializes_with_source() {
    let value = SourcedValue::new("/tmp/fxg", ConfigSource::Env, Some("FXG_HOME"));
    let json = serde_json::to_value(value).expect("serialize");
    assert_eq!(json["value"], "/tmp/fxg");
    assert_eq!(json["source"], "env");
    assert_eq!(json["source_detail"], "FXG_HOME");
}

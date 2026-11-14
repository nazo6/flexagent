//! `fxg doctor` command.

use anyhow::{Result, bail};
use fxg_protocol::paths::{Paths, Role};
use usage::Args;

use crate::output::{KvList, KvRow};

/// `fxg doctor`
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

/// One path in the doctor report.
struct PathEntry {
    name: &'static str,
    path: std::path::PathBuf,
}

/// Lists the role-scoped paths a user may need to inspect.
fn role_paths(paths: &Paths) -> Vec<PathEntry> {
    let mut entries = vec![
        PathEntry {
            name: "config",
            path: paths.config_path(),
        },
        PathEntry {
            name: "database",
            path: paths.db_path(),
        },
        PathEntry {
            name: "auth_token",
            path: paths.auth_token_path(),
        },
        PathEntry {
            name: "secrets",
            path: paths.secrets_dir(),
        },
        PathEntry {
            name: "addr",
            path: paths.addr_path(),
        },
        PathEntry {
            name: "logs",
            path: paths.log_dir(),
        },
        PathEntry {
            name: "cache",
            path: paths.cache_dir(),
        },
        PathEntry {
            name: "vapid",
            path: paths.vapid_path(),
        },
    ];
    if paths.role() == Role::Node {
        entries.push(PathEntry {
            name: "node_token",
            path: paths.node_token_path(),
        });
        entries.push(PathEntry {
            name: "worktrees",
            path: paths.worktrees_dir(),
        });
    }
    entries
}

impl usage::RunAsync for DoctorArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let node = Paths::resolve(&env, Role::Node);
        let hub = Paths::resolve(&env, Role::Hub);
        let mut checks: Vec<(String, bool, String)> = Vec::new();

        let node_config = fxg_protocol::config::NodeConfig::load(&env);
        let hub_config = fxg_protocol::config::HubConfig::load(&env);
        let config_ok = match (&node_config, &hub_config) {
            (Ok(_), Ok(_)) => (true, "node.toml / hub.toml are readable".to_owned()),
            (Err(err), _) => (false, format!("node.toml: {err}")),
            (_, Err(err)) => (false, format!("hub.toml: {err}")),
        };
        checks.push(("config".to_owned(), config_ok.0, config_ok.1));

        checks.push((
            "node data".to_owned(),
            true,
            if node.data_dir().is_dir() {
                node.data_dir().display().to_string()
            } else {
                format!(
                    "{} (created on first node start)",
                    node.data_dir().display()
                )
            },
        ));
        checks.push((
            "hub data".to_owned(),
            true,
            if hub.data_dir().is_dir() {
                hub.data_dir().display().to_string()
            } else {
                format!("{} (created on first hub start)", hub.data_dir().display())
            },
        ));

        let db_path = node.db_path();
        let db_ok = if db_path.is_file() {
            match fxg_db::Db::open(&db_path, Role::Node).await {
                Ok(_) => (true, "node.db opened".to_owned()),
                Err(err) => (false, format!("node.db: {err}")),
            }
        } else {
            (true, "not initialized yet".to_owned())
        };
        checks.push(("database".to_owned(), db_ok.0, db_ok.1));

        checks.push((
            "logs".to_owned(),
            true,
            if node.log_path().is_file() {
                node.log_path().display().to_string()
            } else {
                format!("{} (not created yet)", node.log_path().display())
            },
        ));

        let git_ok = std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        checks.push((
            "git".to_owned(),
            git_ok,
            if git_ok {
                "git is available".to_owned()
            } else {
                "git was not found on PATH".to_owned()
            },
        ));

        let roles = [(Role::Node, &node), (Role::Hub, &hub)];
        if self.json {
            let checks: Vec<_> = checks
                .iter()
                .map(|(name, ok, detail)| {
                    serde_json::json!({ "check": name, "ok": ok, "detail": detail })
                })
                .collect();
            let layouts: Vec<_> = roles
                .iter()
                .map(|(role, paths)| {
                    let entries: Vec<_> = role_paths(paths)
                        .into_iter()
                        .map(|entry| {
                            serde_json::json!({
                                "name": entry.name,
                                "path": entry.path.display().to_string(),
                                "exists": entry.path.exists(),
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "role": role.as_str(),
                        "roots": {
                            "config": paths.roots().config().display().to_string(),
                            "data": paths.roots().data().display().to_string(),
                            "state": paths.roots().state().display().to_string(),
                            "logs": paths.roots().logs().display().to_string(),
                            "cache": paths.roots().cache().display().to_string(),
                        },
                        "paths": entries,
                    })
                })
                .collect();
            let value = serde_json::json!({
                "checks": checks,
                "layouts": layouts,
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            for (name, ok, detail) in &checks {
                println!("{} {name}: {detail}", if *ok { "OK  " } else { "FAIL" });
            }
            for (role, paths) in roles {
                println!("\n{} paths:", role);
                println!(
                    "{}",
                    KvList::new()
                        .indent(2)
                        .gap(1)
                        .field(
                            "roots:",
                            format!("config={}", paths.roots().config().display()),
                        )
                        .push(KvRow::continuation(format!(
                            "data={}",
                            paths.roots().data().display()
                        )))
                        .push(KvRow::continuation(format!(
                            "state={}",
                            paths.roots().state().display()
                        )))
                        .push(KvRow::continuation(format!(
                            "logs={}",
                            paths.roots().logs().display()
                        )))
                        .push(KvRow::continuation(format!(
                            "cache={}",
                            paths.roots().cache().display()
                        )))
                        .render(false)
                );
                let mut path_list = KvList::new().indent(2);
                for entry in role_paths(paths) {
                    let state = if entry.path.exists() {
                        ""
                    } else {
                        " (not created yet)"
                    };
                    path_list =
                        path_list.field(entry.name, format!("{}{}", entry.path.display(), state));
                }
                println!("{}", path_list.render(false));
            }
        }
        if checks.iter().any(|(_, ok, _)| !ok) {
            bail!("doctor found failing checks");
        }
        Ok(())
    }
}

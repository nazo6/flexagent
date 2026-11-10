//! `fxg doctor` command.

use anyhow::{Result, bail};
use usage::Args;

/// `fxg doctor`
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for DoctorArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let env = fxg_protocol::config::process_env;
        let home = fxg_protocol::config::fxg_home(&env);
        let mut checks: Vec<(String, bool, String)> = Vec::new();

        let config_ok = match fxg_protocol::config::GlobalConfig::load(&env) {
            Ok(_) => (true, "config.toml is readable".to_owned()),
            Err(err) => (false, format!("config.toml: {err}")),
        };
        checks.push(("config".to_owned(), config_ok.0, config_ok.1));

        checks.push((
            "home".to_owned(),
            true,
            if home.is_dir() {
                home.display().to_string()
            } else {
                format!("{} (created on first node start)", home.display())
            },
        ));

        let token_path = home.join(fxg_protocol::config::AUTH_TOKEN_FILE_NAME);
        checks.push((
            "auth_token".to_owned(),
            true,
            if token_path.is_file() {
                token_path.display().to_string()
            } else {
                format!("{} (created on first node start)", token_path.display())
            },
        ));

        let db_path = home.join("node.db");
        let db_ok = if db_path.is_file() {
            match fxg_db::Db::open(&db_path, fxg_db::DbRole::Node).await {
                Ok(_) => (true, "node.db opened".to_owned()),
                Err(err) => (false, format!("node.db: {err}")),
            }
        } else {
            (true, "not initialized yet".to_owned())
        };
        checks.push(("database".to_owned(), db_ok.0, db_ok.1));

        let log_path = home.join("logs").join("node.log");
        checks.push((
            "logs".to_owned(),
            true,
            if log_path.is_file() {
                log_path.display().to_string()
            } else {
                format!("{} (not created yet)", log_path.display())
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

        if self.json {
            let value: Vec<_> = checks
                .iter()
                .map(|(name, ok, detail)| {
                    serde_json::json!({ "check": name, "ok": ok, "detail": detail })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            for (name, ok, detail) in &checks {
                println!("{} {name}: {detail}", if *ok { "OK  " } else { "FAIL" });
            }
        }
        if checks.iter().any(|(_, ok, _)| !ok) {
            bail!("doctor found failing checks");
        }
        Ok(())
    }
}

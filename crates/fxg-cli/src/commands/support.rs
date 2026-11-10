//! `fxg support` commands (support bundle / persisted incidents).

use std::path::Path;

use anyhow::{Context, Result};
use fxg_protocol::common::IncidentEntry;
use usage::{Args, Subcommands};

use crate::client::NodeClient;

/// `fxg support <command>`
#[derive(Debug, Args)]
#[usage(run_async, arg_required_else_help)]
pub struct SupportArgs {
    #[usage(subcommand)]
    command: SupportCommands,
}

/// Support artifact commands.
#[derive(Debug, Subcommands)]
#[usage(run_async)]
pub enum SupportCommands {
    /// Write a redacted support bundle (config, health, incidents, logs)
    Bundle(SupportBundleArgs),
    /// List persisted incidents (newest first)
    Incidents(SupportIncidentsArgs),
}

/// `fxg support bundle`
#[derive(Debug, Args)]
pub struct SupportBundleArgs {
    /// Output path (`-` writes the bundle to stdout)
    #[usage(short = 'o', long)]
    output: Option<String>,
}

impl usage::RunAsync for SupportBundleArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        let bundle = client.support_bundle().await?;
        let json = serde_json::to_string_pretty(&bundle)?;
        match self.output.as_deref() {
            Some("-") => println!("{json}"),
            Some(path) => {
                std::fs::write(path, format!("{json}\n"))
                    .with_context(|| format!("failed to write {path}"))?;
                println!("wrote support bundle to {path}");
            }
            None => {
                let path = format!("fxg-support-{}.json", bundle.generated_at);
                std::fs::write(&path, format!("{json}\n"))
                    .with_context(|| format!("failed to write {path}"))?;
                println!("wrote support bundle to {}", Path::new(&path).display());
            }
        }
        Ok(())
    }
}

/// `fxg support incidents`
#[derive(Debug, Args)]
pub struct SupportIncidentsArgs {
    /// Maximum number of incidents to show
    #[usage(long, default = "50")]
    limit: u32,
    /// Include resolved incidents
    #[usage(long)]
    all: bool,
    /// Output as JSON
    #[usage(long)]
    json: bool,
}

impl usage::RunAsync for SupportIncidentsArgs {
    type Output = Result<()>;

    async fn run_async(self) -> Self::Output {
        let mut client = NodeClient::connect().await?;
        let response = client.incidents(self.limit, self.all).await?;
        if self.json {
            println!("{}", serde_json::to_string_pretty(&response)?);
            return Ok(());
        }
        if response.incidents.is_empty() {
            println!("(no incidents recorded)");
            return Ok(());
        }
        for incident in &response.incidents {
            print_incident(incident);
        }
        Ok(())
    }
}

fn print_incident(incident: &IncidentEntry) {
    let resolved = if incident.resolved_at.is_some() {
        " resolved"
    } else {
        ""
    };
    println!(
        "{} {} {} [{}]{} x{}",
        incident.last_seen_at,
        incident.level.as_str(),
        incident.code,
        incident.source.as_str(),
        resolved,
        incident.occurrence_count
    );
    println!("  {}", incident.summary);
    if let Some(session_id) = incident.session_id.as_ref() {
        println!("  session: {session_id}");
    }
    if let Some(request_id) = incident.request_id.as_deref() {
        println!("  request: {request_id}");
    }
}

use anyhow::Result;
use clap::Parser;
use purrcode_daemon::{DaemonConfig, bind_and_report_with_research_gate};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "purrcoded", version)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:7377")]
    bind: SocketAddr,
    #[arg(long)]
    allow_public_bind: bool,
    /// Explicitly enable the research-only alignment=disabled session arm.
    #[arg(long)]
    allow_research_alignment_disabled: bool,
    #[arg(long)]
    database: PathBuf,
    #[arg(long)]
    token_file: PathBuf,
    #[arg(long)]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let (report, server) = bind_and_report_with_research_gate(
        DaemonConfig {
            bind: args.bind,
            allow_public_bind: args.allow_public_bind,
            database: args.database,
            token_file: args.token_file,
            app_config: args.config,
        },
        args.allow_research_alignment_disabled,
    )
    .await?;
    println!("{}", serde_json::to_string(&report)?);
    server.await?;
    Ok(())
}

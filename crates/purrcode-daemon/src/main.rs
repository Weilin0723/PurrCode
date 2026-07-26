use anyhow::Result;
use clap::Parser;
use purrcode_daemon::{bind_and_report, DaemonConfig};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "purrcoded", version)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:7377")]
    bind: SocketAddr,
    #[arg(long)]
    allow_public_bind: bool,
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
    let (report, server) = bind_and_report(DaemonConfig {
        bind: args.bind,
        allow_public_bind: args.allow_public_bind,
        database: args.database,
        token_file: args.token_file,
        app_config: args.config,
    })
    .await?;
    println!("{}", serde_json::to_string(&report)?);
    server.await?;
    Ok(())
}

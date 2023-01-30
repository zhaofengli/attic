use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::cli::Opts;

#[derive(Debug, Parser)]
#[command(about = "Queue paths to upload")]
pub struct Queue {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Daemon(Daemon),
    Relay(Relay),
}

#[derive(Debug, Parser)]
#[command(about = "Start daemon that uploads paths received via the relay")]
struct Daemon {}

#[derive(Debug, Parser)]
#[command(about = "Relay paths to the daemon for uploading")]
struct Relay {}

pub async fn run(_: Opts) -> Result<()> {
    Ok(())
}

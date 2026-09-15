use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::cli::Opts;
use crate::config::Config;

/// Manage configured Attic servers.
#[derive(Debug, Parser)]
pub struct Server {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(alias = "ls")]
    List,
}

pub async fn run(opts: Opts) -> Result<()> {
    let sub = opts.command.as_server().unwrap();

    match sub.command {
        Command::List => list_servers(),
    }
}

fn list_servers() -> Result<()> {
    let config = Config::load()?;
    let mut server_names: Vec<_> = config.servers.keys().collect();
    server_names.sort_by(|a, b| a.as_str().cmp(b.as_str()));

    for server_name in server_names {
        println!("{}", server_name.as_str());
    }

    Ok(())
}

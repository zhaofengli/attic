mod command;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use enum_as_inner::EnumAsInner;

use attic_server::config;
use command::make_token::{self, MakeToken};

/// Attic server administration utilities.
#[derive(Debug, Parser)]
#[clap(version, author = "Zhaofeng Li <hello@zhaofeng.li>")]
#[clap(propagate_version = true)]
pub struct Opts {
    /// Path to the config file.
    #[clap(short = 'f', long, global = true)]
    config: Option<PathBuf>,

    /// The sub-command.
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand, EnumAsInner)]
pub enum Command {
    MakeToken(MakeToken),
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    match opts.command {
        Command::MakeToken(_) => {
            if let Some(config) = config::load_config(opts.config.as_deref()).await {
                make_token::run(config, opts).await?;
            } else {
                eprintln!();
                eprintln!("No config found, please provide a config.toml with -f <CONFIG_PATH or --config <CONFIG_PATH>");
            }
        }
    }
    Ok(())
}

mod command;

use std::env;
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
    #[clap(short = 'f', long)]
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
    let config = if let Some(config_path) = &opts.config {
        config::load_config_from_path(config_path)
    } else if let Ok(config_env) = env::var("ATTIC_SERVER_CONFIG_BASE64") {
        let decoded = String::from_utf8(base64::decode(config_env.as_bytes())?)?;
        config::load_config_from_str(&decoded)
    } else {
        config::load_config_from_path(&config::get_xdg_config_path()?)
    };

    match opts.command {
        Command::MakeToken(_) => make_token::run(config, opts).await?,
    }

    Ok(())
}

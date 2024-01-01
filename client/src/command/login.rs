use anyhow::Result;
use clap::Parser;

use crate::cache::ServerName;
use crate::cli::Opts;
use crate::config::{Config, ServerConfig, ServerTokenConfig};

/// Log into an Attic server.
#[derive(Debug, Parser)]
pub struct Login {
    /// Name of the server.
    name: ServerName,

    /// Endpoint of the server.
    endpoint: String,

    /// Access token.
    token: Option<String>,

    /// Set the server as the default.
    #[clap(long)]
    set_default: bool,
}

pub async fn run(opts: Opts) -> Result<()> {
    let sub = opts.command.as_login().unwrap();
    let mut config = Config::load()?;
    let mut config_m = config.as_mut();

    if let Some(server) = config_m.servers.get_mut(&sub.name) {
        eprintln!("✍️ Overwriting server \"{}\"", sub.name.as_str());

        server.endpoint = sub.endpoint.to_owned();

        if let Some(token) = &sub.token {
            server.token = Some(ServerTokenConfig::Raw {
                token: token.clone(),
            });
        }
    } else {
        eprintln!("✍️ Configuring server \"{}\"", sub.name.as_str());

        config_m.servers.insert(
            sub.name.to_owned(),
            ServerConfig {
                endpoint: sub.endpoint.to_owned(),
                token: sub
                    .token
                    .to_owned()
                    .map(|token| ServerTokenConfig::Raw { token }),
            },
        );
    }

    if sub.set_default || config_m.servers.len() == 1 {
        config_m.default_server = Some(sub.name.to_owned());
    }

    Ok(())
}

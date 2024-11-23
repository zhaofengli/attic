use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use attic_server::config::generate_monolithic_config;
use attic_server::config::load_config;
use clap::{Parser, ValueEnum};
use tokio::join;
use tokio::task::spawn;
use tracing_error::ErrorLayer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use attic_server::config;
use attic_server::config::Config;

/// Nix binary cache server.
#[derive(Debug, Parser)]
#[clap(version, author = "Zhaofeng Li <hello@zhaofeng.li>")]
#[clap(propagate_version = true)]
struct Opts {
    /// Path to the config file.
    #[clap(short = 'f', long)]
    config: Option<PathBuf>,

    /// Socket address to listen on.
    ///
    /// This overrides `listen` in the config.
    #[clap(short = 'l', long)]
    listen: Option<SocketAddr>,

    /// Mode to run.
    #[clap(long, default_value = "monolithic")]
    mode: ServerMode,

    /// Whether to enable tokio-console.
    ///
    /// The console server will listen on its default port.
    #[clap(long)]
    tokio_console: bool,

    /// A flag that prompts the server to generate a root token with a provided configuration
    #[clap(long)]
    init: bool
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ServerMode {
    /// Run all components.
    Monolithic,

    /// Run the API server.
    ApiServer,

    /// Run the garbage collector periodically.
    GarbageCollector,

    /// Run the database migrations then exit.
    DbMigrations,

    /// Run garbage collection then exit.
    GarbageCollectorOnce,

    /// Check the configuration then exit.
    CheckConfig,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();

    init_logging(opts.tokio_console);
    dump_version();

    match opts.mode {
        ServerMode::Monolithic => {
            //seek configuration, start monolithic run
            if let Some(config) = config::load_config(opts.config.as_deref()).await {
                //if we're told to reinit, reinit using provided config, or just pass it back
                if opts.init == true {
                    config::reinit_from_config(config.clone()).await?;
                    attic_server::run_migrations(config.clone()).await?;
                }
                //run server                
                run_monolithic(opts, config).await?;
            } else {
                //no config present, generate monolithic config and run
                generate_monolithic_config().await?;
                let config_path = config::get_xdg_config_path()?;

                if let Some(config) = config::load_config(Some(&config_path)).await {
                    run_monolithic(opts, config).await?;
                } else {
                    todo!("How could we get here?");
                }
            }
        }
        ServerMode::ApiServer => {
            if let Some(config) = config::load_config(opts.config.as_deref()).await {
                //if we're told to reinit, reinit using provided config
                if opts.init == true {
                    config::reinit_from_config(config.clone()).await?;
                    
                    //assuming this is a fresh setup, run db migrations to ready db
                    //TODO: What if it's *not* a fresh setup? Perhaps this can happen with another flag, rather than only happening during one mode
                    attic_server::run_migrations(config.clone()).await?;
                }
                attic_server::run_api_server(opts.listen, config).await?;
            } else {
                //Exit gracefully, no config present
                display_no_config_msg();
            }
        }
        ServerMode::GarbageCollector => {
            if let Some(config) = config::load_config(opts.config.as_deref()).await {
                if opts.init == true {
                    config::reinit_from_config(config.clone()).await?;
                }
                attic_server::gc::run_garbage_collection(config.clone()).await;
            } else {
                //Exit gracefully, no config present
                display_no_config_msg();
            }
            
        }
        ServerMode::DbMigrations => {
            if let Some(config) = config::load_config(opts.config.as_deref()).await {
                if opts.init == true {
                    config::reinit_from_config(config.clone()).await?;
                }
                attic_server::run_migrations(config).await?;
            } else {
                //Exit gracefully, no config present
                display_no_config_msg();
            }
        }
        ServerMode::GarbageCollectorOnce => {
            if let Some(config) = config::load_config(opts.config.as_deref()).await {
                if opts.init == true {
                    config::reinit_from_config(config.clone()).await?;
                }
                attic_server::gc::run_garbage_collection_once(config).await?;
            } else {
                //Exit gracefully, no config present
                display_no_config_msg();
            }
        }
        ServerMode::CheckConfig => {
            //validate config and exit
            //TODO: What other things would be nice to check here? Do we want dry runs with tokens maybe?
            if let Some(_) = config::load_config(opts.config.as_deref()).await {
                eprintln!();
                eprintln!("-----------------");
                eprintln!();
                eprintln!("Config looks good!");
                eprintln!("Documentations and guides:");
                eprintln!();
                eprintln!("    https://docs.attic.rs");
                eprintln!();
                eprintln!("Enjoy!");
                eprintln!("-----------------");
                eprintln!(); 
            } else {
                //Exit gracefully, no config present
                display_no_config_msg();
            }
            
        }
    }

    Ok(())
}

/// Runs the server in monolithic mode
async fn run_monolithic(opts: Opts, config: Config) -> Result<()> {
    let (api_server, _) = join!(
        attic_server::run_api_server(opts.listen, config.clone()),
        attic_server::gc::run_garbage_collection(config.clone()),
    );

    match api_server {
        Ok(()) => Ok(()),
        Err(e) => Err(e)
    }
}

fn display_no_config_msg() {
    eprintln!();
    eprintln!("No config found, please provide a config.toml file");
}

fn init_logging(tokio_console: bool) {
    let env_filter = EnvFilter::from_default_env();
    let fmt_layer = tracing_subscriber::fmt::layer().with_filter(env_filter);

    let error_layer = ErrorLayer::default();

    let console_layer = if tokio_console {
        let (layer, server) = console_subscriber::ConsoleLayer::new();
        spawn(server.serve());
        Some(layer)
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(error_layer)
        .with(console_layer)
        .init();

    if tokio_console {
        eprintln!("Note: tokio-console is enabled");
    }
}

fn dump_version() {
    #[cfg(debug_assertions)]
    eprintln!("Attic Server {} (debug)", env!("CARGO_PKG_VERSION"));

    #[cfg(not(debug_assertions))]
    eprintln!("Attic Server {} (release)", env!("CARGO_PKG_VERSION"));
}

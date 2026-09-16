use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use attic_server::config;
use attic_server::telemetry;
use clap::{Parser, ValueEnum};
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::spawn;
use tokio_util::sync::CancellationToken;

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

    let telemetry = telemetry::init(opts.tokio_console)?;
    dump_version();

    let result = async {
        let config =
            config::load_config(opts.config.as_deref(), opts.mode == ServerMode::Monolithic)
                .await?;

        match opts.mode {
            ServerMode::Monolithic => {
                attic_server::run_migrations(config.clone()).await?;

                let shutdown = run_shutdown_handler();
                let gc_handle = spawn(attic_server::gc::run_garbage_collection(
                    config.clone(),
                    shutdown.clone(),
                ));

                let api_server =
                    attic_server::run_api_server(opts.listen, config.clone(), shutdown.clone())
                        .await;

                shutdown.cancel();
                let _ = gc_handle.await;

                api_server?;
            }
            ServerMode::ApiServer => {
                let shutdown = run_shutdown_handler();
                attic_server::run_api_server(opts.listen, config, shutdown).await?;
            }
            ServerMode::GarbageCollector => {
                let shutdown = run_shutdown_handler();
                attic_server::gc::run_garbage_collection(config.clone(), shutdown).await;
            }
            ServerMode::DbMigrations => {
                attic_server::run_migrations(config).await?;
            }
            ServerMode::GarbageCollectorOnce => {
                attic_server::gc::run_garbage_collection_once(config).await?;
            }
            ServerMode::CheckConfig => {
                // config is valid, let's just exit :)
            }
        }

        Result::<_, anyhow::Error>::Ok(())
    }
    .await;

    let shutdown_result = telemetry.shutdown();
    result?;
    shutdown_result
}

fn run_shutdown_handler() -> CancellationToken {
    async fn wait_for_signal(shutdown: CancellationToken) {
        let mut sigterm = signal(SignalKind::terminate()).expect("Could not listen for SIGTERM");
        let mut sigint = signal(SignalKind::interrupt()).expect("Could not listen for SIGINT");

        tokio::select! {
            _ = sigterm.recv() => {
                eprintln!("Received SIGTERM, exiting gracefully...");
            }
            _ = sigint.recv() => {
                eprintln!("Received SIGINT, exiting gracefully...");
            }
            _ = shutdown.cancelled() => {
                return;
            }
        }

        shutdown.cancel();
    }

    let shutdown = CancellationToken::new();

    spawn({
        let shutdown = shutdown.clone();
        async move {
            wait_for_signal(shutdown).await;
        }
    });

    shutdown
}

fn dump_version() {
    #[cfg(debug_assertions)]
    eprintln!("Attic Server {} (debug)", env!("CARGO_PKG_VERSION"));

    #[cfg(not(debug_assertions))]
    eprintln!("Attic Server {} (release)", env!("CARGO_PKG_VERSION"));
}

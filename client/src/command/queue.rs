use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use attic::nix_store::NixStore;
use clap::{Parser, Subcommand};
use indicatif::MultiProgress;
use tokio::fs::remove_file;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::broadcast::{self, Receiver, Sender};
use tokio::{select, spawn};

use crate::api::ApiClient;
use crate::cache::CacheRef;
use crate::cli::Opts;
use crate::config::Config;
use crate::push::{PushConfig, PushSession, PushSessionConfig, Pusher};

static DIR: &str = "/var/lib/attic/client";
static SOCKET_NAME: &str = "socket";

#[derive(Debug, Parser)]
#[command(about = "Queue paths to upload")]
pub struct Queue {
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Daemon(Daemon),
    Relay(Relay),
}

#[derive(Debug, Parser, Clone)]
#[command(about = "Start daemon that uploads paths received via the relay")]
pub struct Daemon {
    #[arg(help = "Name of cache to push build artifacts to")]
    cache: CacheRef,
}

#[derive(Debug, Parser)]
#[command(about = "Relay paths to the daemon for uploading")]
pub struct Relay {}

pub async fn run(options: Opts) -> Result<()> {
    if let Some(queue) = options.command.as_queue() {
        match &queue.command {
            Command::Daemon(daemon) => {
                run_daemon(daemon.clone()).await?;
            }
            Command::Relay(_) => {
                if let Err(error) = run_relay().await {
                    println!("An error occurred:");
                    println!("{error:#?}");
                }
            }
        }
    }

    Ok(())
}

async fn run_daemon(options: Daemon) -> Result<()> {
    let (shutdown, _) = broadcast::channel(1);

    let paths = spawn(handle_paths(options, shutdown.subscribe()));
    let shutdown = spawn(handle_shutdown(shutdown));

    shutdown.await??;
    paths.await??;

    Ok(())
}

async fn handle_shutdown(sender: Sender<bool>) -> Result<()> {
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sighup = signal(SignalKind::hangup())?;
    let mut sigquit = signal(SignalKind::quit())?;

    select!(
        Some(_) = sigterm.recv() => sender.send(true)?,
        Some(_) = sigint.recv() => sender.send(true)?,
        Some(_) = sighup.recv() => sender.send(true)?,
        Some(_) = sigquit.recv() => sender.send(true)?,
    );

    println!("Sent shutdown signal…");

    Ok(())
}

async fn handle_paths(options: Daemon, mut shutdown: Receiver<bool>) -> Result<()> {
    let socket_location = get_socket_location();
    let socket = UnixListener::bind(&socket_location)?;

    let conf = Config::load()?;
    let (_, server_conf, cache_name) = conf.resolve_cache(&options.cache)?;
    let mut api_client = ApiClient::from_server_config(server_conf.to_owned())?;
    let cache_conf = api_client.get_cache_config(cache_name).await?;
    api_client.set_endpoint(
        &cache_conf
            .api_endpoint
            .as_ref()
            .ok_or(anyhow!("Could not retrieve cache endpoint"))?,
    )?;

    let push_session = Pusher::new(
        Arc::new(NixStore::connect()?),
        api_client,
        cache_name.clone(),
        cache_conf,
        MultiProgress::new(),
        PushConfig {
            num_workers: 2,
            force_preamble: false,
        },
    )
    .into_push_session(PushSessionConfig {
        no_closure: false,
        ignore_upstream_cache_filter: false,
    });

    loop {
        select!(
            Ok(shutdown) = shutdown.recv() => { if shutdown { break; }; }
            Ok((stream, _)) = socket.accept() => {
                spawn(handle_connection(push_session.clone(), stream));
            },
        );
    }

    println!("Shutting down…");
    remove_file(socket_location).await?;

    Ok(())
}

async fn handle_connection(push_session: PushSession, mut stream: UnixStream) -> Result<()> {
    let mut received_paths = String::new();
    let mut store_paths = vec![];

    stream.readable().await?;
    stream.read_to_string(&mut received_paths).await?;

    let received_paths: Vec<PathBuf> = serde_json::from_str(&received_paths)?;

    let nix_store = NixStore::connect()?;
    for path in received_paths {
        let store_path = nix_store.parse_store_path(path)?;
        store_paths.push(store_path);
    }

    push_session.queue_many(store_paths.clone())?;

    if store_paths.len() == 1 {
        println!("Queued one path");
    } else {
        println!("Queued {} paths", store_paths.len())
    }

    Ok(())
}

async fn run_relay() -> Result<()> {
    let socket_location = get_socket_location();
    let mut socket = UnixStream::connect(&socket_location).await?;

    let paths: Vec<_> = env::var("OUT_PATHS")?
        .as_str()
        .split_whitespace()
        .map(PathBuf::from)
        .collect();
    let paths = serde_json::to_string(&paths)?;

    socket.writable().await?;
    socket.write_all(paths.as_bytes()).await?;
    socket.shutdown().await?;

    Ok(())
}

fn get_socket_location() -> PathBuf {
    PathBuf::from(DIR).join(SOCKET_NAME)
}

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use attic::nix_store::NixStore;
use clap::{Parser, Subcommand};
use indicatif::MultiProgress;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::spawn;
use tokio::sync::Mutex;

use crate::api::ApiClient;
use crate::cache::CacheRef;
use crate::cli::Opts;
use crate::config::Config;
use crate::push::{PushConfig, PushSessionConfig, Pusher};

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
    let paths: Vec<PathBuf> = Vec::new();
    let paths = Arc::new(Mutex::new(paths));

    let socket = spawn(receive_paths(paths.clone()));
    let upload = spawn(upload_paths(options, paths.clone()));

    socket.await??;
    upload.await??;

    Ok(())
}

async fn upload_paths(options: Daemon, paths: Arc<Mutex<Vec<PathBuf>>>) -> Result<()> {
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

    let nix_store = NixStore::connect()?;
    let nix_store = Arc::new(nix_store);
    let push_session = Pusher::new(
        Arc::clone(&nix_store),
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
        let mut paths = paths.lock().await;
        if paths.is_empty() {
            continue;
        }

        let mut store_paths = vec![];

        for path in &*paths {
            let store_path = nix_store.parse_store_path(path)?;
            store_paths.push(store_path);
        }

        push_session.queue_many(store_paths.clone())?;

        println!("Queued: {:?}", paths);

        paths.clear();
    }
}

async fn receive_paths(paths: Arc<Mutex<Vec<PathBuf>>>) -> Result<()> {
    let socket_location = get_socket_location()?;
    let socket = UnixListener::bind(&socket_location)?;

    while let Ok((mut stream, _)) = socket.accept().await {
        let mut received_paths = String::new();

        stream.readable().await?;
        stream.read_to_string(&mut received_paths).await?;

        let mut paths = paths.lock().await;

        let received_paths: Vec<PathBuf> = serde_json::from_str(&received_paths)?;
        let mut received_paths = received_paths
            .into_iter()
            .filter(|p| !paths.contains(&p))
            .collect();

        println!("Received: {:?}", received_paths);

        paths.append(&mut received_paths);
    }

    Ok(())
}

async fn run_relay() -> Result<()> {
    let socket_location = get_socket_location()?;
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

fn get_socket_location() -> Result<PathBuf> {
    Ok(PathBuf::from(DIR).join(SOCKET_NAME))
}

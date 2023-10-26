use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use attic::nix_store::NixStore;
use clap::{Parser, Subcommand};
use indicatif::MultiProgress;
use tokio::fs::{read_to_string, write};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::spawn;

use crate::api::ApiClient;
use crate::cache::CacheRef;
use crate::cli::Opts;
use crate::config::Config;
use crate::push::{PushConfig, PushSession, PushSessionConfig, Pusher};

static DIR: &str = "/var/lib/attic-client";
static SOCKET_NAME: &str = "socket";
static FALLBACK_FILE: &str = "fallback.json";

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

    let fallback_file = get_fallback_file_location();
    let empty_vec: Vec<PathBuf> = vec![];
    let empty_file_content = serde_json::to_string(&empty_vec)?;
    if fallback_file.exists() {
        println!("Loading fallback file…");

        let paths = read_to_string(&fallback_file).await?;
        let paths: Vec<PathBuf> = serde_json::from_str(&paths)?;
        let paths = paths.into_iter().filter(|p| p.exists()).collect();

        upload_paths(&push_session, paths)?;

        write(fallback_file, empty_file_content).await?;
    } else {
        // create file so that relay can fall back to it if need be
        // needs to be created by daemon to ensure readability
        write(fallback_file, empty_file_content).await?;
    }

    while let Ok((stream, _)) = socket.accept().await {
        spawn(handle_connection(push_session.clone(), stream));
    }

    Ok(())
}

async fn handle_connection(push_session: PushSession, mut stream: UnixStream) -> Result<()> {
    let mut received_paths = String::new();

    stream.readable().await?;
    stream.read_to_string(&mut received_paths).await?;

    let received_paths: Vec<PathBuf> = serde_json::from_str(&received_paths)?;

    upload_paths(&push_session, received_paths)?;

    Ok(())
}

fn upload_paths(push_session: &PushSession, paths: Vec<PathBuf>) -> Result<()> {
    let nix_store = NixStore::connect()?;
    let mut store_paths = vec![];

    for path in paths {
        let store_path = nix_store.parse_store_path(path)?;
        store_paths.push(store_path);
    }

    push_session.queue_many(store_paths.clone())?;

    Ok(())
}

async fn run_relay() -> Result<()> {
    let socket_location = get_socket_location();
    let mut paths: Vec<_> = env::var("OUT_PATHS")?
        .as_str()
        .split_whitespace()
        .map(PathBuf::from)
        .collect();

    if socket_location.exists() {
        let mut socket = UnixStream::connect(&socket_location).await?;

        let paths = serde_json::to_string(&paths)?;

        socket.writable().await?;
        socket.write_all(paths.as_bytes()).await?;
        socket.shutdown().await?;
    } else {
        let fallback_file = get_fallback_file_location();

        if fallback_file.exists() {
            let fallback_file_content = read_to_string(&fallback_file).await?;
            paths.append(&mut serde_json::from_str(&fallback_file_content)?);

            // write only if file exists to ensure readability of file by attic-client daemon
            let paths = serde_json::to_string(&paths)?;
            write(fallback_file, paths).await?;
        }
    }

    Ok(())
}

fn get_socket_location() -> PathBuf {
    PathBuf::from(DIR).join(SOCKET_NAME)
}

fn get_fallback_file_location() -> PathBuf {
    PathBuf::from(DIR).join(FALLBACK_FILE)
}

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::spawn;
use tokio::sync::Mutex;

use crate::cli::Opts;

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

#[derive(Debug, Parser)]
#[command(about = "Start daemon that uploads paths received via the relay")]
pub struct Daemon {}

#[derive(Debug, Parser)]
#[command(about = "Relay paths to the daemon for uploading")]
pub struct Relay {}

pub async fn run(options: Opts) -> Result<()> {
    if let Some(queue) = options.command.as_queue() {
        match &queue.command {
            Command::Daemon(_) => {
                run_daemon().await?;
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

async fn run_daemon() -> Result<()> {
    let paths: Vec<PathBuf> = Vec::new();
    let paths = Arc::new(Mutex::new(paths));

    let socket = spawn(receive_paths(paths.clone()));

    socket.await??;

    Ok(())
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

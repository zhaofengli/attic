use std::env;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::{io::AsyncWriteExt, net::UnixStream};

use crate::cli::Opts;

static DIR: &str = "/var/lib/attic/client";
static SOCKET_NAME: &str = "socket";

#[derive(Debug, Parser)]
#[command(about = "Queue paths to upload")]
pub struct Queue {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Daemon(Daemon),
    Relay(Relay),
}

#[derive(Debug, Parser)]
#[command(about = "Start daemon that uploads paths received via the relay")]
struct Daemon {}

#[derive(Debug, Parser)]
#[command(about = "Relay paths to the daemon for uploading")]
struct Relay {}

pub async fn run(options: Opts) -> Result<()> {
    if let Some(queue) = options.command.as_queue() {
        match queue.command {
            Command::Daemon(_) => {}
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

pub async fn run_relay() -> Result<()> {
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

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::api::ApiClient;
use crate::cache::CacheRef;
use crate::cli::Opts;
use crate::config::Config;
use attic::nix_store::NixStore;
use attic::pin::PinName;

/// Manage pins on an Attic server.
#[derive(Debug, Parser)]
pub struct Pin {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    List(List),
    Get(Get),
    Create(Create),
    Destroy(Destroy),
}

/// List all pins in a cache.
///
/// You need the `pull` permission on the cache that you are listing on.
#[derive(Debug, Clone, Parser)]
struct List {
    /// Name of the cache to unpin from.
    ///
    /// This can be either `servername:cachename` or `cachename`
    /// when using the default server.
    cache: CacheRef,
}

/// Get an existing pin.
///
/// You need the `pull` permission on the cache that you are getting from.
#[derive(Debug, Clone, Parser)]
struct Get {
    /// Name of the cache to get from.
    ///
    /// This can be either `servername:cachename` or `cachename`
    /// when using the default server.
    cache: CacheRef,

    /// Name of the pin to get.
    name: PinName,
}

/// Create a new pin, or update an existing one.
///
/// You need the `push` permission on the cache that you are pinning on.
#[derive(Debug, Clone, Parser)]
struct Create {
    /// Name of the cache to pin on.
    ///
    /// This can be either `servername:cachename` or `cachename`
    /// when using the default server.
    cache: CacheRef,

    /// Name of the pin to create.
    name: PinName,

    /// The store path to pin.
    path: PathBuf,
}

/// Destroy a new pin, or do nothing if it does not exist.
///
/// You need the `push` permission on the cache that you are unpinning from.
#[derive(Debug, Clone, Parser)]
struct Destroy {
    /// Name of the cache to unpin from.
    ///
    /// This can be either `servername:cachename` or `cachename`
    /// when using the default server.
    cache: CacheRef,

    /// Name of the pin to destroy.
    name: PinName,
}

pub async fn run(opts: Opts) -> Result<()> {
    let sub = opts.command.as_pin().unwrap();
    match &sub.command {
        Command::List(sub) => list_pins(sub.to_owned()).await,
        Command::Get(sub) => get_pin(sub.to_owned()).await,
        Command::Create(sub) => create_pin(sub.to_owned()).await,
        Command::Destroy(sub) => destroy_pin(sub.to_owned()).await,
    }
}

async fn list_pins(sub: List) -> Result<()> {
    let config = Config::load()?;
    let (_, server, cache) = config.resolve_cache(&sub.cache)?;
    let api = ApiClient::from_server_config(server.clone())?;

    let pins = api.list_pins(cache).await?;
    let width = pins.iter().map(|(_, p)| p.len()).max().unwrap_or(0);
    for (pin_name, store_path) in pins {
        println!("{:>width$} -> {}", pin_name, store_path, width = width);
    }

    Ok(())
}

async fn get_pin(sub: Get) -> Result<()> {
    let config = Config::load()?;
    let (_, server, cache) = config.resolve_cache(&sub.cache)?;
    let api = ApiClient::from_server_config(server.clone())?;

    let store_path = api.get_pin(cache, &sub.name).await?;
    println!("{}", store_path);

    Ok(())
}

async fn create_pin(sub: Create) -> Result<()> {
    let store = Arc::new(NixStore::connect()?);
    let config = Config::load()?;
    let (server_name, server, cache) = config.resolve_cache(&sub.cache)?;
    let api = ApiClient::from_server_config(server.clone())?;

    let real_store_path = store.get_full_path(&store.follow_store_path(sub.path)?);
    let store_path = real_store_path.to_str().unwrap();
    api.create_pin(cache, &sub.name, store_path).await?;
    eprintln!(
        "✨ Created pin \"{}\" to \"{}\" on \"{}:{}\"",
        sub.name.as_str(),
        store_path,
        server_name.as_str(),
        cache.as_str(),
    );

    Ok(())
}

async fn destroy_pin(sub: Destroy) -> Result<()> {
    let config = Config::load()?;
    let (server_name, server, cache) = config.resolve_cache(&sub.cache)?;
    let api = ApiClient::from_server_config(server.clone())?;

    api.destroy_pin(cache, &sub.name).await?;
    eprintln!(
        "🗑️ Destroyed pin \"{}\" on \"{}:{}\"",
        sub.name.as_str(),
        server_name.as_str(),
        cache.as_str(),
    );

    Ok(())
}

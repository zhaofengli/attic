use anyhow::{anyhow, Result};
use attic::nix_store::NixStore;
use clap::{Parser, Subcommand};
use dialoguer::Input;
use humantime::Duration;
use std::path::PathBuf;

use crate::api::ApiClient;
use crate::cache::CacheRef;
use crate::cli::Opts;
use crate::config::Config;
use attic::api::v1::cache_config::{
    CacheConfig, CreateCacheRequest, KeypairConfig, RetentionPeriodConfig,
};
use attic::signing::NixKeypair;

/// Manage caches on an Attic server.
#[derive(Debug, Parser)]
pub struct Cache {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Create(Create),
    Configure(Configure),
    DeletePath(DeletePath),
    Destroy(Destroy),
    Info(Info),
}

/// Create a cache.
///
/// You need the `create_cache` permission on the cache that
/// you are creating.
#[derive(Debug, Clone, Parser)]
struct Create {
    /// Name of the cache to create.
    ///
    /// This can be either `servername:cachename` or `cachename`
    /// when using the default server.
    cache: CacheRef,

    /// Make the cache public.
    ///
    /// Public caches can be pulled from by anyone without
    /// a token. Only those with the `push` permission can push.
    ///
    /// By default, caches are private.
    #[clap(long)]
    public: bool,

    /// The Nix store path this binary cache uses.
    ///
    /// You probably don't want to change this. Changing
    /// this can make your cache unusable.
    #[clap(long, hide = true, default_value = "/nix/store")]
    store_dir: String,

    /// The priority of the binary cache.
    ///
    /// A lower number denotes a higher priority.
    /// <https://cache.nixos.org> has a priority of 40.
    #[clap(long, default_value = "41")]
    priority: i32,

    /// The signing key name of an upstream cache.
    ///
    /// When pushing to the cache, paths signed with this key
    /// will be skipped by default. Specify this flag multiple
    /// times to add multiple key names.
    #[clap(
        name = "NAME",
        long = "upstream-cache-key-name",
        default_value = "cache.nixos.org-1"
    )]
    upstream_cache_key_names: Vec<String>,

    /// The signing keypair to use for the cache.
    ///
    /// If not specified, a new keypair will be generated.
    #[clap(long)]
    keypair_path: Option<String>,
}

/// Configure a cache.
///
/// You need the `configure_cache` permission on the cache that
/// you are configuring.
#[derive(Debug, Clone, Parser)]
struct Configure {
    /// Name of the cache to configure.
    cache: CacheRef,

    /// Regenerate the signing keypair.
    ///
    /// The server-side signing key will be regenerated and
    /// all users will need to configure the new signing key
    /// in `nix.conf`.
    #[clap(long)]
    regenerate_keypair: bool,

    /// Set a keypair for the cache.
    ///
    /// The server-side signing key will be set to the
    /// specified keypair. This is useful for setting up
    /// a cache with a pre-existing keypair.
    #[clap(long, conflicts_with = "regenerate_keypair")]
    keypair_path: Option<String>,

    /// Make the cache public.
    ///
    /// Use `--private` to make it private.
    #[clap(long)]
    public: bool,

    /// Make the cache private.
    ///
    /// Use `--public` to make it public.
    #[clap(long)]
    private: bool,

    /// The Nix store path this binary cache uses.
    ///
    /// You probably don't want to change this. Changing
    /// this can make your cache unusable.
    #[clap(long, hide = true)]
    store_dir: Option<String>,

    /// The priority of the binary cache.
    ///
    /// A lower number denotes a higher priority.
    /// <https://cache.nixos.org> has a priority of 40.
    #[clap(long)]
    priority: Option<i32>,

    /// The signing key name of an upstream cache.
    ///
    /// When pushing to the cache, paths signed with this key
    /// will be skipped by default. Specify this flag multiple
    /// times to add multiple key names.
    #[clap(value_name = "NAME", long = "upstream-cache-key-name")]
    upstream_cache_key_names: Option<Vec<String>>,

    /// Set the retention period of the cache.
    ///
    /// You can use expressions like "2 years", "3 months"
    /// and "1y".
    #[clap(long, value_name = "PERIOD")]
    retention_period: Option<Duration>,

    /// Reset the retention period of the cache to global default.
    #[clap(long)]
    reset_retention_period: bool,
}

/// Delete a path from a cache.
///
/// This command is used to delete a path from a cache.
///
/// You need the `delete` permission on the cache that
/// you are deleting from.
///
/// The path is specified as a store path.
#[derive(Debug, Clone, Parser)]
struct DeletePath {
    /// Name of the cache to delete from.
    cache: CacheRef,

    /// The store path to delete.
    store_path: PathBuf,
}

/// Destroy a cache.
///
/// Destroying a cache causes it to become unavailable but the
/// underlying data may not be deleted immediately. Depending
/// on the server configuration, you may or may not be able to
/// create the cache of the same name.
///
/// You need the `destroy_cache` permission on the cache that
/// you are destroying.
#[derive(Debug, Clone, Parser)]
struct Destroy {
    /// Name of the cache to destroy.
    cache: CacheRef,

    /// Don't ask for interactive confirmation.
    #[clap(long)]
    no_confirm: bool,
}

/// Show the current configuration of a cache.
#[derive(Debug, Clone, Parser)]
struct Info {
    /// Name of the cache to query.
    cache: CacheRef,
}

pub async fn run(opts: Opts) -> Result<()> {
    let sub = opts.command.as_cache().unwrap();
    match &sub.command {
        Command::Create(sub) => create_cache(sub.to_owned()).await,
        Command::Configure(sub) => configure_cache(sub.to_owned()).await,
        Command::DeletePath(sub) => delete_path(sub.to_owned()).await,
        Command::Destroy(sub) => destroy_cache(sub.to_owned()).await,
        Command::Info(sub) => show_cache_config(sub.to_owned()).await,
    }
}

async fn create_cache(sub: Create) -> Result<()> {
    let config = Config::load()?;

    let (server_name, server, cache) = config.resolve_cache(&sub.cache)?;
    let api = ApiClient::from_server_config(server.clone())?;

    let mut keypair = KeypairConfig::Generate;
    if let Some(keypair_path) = &sub.keypair_path {
        let contents = std::fs::read_to_string(keypair_path)?;
        keypair = KeypairConfig::Keypair(NixKeypair::from_str(&contents)?);
    }

    let request = CreateCacheRequest {
        keypair,
        is_public: sub.public,
        priority: sub.priority,
        store_dir: sub.store_dir,
        upstream_cache_key_names: sub.upstream_cache_key_names,
    };

    api.create_cache(cache, request).await?;
    eprintln!(
        "✨ Created cache \"{}\" on \"{}\"",
        cache.as_str(),
        server_name.as_str()
    );

    Ok(())
}

async fn configure_cache(sub: Configure) -> Result<()> {
    let config = Config::load()?;

    let (server_name, server, cache) = config.resolve_cache(&sub.cache)?;
    let mut patch = CacheConfig::blank();

    if sub.public && sub.private {
        return Err(anyhow!(
            "`--public` and `--private` cannot be set at the same time."
        ));
    }

    if sub.retention_period.is_some() && sub.reset_retention_period {
        return Err(anyhow!(
            "`--retention-period` and `--reset-retention-period` cannot be set at the same time."
        ));
    }

    if sub.public {
        patch.is_public = Some(true);
    } else if sub.private {
        patch.is_public = Some(false);
    }

    if let Some(period) = sub.retention_period {
        patch.retention_period = Some(RetentionPeriodConfig::Period(period.as_secs() as u32));
    } else {
        patch.retention_period = Some(RetentionPeriodConfig::Global);
    }

    if sub.regenerate_keypair {
        patch.keypair = Some(KeypairConfig::Generate);
    } else if let Some(keypair_path) = &sub.keypair_path {
        let contents = std::fs::read_to_string(keypair_path)?;
        let keypair = KeypairConfig::Keypair(NixKeypair::from_str(&contents)?);
        patch.keypair = Some(keypair);
    }

    patch.store_dir = sub.store_dir;
    patch.priority = sub.priority;
    patch.upstream_cache_key_names = sub.upstream_cache_key_names;

    let api = ApiClient::from_server_config(server.clone())?;
    api.configure_cache(cache, &patch).await?;

    eprintln!(
        "✅ Configured \"{}\" on \"{}\"",
        cache.as_str(),
        server_name.as_str()
    );

    Ok(())
}

async fn delete_path(sub: DeletePath) -> Result<()> {
    let config = Config::load()?;

    let (server_name, server, cache) = config.resolve_cache(&sub.cache)?;
    let api = ApiClient::from_server_config(server.clone())?;

    let store = NixStore::connect()?;

    api.delete_path(cache, &store.parse_store_path(&sub.store_path)?.to_hash())
        .await?;

    eprintln!(
        "🗑️ Deleted path \"{}\" from cache \"{}\" on \"{}\"",
        &sub.store_path,
        cache.as_str(),
        server_name.as_str()
    );

    Ok(())
}

async fn destroy_cache(sub: Destroy) -> Result<()> {
    let config = Config::load()?;

    let (server_name, server, cache) = config.resolve_cache(&sub.cache)?;

    if !sub.no_confirm {
        eprintln!("When you destory a cache:");
        eprintln!();
        eprintln!("1. Everyone will lose access.");
        eprintln!("2. The underlying data won't be deleted immediately.");
        eprintln!("3. You may not be able to create a cache of the same name.");
        eprintln!();

        let answer: String = Input::new()
            .with_prompt(format!(
                "⚠️ Type the cache name to confirm destroying \"{}\" on \"{}\"",
                cache.as_str(),
                server_name.as_str()
            ))
            .allow_empty(true)
            .interact()?;

        if answer != cache.as_str() {
            return Err(anyhow!("Incorrect answer. Aborting..."));
        }
    }

    let api = ApiClient::from_server_config(server.clone())?;
    api.destroy_cache(cache).await?;

    eprintln!("🗑️ The cache was destroyed.");

    Ok(())
}

async fn show_cache_config(sub: Info) -> Result<()> {
    let config = Config::load()?;

    let (_, server, cache) = config.resolve_cache(&sub.cache)?;
    let api = ApiClient::from_server_config(server.clone())?;
    let cache_config = api.get_cache_config(cache).await?;

    if let Some(is_public) = cache_config.is_public {
        eprintln!("               Public: {}", is_public);
    }

    if let Some(public_key) = cache_config.public_key {
        eprintln!("           Public Key: {}", public_key);
    }

    if let Some(substituter_endpoint) = cache_config.substituter_endpoint {
        eprintln!("Binary Cache Endpoint: {}", substituter_endpoint);
    }

    if let Some(api_endpoint) = cache_config.api_endpoint {
        eprintln!("         API Endpoint: {}", api_endpoint);
    }

    if let Some(store_dir) = cache_config.store_dir {
        eprintln!("      Store Directory: {}", store_dir);
    }

    if let Some(priority) = cache_config.priority {
        eprintln!("             Priority: {}", priority);
    }

    if let Some(upstream_cache_key_names) = cache_config.upstream_cache_key_names {
        eprintln!("  Upstream Cache Keys: {:?}", upstream_cache_key_names);
    }

    if let Some(retention_period) = cache_config.retention_period {
        match retention_period {
            RetentionPeriodConfig::Period(period) => {
                eprintln!("     Retention Period: {:?}", period);
            }
            RetentionPeriodConfig::Global => {
                eprintln!("     Retention Period: Global Default");
            }
        }
    }

    Ok(())
}

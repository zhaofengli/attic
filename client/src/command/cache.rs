use std::process::Stdio;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use dialoguer::Input;
use humantime::Duration;
use reqwest::Url;
use serde::Serialize;
use tokio::process::Command as ProcessCommand;

use crate::api::ApiClient;
use crate::cache::CacheRef;
use crate::cli::Opts;
use crate::config::Config;
use attic::api::v1::cache_config::{
    CacheConfig, CreateCacheRequest, KeypairConfig, RetentionPeriodConfig,
};

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
    Destroy(Destroy),
    Info(Info),
    ListEnabled(ListEnabled),
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

/// List caches enabled in the current user's effective Nix configuration.
#[derive(Debug, Clone, Parser)]
struct ListEnabled {
    /// Print a JSON array of enabled cache objects.
    #[clap(long)]
    json: bool,
}

#[derive(Debug, Serialize)]
struct EnabledCache {
    url: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    alias: Option<String>,
}

pub async fn run(opts: Opts) -> Result<()> {
    let sub = opts.command.as_cache().unwrap();
    match &sub.command {
        Command::Create(sub) => create_cache(sub.to_owned()).await,
        Command::Configure(sub) => configure_cache(sub.to_owned()).await,
        Command::Destroy(sub) => destroy_cache(sub.to_owned()).await,
        Command::Info(sub) => show_cache_config(sub.to_owned()).await,
        Command::ListEnabled(sub) => list_enabled_caches(sub.to_owned()).await,
    }
}

async fn list_enabled_caches(sub: ListEnabled) -> Result<()> {
    let config = Config::load()?;
    let output = ProcessCommand::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "config",
            "show",
            "substituters",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .await
        .context("Failed to query the effective Nix substituters")?;

    if !output.status.success() {
        return Err(anyhow!(
            "Failed to query the effective Nix substituters: nix exited with {}",
            output.status
        ));
    }

    let substituters = String::from_utf8(output.stdout)?;
    let mut caches: Vec<_> = substituters
        .split_whitespace()
        .map(|url| EnabledCache {
            url: url.to_owned(),
            alias: find_cache_alias(url, &config),
        })
        .collect();
    caches.sort_by_key(enabled_cache_display);

    if sub.json {
        println!("{}", serde_json::to_string(&caches)?);
    } else {
        for cache in caches {
            println!("{}", enabled_cache_display(&cache));
        }
    }

    Ok(())
}

fn find_cache_alias(url: &str, config: &Config) -> Option<String> {
    let substituter = Url::parse(url).ok()?;
    let mut aliases = config.servers.iter().filter_map(|(server_name, server)| {
        let endpoint = Url::parse(&server.endpoint).ok()?;
        let cache_name = cache_name_from_url(&substituter, &endpoint)?;
        Some(format!("{}:{cache_name}", server_name.as_str()))
    });
    let alias = aliases.next()?;

    if aliases.next().is_some() {
        None
    } else {
        Some(alias)
    }
}

fn cache_name_from_url(substituter: &Url, endpoint: &Url) -> Option<attic::cache::CacheName> {
    if substituter.scheme() != endpoint.scheme()
        || substituter.host_str() != endpoint.host_str()
        || substituter.port_or_known_default() != endpoint.port_or_known_default()
        || substituter.username() != endpoint.username()
        || substituter.password() != endpoint.password()
        || substituter.fragment().is_some()
    {
        return None;
    }

    let endpoint_path = endpoint.path().trim_end_matches('/');
    let cache_path = substituter.path().strip_prefix(endpoint_path)?;
    let cache_name = cache_path.strip_prefix('/')?;

    if cache_name.contains('/') {
        return None;
    }

    attic::cache::CacheName::new(cache_name.to_owned()).ok()
}

fn enabled_cache_display(cache: &EnabledCache) -> String {
    match &cache.alias {
        Some(alias) => format!("{alias} ({})", cache.url),
        None => cache.url.clone(),
    }
}

async fn create_cache(sub: Create) -> Result<()> {
    let config = Config::load()?;

    let (server_name, server, cache) = config.resolve_cache(&sub.cache)?;
    let api = ApiClient::from_server_config(server.clone())?;

    let request = CreateCacheRequest {
        // TODO: Make this configurable?
        keypair: KeypairConfig::Generate,
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

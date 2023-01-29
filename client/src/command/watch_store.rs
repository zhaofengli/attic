use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use clap::Parser;
use indicatif::MultiProgress;
use notify::{RecursiveMode, Watcher, EventKind};

use crate::api::ApiClient;
use crate::cache::CacheRef;
use crate::cli::Opts;
use crate::config::Config;
use crate::push::{Pusher, PushConfig, PushSessionConfig};
use attic::nix_store::{NixStore, StorePath};

/// Watch the Nix Store for new paths and upload them to a binary cache.
#[derive(Debug, Parser)]
pub struct WatchStore {
    /// The cache to push to.
    cache: CacheRef,

    /// Push the new paths only and do not compute closures.
    #[clap(long)]
    no_closure: bool,

    /// Ignore the upstream cache filter.
    #[clap(long)]
    ignore_upstream_cache_filter: bool,

    /// The maximum number of parallel upload processes.
    #[clap(short = 'j', long, default_value = "5")]
    jobs: usize,

    /// Always send the upload info as part of the payload.
    #[clap(long, hide = true)]
    force_preamble: bool,
}

pub async fn run(opts: Opts) -> Result<()> {
    let sub = opts.command.as_watch_store().unwrap();
    if sub.jobs == 0 {
        return Err(anyhow!("The number of jobs cannot be 0"));
    }

    let config = Config::load()?;

    let store = Arc::new(NixStore::connect()?);
    let store_dir = store.store_dir().to_owned();

    let (server_name, server, cache) = config.resolve_cache(&sub.cache)?;
    let mut api = ApiClient::from_server_config(server.clone())?;

    // Confirm remote cache validity, query cache config
    let cache_config = api.get_cache_config(cache).await?;

    if let Some(api_endpoint) = &cache_config.api_endpoint {
        // Use delegated API endpoint
        api.set_endpoint(api_endpoint)?;
    }

    let push_config = PushConfig {
        num_workers: sub.jobs,
        force_preamble: sub.force_preamble,
    };

    let push_session_config = PushSessionConfig {
        no_closure: sub.no_closure,
        ignore_upstream_cache_filter: sub.ignore_upstream_cache_filter,
    };

    let mp = MultiProgress::new();
    let session = Pusher::new(store.clone(), api, cache.to_owned(), cache_config, mp, push_config)
        .into_push_session(push_session_config);

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        match res {
            Ok(event) => {
                // We watch the removals of lock files which signify
                // store paths becoming valid
                if let EventKind::Remove(_) = event.kind {
                    let paths = event.paths
                        .iter()
                        .filter_map(|p| {
                            let base = strip_lock_file(&p)?;
                            store.parse_store_path(base).ok()
                        })
                        .collect::<Vec<StorePath>>();

                    if !paths.is_empty() {
                        session.queue_many(paths).unwrap();
                    }
                }
            }
            Err(e) => eprintln!("Error during watch: {:?}", e),
        }
    })?;

    watcher.watch(&store_dir, RecursiveMode::NonRecursive)?;

    eprintln!("👀 Pushing new store paths to \"{cache}\" on \"{server}\"",
        cache = cache.as_str(),
        server = server_name.as_str(),
    );

    loop {
    }
}

fn strip_lock_file(p: &Path) -> Option<PathBuf> {
    p.to_str()
        .and_then(|p| p.strip_suffix(".lock"))
        .filter(|t| !t.ends_with(".drv"))
        .map(PathBuf::from)
}

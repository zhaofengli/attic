use std::collections::{HashMap, HashSet};
use std::cmp;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use clap::Parser;
use futures::future::join_all;
use indicatif::MultiProgress;

use crate::api::ApiClient;
use crate::cache::{CacheName, CacheRef};
use crate::cli::Opts;
use crate::config::Config;
use crate::push::{Pusher, PushConfig};
use attic::nix_store::{NixStore, StorePath, StorePathHash, ValidPathInfo};

/// Push closures to a binary cache.
#[derive(Debug, Parser)]
pub struct Push {
    /// The cache to push to.
    cache: CacheRef,

    /// The store paths to push.
    paths: Vec<PathBuf>,

    /// Push the specified paths only and do not compute closures.
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

struct PushPlan {
    /// Store paths to push.
    store_path_map: HashMap<StorePathHash, ValidPathInfo>,

    /// The number of paths in the original full closure.
    num_all_paths: usize,

    /// Number of paths that have been filtered out because they are already cached.
    num_already_cached: usize,

    /// Number of paths that have been filtered out because they are signed by an upstream cache.
    num_upstream: usize,
}

pub async fn run(opts: Opts) -> Result<()> {
    let sub = opts.command.as_push().unwrap();
    if sub.jobs == 0 {
        return Err(anyhow!("The number of jobs cannot be 0"));
    }

    let config = Config::load()?;

    let store = Arc::new(NixStore::connect()?);
    let roots = sub
        .paths
        .clone()
        .into_iter()
        .map(|p| store.follow_store_path(&p))
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let (server_name, server, cache) = config.resolve_cache(&sub.cache)?;

    let mut api = ApiClient::from_server_config(server.clone())?;
    let plan = PushPlan::plan(
        store.clone(),
        &mut api,
        cache,
        roots,
        sub.no_closure,
        sub.ignore_upstream_cache_filter,
    )
    .await?;

    if plan.store_path_map.is_empty() {
        if plan.num_all_paths == 0 {
            eprintln!("🤷 Nothing selected.");
        } else {
            eprintln!(
                "✅ All done! ({num_already_cached} already cached, {num_upstream} in upstream)",
                num_already_cached = plan.num_already_cached,
                num_upstream = plan.num_upstream,
            );
        }

        return Ok(());
    } else {
        eprintln!("⚙️ Pushing {num_missing_paths} paths to \"{cache}\" on \"{server}\" ({num_already_cached} already cached, {num_upstream} in upstream)...",
            cache = cache.as_str(),
            server = server_name.as_str(),
            num_missing_paths = plan.store_path_map.len(),
            num_already_cached = plan.num_already_cached,
            num_upstream = plan.num_upstream,
        );
    }

    let push_config = PushConfig {
        num_workers: cmp::min(sub.jobs, plan.store_path_map.len()),
        force_preamble: sub.force_preamble,
    };

    let mp = MultiProgress::new();

    let pusher = Pusher::new(store, api, cache.to_owned(), mp, push_config);
    for (_, path_info) in plan.store_path_map {
        pusher.push(path_info).await?;
    }

    let results = pusher.wait().await;
    results.into_iter().map(|(_, result)| result).collect::<Result<Vec<()>>>()?;

    Ok(())
}

impl PushPlan {
    /// Creates a plan.
    async fn plan(
        store: Arc<NixStore>,
        api: &mut ApiClient,
        cache: &CacheName,
        roots: Vec<StorePath>,
        no_closure: bool,
        ignore_upstream_filter: bool,
    ) -> Result<Self> {
        // Compute closure
        let closure = if no_closure {
            roots
        } else {
            store
                .compute_fs_closure_multi(roots, false, false, false)
                .await?
        };

        let mut store_path_map: HashMap<StorePathHash, ValidPathInfo> = {
            let futures = closure
                .iter()
                .map(|path| {
                    let store = store.clone();
                    let path = path.clone();
                    let path_hash = path.to_hash();

                    async move {
                        let path_info = store.query_path_info(path).await?;
                        Ok((path_hash, path_info))
                    }
                })
                .collect::<Vec<_>>();

            join_all(futures).await.into_iter().collect::<Result<_>>()?
        };

        let num_all_paths = store_path_map.len();
        if store_path_map.is_empty() {
            return Ok(Self {
                store_path_map,
                num_all_paths,
                num_already_cached: 0,
                num_upstream: 0,
            });
        }

        // Confirm remote cache validity, query cache config
        let cache_config = api.get_cache_config(cache).await?;

        if let Some(api_endpoint) = &cache_config.api_endpoint {
            // Use delegated API endpoint
            api.set_endpoint(api_endpoint)?;
        }

        if !ignore_upstream_filter {
            // Filter out paths signed by upstream caches
            let upstream_cache_key_names =
                cache_config.upstream_cache_key_names.unwrap_or_default();
            store_path_map.retain(|_, pi| {
                for sig in &pi.sigs {
                    if let Some((name, _)) = sig.split_once(':') {
                        if upstream_cache_key_names.iter().any(|u| name == u) {
                            return false;
                        }
                    }
                }

                true
            });
        }

        let num_filtered_paths = store_path_map.len();
        if store_path_map.is_empty() {
            return Ok(Self {
                store_path_map,
                num_all_paths,
                num_already_cached: 0,
                num_upstream: num_all_paths - num_filtered_paths,
            });
        }

        // Query missing paths
        let missing_path_hashes: HashSet<StorePathHash> = {
            let store_path_hashes = store_path_map.keys().map(|sph| sph.to_owned()).collect();
            let res = api.get_missing_paths(cache, store_path_hashes).await?;
            res.missing_paths.into_iter().collect()
        };
        store_path_map.retain(|sph, _| missing_path_hashes.contains(sph));
        let num_missing_paths = store_path_map.len();

        Ok(Self {
            store_path_map,
            num_all_paths,
            num_already_cached: num_filtered_paths - num_missing_paths,
            num_upstream: num_all_paths - num_filtered_paths,
        })
    }
}

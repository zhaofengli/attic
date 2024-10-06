use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use clap::Parser;
use indicatif::MultiProgress;
use tokio::io::{self, AsyncBufReadExt, BufReader};

use crate::api::ApiClient;
use crate::cache::{CacheName, CacheRef, ServerName};
use crate::cli::Opts;
use crate::config::Config;
use crate::push::{PushConfig, PushSessionConfig, Pusher};
use attic::nix_store::NixStore;

/// Push closures to a binary cache.
#[derive(Debug, Parser)]
pub struct Push {
    /// The cache to push to.
    ///
    /// This can be either `servername:cachename` or `cachename`
    /// when using the default server.
    cache: CacheRef,

    /// The store paths to push.
    paths: Vec<PathBuf>,

    /// Read paths from the standard input.
    #[clap(long)]
    stdin: bool,

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

struct PushContext {
    store: Arc<NixStore>,
    cache_name: CacheName,
    server_name: ServerName,
    pusher: Pusher,
    no_closure: bool,
    ignore_upstream_cache_filter: bool,
}

impl PushContext {
    async fn push_static(self, paths: Vec<PathBuf>) -> Result<()> {
        let roots = paths
            .into_iter()
            .map(|p| self.store.follow_store_path(p))
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let plan = self
            .pusher
            .plan(roots, self.no_closure, self.ignore_upstream_cache_filter)
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
                cache = self.cache_name.as_str(),
                server = self.server_name.as_str(),
                num_missing_paths = plan.store_path_map.len(),
                num_already_cached = plan.num_already_cached,
                num_upstream = plan.num_upstream,
            );
        }

        for (_, path_info) in plan.store_path_map {
            self.pusher.queue(path_info).await?;
        }

        let results = self.pusher.wait().await;
        results.into_values().collect::<Result<Vec<()>>>()?;

        Ok(())
    }

    async fn push_stdin(self) -> Result<()> {
        let session = self.pusher.into_push_session(PushSessionConfig {
            no_closure: self.no_closure,
            ignore_upstream_cache_filter: self.ignore_upstream_cache_filter,
        });

        let stdin = BufReader::new(io::stdin());
        let mut lines = stdin.lines();
        while let Some(line) = lines.next_line().await? {
            if line.is_empty() {
                continue;
            }

            let path = self.store.follow_store_path(line)?;
            session.queue_many(vec![path])?;
        }

        let results = session.wait().await?;
        results.into_values().collect::<Result<Vec<()>>>()?;

        Ok(())
    }
}

pub async fn run(opts: Opts) -> Result<()> {
    let sub = opts.command.as_push().unwrap();
    if sub.jobs == 0 {
        return Err(anyhow!("The number of jobs cannot be 0"));
    }

    let config = Config::load()?;

    let store = Arc::new(NixStore::connect()?);

    let (server_name, server, cache_name) = config.resolve_cache(&sub.cache)?;

    let mut api = ApiClient::from_server_config(server.clone())?;

    // Confirm remote cache validity, query cache config
    let cache_config = api.get_cache_config(cache_name).await?;

    if let Some(api_endpoint) = &cache_config.api_endpoint {
        // Use delegated API endpoint
        api.set_endpoint(api_endpoint)?;
    }

    let push_config = PushConfig {
        num_workers: sub.jobs,
        force_preamble: sub.force_preamble,
    };

    let mp = MultiProgress::new();

    let pusher = Pusher::new(
        store.clone(),
        api,
        cache_name.to_owned(),
        cache_config,
        mp,
        push_config,
    );

    let push_ctx = PushContext {
        store,
        cache_name: cache_name.clone(),
        server_name: server_name.clone(),
        pusher,
        no_closure: sub.no_closure,
        ignore_upstream_cache_filter: sub.ignore_upstream_cache_filter,
    };

    if sub.stdin {
        push_ctx.push_stdin().await?;
    } else {
        push_ctx.push_static(sub.paths.clone()).await?;
    }

    Ok(())
}

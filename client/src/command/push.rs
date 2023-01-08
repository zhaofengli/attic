use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use clap::Parser;
use futures::future::join_all;
use futures::stream::Stream;
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use tokio::sync::Semaphore;

use crate::api::ApiClient;
use crate::cache::{CacheName, CacheRef};
use crate::cli::Opts;
use crate::config::Config;
use attic::api::v1::upload_path::{UploadPathNarInfo, UploadPathResultKind};
use attic::error::AtticResult;
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
    #[clap(short = 'j', long, default_value = "10")]
    jobs: usize,
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

/// Wrapper to update a progress bar as a NAR is streamed.
struct NarStreamProgress<S> {
    stream: S,
    bar: ProgressBar,
}

/// Uploads a single path to a cache.
pub async fn upload_path(
    store: Arc<NixStore>,
    path_info: ValidPathInfo,
    api: ApiClient,
    cache: &CacheName,
    mp: MultiProgress,
) -> Result<()> {
    let path = &path_info.path;
    let upload_info = {
        let full_path = store
            .get_full_path(path)
            .to_str()
            .ok_or_else(|| anyhow!("Path contains non-UTF-8"))?
            .to_string();

        let references = path_info
            .references
            .into_iter()
            .map(|pb| {
                pb.to_str()
                    .ok_or_else(|| anyhow!("Reference contains non-UTF-8"))
                    .map(|s| s.to_owned())
            })
            .collect::<Result<Vec<String>, anyhow::Error>>()?;

        UploadPathNarInfo {
            cache: cache.to_owned(),
            store_path_hash: path.to_hash(),
            store_path: full_path,
            references,
            system: None,  // TODO
            deriver: None, // TODO
            sigs: path_info.sigs,
            ca: path_info.ca,
            nar_hash: path_info.nar_hash.to_owned(),
            nar_size: path_info.nar_size as usize,
        }
    };

    let template = format!(
        "{{spinner}} {: <20.20} {{bar:40.green/blue}} {{human_bytes:10}} ({{average_speed}})",
        path.name(),
    );
    let style = ProgressStyle::with_template(&template)
        .unwrap()
        .tick_chars("🕛🕐🕑🕒🕓🕔🕕🕖🕗🕘🕙🕚✅")
        .progress_chars("██ ")
        .with_key("human_bytes", |state: &ProgressState, w: &mut dyn Write| {
            write!(w, "{}", HumanBytes(state.pos())).unwrap();
        })
        // Adapted from
        // <https://github.com/console-rs/indicatif/issues/394#issuecomment-1309971049>
        .with_key(
            "average_speed",
            |state: &ProgressState, w: &mut dyn Write| match (state.pos(), state.elapsed()) {
                (pos, elapsed) if elapsed > Duration::ZERO => {
                    write!(w, "{}", average_speed(pos, elapsed)).unwrap();
                }
                _ => write!(w, "-").unwrap(),
            },
        );
    let bar = mp.add(ProgressBar::new(path_info.nar_size));
    bar.set_style(style);
    let nar_stream = NarStreamProgress::new(store.nar_from_path(path.to_owned()), bar.clone());

    let start = Instant::now();
    match api.upload_path(upload_info, nar_stream).await {
        Ok(r) => {
            if r.is_none() {
                mp.suspend(|| {
                    eprintln!("Warning: Please update your server. Compatibility will be removed in the first stable release.");
                })
            }

            let deduplicated = if let Some(r) = r {
                r.kind == UploadPathResultKind::Deduplicated
            } else {
                false
            };

            if deduplicated {
                mp.suspend(|| {
                    eprintln!("✅ {} (deduplicated)", path.as_os_str().to_string_lossy());
                });
                bar.finish_and_clear();
            } else {
                let elapsed = start.elapsed();
                let seconds = elapsed.as_secs_f64();
                let speed = (path_info.nar_size as f64 / seconds) as u64;

                mp.suspend(|| {
                    eprintln!(
                        "✅ {} ({}/s)",
                        path.as_os_str().to_string_lossy(),
                        HumanBytes(speed)
                    );
                });
                bar.finish_and_clear();
            }

            Ok(())
        }
        Err(e) => {
            mp.suspend(|| {
                eprintln!("❌ {}: {}", path.as_os_str().to_string_lossy(), e);
            });
            bar.finish_and_clear();
            Err(e)
        }
    }
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

    let mp = MultiProgress::new();
    let upload_limit = Arc::new(Semaphore::new(sub.jobs));
    let futures = plan
        .store_path_map
        .into_iter()
        .map(|(_, path_info)| {
            let store = store.clone();
            let api = api.clone();
            let mp = mp.clone();
            let upload_limit = upload_limit.clone();

            async move {
                let permit = upload_limit.acquire().await?;

                upload_path(store.clone(), path_info, api, cache, mp.clone()).await?;

                drop(permit);
                Ok::<(), anyhow::Error>(())
            }
        })
        .collect::<Vec<_>>();

    futures::future::join_all(futures)
        .await
        .into_iter()
        .collect::<Result<Vec<()>>>()?;

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

impl<S: Stream<Item = AtticResult<Vec<u8>>>> NarStreamProgress<S> {
    fn new(stream: S, bar: ProgressBar) -> Self {
        Self { stream, bar }
    }
}

impl<S: Stream<Item = AtticResult<Vec<u8>>> + Unpin> Stream for NarStreamProgress<S> {
    type Item = AtticResult<Vec<u8>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.stream).as_mut().poll_next(cx) {
            Poll::Ready(Some(data)) => {
                if let Ok(data) = &data {
                    self.bar.inc(data.len() as u64);
                }

                Poll::Ready(Some(data))
            }
            other => other,
        }
    }
}

// Just the average, no fancy sliding windows that cause wild fluctuations
// <https://github.com/console-rs/indicatif/issues/394>
fn average_speed(bytes: u64, duration: Duration) -> String {
    let speed = bytes as f64 * 1000_f64 / duration.as_millis() as f64;
    format!("{}/s", HumanBytes(speed as u64))
}

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use clap::Parser;
use indicatif::MultiProgress;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::api::ApiClient;
use crate::cache::CacheRef;
use crate::cli::Opts;
use crate::config::Config;
use crate::push::{PushConfig, PushSessionConfig, Pusher};
use attic::nix_store::{NixStore, StorePath};

/// Watch the Nix Store for new paths and upload them to a binary cache.
#[derive(Debug, Parser)]
pub struct WatchStore {
    /// The cache to push to.
    ///
    /// This can be either `servername:cachename` or `cachename`
    /// when using the default server.
    cache: CacheRef,

    /// Push the new paths only and do not compute closures.
    #[clap(long, hide = true)]
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
    let session = Pusher::new(
        store.clone(),
        api,
        cache.to_owned(),
        cache_config,
        mp,
        push_config,
    )
    .into_push_session(push_session_config);

    let (tx, mut rx) = mpsc::unbounded_channel();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        tx.send(res).unwrap();
    })?;

    watcher.watch(&store_dir, RecursiveMode::NonRecursive)?;

    eprintln!(
        "👀 Pushing new store paths to \"{cache}\" on \"{server}\"",
        cache = cache.as_str(),
        server = server_name.as_str(),
    );

    while let Some(res) = rx.recv().await {
        match res {
            Ok(event) => {
                // We watch the removals of lock files which signify
                // store paths becoming valid
                if let EventKind::Remove(_) = event.kind {
                    let paths = event
                        .paths
                        .iter()
                        .filter_map(|p| {
                            let base = strip_lock_file(p)?;
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
    }

    Ok(())
}

fn strip_lock_file(p: &Path) -> Option<PathBuf> {
    p.to_str()
        .and_then(|p| p.strip_suffix(".lock"))
        .filter(|t| !t.ends_with(".drv") && !t.ends_with("-source"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::mpsc;
    use std::time::Duration;

    /// Core watcher test: create 256 dirs, watch with Recursive mode
    /// (kqueue opens 1 FD per entry), write+remove a .lock file, and
    /// assert the Remove event arrives within 5 s.
    fn watcher_with_many_entries() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let store = dir.path();

        for i in 0..256 {
            std::fs::create_dir(store.join(format!("{i:032x}-pkg-{i}"))).unwrap();
        }

        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            tx.send(res).ok();
        })
        .expect("create watcher");

        // Recursive mode: kqueue opens 1 FD per entry via WalkDir.
        // With 256 entries + base FDs this exceeds ulimit 256.
        // FSEvents uses O(1) FDs regardless.
        watcher
            .watch(store, RecursiveMode::Recursive)
            .expect("watch store dir");

        let lock = store.join("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello-1.0.lock");
        std::fs::write(&lock, "").unwrap();
        std::fs::remove_file(&lock).unwrap();

        let mut saw_remove = false;
        while let Ok(Ok(event)) = rx.recv_timeout(Duration::from_secs(5)) {
            if matches!(event.kind, EventKind::Remove(_)) {
                if event
                    .paths
                    .iter()
                    .any(|p| p.to_string_lossy().ends_with(".lock"))
                {
                    saw_remove = true;
                    break;
                }
            }
        }
        assert!(
            saw_remove,
            "never received Remove event for .lock file — kqueue likely hit FD limit"
        );
    }

    /// Regression: kqueue opens one FD per entry when using Recursive mode
    /// via WalkDir.  Once the per-process FD limit is exceeded, new watches
    /// silently fail and events are dropped.  FSEvents uses a single
    /// descriptor regardless of directory size or recursion mode.
    ///
    /// Re-executes itself under `ulimit -Sn 256` so the test triggers
    /// FD exhaustion deterministically regardless of the runner's default.
    #[test]
    fn test_watcher_survives_many_entries() {
        if std::env::var_os("ATTIC_TEST_FD_LIMITED").is_some() {
            watcher_with_many_entries();
            return;
        }

        let exe = std::env::current_exe().expect("current exe");
        let out = std::process::Command::new("bash")
            .args([
                "-c",
                &format!(
                    "ulimit -Sn 256 && ATTIC_TEST_FD_LIMITED=1 exec '{}' \
                     command::watch_store::tests::test_watcher_survives_many_entries \
                     --exact --test-threads=1 --nocapture 2>&1",
                    exe.display()
                ),
            ])
            .output()
            .expect("spawn subprocess");

        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "test failed under ulimit -Sn 256:\n{stdout}"
        );
    }
}

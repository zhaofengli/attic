//! Store path uploader.
//!
//! Multiple workers are spawned to upload store paths concurrently.
//!
//! TODO: Refactor out progress reporting and support a simple output style without progress bars

use std::collections::HashMap;
use std::fmt::Write;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use async_channel as channel;
use bytes::Bytes;
use futures::stream::{Stream, TryStreamExt};
use futures::future::join_all;
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use tokio::task::{JoinHandle, spawn};

use attic::api::v1::upload_path::{UploadPathNarInfo, UploadPathResult, UploadPathResultKind};
use attic::cache::CacheName;
use attic::error::AtticResult;
use attic::nix_store::{NixStore, StorePath, ValidPathInfo};
use crate::api::ApiClient;

type JobSender = channel::Sender<ValidPathInfo>;
type JobReceiver = channel::Receiver<ValidPathInfo>;

/// Configuration for pushing store paths.
#[derive(Clone, Copy, Debug)]
pub struct PushConfig {
    /// The number of workers to spawn.
    pub num_workers: usize,

    /// Whether to always include the upload info in the PUT payload.
    pub force_preamble: bool,
}

/// A handle to push store paths to a cache.
///
/// The caller is responsible for computing closures and
/// checking for paths that already exist on the remote
/// cache.
pub struct Pusher {
    workers: Vec<JoinHandle<HashMap<StorePath, Result<()>>>>,
    sender: JobSender,
}

/// Wrapper to update a progress bar as a NAR is streamed.
struct NarStreamProgress<S> {
    stream: S,
    bar: ProgressBar,
}

impl Pusher {
    pub fn new(store: Arc<NixStore>, api: ApiClient, cache: CacheName, mp: MultiProgress, config: PushConfig) -> Self {
        let (sender, receiver) = channel::unbounded();
        let mut workers = Vec::new();

        for _ in 0..config.num_workers {
            workers.push(spawn(worker(
                receiver.clone(),
                store.clone(),
                api.clone(),
                cache.clone(),
                mp.clone(),
                config.clone(),
            )));
        }

        Self { workers, sender }
    }

    /// Sends a path to be pushed.
    pub async fn push(&self, path_info: ValidPathInfo) -> Result<()> {
        self.sender.send(path_info).await
            .map_err(|e| anyhow!(e))
    }

    /// Waits for all workers to terminate, returning all results.
    ///
    /// TODO: Stream the results with another channel
    pub async fn wait(self) -> HashMap<StorePath, Result<()>> {
        drop(self.sender);

        let results = join_all(self.workers)
            .await
            .into_iter()
            .map(|joinresult| joinresult.unwrap())
            .fold(HashMap::new(), |mut acc, results| {
                acc.extend(results);
                acc
            });

        results
    }
}

async fn worker(
    receiver: JobReceiver,
    store: Arc<NixStore>,
    api: ApiClient,
    cache: CacheName,
    mp: MultiProgress,
    config: PushConfig,
) -> HashMap<StorePath, Result<()>> {
    let mut results = HashMap::new();

    loop {
        let path_info = match receiver.recv().await {
            Ok(path_info) => path_info,
            Err(_) => {
                // channel is closed - we are done
                break;
            }
        };

        let store_path = path_info.path.clone();

        let r = upload_path(
            path_info,
            store.clone(),
            api.clone(),
            &cache,
            mp.clone(),
            config.force_preamble,
        ).await;

        results.insert(store_path, r);
    }

    results
}

/// Uploads a single path to a cache.
pub async fn upload_path(
    path_info: ValidPathInfo,
    store: Arc<NixStore>,
    api: ApiClient,
    cache: &CacheName,
    mp: MultiProgress,
    force_preamble: bool,
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
    let nar_stream = NarStreamProgress::new(store.nar_from_path(path.to_owned()), bar.clone())
        .map_ok(Bytes::from);

    let start = Instant::now();
    match api
        .upload_path(upload_info, nar_stream, force_preamble)
        .await
    {
        Ok(r) => {
            let r = r.unwrap_or(UploadPathResult {
                kind: UploadPathResultKind::Uploaded,
                file_size: None,
                frac_deduplicated: None,
            });

            let info_string: String = match r.kind {
                UploadPathResultKind::Deduplicated => "deduplicated".to_string(),
                _ => {
                    let elapsed = start.elapsed();
                    let seconds = elapsed.as_secs_f64();
                    let speed = (path_info.nar_size as f64 / seconds) as u64;

                    let mut s = format!("{}/s", HumanBytes(speed));

                    if let Some(frac_deduplicated) = r.frac_deduplicated {
                        if frac_deduplicated > 0.01f64 {
                            s += &format!(", {:.1}% deduplicated", frac_deduplicated * 100.0);
                        }
                    }

                    s
                }
            };

            mp.suspend(|| {
                eprintln!(
                    "✅ {} ({})",
                    path.as_os_str().to_string_lossy(),
                    info_string
                );
            });
            bar.finish_and_clear();

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

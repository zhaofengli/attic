//! Store path uploader.
//!
//! There are two APIs: `Pusher` and `PushSession`.
//!
//! A `Pusher` simply dispatches `ValidPathInfo`s for workers to push. Use this
//! when you know all store paths to push beforehand. The push plan (closure, missing
//! paths, all path metadata) should be computed prior to pushing.
//!
//! A `PushSession`, on the other hand, accepts a stream of `StorePath`s and
//! takes care of retrieving the closure and path metadata. It automatically
//! batches expensive operations (closure computation, querying missing paths).
//! Use this when the list of store paths is streamed from some external
//! source (e.g., FS watcher, Unix Domain Socket) and a push plan cannot be
//! created statically.
//!
//! TODO: Refactor out progress reporting and support a simple output style without progress bars

use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use async_channel as channel;
use bytes::{Bytes, BytesMut};
use futures::future::join_all;
use futures::stream::{Stream, TryStreamExt};
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use tokio::sync::{mpsc, Mutex};
use tokio::task::{spawn, JoinHandle};
use tokio::time;

use crate::api::{ApiClient, ApiError};
use attic::api::v1::cache_config::CacheConfig;
use attic::api::v1::upload_path::{
    FinalizeUploadSessionResponse, StartUploadPathSessionResponse, UploadChunkingConfig,
    UploadPathNarInfo, UploadPathResult, UploadPathResultKind,
};
use attic::cache::CacheName;
use attic::error::AtticResult;
use attic::nix_store::{NixStore, StorePath, StorePathHash, ValidPathInfo};

type JobSender = channel::Sender<ValidPathInfo>;
type JobReceiver = channel::Receiver<ValidPathInfo>;

/// Configuration for pushing store paths.
#[derive(Clone, Copy, Debug)]
pub struct PushConfig {
    /// The number of workers to spawn.
    pub num_workers: usize,

    /// Whether to always include the upload info in the PUT payload.
    pub force_preamble: bool,

    /// Maximum size of each transport upload part.
    pub chunk_size: Option<usize>,
}

/// Configuration for a push session.
#[derive(Clone, Copy, Debug)]
pub struct PushSessionConfig {
    /// Push the specified paths only and do not compute closures.
    pub no_closure: bool,

    /// Ignore the upstream cache filter.
    pub ignore_upstream_cache_filter: bool,
}

/// A handle to push store paths to a cache.
///
/// The caller is responsible for computing closures and
/// checking for paths that already exist on the remote
/// cache.
pub struct Pusher {
    api: ApiClient,
    store: Arc<NixStore>,
    cache: CacheName,
    cache_config: CacheConfig,
    workers: Vec<JoinHandle<HashMap<StorePath, Result<()>>>>,
    sender: JobSender,
}

/// A wrapper over a `Pusher` that accepts a stream of `StorePath`s.
///
/// Unlike a `Pusher`, a `PushSession` takes a stream of `StorePath`s
/// instead of `ValidPathInfo`s, taking care of retrieving the closure
/// and path metadata.
///
/// This is useful when the list of store paths is streamed from some
/// external source (e.g., FS watcher, Unix Domain Socket) and a push
/// plan cannot be computed statically.
///
/// ## Batching
///
/// Many store paths can be built in a short period of time, with each
/// having a big closure. It can be very inefficient if we were to compute
/// closure and query for missing paths for each individual path. This is
/// especially true if we have a lot of remote builders (e.g., `attic watch-store`
/// running alongside a beefy Hydra instance).
///
/// `PushSession` batches operations in order to minimize the number of
/// closure computations and API calls. It also remembers which paths already
/// exist on the remote cache. By default, it submits a batch if it's been 2
/// seconds since the last path is queued or it's been 10 seconds in total.
pub struct PushSession {
    /// Sender to the batching future.
    sender: channel::Sender<SessionQueueCommand>,

    /// Receiver of results.
    result_receiver: mpsc::Receiver<Result<HashMap<StorePath, Result<()>>>>,
}

enum SessionQueueCommand {
    Paths(Vec<StorePath>),
    Flush,
    Terminate,
}

enum SessionQueuePoll {
    Paths(Vec<StorePath>),
    Flush,
    Terminate,
    Closed,
    TimedOut,
}

#[derive(Debug)]
pub struct PushPlan {
    /// Store paths to push.
    pub store_path_map: HashMap<StorePathHash, ValidPathInfo>,

    /// The number of paths in the original full closure.
    pub num_all_paths: usize,

    /// Number of paths that have been filtered out because they are already cached.
    pub num_already_cached: usize,

    /// Number of paths that have been filtered out because they are signed by an upstream cache.
    pub num_upstream: usize,
}

/// Wrapper to update a progress bar as a NAR is streamed.
struct NarStreamProgress<S> {
    stream: S,
    bar: ProgressBar,
}

struct UploadProgress {
    bar: ProgressBar,
    uploaded: AtomicU64,
}

impl Pusher {
    pub fn new(
        store: Arc<NixStore>,
        api: ApiClient,
        cache: CacheName,
        cache_config: CacheConfig,
        mp: MultiProgress,
        config: PushConfig,
    ) -> Self {
        let (sender, receiver) = channel::unbounded();
        let mut workers = Vec::new();

        for _ in 0..config.num_workers {
            workers.push(spawn(Self::worker(
                receiver.clone(),
                store.clone(),
                api.clone(),
                cache.clone(),
                cache_config.upload_chunking.clone(),
                mp.clone(),
                config,
            )));
        }

        Self {
            api,
            store,
            cache,
            cache_config,
            workers,
            sender,
        }
    }

    /// Queues a store path to be pushed.
    pub async fn queue(&self, path_info: ValidPathInfo) -> Result<()> {
        self.sender.send(path_info).await.map_err(|e| anyhow!(e))
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

    /// Creates a push plan.
    pub async fn plan(
        &self,
        roots: Vec<StorePath>,
        no_closure: bool,
        ignore_upstream_filter: bool,
    ) -> Result<PushPlan> {
        PushPlan::plan(
            self.store.clone(),
            &self.api,
            &self.cache,
            &self.cache_config,
            roots,
            no_closure,
            ignore_upstream_filter,
        )
        .await
    }

    /// Converts the pusher into a `PushSession`.
    ///
    /// This is useful when the list of store paths is streamed from some
    /// external source (e.g., FS watcher, Unix Domain Socket) and a push
    /// plan cannot be computed statically.
    pub fn into_push_session(self, config: PushSessionConfig) -> PushSession {
        PushSession::with_pusher(self, config)
    }

    async fn worker(
        receiver: JobReceiver,
        store: Arc<NixStore>,
        api: ApiClient,
        cache: CacheName,
        upload_chunking: Option<UploadChunkingConfig>,
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
                upload_chunking.clone(),
                mp.clone(),
                config,
            )
            .await;

            results.insert(store_path, r);
        }

        results
    }
}

impl PushSession {
    pub fn with_pusher(pusher: Pusher, config: PushSessionConfig) -> Self {
        let (sender, receiver) = channel::unbounded();
        let (result_sender, result_receiver) = mpsc::channel(1);

        let known_paths_mutex = Arc::new(Mutex::new(HashSet::new()));

        spawn(async move {
            if let Err(e) = Self::worker(
                pusher,
                config,
                known_paths_mutex.clone(),
                receiver.clone(),
                result_sender.clone(),
            )
            .await
            {
                let _ = result_sender.send(Err(e)).await;
            }
        });

        Self {
            sender,
            result_receiver,
        }
    }

    async fn worker(
        pusher: Pusher,
        config: PushSessionConfig,
        known_paths_mutex: Arc<Mutex<HashSet<StorePathHash>>>,
        receiver: channel::Receiver<SessionQueueCommand>,
        result_sender: mpsc::Sender<Result<HashMap<StorePath, Result<()>>>>,
    ) -> Result<()> {
        let mut roots = HashSet::new();

        loop {
            // Get outstanding paths in queue
            let done = tokio::select! {
                // 2 seconds since last queued path
                done = async {
                    loop {
                        let poll = tokio::select! {
                            r = receiver.recv() => match r {
                                Ok(SessionQueueCommand::Paths(paths)) => SessionQueuePoll::Paths(paths),
                                Ok(SessionQueueCommand::Flush) => SessionQueuePoll::Flush,
                                Ok(SessionQueueCommand::Terminate) => SessionQueuePoll::Terminate,
                                _ => SessionQueuePoll::Closed,
                            },
                            _ = time::sleep(Duration::from_secs(2)) => SessionQueuePoll::TimedOut,
                        };

                        match poll {
                            SessionQueuePoll::Paths(store_paths) => {
                                roots.extend(store_paths.into_iter());
                            }
                            SessionQueuePoll::Closed | SessionQueuePoll::Terminate => {
                                break true;
                            }
                            SessionQueuePoll::Flush | SessionQueuePoll::TimedOut => {
                                break false;
                            }
                        }
                    }
                } => done,

                // 10 seconds
                _ = time::sleep(Duration::from_secs(10)) => {
                    false
                },
            };

            // Compute push plan
            let roots_vec: Vec<StorePath> = {
                let known_paths = known_paths_mutex.lock().await;
                roots
                    .drain()
                    .filter(|root| !known_paths.contains(&root.to_hash()))
                    .collect()
            };

            let mut plan = pusher
                .plan(
                    roots_vec,
                    config.no_closure,
                    config.ignore_upstream_cache_filter,
                )
                .await?;

            let mut known_paths = known_paths_mutex.lock().await;
            plan.store_path_map
                .retain(|sph, _| !known_paths.contains(sph));

            // Push everything
            for (store_path_hash, path_info) in plan.store_path_map.into_iter() {
                pusher.queue(path_info).await?;
                known_paths.insert(store_path_hash);
            }

            drop(known_paths);

            if done {
                let result = pusher.wait().await;
                result_sender.send(Ok(result)).await?;
                return Ok(());
            }
        }
    }

    /// Waits for all workers to terminate, returning all results.
    pub async fn wait(mut self) -> Result<HashMap<StorePath, Result<()>>> {
        self.flush()?;

        // The worker might have died
        let _ = self.sender.send(SessionQueueCommand::Terminate).await;

        self.result_receiver
            .recv()
            .await
            .expect("Nothing in result channel")
    }

    /// Queues multiple store paths to be pushed.
    pub fn queue_many(&self, store_paths: Vec<StorePath>) -> Result<()> {
        self.sender
            .send_blocking(SessionQueueCommand::Paths(store_paths))
            .map_err(|e| anyhow!(e))
    }

    /// Flushes the worker queue.
    pub fn flush(&self) -> Result<()> {
        self.sender
            .send_blocking(SessionQueueCommand::Flush)
            .map_err(|e| anyhow!(e))
    }
}

impl PushPlan {
    /// Creates a plan.
    async fn plan(
        store: Arc<NixStore>,
        api: &ApiClient,
        cache: &CacheName,
        cache_config: &CacheConfig,
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

        if !ignore_upstream_filter {
            // Filter out paths signed by upstream caches
            let upstream_cache_key_names = cache_config
                .upstream_cache_key_names
                .as_ref()
                .map_or([].as_slice(), |v| v.as_slice());
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

/// Uploads a single path to a cache.
pub async fn upload_path(
    path_info: ValidPathInfo,
    store: Arc<NixStore>,
    api: ApiClient,
    cache: &CacheName,
    upload_chunking: Option<UploadChunkingConfig>,
    mp: MultiProgress,
    config: PushConfig,
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
        "{{spinner}} {: <20.20} {{bar:40.green/blue}} {{human_bytes:10}} ({{average_speed}}) {{msg}}",
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
    let nar_stream = || store.nar_from_path(path.to_owned());

    let start = Instant::now();
    let upload_chunking = upload_chunking.filter(|c| c.max_chunk_size > 0);
    if config.chunk_size.is_some() && upload_chunking.is_none() {
        return Err(anyhow!("Chunked transport upload is not enabled by server"));
    }

    let upload_result = if let Some(upload_chunking) = upload_chunking {
        let chunk_size = match config.chunk_size {
            Some(chunk_size) if chunk_size > upload_chunking.max_chunk_size => {
                return Err(anyhow!("Chunk size exceeds server limit"));
            }
            Some(chunk_size) => chunk_size,
            None => upload_chunking.max_chunk_size,
        };

        if (path_info.nar_size as usize) <= chunk_size.max(1) {
            api.upload_path(
                upload_info,
                NarStreamProgress::new(nar_stream(), bar.clone()).map_ok(Bytes::from),
                config.force_preamble,
            )
            .await
        } else {
            let progress = Arc::new(UploadProgress::new(bar.clone()));

            upload_path_chunked(
                &api,
                upload_info,
                nar_stream().map_ok(Bytes::from),
                chunk_size,
                progress,
            )
            .await
            .map(Some)
        }
    } else {
        api.upload_path(
            upload_info,
            NarStreamProgress::new(nar_stream(), bar.clone()).map_ok(Bytes::from),
            config.force_preamble,
        )
        .await
    };

    match upload_result {
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

async fn upload_path_chunked<S>(
    api: &ApiClient,
    upload_info: UploadPathNarInfo,
    stream: S,
    chunk_size: usize,
    progress: Arc<UploadProgress>,
) -> Result<UploadPathResult>
where
    S: Stream<Item = AtticResult<Bytes>> + Send + 'static,
{
    let session = api
        .start_upload_session(upload_info, Some(chunk_size))
        .await?;
    let (session_id, chunk_size) = match session {
        StartUploadPathSessionResponse::Session {
            session_id,
            chunk_size,
            ..
        } => (session_id, chunk_size),
        StartUploadPathSessionResponse::Completed { result } => return Ok(result),
    };
    let part_stream = byte_chunker(stream, chunk_size.max(1));
    futures::pin_mut!(part_stream);

    while let Some((seq, bytes)) = match part_stream.try_next().await {
        Ok(part) => part,
        Err(e) => {
            let _ = api.abort_upload_session(session_id).await;
            return Err(e);
        }
    } {
        let upload_progress = progress.clone();
        if let Err(e) = api
            .upload_session_part_with_progress(session_id, seq, bytes, move |len| {
                upload_progress.add(len);
            })
            .await
        {
            let _ = api.abort_upload_session(session_id).await;
            return Err(e);
        }
    }

    progress.set_message("finalizing");
    let mut error_delay = Duration::from_secs(1);
    loop {
        match api.finalize_upload_session(session_id).await {
            Ok(FinalizeUploadSessionResponse::Completed { result }) => return Ok(result),
            Ok(FinalizeUploadSessionResponse::Failed { message }) => return Err(anyhow!(message)),
            Ok(FinalizeUploadSessionResponse::Pending) => {
                error_delay = Duration::from_secs(1);
                time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => {
                if e.downcast_ref::<ApiError>()
                    .is_some_and(|e| !e.is_retryable())
                {
                    return Err(e);
                }

                progress.set_message("finalizing (retrying)");
                time::sleep(error_delay).await;
                error_delay = (error_delay * 2).min(Duration::from_secs(30));
                progress.set_message("finalizing");
            }
        }
    }
}

fn byte_chunker<S>(stream: S, chunk_size: usize) -> impl Stream<Item = Result<(u32, Bytes)>>
where
    S: Stream<Item = AtticResult<Bytes>> + Send + 'static,
{
    async_stream::try_stream! {
        futures::pin_mut!(stream);
        let mut seq = 0u32;
        let mut buffer = BytesMut::new();

        while let Some(bytes) = stream.try_next().await? {
            buffer.extend_from_slice(&bytes);
            while buffer.len() >= chunk_size {
                let chunk = buffer.split_to(chunk_size).freeze();
                yield (seq, chunk);
                seq = seq.checked_add(1).ok_or_else(|| anyhow!("Too many upload parts"))?;
            }
        }

        if !buffer.is_empty() {
            yield (seq, buffer.freeze());
        }
    }
}

impl<S: Stream<Item = AtticResult<Vec<u8>>>> NarStreamProgress<S> {
    fn new(stream: S, bar: ProgressBar) -> Self {
        Self { stream, bar }
    }
}

impl UploadProgress {
    fn new(bar: ProgressBar) -> Self {
        Self {
            bar,
            uploaded: AtomicU64::new(0),
        }
    }

    fn add(&self, amount: u64) {
        let new_position = self.uploaded.fetch_add(amount, Ordering::Relaxed) + amount;
        self.bar.set_position(new_position);
    }

    fn set_message(&self, message: &'static str) {
        self.bar.set_message(message);
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

#[cfg(test)]
mod tests {
    use super::*;

    use futures::stream;

    #[tokio::test]
    async fn byte_chunker_splits_fragmented_stream_into_fixed_parts() {
        let input = stream::iter([
            Ok(Bytes::from_static(b"ab")),
            Ok(Bytes::from_static(b"cdefg")),
            Ok(Bytes::from_static(b"hij")),
        ]);

        let chunks = byte_chunker(input, 4)
            .try_collect::<Vec<_>>()
            .await
            .unwrap();

        assert_eq!(
            chunks,
            vec![
                (0, Bytes::from_static(b"abcd")),
                (1, Bytes::from_static(b"efgh")),
                (2, Bytes::from_static(b"ij")),
            ]
        );
    }

    #[tokio::test]
    async fn byte_chunker_does_not_emit_empty_tail_for_exact_multiple() {
        let input = stream::iter([Ok(Bytes::from_static(b"abcdefgh"))]);

        let chunks = byte_chunker(input, 4)
            .try_collect::<Vec<_>>()
            .await
            .unwrap();

        assert_eq!(
            chunks,
            vec![
                (0, Bytes::from_static(b"abcd")),
                (1, Bytes::from_static(b"efgh")),
            ]
        );
    }

    #[tokio::test]
    async fn byte_chunker_emits_no_parts_for_empty_stream() {
        let input = stream::empty();

        let chunks = byte_chunker(input, 4)
            .try_collect::<Vec<_>>()
            .await
            .unwrap();

        assert!(chunks.is_empty());
    }
}

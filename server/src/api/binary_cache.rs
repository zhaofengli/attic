//! Nix Binary Cache server.
//!
//! This module implements the Nix Binary Cache API.
//!
//! The implementation is based on the specifications at <https://github.com/fzakaria/nix-http-binary-cache-api-spec>.

use std::collections::VecDeque;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_stream::try_stream;
use axum::http;
use axum::{
    body::Body,
    extract::{Extension, Path},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use bytes::Bytes;
use futures::stream::{BoxStream, StreamExt as _};
use futures::TryStreamExt as _;
use serde::Serialize;
use tokio::io::AsyncRead;
use tokio::time::sleep;
use tokio_util::io::ReaderStream;
use tracing::instrument;

use crate::database::entity::chunk::ChunkModel;
use crate::database::ObjectAndChunks;
use crate::error::{ErrorKind, ServerResult};
use crate::narinfo::NarInfo;
use crate::nix_manifest;
use crate::storage::{Download, RemoteFile, StorageBackend};
use crate::{RequestState, State};
use attic::cache::CacheName;
use attic::io::merge_chunks;
use attic::mime;
use attic::nix_store::StorePathHash;

/// How many attempts in a row may fail to move a chunk forward before the NAR
/// stream is failed. Attempts that do make progress don't count against it.
const CHUNK_STREAM_MAX_ATTEMPTS: usize = 5;

/// Hard stop for a single chunk, so a body that keeps dying after a byte or two
/// can't keep a NAR stream alive forever.
const CHUNK_STREAM_MAX_TOTAL_ATTEMPTS: usize = 20;

/// Nix cache information.
///
/// An example of a correct response is as follows:
///
/// ```text
/// StoreDir: /nix/store
/// WantMassQuery: 1
/// Priority: 40
/// ```
#[derive(Debug, Clone, Serialize)]
struct NixCacheInfo {
    /// Whether this binary cache supports bulk queries.
    #[serde(rename = "WantMassQuery")]
    want_mass_query: bool,

    /// The Nix store path this binary cache uses.
    #[serde(rename = "StoreDir")]
    store_dir: PathBuf,

    /// The priority of the binary cache.
    ///
    /// A lower number denotes a higher priority.
    /// <https://cache.nixos.org> has a priority of 40.
    #[serde(rename = "Priority")]
    priority: i32,
}

impl IntoResponse for NixCacheInfo {
    fn into_response(self) -> Response {
        match nix_manifest::to_string(&self) {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", mime::NIX_CACHE_INFO)
                .body(body)
                .unwrap()
                .into_response(),
            Err(e) => e.into_response(),
        }
    }
}

/// Gets information on a cache.
#[instrument(skip_all, fields(cache_name))]
async fn get_nix_cache_info(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path(cache_name): Path<CacheName>,
) -> ServerResult<NixCacheInfo> {
    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_pull()?;
            Ok(cache)
        })
        .await?;

    req_state.set_public_cache(cache.is_public);

    let info = NixCacheInfo {
        want_mass_query: true,
        store_dir: cache.store_dir.into(),
        priority: cache.priority,
    };

    Ok(info)
}

/// Gets various information on a store path hash.
///
/// `/:cache/:path`, which may be one of
/// - GET `/:cache/{storePathHash}.narinfo`
/// - HEAD `/:cache/{storePathHash}.narinfo`
/// - GET `/:cache/{storePathHash}.ls` (not implemented)
#[instrument(skip_all, fields(cache_name, path))]
#[axum_macros::debug_handler]
async fn get_store_path_info(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path((cache_name, path)): Path<(CacheName, String)>,
) -> ServerResult<NarInfo> {
    let components: Vec<&str> = path.splitn(2, '.').collect();

    if components.len() != 2 {
        return Err(ErrorKind::NotFound.into());
    }

    // TODO: Other endpoints
    if components[1] != "narinfo" {
        return Err(ErrorKind::NotFound.into());
    }

    let store_path_hash = StorePathHash::new(components[0].to_string())?;

    tracing::debug!(
        "Received request for {}.narinfo in {:?}",
        store_path_hash.as_str(),
        cache_name
    );

    let ObjectAndChunks {
        object, cache, nar, ..
    } = state
        .find_object_and_chunks_cached(&cache_name, &store_path_hash, false)
        .await?;

    let permission = req_state
        .auth
        .get_permission_for_cache(&cache_name, cache.is_public);
    permission.require_pull()?;

    req_state.set_public_cache(cache.is_public);

    let mut narinfo = object.to_nar_info(&nar)?;

    if narinfo.signature().is_none() {
        let keypair = cache.keypair()?;
        narinfo.sign(&keypair);
    }

    Ok(narinfo)
}

fn io_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> IoError {
    IoError::other(e)
}

/// Renders an error together with its `source()` chain.
///
/// aws-smithy's `ByteStream` error displays as a bare "streaming error"; the
/// cause (connection reset, timeout, storage 5xx) only lives in the chain.
fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut chain = e.to_string();
    let mut source = e.source();

    while let Some(cause) = source {
        chain.push_str(": ");
        chain.push_str(&cause.to_string());
        source = cause.source();
    }

    chain
}

/// 100ms, 200ms, 400ms, 800ms. Kept short on purpose: the client is sitting on
/// an open chunked response and nix gives up on a stalled download after 5min.
fn chunk_retry_backoff(attempt: usize) -> Duration {
    Duration::from_millis(100 << (attempt.clamp(1, 4) - 1))
}

/// Streams one chunk, resuming with a ranged request if its body dies midway.
///
/// A NAR is reassembled from up to thousands of chunks, and a single failed
/// body used to kill the whole response: the client sees a truncated chunked
/// body ("Transferred a partial file") and nix restarts the NAR from byte 0.
fn resumable_chunk_stream(
    remote_file: RemoteFile,
    remote_file_id: String,
    // Compressed size of the chunk, i.e. exactly the number of bytes we stream
    // out. `None` when the file hashes were never confirmed.
    expected_size: Option<u64>,
    storage: Arc<Box<dyn StorageBackend + 'static>>,
    first_reader: Box<dyn AsyncRead + Unpin + Send>,
) -> BoxStream<'static, Result<Bytes, IoError>> {
    let stream = try_stream! {
        // The first body is already open: merge_chunks prefetches it so the
        // storage round-trip overlaps with the preceding chunks.
        let mut opened = Some(first_reader);
        let mut offset: u64 = 0;
        let mut attempt: usize = 0;
        let mut total_attempts: usize = 0;
        let mut progress_at: u64 = 0;

        loop {
            attempt += 1;
            total_attempts += 1;

            let reader = if let Some(reader) = opened.take() {
                reader
            } else {
                match storage.stream_file_db_from(&remote_file, offset).await {
                    Ok(reader) => reader,
                    Err(e) => {
                        if attempt >= CHUNK_STREAM_MAX_ATTEMPTS
                            || total_attempts >= CHUNK_STREAM_MAX_TOTAL_ATTEMPTS
                        {
                            Err::<(), IoError>(io_error(e))?;
                        } else {
                            tracing::warn!(
                                remote_file_id = %remote_file_id,
                                offset,
                                attempt,
                                error = %error_chain(&e),
                                "Reopening chunk failed, retrying",
                            );
                            metrics::counter!(
                                "atticd_chunk_stream_retries_total",
                                "reason" => "reopen",
                            )
                            .increment(1);
                            sleep(chunk_retry_backoff(attempt)).await;
                        }
                        continue;
                    }
                }
            };

            let mut body = ReaderStream::new(reader);
            let mut failure = None;

            while let Some(item) = body.next().await {
                match item {
                    Ok(bytes) => {
                        offset += bytes.len() as u64;
                        yield bytes;
                    }
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }

            // A body that just ends early is as fatal as one that errors: it
            // silently corrupts the NAR. file_size is the compressed size,
            // which is exactly what we stream out.
            if failure.is_none() {
                if let Some(expected) = expected_size {
                    if offset < expected {
                        failure = Some(IoError::new(
                            IoErrorKind::UnexpectedEof,
                            format!("chunk body ended at {} of {} bytes", offset, expected),
                        ));
                    }
                }
            }

            let Some(e) = failure else {
                break;
            };

            if offset > progress_at {
                // This attempt moved the stream forward, so earlier blips on
                // the same chunk shouldn't count against it.
                progress_at = offset;
                attempt = 0;
            }

            if attempt >= CHUNK_STREAM_MAX_ATTEMPTS
                || total_attempts >= CHUNK_STREAM_MAX_TOTAL_ATTEMPTS
            {
                Err::<(), IoError>(e)?;
            } else {
                tracing::warn!(
                    remote_file_id = %remote_file_id,
                    offset,
                    attempt,
                    error = %error_chain(&e),
                    "Chunk body failed, resuming from offset",
                );
                metrics::counter!(
                    "atticd_chunk_stream_retries_total",
                    "reason" => "body",
                )
                .increment(1);
                sleep(chunk_retry_backoff(attempt)).await;
            }
        }
    };

    Box::pin(stream)
}

/// Gets a NAR.
///
/// - GET `:cache/nar/{storePathHash}.nar`
///
/// Here we use the store path hash not the NAR hash or file hash
/// for better logging. In reality, the files are deduplicated by
/// content-addressing.
#[instrument(skip_all, fields(cache_name, path))]
async fn get_nar(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path((cache_name, path)): Path<(CacheName, String)>,
) -> ServerResult<Response> {
    let components: Vec<&str> = path.splitn(2, '.').collect();

    if components.len() != 2 {
        return Err(ErrorKind::NotFound.into());
    }

    if components[1] != "nar" {
        return Err(ErrorKind::NotFound.into());
    }

    let store_path_hash = StorePathHash::new(components[0].to_string())?;

    tracing::debug!(
        "Received request for {}.nar in {:?}",
        store_path_hash.as_str(),
        cache_name
    );

    let ObjectAndChunks {
        object,
        cache,
        chunks,
        ..
    } = state
        .find_object_and_chunks_cached(&cache_name, &store_path_hash, true)
        .await?;

    let permission = req_state
        .auth
        .get_permission_for_cache(&cache_name, cache.is_public);
    permission.require_pull()?;

    req_state.set_public_cache(cache.is_public);

    if chunks.iter().any(Option::is_none) {
        // at least one of the chunks is missing :(
        return Err(ErrorKind::IncompleteNar.into());
    }

    // Batched off the hot path: one UPDATE per flush interval instead of one
    // per NAR download (was a third of all DB queries during a wave).
    state.queue_bump_object_last_accessed(object.id);

    if chunks.len() == 1 {
        // single chunk
        let chunk = chunks[0].as_ref().unwrap();
        let remote_file = &chunk.remote_file.0;
        let storage = state.storage().await?;
        match storage.download_file_db(remote_file, false).await? {
            Download::Url(url) => Ok(Redirect::temporary(&url).into_response()),
            Download::AsyncRead(stream) => {
                let hash = store_path_hash.as_str().to_owned();
                let stream = ReaderStream::new(stream).map_err(move |e| {
                    tracing::error!(
                        store_path_hash = %hash,
                        error = %error_chain(&e),
                        "NAR stream aborted",
                    );
                    metrics::counter!("atticd_nar_stream_aborted_total").increment(1);
                    e
                });
                let body = Body::from_stream(stream);

                Ok((
                    [(
                        http::header::CONTENT_TYPE,
                        http::HeaderValue::from_static(mime::NAR),
                    )],
                    body,
                )
                    .into_response())
            }
        }
    } else {
        // reassemble NAR
        let streamer = |chunk: ChunkModel, storage: Arc<Box<dyn StorageBackend + 'static>>| async move {
            // Opened eagerly so the prefetch in merge_chunks keeps hiding the
            // storage round-trip; retries of *this* body happen inside the
            // returned stream, which knows how many bytes already went out.
            let remote_file = chunk.remote_file.0.clone();
            let reader = storage
                .stream_file_db_from(&remote_file, 0)
                .await
                .map_err(io_error)?;

            Ok(resumable_chunk_stream(
                remote_file,
                chunk.remote_file_id,
                chunk.file_size.map(|size| size as u64),
                storage,
                reader,
            ))
        };

        let chunks: VecDeque<_> = chunks.into_iter().map(Option::unwrap).collect();
        let storage = state.storage().await?.clone();

        // The ideal prefetch depends on the average chunk size and storage RTT.
        //
        // Bumped from 2 to 16 (now `num-prefetch` in config) to reduce
        // per-NAR-stream latency: with OSS RTT ~33ms and chunks of ~64 KiB,
        // prefetch=2 caps a single NAR-stream at ~3.8 MiB/s, causing 10-min
        // nix-store timeouts on large (LLVM, GCC, etc.) derivations under
        // concurrent worker pulls. At prefetch=16 the per-stream cap becomes
        // ~30 MiB/s. Raising it multiplies concurrent storage requests per
        // stream — do that only after re-chunking to larger chunks.
        let hash = store_path_hash.as_str().to_owned();
        let merged =
            merge_chunks(chunks, streamer, storage, state.config.num_prefetch).map_err(move |e| {
                tracing::error!(
                    store_path_hash = %hash,
                    error = %error_chain(&e),
                    "NAR stream aborted",
                );
                metrics::counter!("atticd_nar_stream_aborted_total").increment(1);
                e
            });
        let body = Body::from_stream(merged);

        Ok((
            [(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static(mime::NAR),
            )],
            body,
        )
            .into_response())
    }
}

pub fn get_router() -> Router {
    Router::new()
        .route("/:cache/nix-cache-info", get(get_nix_cache_info))
        .route("/:cache/:path", get(get_store_path_info))
        .route("/:cache/nar/:path", get(get_nar))
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll};

    use async_trait::async_trait;
    use tokio::io::ReadBuf;

    use super::*;
    use crate::storage::HttpRemoteFile;

    /// Hands out at most 8 bytes per poll, then either stops or errors.
    struct FlakyReader {
        data: Vec<u8>,
        pos: usize,
        /// Error out after this many bytes instead of reaching EOF.
        fail_at: Option<usize>,
    }

    impl AsyncRead for FlakyReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<Result<(), IoError>> {
            let limit = self.fail_at.unwrap_or(self.data.len());

            if self.pos >= limit {
                if self.fail_at.is_some() {
                    return Poll::Ready(Err(IoError::new(
                        IoErrorKind::ConnectionReset,
                        "injected body failure",
                    )));
                }

                return Poll::Ready(Ok(())); // EOF
            }

            let n = (limit - self.pos).min(8).min(buf.remaining());
            let bytes = self.data[self.pos..self.pos + n].to_vec();
            buf.put_slice(&bytes);
            self.pos += n;

            Poll::Ready(Ok(()))
        }
    }

    /// Serves ranged reads out of an in-memory blob, recording the offsets.
    #[derive(Debug)]
    struct RangeBackend {
        data: Vec<u8>,
        offsets: Mutex<Vec<u64>>,
        /// Fail every reopen instead of serving it.
        broken: bool,
        /// Let each reopened body die after this many bytes.
        fail_body_after: Option<usize>,
    }

    impl RangeBackend {
        fn serving(data: Vec<u8>) -> Arc<Box<dyn StorageBackend + 'static>> {
            Arc::new(Box::new(Self {
                data,
                offsets: Mutex::new(Vec::new()),
                broken: false,
                fail_body_after: None,
            }))
        }

        fn broken() -> Arc<Box<dyn StorageBackend + 'static>> {
            Arc::new(Box::new(Self {
                data: Vec::new(),
                offsets: Mutex::new(Vec::new()),
                broken: true,
                fail_body_after: None,
            }))
        }

        fn with_flaky_bodies(
            data: Vec<u8>,
            fail_body_after: usize,
        ) -> Arc<Box<dyn StorageBackend + 'static>> {
            Arc::new(Box::new(Self {
                data,
                offsets: Mutex::new(Vec::new()),
                broken: false,
                fail_body_after: Some(fail_body_after),
            }))
        }
    }

    #[async_trait]
    impl StorageBackend for RangeBackend {
        async fn upload_file(
            &self,
            _name: String,
            _stream: &mut (dyn AsyncRead + Unpin + Send),
        ) -> ServerResult<RemoteFile> {
            unimplemented!()
        }

        async fn delete_file(&self, _name: String) -> ServerResult<()> {
            unimplemented!()
        }

        async fn delete_file_db(&self, _file: &RemoteFile) -> ServerResult<()> {
            unimplemented!()
        }

        async fn download_file_db(
            &self,
            _file: &RemoteFile,
            _prefer_stream: bool,
        ) -> ServerResult<Download> {
            unimplemented!()
        }

        async fn make_db_reference(&self, _name: String) -> ServerResult<RemoteFile> {
            unimplemented!()
        }

        async fn stream_file_db_from(
            &self,
            _file: &RemoteFile,
            offset: u64,
        ) -> ServerResult<Box<dyn AsyncRead + Unpin + Send>> {
            self.offsets.lock().unwrap().push(offset);

            if self.broken {
                return Err(ErrorKind::StorageError(anyhow::anyhow!("storage is down")).into());
            }

            let remaining = self.data[offset as usize..].to_vec();
            let fail_at = self
                .fail_body_after
                .filter(|fail_after| *fail_after < remaining.len());

            Ok(Box::new(FlakyReader {
                data: remaining,
                pos: 0,
                fail_at,
            }))
        }
    }

    fn remote_file() -> RemoteFile {
        RemoteFile::Http(HttpRemoteFile {
            url: "http://invalid.invalid/chunk".to_string(),
        })
    }

    async fn collect(
        mut stream: BoxStream<'static, Result<Bytes, IoError>>,
    ) -> Result<Vec<u8>, IoError> {
        let mut out = Vec::new();

        while let Some(item) = stream.next().await {
            out.extend_from_slice(&item?);
        }

        Ok(out)
    }

    #[tokio::test]
    async fn resumes_a_body_that_dies_midway() {
        let data: Vec<u8> = (0..=255).collect();
        let storage = RangeBackend::serving(data.clone());
        let first = Box::new(FlakyReader {
            data: data.clone(),
            pos: 0,
            fail_at: Some(100),
        });

        let out = collect(resumable_chunk_stream(
            remote_file(),
            "chunk".to_string(),
            Some(data.len() as u64),
            storage.clone(),
            first,
        ))
        .await
        .expect("stream should recover");

        // Not a byte lost, not a byte doubled.
        assert_eq!(out, data);
    }

    #[tokio::test]
    async fn resumes_a_body_that_ends_early_without_an_error() {
        let data: Vec<u8> = (0..=255).collect();
        let storage = RangeBackend::serving(data.clone());
        // Ends cleanly at 100 bytes — the failure mode nix cannot detect either.
        let first = Box::new(FlakyReader {
            data: data[..100].to_vec(),
            pos: 0,
            fail_at: None,
        });

        let out = collect(resumable_chunk_stream(
            remote_file(),
            "chunk".to_string(),
            Some(data.len() as u64),
            storage.clone(),
            first,
        ))
        .await
        .expect("short body should be resumed");

        assert_eq!(out, data);
    }

    #[tokio::test]
    async fn keeps_going_while_retries_make_progress() {
        let data: Vec<u8> = (0..=255).collect();
        // Every body dies after 32 bytes, so the chunk needs 8 opens in total —
        // more than CHUNK_STREAM_MAX_ATTEMPTS, but each one moves forward.
        let storage = RangeBackend::with_flaky_bodies(data.clone(), 32);
        let first = Box::new(FlakyReader {
            data: data.clone(),
            pos: 0,
            fail_at: Some(32),
        });

        let out = collect(resumable_chunk_stream(
            remote_file(),
            "chunk".to_string(),
            Some(data.len() as u64),
            storage.clone(),
            first,
        ))
        .await
        .expect("attempts that make progress must not exhaust the budget");

        assert_eq!(out, data);
    }

    #[tokio::test]
    async fn fails_the_stream_once_attempts_run_out() {
        let data: Vec<u8> = (0..=255).collect();
        let storage = RangeBackend::broken();
        let first = Box::new(FlakyReader {
            data: data.clone(),
            pos: 0,
            fail_at: Some(10),
        });

        let err = collect(resumable_chunk_stream(
            remote_file(),
            "chunk".to_string(),
            Some(data.len() as u64),
            storage.clone(),
            first,
        ))
        .await
        .expect_err("a permanently broken storage must fail the stream");

        assert!(
            error_chain(&err).contains("storage is down"),
            "cause should survive in the chain: {}",
            error_chain(&err)
        );
    }
}

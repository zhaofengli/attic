//! Nix Binary Cache server.
//!
//! This module implements the Nix Binary Cache API.
//!
//! The implementation is based on the specifications at <https://github.com/fzakaria/nix-http-binary-cache-api-spec>.

use std::collections::VecDeque;
use std::io::Error as IoError;
use std::path::PathBuf;
use std::sync::Arc;

use axum::http;
use axum::{
    Router,
    body::Body,
    extract::{Extension, Path},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use futures::StreamExt as _;
use futures::TryStreamExt as _;
use futures::stream::BoxStream;
use serde::Serialize;
use tokio_util::io::ReaderStream;
use tracing::instrument;

use crate::database::AtticDatabase;
use crate::database::entity::chunk::ChunkModel;
use crate::error::{ErrorKind, ServerResult};
use crate::narinfo::NarInfo;
use crate::nix_manifest;
use crate::storage::{Download, RemoteFile, StorageBackend, StorageBackendImpl};
use crate::{RequestState, State};
use attic::cache::CacheName;
use attic::io::merge_chunks;
use attic::mime;
use attic::nix_store::StorePathHash;

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

    let (object, cache, nar, _) = state
        .database()
        .await?
        .find_object_and_chunks_by_store_path_hash(&cache_name, &store_path_hash, false)
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

/// Upper bound on how many chunks a NAR may have before we stop probing all of
/// them. Large chunked NARs (a multi-GiB image can be tens of thousands of
/// chunks) would otherwise fan out one HEAD per chunk on every GET, which -- at
/// the substituter parallelism a fleet reaches during a rebuild -- becomes a
/// self-inflicted load spike on the object store and a tail-latency floor on
/// healthy pulls. Above this we probe only the first chunk (the observed
/// corruption is all-or-nothing) and rely on the hard mid-stream abort for the
/// rest.
const PROBE_ALL_MAX_CHUNKS: usize = 64;

/// Maximum concurrent presence probes for a single NAR, so a many-chunk NAR
/// does not open a burst of connections to the backend at once.
const PROBE_CONCURRENCY: usize = 32;

/// Probes one chunk. Returns its storage key if the object is definitively
/// missing, or `None` if it is present *or* the probe failed transiently.
///
/// A transient probe error (anything but a definitive "not found") is treated
/// as present: a busy-but-healthy backend must stay available, and the
/// mid-stream abort remains the safety net for a chunk that turns out to be
/// gone. Takes an owned `RemoteFile` and an `Arc` to the backend so the future
/// borrows nothing, which keeps the enclosing handler future `Send` under
/// `buffer_unordered` (borrowed captures there trip a higher-ranked-lifetime
/// `Send` bound, rust-lang/rust#102211).
async fn probe_chunk(storage: Arc<StorageBackendImpl>, remote_file: RemoteFile) -> Option<String> {
    match storage.file_exists_db(&remote_file).await {
        Ok(true) => None,
        Ok(false) => Some(remote_file.remote_file_id()),
        Err(e) => {
            tracing::warn!(%e, chunk = %remote_file.remote_file_id(), "chunk presence probe failed; assuming present");
            None
        }
    }
}

/// Returns the storage key of the first chunk found missing from the backing
/// store, or `None` if the probed chunks are all present (or failed transiently).
///
/// Always probes the first chunk; probes the rest only when the NAR is small
/// enough (see [`PROBE_ALL_MAX_CHUNKS`]). `chunks` must contain no `None`
/// (checked by the caller).
async fn find_missing_chunk(
    storage: Arc<StorageBackendImpl>,
    chunks: &[Option<ChunkModel>],
) -> Option<String> {
    let remote_file = |i: usize| chunks[i].as_ref().unwrap().remote_file.0.clone();

    if let Some(missing) = probe_chunk(storage.clone(), remote_file(0)).await {
        return Some(missing);
    }

    if chunks.len() > PROBE_ALL_MAX_CHUNKS {
        return None;
    }

    futures::stream::iter(1..chunks.len())
        .map(|i| probe_chunk(storage.clone(), remote_file(i)))
        .buffer_unordered(PROBE_CONCURRENCY)
        .filter_map(std::future::ready)
        .next()
        .await
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

    let database = state.database().await?;

    let (object, cache, _nar, chunks) = database
        .find_object_and_chunks_by_store_path_hash(&cache_name, &store_path_hash, true)
        .await?;

    let permission = req_state
        .auth
        .get_permission_for_cache(&cache_name, cache.is_public);
    permission.require_pull()?;

    req_state.set_public_cache(cache.is_public);

    // TODO: Fully kill chunk recovery
    if chunks.iter().any(Option::is_none) {
        // at least one of the chunks is missing :(
        return Err(ErrorKind::IncompleteNar.into());
    }

    // Fail closed on chunks whose object is gone from the backing store.
    //
    // The chunk rows all exist (checked above), but the object each row points
    // at can still be absent from storage (e.g. lost to backend corruption).
    // Because the response below streams chunks lazily, such a miss would
    // otherwise surface only after the `200 OK` headers are committed, and the
    // client would receive a truncated body it reads as a successful-but-short
    // transfer. Probe chunk presence up front so a miss returns a clean error
    // before a single body byte is sent, rather than a poisoned 200.
    let storage = state.storage().await?;
    if let Some(missing) = find_missing_chunk(storage.clone(), &chunks).await {
        tracing::error!(
            store_path_hash = %store_path_hash.as_str(),
            chunk = %missing,
            "NAR references a chunk missing from storage; refusing to serve a truncated response"
        );
        return Err(ErrorKind::IncompleteNar.into());
    }

    database.bump_object_last_accessed(object.id).await?;

    if chunks.len() == 1 {
        // single chunk
        let chunk = chunks[0].as_ref().unwrap();
        let remote_file = &chunk.remote_file.0;
        match storage.download_file_db(remote_file, false).await? {
            Download::Url(url) => Ok(Redirect::temporary(&url).into_response()),
            Download::AsyncRead(stream) => {
                let store_path_hash = store_path_hash.as_str().to_owned();
                let stream = ReaderStream::new(stream).map_err(move |e| {
                    tracing::error!(%e, %store_path_hash, "Stream error");
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
        fn io_error<E: std::error::Error + Send + Sync + 'static>(e: E) -> IoError {
            IoError::other(e)
        }

        let streamer = |chunk: ChunkModel, storage: Arc<StorageBackendImpl>| async move {
            match storage
                .download_file_db(&chunk.remote_file.0, true)
                .await
                .map_err(io_error)?
            {
                Download::Url(_) => Err(IoError::other("URLs not supported for NAR reassembly")),
                Download::AsyncRead(stream) => {
                    let stream: BoxStream<_> = Box::pin(ReaderStream::new(stream));
                    Ok(stream)
                }
            }
        };

        let chunks: VecDeque<_> = chunks.into_iter().map(Option::unwrap).collect();
        let storage = storage.clone();

        // TODO: Make num_prefetch configurable
        // The ideal size depends on the average chunk size
        let store_path_hash = store_path_hash.as_str().to_owned();
        let merged = merge_chunks(chunks, streamer, storage, 2).map_err(move |e| {
            tracing::error!(%e, %store_path_hash, "Stream error");
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
        .route("/{cache}/nix-cache-info", get(get_nix_cache_info))
        .route("/{cache}/{path}", get(get_store_path_info))
        .route("/{cache}/nar/{path}", get(get_nar))
}

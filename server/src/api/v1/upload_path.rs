use std::io;

use std::io::Cursor;
use std::marker::Unpin;
use std::sync::Arc;

use anyhow::anyhow;
use async_compression::tokio::bufread::{BrotliEncoder, XzEncoder, ZstdEncoder};
use async_compression::Level as CompressionLevel;
use axum::{
    extract::{BodyStream, Extension, Json},
    http::HeaderMap,
};
use bytes::Bytes;
use chrono::Utc;
use digest::Output as DigestOutput;
use futures::future::join_all;
use futures::StreamExt;
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::Expr;
use sea_orm::ActiveValue::Set;
use sea_orm::{QuerySelect, TransactionTrait};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufRead, AsyncRead, BufReader};
use tokio::sync::{OnceCell, Semaphore};
use tokio::task::spawn;
use tokio_util::io::StreamReader;
use tracing::instrument;
use uuid::Uuid;

use crate::config::CompressionType;
use crate::error::{ErrorKind, ServerError, ServerResult};
use crate::narinfo::Compression;
use crate::{RequestState, State};
use attic::api::v1::upload_path::{UploadPathNarInfo, UploadPathResult, UploadPathResultKind};
use attic::hash::Hash;
use attic::stream::StreamHasher;
use attic::util::Finally;

use crate::chunking::chunk_stream;
use crate::database::entity::cache;
use crate::database::entity::chunk::{self, ChunkState, Entity as Chunk};
use crate::database::entity::chunkref::{self, Entity as ChunkRef};
use crate::database::entity::nar::{self, Entity as Nar, NarState};
use crate::database::entity::object::{self, Entity as Object};
use crate::database::entity::Json as DbJson;
use crate::database::{AtticDatabase, ChunkGuard, NarGuard};

/// Number of chunks to upload to the storage backend at once.
///
/// TODO: Make this configurable
const CONCURRENT_CHUNK_UPLOADS: usize = 10;

type CompressorFn<C> = Box<dyn FnOnce(C) -> Box<dyn AsyncRead + Unpin + Send> + Send>;

/// Data of a chunk.
enum ChunkData {
    /// Some bytes in memory.
    Bytes(Bytes),

    /// A stream with a user-claimed hash and size that are potentially incorrect.
    Stream(Box<dyn AsyncRead + Send + Unpin + 'static>, Hash, usize),
}

/// Applies compression to a stream, computing hashes along the way.
///
/// Our strategy is to stream directly onto a UUID-keyed file on the
/// storage backend, performing compression and computing the hashes
/// along the way. We delete the file if the hashes do not match.
///
/// ```text
///                    ┌───────────────────────────────────►NAR Hash
///                    │
///                    │
///                    ├───────────────────────────────────►NAR Size
///                    │
///              ┌─────┴────┐  ┌──────────┐  ┌───────────┐
/// NAR Stream──►│NAR Hasher├─►│Compressor├─►│File Hasher├─►File Stream
///              └──────────┘  └──────────┘  └─────┬─────┘
///                                                │
///                                                ├───────►File Hash
///                                                │
///                                                │
///                                                └───────►File Size
/// ```
struct CompressionStream {
    stream: Box<dyn AsyncRead + Unpin + Send>,
    nar_compute: Arc<OnceCell<(DigestOutput<Sha256>, usize)>>,
    file_compute: Arc<OnceCell<(DigestOutput<Sha256>, usize)>>,
}

trait UploadPathNarInfoExt {
    fn to_active_model(&self) -> object::ActiveModel;
}

/// Uploads a new object to the cache.
///
/// When clients request to upload an object, we first try to increment
/// the `holders_count` of one `nar` row with same NAR hash. If rows were
/// updated, it means the NAR exists in the global cache and we can deduplicate
/// after confirming the NAR hash ("Deduplicate" case). Otherwise, we perform
/// a new upload to the storage backend ("New NAR" case).
#[instrument(skip_all)]
#[axum_macros::debug_handler]
pub(crate) async fn upload_path(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    headers: HeaderMap,
    stream: BodyStream,
) -> ServerResult<Json<UploadPathResult>> {
    let upload_info: UploadPathNarInfo = {
        let header = headers
            .get("X-Attic-Nar-Info")
            .ok_or_else(|| ErrorKind::RequestError(anyhow!("X-Attic-Nar-Info must be set")))?;

        serde_json::from_slice(header.as_bytes()).map_err(ServerError::request_error)?
    };
    let cache_name = &upload_info.cache;

    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, cache_name, |cache, permission| {
            permission.require_push()?;
            Ok(cache)
        })
        .await?;

    let stream = StreamReader::new(
        stream.map(|r| r.map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))),
    );

    let username = req_state.auth.username().map(str::to_string);

    // Try to acquire a lock on an existing NAR
    let existing_nar = database.find_and_lock_nar(&upload_info.nar_hash).await?;
    match existing_nar {
        // FIXME: existing NAR may be missing chunks
        Some(existing_nar) => {
            // Deduplicate?
            let missing_chunk = ChunkRef::find()
                .filter(chunkref::Column::NarId.eq(existing_nar.id))
                .filter(chunkref::Column::ChunkId.is_null())
                .limit(1)
                .one(database)
                .await
                .map_err(ServerError::database_error)?;

            if missing_chunk.is_some() {
                // Need to repair
                upload_path_new(username, cache, upload_info, stream, database, &state).await
            } else {
                // Can actually be deduplicated
                upload_path_dedup(
                    username,
                    cache,
                    upload_info,
                    stream,
                    database,
                    &state,
                    existing_nar,
                )
                .await
            }
        }
        None => {
            // New NAR
            upload_path_new(username, cache, upload_info, stream, database, &state).await
        }
    }
}

/// Uploads a path when there is already a matching NAR in the global cache.
async fn upload_path_dedup(
    username: Option<String>,
    cache: cache::Model,
    upload_info: UploadPathNarInfo,
    stream: impl AsyncRead + Unpin,
    database: &DatabaseConnection,
    state: &State,
    existing_nar: NarGuard,
) -> ServerResult<Json<UploadPathResult>> {
    if state.config.require_proof_of_possession {
        let (mut stream, nar_compute) = StreamHasher::new(stream, Sha256::new());
        tokio::io::copy(&mut stream, &mut tokio::io::sink())
            .await
            .map_err(ServerError::request_error)?;

        // FIXME: errors
        let (nar_hash, nar_size) = nar_compute.get().unwrap();
        let nar_hash = Hash::Sha256(nar_hash.as_slice().try_into().unwrap());

        // Confirm that the NAR Hash and Size are correct
        if nar_hash.to_typed_base16() != existing_nar.nar_hash
            || *nar_size != upload_info.nar_size
            || *nar_size != existing_nar.nar_size as usize
        {
            return Err(ErrorKind::RequestError(anyhow!("Bad NAR Hash or Size")).into());
        }
    }

    // Finally...
    let txn = database
        .begin()
        .await
        .map_err(ServerError::database_error)?;

    // Create a mapping granting the local cache access to the NAR
    Object::delete_many()
        .filter(object::Column::CacheId.eq(cache.id))
        .filter(object::Column::StorePathHash.eq(upload_info.store_path_hash.to_string()))
        .exec(&txn)
        .await
        .map_err(ServerError::database_error)?;
    Object::insert({
        let mut new_object = upload_info.to_active_model();
        new_object.cache_id = Set(cache.id);
        new_object.nar_id = Set(existing_nar.id);
        new_object.created_at = Set(Utc::now());
        new_object.created_by = Set(username);
        new_object
    })
    .exec(&txn)
    .await
    .map_err(ServerError::database_error)?;

    // Also mark the NAR as complete again
    //
    // This is racy (a chunkref might have been broken in the
    // meantime), but it's okay since it's just a hint to
    // `get-missing-paths` so clients don't attempt to upload
    // again. Also see the comments in `server/src/database/entity/nar.rs`.
    Nar::update(nar::ActiveModel {
        id: Set(existing_nar.id),
        completeness_hint: Set(true),
        ..Default::default()
    })
    .exec(&txn)
    .await
    .map_err(ServerError::database_error)?;

    txn.commit().await.map_err(ServerError::database_error)?;

    // Ensure it's not unlocked earlier
    drop(existing_nar);

    Ok(Json(UploadPathResult {
        kind: UploadPathResultKind::Deduplicated,
        file_size: None, // TODO: Sum the chunks
    }))
}

/// Uploads a path when there is no matching NAR in the global cache.
///
/// It's okay if some other client races to upload the same NAR before
/// us. The `nar` table can hold duplicate NARs which can be deduplicated
/// in a background process.
async fn upload_path_new(
    username: Option<String>,
    cache: cache::Model,
    upload_info: UploadPathNarInfo,
    stream: impl AsyncRead + Send + Unpin + 'static,
    database: &DatabaseConnection,
    state: &State,
) -> ServerResult<Json<UploadPathResult>> {
    let nar_size_threshold = state.config.chunking.nar_size_threshold;

    if nar_size_threshold == 0 || upload_info.nar_size < nar_size_threshold {
        upload_path_new_unchunked(username, cache, upload_info, stream, database, state).await
    } else {
        upload_path_new_chunked(username, cache, upload_info, stream, database, state).await
    }
}

/// Uploads a path when there is no matching NAR in the global cache (chunked).
async fn upload_path_new_chunked(
    username: Option<String>,
    cache: cache::Model,
    upload_info: UploadPathNarInfo,
    stream: impl AsyncRead + Send + Unpin + 'static,
    database: &DatabaseConnection,
    state: &State,
) -> ServerResult<Json<UploadPathResult>> {
    let chunking_config = &state.config.chunking;
    let compression_config = &state.config.compression;
    let compression_type = compression_config.r#type;
    let compression_level = compression_config.level();
    let compression: Compression = compression_type.into();

    let nar_size_db = i64::try_from(upload_info.nar_size).map_err(ServerError::request_error)?;

    // FIXME: Maybe the client will send much more data than claimed
    let (stream, nar_compute) = StreamHasher::new(stream, Sha256::new());
    let mut chunks = chunk_stream(
        stream,
        chunking_config.min_size,
        chunking_config.avg_size,
        chunking_config.max_size,
    );

    let upload_chunk_limit = Arc::new(Semaphore::new(CONCURRENT_CHUNK_UPLOADS));
    let mut futures = Vec::new();

    while let Some(bytes) = chunks.next().await {
        let bytes = bytes.map_err(ServerError::request_error)?;
        let data = ChunkData::Bytes(bytes);

        // Wait for a permit before spawning
        //
        // We want to block the receive process as well, otherwise it stays ahead and
        // consumes too much memory
        let permit = upload_chunk_limit.clone().acquire_owned().await.unwrap();
        futures.push({
            let database = database.clone();
            let state = state.clone();
            let require_proof_of_possession = state.config.require_proof_of_possession;

            spawn(async move {
                let chunk = upload_chunk(
                    data,
                    compression_type,
                    compression_level,
                    database,
                    state,
                    require_proof_of_possession,
                )
                .await?;
                drop(permit);
                Ok(chunk)
            })
        });
    }

    // Confirm that the NAR Hash and Size are correct
    // FIXME: errors
    let (nar_hash, nar_size) = nar_compute.get().unwrap();
    let nar_hash = Hash::Sha256(nar_hash.as_slice().try_into().unwrap());

    if nar_hash != upload_info.nar_hash || *nar_size != upload_info.nar_size {
        return Err(ErrorKind::RequestError(anyhow!("Bad NAR Hash or Size")).into());
    }

    // Wait for all uploads to complete
    let chunks: Vec<ChunkGuard> = join_all(futures)
        .await
        .into_iter()
        .map(|join_result| join_result.unwrap())
        .collect::<ServerResult<Vec<_>>>()?;

    let file_size = chunks
        .iter()
        .fold(0, |acc, c| acc + c.file_size.unwrap() as usize);

    // Finally...
    let txn = database
        .begin()
        .await
        .map_err(ServerError::database_error)?;

    // Create a NAR entry
    let nar_id = {
        let model = nar::ActiveModel {
            state: Set(NarState::Valid),
            compression: Set(compression.to_string()),

            nar_hash: Set(upload_info.nar_hash.to_typed_base16()),
            nar_size: Set(nar_size_db),

            num_chunks: Set(chunks.len() as i32),

            created_at: Set(Utc::now()),
            ..Default::default()
        };

        let insertion = Nar::insert(model)
            .exec(&txn)
            .await
            .map_err(ServerError::database_error)?;

        insertion.last_insert_id
    };

    // Create mappings from the NAR to the chunks
    for (i, chunk) in chunks.iter().enumerate() {
        ChunkRef::insert(chunkref::ActiveModel {
            nar_id: Set(nar_id),
            seq: Set(i as i32),
            chunk_id: Set(Some(chunk.id)),
            chunk_hash: Set(chunk.chunk_hash.clone()),
            compression: Set(chunk.compression.clone()),
            ..Default::default()
        })
        .exec(&txn)
        .await
        .map_err(ServerError::database_error)?;
    }

    // Create a mapping granting the local cache access to the NAR
    Object::delete_many()
        .filter(object::Column::CacheId.eq(cache.id))
        .filter(object::Column::StorePathHash.eq(upload_info.store_path_hash.to_string()))
        .exec(&txn)
        .await
        .map_err(ServerError::database_error)?;
    Object::insert({
        let mut new_object = upload_info.to_active_model();
        new_object.cache_id = Set(cache.id);
        new_object.nar_id = Set(nar_id);
        new_object.created_at = Set(Utc::now());
        new_object.created_by = Set(username);
        new_object
    })
    .exec(&txn)
    .await
    .map_err(ServerError::database_error)?;

    txn.commit().await.map_err(ServerError::database_error)?;

    Ok(Json(UploadPathResult {
        kind: UploadPathResultKind::Uploaded,
        file_size: Some(file_size),
    }))
}

/// Uploads a path when there is no matching NAR in the global cache (unchunked).
///
/// We upload the entire NAR as a single chunk.
async fn upload_path_new_unchunked(
    username: Option<String>,
    cache: cache::Model,
    upload_info: UploadPathNarInfo,
    stream: impl AsyncRead + Send + Unpin + 'static,
    database: &DatabaseConnection,
    state: &State,
) -> ServerResult<Json<UploadPathResult>> {
    let compression_config = &state.config.compression;
    let compression_type = compression_config.r#type;
    let compression: Compression = compression_type.into();

    // Upload the entire NAR as a single chunk
    let data = ChunkData::Stream(
        Box::new(stream),
        upload_info.nar_hash.clone(),
        upload_info.nar_size,
    );
    let chunk = upload_chunk(
        data,
        compression_type,
        compression_config.level(),
        database.clone(),
        state.clone(),
        state.config.require_proof_of_possession,
    )
    .await?;
    let file_size = chunk.file_size.unwrap() as usize;

    // Finally...
    let txn = database
        .begin()
        .await
        .map_err(ServerError::database_error)?;

    // Create a NAR entry
    let nar_id = {
        let model = nar::ActiveModel {
            state: Set(NarState::Valid),
            compression: Set(compression.to_string()),

            nar_hash: Set(upload_info.nar_hash.to_typed_base16()),
            nar_size: Set(chunk.chunk_size),

            num_chunks: Set(1),

            created_at: Set(Utc::now()),
            ..Default::default()
        };

        let insertion = Nar::insert(model)
            .exec(&txn)
            .await
            .map_err(ServerError::database_error)?;

        insertion.last_insert_id
    };

    // Create a mapping from the NAR to the chunk
    ChunkRef::insert(chunkref::ActiveModel {
        nar_id: Set(nar_id),
        seq: Set(0),
        chunk_id: Set(Some(chunk.id)),
        chunk_hash: Set(upload_info.nar_hash.to_typed_base16()),
        ..Default::default()
    })
    .exec(&txn)
    .await
    .map_err(ServerError::database_error)?;

    // Create a mapping granting the local cache access to the NAR
    Object::delete_many()
        .filter(object::Column::CacheId.eq(cache.id))
        .filter(object::Column::StorePathHash.eq(upload_info.store_path_hash.to_string()))
        .exec(&txn)
        .await
        .map_err(ServerError::database_error)?;
    Object::insert({
        let mut new_object = upload_info.to_active_model();
        new_object.cache_id = Set(cache.id);
        new_object.nar_id = Set(nar_id);
        new_object.created_at = Set(Utc::now());
        new_object.created_by = Set(username);
        new_object
    })
    .exec(&txn)
    .await
    .map_err(ServerError::database_error)?;

    txn.commit().await.map_err(ServerError::database_error)?;

    Ok(Json(UploadPathResult {
        kind: UploadPathResultKind::Uploaded,
        file_size: Some(file_size),
    }))
}

/// Uploads a chunk with the desired compression.
///
/// This will automatically perform deduplication if the chunk exists.
async fn upload_chunk(
    data: ChunkData,
    compression_type: CompressionType,
    compression_level: CompressionLevel,
    database: DatabaseConnection,
    state: State,
    require_proof_of_possession: bool,
) -> ServerResult<ChunkGuard> {
    let compression: Compression = compression_type.into();

    let given_chunk_hash = data.hash();
    let given_chunk_size = data.size();

    if let Some(existing_chunk) = database
        .find_and_lock_chunk(&given_chunk_hash, compression)
        .await?
    {
        // There's an existing chunk matching the hash
        if require_proof_of_possession && !data.is_hash_trusted() {
            let stream = data.into_async_read();

            let (mut stream, nar_compute) = StreamHasher::new(stream, Sha256::new());
            tokio::io::copy(&mut stream, &mut tokio::io::sink())
                .await
                .map_err(ServerError::request_error)?;

            // FIXME: errors
            let (nar_hash, nar_size) = nar_compute.get().unwrap();
            let nar_hash = Hash::Sha256(nar_hash.as_slice().try_into().unwrap());

            // Confirm that the NAR Hash and Size are correct
            if nar_hash.to_typed_base16() != existing_chunk.chunk_hash
                || *nar_size != given_chunk_size
                || *nar_size != existing_chunk.chunk_size as usize
            {
                return Err(ErrorKind::RequestError(anyhow!("Bad chunk hash or size")).into());
            }
        }

        return Ok(existing_chunk);
    }

    let key = format!("{}.chunk", Uuid::new_v4());

    let backend = state.storage().await?;
    let remote_file = backend.make_db_reference(key.clone()).await?;
    let remote_file_id = remote_file.remote_file_id();

    let chunk_size_db = i64::try_from(given_chunk_size).map_err(ServerError::request_error)?;

    let chunk_id = {
        let model = chunk::ActiveModel {
            state: Set(ChunkState::PendingUpload),
            compression: Set(compression.to_string()),

            // Untrusted data - To be confirmed later
            chunk_hash: Set(given_chunk_hash.to_typed_base16()),
            chunk_size: Set(chunk_size_db),

            remote_file: Set(DbJson(remote_file)),
            remote_file_id: Set(remote_file_id),

            created_at: Set(Utc::now()),
            ..Default::default()
        };

        let insertion = Chunk::insert(model)
            .exec(&database)
            .await
            .map_err(ServerError::database_error)?;

        insertion.last_insert_id
    };

    let cleanup = Finally::new({
        let database = database.clone();
        let chunk_model = chunk::ActiveModel {
            id: Set(chunk_id),
            ..Default::default()
        };
        let backend = backend.clone();
        let key = key.clone();

        async move {
            tracing::warn!("Error occurred - Cleaning up uploaded file and chunk entry");

            if let Err(e) = backend.delete_file(key).await {
                tracing::warn!("Failed to clean up failed upload: {}", e);
            }

            if let Err(e) = Chunk::delete(chunk_model).exec(&database).await {
                tracing::warn!("Failed to unregister failed chunk: {}", e);
            }
        }
    });

    // Compress and stream to the storage backend
    let compressor = get_compressor_fn(compression_type, compression_level);
    let mut stream = CompressionStream::new(data.into_async_read(), compressor);

    backend
        .upload_file(key, stream.stream())
        .await
        .map_err(ServerError::storage_error)?;

    // Confirm that the chunk hash is correct
    let (chunk_hash, chunk_size) = stream.nar_hash_and_size().unwrap();
    let (file_hash, file_size) = stream.file_hash_and_size().unwrap();

    let chunk_hash = Hash::Sha256(chunk_hash.as_slice().try_into().unwrap());
    let file_hash = Hash::Sha256(file_hash.as_slice().try_into().unwrap());

    if chunk_hash != given_chunk_hash || *chunk_size != given_chunk_size {
        return Err(ErrorKind::RequestError(anyhow!("Bad chunk hash or size")).into());
    }

    // Finally...
    let txn = database
        .begin()
        .await
        .map_err(ServerError::database_error)?;

    // Update the file hash and size, and set the chunk to valid
    let file_size_db = i64::try_from(*file_size).map_err(ServerError::request_error)?;
    let chunk = Chunk::update(chunk::ActiveModel {
        id: Set(chunk_id),
        state: Set(ChunkState::Valid),
        file_hash: Set(Some(file_hash.to_typed_base16())),
        file_size: Set(Some(file_size_db)),
        holders_count: Set(1),
        ..Default::default()
    })
    .exec(&txn)
    .await
    .map_err(ServerError::database_error)?;

    // Also repair broken chunk references pointing at the same chunk
    let repaired = ChunkRef::update_many()
        .col_expr(chunkref::Column::ChunkId, Expr::value(chunk_id))
        .filter(chunkref::Column::ChunkId.is_null())
        .filter(chunkref::Column::ChunkHash.eq(chunk_hash.to_typed_base16()))
        .filter(chunkref::Column::Compression.eq(compression.to_string()))
        .exec(&txn)
        .await
        .map_err(ServerError::database_error)?;

    txn.commit().await.map_err(ServerError::database_error)?;

    cleanup.cancel();

    tracing::debug!("Repaired {} chunkrefs", repaired.rows_affected);

    let guard = ChunkGuard::from_locked(database.clone(), chunk);

    Ok(guard)
}

/// Returns a compressor function that takes some stream as input.
fn get_compressor_fn<C: AsyncBufRead + Unpin + Send + 'static>(
    ctype: CompressionType,
    level: CompressionLevel,
) -> CompressorFn<C> {
    match ctype {
        CompressionType::None => Box::new(|c| Box::new(c)),
        CompressionType::Brotli => {
            Box::new(move |s| Box::new(BrotliEncoder::with_quality(s, level)))
        }
        CompressionType::Zstd => Box::new(move |s| Box::new(ZstdEncoder::with_quality(s, level))),
        CompressionType::Xz => Box::new(move |s| Box::new(XzEncoder::with_quality(s, level))),
    }
}

impl ChunkData {
    /// Returns the potentially-incorrect hash of the chunk.
    fn hash(&self) -> Hash {
        match self {
            Self::Bytes(bytes) => {
                let mut hasher = Sha256::new();
                hasher.update(bytes);
                let hash = hasher.finalize();
                Hash::Sha256(hash.as_slice().try_into().unwrap())
            }
            Self::Stream(_, hash, _) => hash.clone(),
        }
    }

    /// Returns the potentially-incorrect size of the chunk.
    fn size(&self) -> usize {
        match self {
            Self::Bytes(bytes) => bytes.len(),
            Self::Stream(_, _, size) => *size,
        }
    }

    /// Returns whether the hash is trusted.
    fn is_hash_trusted(&self) -> bool {
        matches!(self, ChunkData::Bytes(_))
    }

    /// Turns the data into a stream.
    fn into_async_read(self) -> Box<dyn AsyncRead + Unpin + Send> {
        match self {
            Self::Bytes(bytes) => Box::new(Cursor::new(bytes)),
            Self::Stream(stream, _, _) => stream,
        }
    }
}

impl CompressionStream {
    /// Creates a new compression stream.
    fn new<R>(stream: R, compressor: CompressorFn<BufReader<StreamHasher<R, Sha256>>>) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        // compute NAR hash and size
        let (stream, nar_compute) = StreamHasher::new(stream, Sha256::new());

        // compress NAR
        let stream = compressor(BufReader::new(stream));

        // compute file hash and size
        let (stream, file_compute) = StreamHasher::new(stream, Sha256::new());

        Self {
            stream: Box::new(stream),
            nar_compute,
            file_compute,
        }
    }

    /*
    /// Creates a compression stream without compute the uncompressed hash/size.
    ///
    /// This is useful if you already know the hash. `nar_hash_and_size` will
    /// always return `None`.
    fn new_without_nar_hash<R>(stream: R, compressor: CompressorFn<BufReader<R>>) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        // compress NAR
        let stream = compressor(BufReader::new(stream));

        // compute file hash and size
        let (stream, file_compute) = StreamHasher::new(stream, Sha256::new());

        Self {
            stream: Box::new(stream),
            nar_compute: Arc::new(OnceCell::new()),
            file_compute,
        }
    }
    */

    /// Returns the stream of the compressed object.
    fn stream(&mut self) -> &mut (impl AsyncRead + Unpin) {
        &mut self.stream
    }

    /// Returns the NAR hash and size.
    ///
    /// The hash is only finalized when the stream is fully read.
    /// Otherwise, returns `None`.
    fn nar_hash_and_size(&self) -> Option<&(DigestOutput<Sha256>, usize)> {
        self.nar_compute.get()
    }

    /// Returns the file hash and size.
    ///
    /// The hash is only finalized when the stream is fully read.
    /// Otherwise, returns `None`.
    fn file_hash_and_size(&self) -> Option<&(DigestOutput<Sha256>, usize)> {
        self.file_compute.get()
    }
}

impl UploadPathNarInfoExt for UploadPathNarInfo {
    fn to_active_model(&self) -> object::ActiveModel {
        object::ActiveModel {
            store_path_hash: Set(self.store_path_hash.to_string()),
            store_path: Set(self.store_path.clone()),
            references: Set(DbJson(self.references.clone())),
            deriver: Set(self.deriver.clone()),
            sigs: Set(DbJson(self.sigs.clone())),
            ca: Set(self.ca.clone()),
            ..Default::default()
        }
    }
}

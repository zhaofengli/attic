use std::io;

use std::marker::Unpin;
use std::sync::Arc;

use anyhow::anyhow;
use async_compression::tokio::bufread::{BrotliEncoder, XzEncoder, ZstdEncoder};
use axum::{
    extract::{BodyStream, Extension},
    http::HeaderMap,
};
use chrono::Utc;
use digest::Output as DigestOutput;
use futures::StreamExt;
use sea_orm::entity::prelude::*;
use sea_orm::ActiveValue::Set;
use sea_orm::TransactionTrait;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, BufReader};
use tokio::sync::OnceCell;
use tokio_util::io::StreamReader;
use tracing::instrument;
use uuid::Uuid;

use crate::config::CompressionType;
use crate::error::{ErrorKind, ServerError, ServerResult};
use crate::narinfo::Compression;
use crate::{RequestState, State};
use attic::api::v1::upload_path::UploadPathNarInfo;
use attic::hash::Hash;
use attic::stream::StreamHasher;
use attic::util::Finally;

use crate::database::entity::cache;
use crate::database::entity::nar::{self, Entity as Nar, NarState};
use crate::database::entity::object::{self, Entity as Object};
use crate::database::entity::Json;
use crate::database::{AtticDatabase, NarGuard};

type CompressorFn<C> = Box<dyn FnOnce(C) -> Box<dyn AsyncRead + Unpin + Send> + Send>;

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
) -> ServerResult<String> {
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
        Some(existing_nar) => {
            // Deduplicate
            upload_path_dedup(username, cache, upload_info, stream, existing_nar, database).await
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
    existing_nar: NarGuard,
    database: &DatabaseConnection,
) -> ServerResult<String> {
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

    txn.commit().await.map_err(ServerError::database_error)?;

    // Ensure it's not unlocked earlier
    drop(existing_nar);

    // TODO
    Ok("Success".to_string())
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
) -> ServerResult<String> {
    let compression_config = &state.config.compression;
    let compression: Compression = compression_config.r#type.into();
    let level = compression_config.level();
    let compressor: CompressorFn<_> = match compression_config.r#type {
        CompressionType::None => Box::new(|c| Box::new(c)),
        CompressionType::Brotli => {
            Box::new(move |s| Box::new(BrotliEncoder::with_quality(s, level)))
        }
        CompressionType::Zstd => Box::new(move |s| Box::new(ZstdEncoder::with_quality(s, level))),
        CompressionType::Xz => Box::new(move |s| Box::new(XzEncoder::with_quality(s, level))),
    };

    let backend = state.storage().await?;

    let key = format!("{}.nar", Uuid::new_v4());

    let remote_file = backend.make_db_reference(key.clone()).await?;
    let remote_file_id = remote_file.remote_file_id();
    let nar_id = {
        let nar_size_db =
            i64::try_from(upload_info.nar_size).map_err(ServerError::request_error)?;
        let model = nar::ActiveModel {
            state: Set(NarState::PendingUpload),
            compression: Set(compression.to_string()),

            // Untrusted data - To be confirmed later
            nar_hash: Set(upload_info.nar_hash.to_typed_base16()),
            nar_size: Set(nar_size_db),

            remote_file: Set(Json(remote_file)),
            remote_file_id: Set(remote_file_id),

            created_at: Set(Utc::now()),
            ..Default::default()
        };

        let insertion = Nar::insert(model)
            .exec(database)
            .await
            .map_err(ServerError::database_error)?;

        insertion.last_insert_id
    };

    let cleanup = Finally::new({
        let database = database.clone();
        let nar_model = nar::ActiveModel {
            id: Set(nar_id),
            ..Default::default()
        };
        let backend = backend.clone();
        let key = key.clone();

        async move {
            tracing::warn!("Error occurred - Cleaning up uploaded file and NAR entry");

            if let Err(e) = backend.delete_file(key).await {
                tracing::warn!("Failed to clean up failed upload: {}", e);
            }

            if let Err(e) = Nar::delete(nar_model).exec(&database).await {
                tracing::warn!("Failed to unregister failed NAR: {}", e);
            }
        }
    });

    let mut stream = CompressionStream::new(stream, compressor);

    // Stream the object to the storage backend
    backend
        .upload_file(key, stream.stream())
        .await
        .map_err(ServerError::storage_error)?;

    // Confirm that the NAR Hash and Size are correct
    // FIXME: errors
    let (nar_hash, nar_size) = stream.nar_hash_and_size().unwrap();
    let (file_hash, file_size) = stream.file_hash_and_size().unwrap();

    let nar_hash = Hash::Sha256(nar_hash.as_slice().try_into().unwrap());
    let file_hash = Hash::Sha256(file_hash.as_slice().try_into().unwrap());

    if nar_hash != upload_info.nar_hash || *nar_size != upload_info.nar_size {
        return Err(ErrorKind::RequestError(anyhow!("Bad NAR Hash or Size")).into());
    }

    // Finally...
    let txn = database
        .begin()
        .await
        .map_err(ServerError::database_error)?;

    // Update the file hash and size, and set the nar to valid
    let file_size_db = i64::try_from(*file_size).map_err(ServerError::request_error)?;
    Nar::update(nar::ActiveModel {
        id: Set(nar_id),
        state: Set(NarState::Valid),
        file_hash: Set(Some(file_hash.to_typed_base16())),
        file_size: Set(Some(file_size_db)),
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

    cleanup.cancel();

    // TODO
    Ok("Success".to_string())
}

impl CompressionStream {
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
            references: Set(Json(self.references.clone())),
            deriver: Set(self.deriver.clone()),
            sigs: Set(Json(self.sigs.clone())),
            ca: Set(self.ca.clone()),
            ..Default::default()
        }
    }
}

use std::io;
use std::io::Cursor;
use std::marker::Unpin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use async_compression::Level as CompressionLevel;
use async_compression::tokio::bufread::{BrotliEncoder, XzEncoder, ZstdEncoder};
use async_stream::try_stream;
use axum::{
    body::Body,
    extract::{Extension, Json, Path},
    http::HeaderMap,
};
use bytes::{Bytes, BytesMut};
use chrono::Utc;
use futures::StreamExt;
use futures::future::join_all;
use sea_orm::ActiveValue::Set;
use sea_orm::entity::prelude::*;
use sea_orm::{PaginatorTrait, QueryOrder, QuerySelect, TransactionTrait};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt};
use tokio::sync::Semaphore;
use tokio::task::{JoinHandle, spawn};
use tokio::time;
use tokio_util::io::{ReaderStream, StreamReader};
use tracing::instrument;
use uuid::Uuid;

use crate::compression::{CompressionStream, CompressorFn};
use crate::config::CompressionType;
use crate::error::{ErrorKind, ServerError, ServerResult};
use crate::narinfo::Compression;
use crate::storage::StorageBackend;
use crate::{RequestState, State};
use attic::api::v1::upload_path::{
    ATTIC_NAR_INFO, ATTIC_NAR_INFO_PREAMBLE_SIZE, FinalizeUploadSessionResponse,
    StartUploadPathSessionRequest, StartUploadPathSessionResponse, UploadPathNarInfo,
    UploadPathResult, UploadPathResultKind,
};
use attic::chunking::chunk_stream;
use attic::hash::Hash;
use attic::io::{HashReader, read_chunk_async};
use attic::util::Finally;

use crate::database::entity::Json as DbJson;
use crate::database::entity::cache;
use crate::database::entity::chunk::{self, ChunkState, Entity as Chunk};
use crate::database::entity::chunkref::{self, Entity as ChunkRef};
use crate::database::entity::nar::{self, Entity as Nar, NarState};
use crate::database::entity::object::{self, Entity as Object, InsertExt};
use crate::database::entity::upload_session::{self, Entity as UploadSession, UploadSessionState};
use crate::database::entity::upload_session_part::{
    self, Entity as UploadSessionPart, UploadSessionPartState,
};
use crate::database::{AtticDatabase, ChunkGuard, NarGuard};
use crate::storage::{Download, RemoteFile};

/// Number of chunks to upload to the storage backend at once.
///
/// TODO: Make this configurable
const CONCURRENT_CHUNK_UPLOADS: usize = 10;

const UPLOAD_SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const UPLOAD_SESSION_FINALIZE_STALE_AFTER: Duration = Duration::from_secs(2 * 60);
const UPLOAD_SESSION_FINALIZE_HEARTBEAT: Duration = Duration::from_secs(60);
/// Data of a chunk.
enum ChunkData {
    /// Some bytes in memory.
    Bytes(Bytes),

    /// A stream with a user-claimed hash and size that are potentially incorrect.
    Stream(Box<dyn AsyncBufRead + Send + Unpin + 'static>, Hash, usize),
}

/// Result of a chunk upload.
struct UploadChunkResult {
    guard: ChunkGuard,
    deduplicated: bool,
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
    body: Body,
) -> ServerResult<Json<UploadPathResult>> {
    let stream = body.into_data_stream();
    let mut stream =
        StreamReader::new(stream.map(|r| r.map_err(|e| io::Error::other(e.to_string()))));

    let upload_info: UploadPathNarInfo = {
        if let Some(preamble_size_bytes) = headers.get(ATTIC_NAR_INFO_PREAMBLE_SIZE) {
            // Read from the beginning of the PUT body
            let preamble_size: usize = preamble_size_bytes
                .to_str()
                .map_err(|_| {
                    ErrorKind::RequestError(anyhow!(
                        "{} has invalid encoding",
                        ATTIC_NAR_INFO_PREAMBLE_SIZE
                    ))
                })?
                .parse()
                .map_err(|_| {
                    ErrorKind::RequestError(anyhow!(
                        "{} must be a valid unsigned integer",
                        ATTIC_NAR_INFO_PREAMBLE_SIZE
                    ))
                })?;

            if preamble_size > state.config.max_nar_info_size {
                return Err(ErrorKind::RequestError(anyhow!("Upload info is too large")).into());
            }

            let buf = BytesMut::with_capacity(preamble_size);
            let preamble = read_chunk_async(&mut stream, buf)
                .await
                .map_err(|e| ErrorKind::RequestError(e.into()))?;

            if preamble.len() != preamble_size {
                return Err(ErrorKind::RequestError(anyhow!(
                    "Upload info doesn't match specified size"
                ))
                .into());
            }

            serde_json::from_slice(&preamble).map_err(ServerError::request_error)?
        } else if let Some(nar_info_bytes) = headers.get(ATTIC_NAR_INFO) {
            // Read from X-Attic-Nar-Info header
            serde_json::from_slice(nar_info_bytes.as_bytes()).map_err(ServerError::request_error)?
        } else {
            return Err(ErrorKind::RequestError(anyhow!("{} must be set", ATTIC_NAR_INFO)).into());
        }
    };

    ingest_upload_path(state, req_state, upload_info, stream).await
}

async fn ingest_upload_path(
    state: State,
    req_state: RequestState,
    upload_info: UploadPathNarInfo,
    stream: impl AsyncBufRead + Send + Unpin + 'static,
) -> ServerResult<Json<UploadPathResult>> {
    let cache_name = &upload_info.cache;

    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, cache_name, |cache, permission| {
            permission.require_push()?;
            Ok(cache)
        })
        .await?;

    let username = req_state.auth.username().map(str::to_string);

    // Try to acquire a lock on an existing NAR
    if let Some(existing_nar) = database.find_and_lock_nar(&upload_info.nar_hash).await? {
        // Deduplicate?
        // TODO: Fully kill chunk recovery (no more missing chunks)
        let missing_chunk = ChunkRef::find()
            .filter(chunkref::Column::NarId.eq(existing_nar.id))
            .filter(chunkref::Column::ChunkId.is_null())
            .limit(1)
            .one(database)
            .await
            .map_err(ServerError::database_error)?;

        if missing_chunk.is_none() {
            // Can actually be deduplicated
            return upload_path_dedup(
                username,
                cache,
                upload_info,
                stream,
                database,
                &state,
                existing_nar,
            )
            .await;
        }
    }

    upload_path_new(username, cache, upload_info, stream, database, &state).await
}

/// Starts a chunked transport upload session.
#[instrument(skip_all)]
pub(crate) async fn start_upload_session(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Json(request): Json<StartUploadPathSessionRequest>,
) -> ServerResult<Json<StartUploadPathSessionResponse>> {
    let upload_info = request.nar_info;
    let cache_name = &upload_info.cache;
    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, cache_name, |cache, permission| {
            permission.require_push()?;
            Ok(cache)
        })
        .await?;

    if !state.config.require_proof_of_possession
        && let Some(existing_nar) = database.find_and_lock_nar(&upload_info.nar_hash).await?
    {
        let missing_chunk = ChunkRef::find()
            .filter(chunkref::Column::NarId.eq(existing_nar.id))
            .filter(chunkref::Column::ChunkId.is_null())
            .limit(1)
            .one(database)
            .await
            .map_err(ServerError::database_error)?;

        if missing_chunk.is_none() {
            let Json(result) = upload_path_dedup(
                req_state.auth.username().map(str::to_string),
                cache,
                upload_info,
                Cursor::new(Bytes::new()),
                database,
                &state,
                existing_nar,
            )
            .await?;

            return Ok(Json(StartUploadPathSessionResponse::Completed { result }));
        }
    }

    let max_part_size = state
        .config
        .upload
        .max_chunk_size
        .filter(|size| *size > 0)
        .ok_or_else(|| {
            ErrorKind::RequestError(anyhow!("Chunked transport upload is not enabled"))
        })?;
    let chunk_size = request.chunk_size.unwrap_or(max_part_size);
    if chunk_size == 0 {
        return Err(ErrorKind::RequestError(anyhow!("Chunk size cannot be 0")).into());
    }
    if chunk_size > max_part_size {
        return Err(ErrorKind::RequestError(anyhow!("Chunk size exceeds server limit")).into());
    }

    let expected_parts = if upload_info.nar_size == 0 {
        0
    } else {
        upload_info.nar_size.div_ceil(chunk_size)
    };
    let now = Utc::now();
    let expires_at =
        now + chrono::Duration::from_std(UPLOAD_SESSION_TTL).map_err(ServerError::request_error)?;
    let session_id = Uuid::new_v4();

    UploadSession::insert(upload_session::ActiveModel {
        id: Set(session_id.to_string()),
        cache_id: Set(cache.id),
        upload_info: Set(serde_json::to_string(&upload_info).map_err(ServerError::request_error)?),
        expected_parts: Set(i32::try_from(expected_parts).map_err(ServerError::request_error)?),
        state: Set(UploadSessionState::Uploading.as_str().to_string()),
        created_by: Set(req_state.auth.username().map(str::to_string)),
        created_at: Set(now),
        updated_at: Set(now),
        expires_at: Set(expires_at),
        ..Default::default()
    })
    .exec(database)
    .await
    .map_err(ServerError::database_error)?;

    Ok(Json(StartUploadPathSessionResponse::Session {
        session_id,
        chunk_size,
    }))
}

/// Uploads a chunked transport upload session part.
#[instrument(skip_all, fields(session_id = %session_id, seq))]
pub(crate) async fn upload_session_part(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path((session_id, seq)): Path<(Uuid, u32)>,
    body: Body,
) -> ServerResult<()> {
    let database = state.database().await?;
    let session = load_session(database, session_id).await?;
    let upload_info = parse_upload_info(&session)?;
    req_state
        .auth
        .auth_cache(database, &upload_info.cache, |_, permission| {
            permission.require_push()?;
            Ok(())
        })
        .await?;

    if session.state != UploadSessionState::Uploading.as_str() {
        return Err(
            ErrorKind::RequestError(anyhow!("Upload session is not accepting parts")).into(),
        );
    }
    if Utc::now() > session.expires_at {
        return Err(ErrorKind::RequestError(anyhow!("Upload session has expired")).into());
    }
    let update = UploadSession::update_many()
        .col_expr(upload_session::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(upload_session::Column::Id.eq(session.id.clone()))
        .filter(upload_session::Column::State.eq(UploadSessionState::Uploading.as_str()))
        .exec(database)
        .await
        .map_err(ServerError::database_error)?;
    if update.rows_affected != 1 {
        return Err(
            ErrorKind::RequestError(anyhow!("Upload session is not accepting parts")).into(),
        );
    }

    let expected_parts =
        usize::try_from(session.expected_parts).map_err(ServerError::request_error)?;
    let seq_usize = usize::try_from(seq).map_err(ServerError::request_error)?;
    let seq_db = i32::try_from(seq).map_err(ServerError::request_error)?;
    if seq_usize >= expected_parts {
        return Err(ErrorKind::RequestError(anyhow!("Part sequence is out of range")).into());
    }

    let backend = state.storage().await?;

    let key = format!(
        "upload-session/{}/{}-{}.part",
        session.id,
        seq,
        Uuid::new_v4()
    );
    let remote_file = backend.make_db_reference(key.clone()).await?;

    let now = Utc::now();
    let insert_result = UploadSessionPart::insert(upload_session_part::ActiveModel {
        session_id: Set(session.id.clone()),
        seq: Set(seq_db),
        state: Set(UploadSessionPartState::Pending.as_str().to_string()),
        remote_file: Set(serde_json::to_string(&remote_file).map_err(ServerError::request_error)?),
        created_at: Set(now),
        ..Default::default()
    })
    .exec(database)
    .await;

    let insert_result = match insert_result {
        Ok(insert_result) => insert_result,
        Err(e) => {
            if let Some(existing) = UploadSessionPart::find()
                .filter(upload_session_part::Column::SessionId.eq(session.id.clone()))
                .filter(upload_session_part::Column::Seq.eq(seq_db))
                .one(database)
                .await
                .map_err(ServerError::database_error)?
            {
                if existing.state == UploadSessionPartState::Pending.as_str() {
                    return Err(ErrorKind::RequestError(anyhow!(
                        "Part upload is already in progress"
                    ))
                    .into());
                }

                return Err(
                    ErrorKind::RequestError(anyhow!("Part has already been uploaded")).into(),
                );
            }

            return Err(ServerError::database_error(e));
        }
    };

    let cleanup = Finally::new({
        let database = database.clone();
        let backend = backend.clone();
        let remote_file = remote_file.clone();
        let part_id = insert_result.last_insert_id;

        async move {
            if let Err(e) = backend.delete_file_db(&remote_file).await
                && !e.is_storage_not_found()
            {
                tracing::warn!("Failed to clean up upload session part object: {}", e);
            }

            if let Err(e) = UploadSessionPart::delete(upload_session_part::ActiveModel {
                id: Set(part_id),
                ..Default::default()
            })
            .exec(&database)
            .await
            {
                tracing::warn!("Failed to clean up upload session part row: {}", e);
            }
        }
    });

    let stream = body
        .into_data_stream()
        .map(|r| r.map_err(|e| io::Error::other(e.to_string())));
    let mut reader = StreamReader::new(stream);
    backend.upload_file(key, &mut reader).await?;

    let session = load_session(database, session_id).await?;
    if session.state != UploadSessionState::Uploading.as_str() || Utc::now() > session.expires_at {
        return Err(ErrorKind::RequestError(anyhow!(
            "Upload session is no longer accepting parts"
        ))
        .into());
    }

    let update = UploadSessionPart::update_many()
        .col_expr(
            upload_session_part::Column::State,
            Expr::value(UploadSessionPartState::Valid.as_str()),
        )
        .filter(upload_session_part::Column::Id.eq(insert_result.last_insert_id))
        .filter(upload_session_part::Column::State.eq(UploadSessionPartState::Pending.as_str()))
        .exec(database)
        .await
        .map_err(ServerError::database_error)?;
    if update.rows_affected != 1 {
        return Err(ErrorKind::RequestError(anyhow!("Upload session part was removed")).into());
    }

    let now = Utc::now();
    let session_update = UploadSession::update_many()
        .col_expr(upload_session::Column::UpdatedAt, Expr::value(now))
        .filter(upload_session::Column::Id.eq(session.id.clone()))
        .filter(upload_session::Column::State.eq(UploadSessionState::Uploading.as_str()))
        .filter(upload_session::Column::ExpiresAt.gt(now))
        .exec(database)
        .await
        .map_err(ServerError::database_error)?;
    if session_update.rows_affected != 1 {
        return Err(ErrorKind::RequestError(anyhow!(
            "Upload session is no longer accepting parts"
        ))
        .into());
    }

    cleanup.cancel();

    Ok(())
}

/// Finalizes a chunked transport upload session.
#[instrument(skip_all, fields(session_id = %session_id))]
pub(crate) async fn finalize_upload_session(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path(session_id): Path<Uuid>,
) -> ServerResult<Json<FinalizeUploadSessionResponse>> {
    let database = state.database().await?;
    let session = load_session(database, session_id).await?;
    let upload_info = parse_upload_info(&session)?;
    req_state
        .auth
        .auth_cache(database, &upload_info.cache, |_, permission| {
            permission.require_push()?;
            Ok(())
        })
        .await?;

    if session.state == UploadSessionState::Completed.as_str() {
        let result_json = session.result.ok_or(ErrorKind::InternalServerError)?;
        let result = serde_json::from_str(&result_json).map_err(ServerError::database_error)?;
        return Ok(Json(FinalizeUploadSessionResponse::Completed { result }));
    }
    if session.state == UploadSessionState::Failed.as_str() {
        return Ok(Json(FinalizeUploadSessionResponse::Failed {
            message: "Upload session finalization failed".to_string(),
        }));
    }

    let finalizing_is_stale = session.state == UploadSessionState::Finalizing.as_str()
        && session.updated_at < stale_before(UPLOAD_SESSION_FINALIZE_STALE_AFTER)?;
    if session.state == UploadSessionState::Finalizing.as_str() && !finalizing_is_stale {
        return Ok(Json(FinalizeUploadSessionResponse::Pending));
    }
    if session.state != UploadSessionState::Uploading.as_str() && !finalizing_is_stale {
        return Err(ErrorKind::RequestError(anyhow!("Upload session cannot be finalized")).into());
    }

    if session.state == UploadSessionState::Uploading.as_str() {
        let expected_parts =
            u64::try_from(session.expected_parts).map_err(ServerError::request_error)?;
        let uploaded_parts = UploadSessionPart::find()
            .filter(upload_session_part::Column::SessionId.eq(session.id.clone()))
            .filter(upload_session_part::Column::State.eq(UploadSessionPartState::Valid.as_str()))
            .count(database)
            .await
            .map_err(ServerError::database_error)?;

        if uploaded_parts != expected_parts {
            return Ok(Json(FinalizeUploadSessionResponse::Pending));
        }
    }

    let update = UploadSession::update_many()
        .col_expr(
            upload_session::Column::State,
            Expr::value(UploadSessionState::Finalizing.as_str()),
        )
        .col_expr(upload_session::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(upload_session::Column::Id.eq(session.id.clone()));

    let update = if finalizing_is_stale {
        update
            .filter(upload_session::Column::State.eq(UploadSessionState::Finalizing.as_str()))
            .filter(
                upload_session::Column::UpdatedAt
                    .lt(stale_before(UPLOAD_SESSION_FINALIZE_STALE_AFTER)?),
            )
    } else {
        update.filter(upload_session::Column::State.eq(session.state.clone()))
    };

    let updated = update
        .exec(database)
        .await
        .map_err(ServerError::database_error)?;
    if updated.rows_affected != 1 {
        return Ok(Json(FinalizeUploadSessionResponse::Pending));
    }

    spawn_finalize_upload_session(state.clone(), req_state, session, upload_info);

    Ok(Json(FinalizeUploadSessionResponse::Pending))
}

fn spawn_finalize_upload_session(
    state: State,
    req_state: RequestState,
    session: upload_session::Model,
    upload_info: UploadPathNarInfo,
) {
    spawn(async move {
        let session_id = session.id.clone();
        let finalize =
            finalize_upload_session_inner(state.clone(), req_state, session, upload_info);
        tokio::pin!(finalize);
        let mut heartbeat = time::interval(UPLOAD_SESSION_FINALIZE_HEARTBEAT);

        let result = loop {
            tokio::select! {
                result = &mut finalize => break result,
                _ = heartbeat.tick() => {
                    heartbeat_upload_session_finalization(&state, &session_id).await;
                }
            }
        };

        if let Err(e) = result {
            tracing::warn!("Failed to finalize upload session {}: {}", session_id, e);
            if let Ok(database) = state.database().await {
                let _ = UploadSession::update_many()
                    .col_expr(
                        upload_session::Column::State,
                        Expr::value(UploadSessionState::Failed.as_str()),
                    )
                    .col_expr(upload_session::Column::UpdatedAt, Expr::value(Utc::now()))
                    .filter(upload_session::Column::Id.eq(session_id))
                    .filter(
                        upload_session::Column::State.eq(UploadSessionState::Finalizing.as_str()),
                    )
                    .exec(database)
                    .await;
            }
        }
    });
}

async fn heartbeat_upload_session_finalization(state: &State, session_id: &str) {
    if let Ok(database) = state.database().await
        && let Err(e) = UploadSession::update_many()
            .col_expr(upload_session::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(upload_session::Column::Id.eq(session_id.to_string()))
            .filter(upload_session::Column::State.eq(UploadSessionState::Finalizing.as_str()))
            .exec(database)
            .await
    {
        tracing::warn!(
            "Failed to heartbeat upload session finalization {}: {}",
            session_id,
            e
        );
    }
}

async fn finalize_upload_session_inner(
    state: State,
    req_state: RequestState,
    session: upload_session::Model,
    upload_info: UploadPathNarInfo,
) -> ServerResult<UploadPathResult> {
    let database = state.database().await?;
    let parts = UploadSessionPart::find()
        .filter(upload_session_part::Column::SessionId.eq(session.id.clone()))
        .filter(upload_session_part::Column::State.eq(UploadSessionPartState::Valid.as_str()))
        .order_by_asc(upload_session_part::Column::Seq)
        .all(database)
        .await
        .map_err(ServerError::database_error)?;

    let expected_parts =
        usize::try_from(session.expected_parts).map_err(ServerError::request_error)?;
    if parts.len() != expected_parts {
        return Err(ErrorKind::RequestError(anyhow!("Upload session is missing parts")).into());
    }

    for (idx, part) in parts.iter().enumerate() {
        if part.seq != idx as i32 {
            return Err(ErrorKind::RequestError(anyhow!("Upload session parts have a gap")).into());
        }
    }

    let remote_files = parts
        .iter()
        .map(|p| {
            serde_json::from_str::<RemoteFile>(&p.remote_file).map_err(ServerError::database_error)
        })
        .collect::<ServerResult<Vec<_>>>()?;
    let stream = Box::pin(stream_remote_files(state.clone(), remote_files));
    let stream = StreamReader::new(stream);

    let Json(result) = ingest_upload_path(state.clone(), req_state, upload_info, stream).await?;
    let updated = UploadSession::update_many()
        .col_expr(
            upload_session::Column::State,
            Expr::value(UploadSessionState::Completed.as_str()),
        )
        .col_expr(upload_session::Column::UpdatedAt, Expr::value(Utc::now()))
        .col_expr(
            upload_session::Column::Result,
            Expr::value(Some(
                serde_json::to_string(&result).map_err(ServerError::request_error)?,
            )),
        )
        .filter(upload_session::Column::Id.eq(session.id.clone()))
        .filter(upload_session::Column::State.eq(UploadSessionState::Finalizing.as_str()))
        .exec(database)
        .await
        .map_err(ServerError::database_error)?;
    if updated.rows_affected != 1 {
        return Err(
            ErrorKind::RequestError(anyhow!("Upload session finalization was cancelled")).into(),
        );
    }

    if let Err(e) = crate::gc::delete_upload_session_parts(&state, &session.id).await {
        tracing::warn!(
            "Failed to clean up completed upload session parts for session {}: {}",
            session.id,
            e
        );
    }

    Ok(result)
}

/// Aborts a chunked transport upload session.
#[instrument(skip_all, fields(session_id = %session_id))]
pub(crate) async fn abort_upload_session(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path(session_id): Path<Uuid>,
) -> ServerResult<()> {
    let database = state.database().await?;
    let session = load_session(database, session_id).await?;
    let upload_info = parse_upload_info(&session)?;
    req_state
        .auth
        .auth_cache(database, &upload_info.cache, |_, permission| {
            permission.require_push()?;
            Ok(())
        })
        .await?;

    if session.state == UploadSessionState::Aborted.as_str() {
        return Ok(());
    }
    if session.state == UploadSessionState::Finalizing.as_str()
        || session.state == UploadSessionState::Completed.as_str()
    {
        return Err(ErrorKind::RequestError(anyhow!("Upload session cannot be aborted")).into());
    }

    let updated = UploadSession::update_many()
        .col_expr(
            upload_session::Column::State,
            Expr::value(UploadSessionState::Aborted.as_str()),
        )
        .col_expr(upload_session::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(upload_session::Column::Id.eq(session.id.clone()))
        .filter(upload_session::Column::State.is_in([
            UploadSessionState::Uploading.as_str(),
            UploadSessionState::Failed.as_str(),
        ]))
        .exec(database)
        .await
        .map_err(ServerError::database_error)?;
    if updated.rows_affected != 1 {
        return Err(ErrorKind::RequestError(anyhow!("Upload session cannot be aborted")).into());
    }

    if let Err(e) = crate::gc::delete_upload_session_parts(&state, &session.id).await {
        tracing::warn!(
            "Failed to clean up aborted upload session parts for session {}: {}",
            session.id,
            e
        );
    }
    Ok(())
}

/// Uploads a path when there is already a matching NAR in the global cache.
async fn upload_path_dedup(
    username: Option<String>,
    cache: cache::Model,
    upload_info: UploadPathNarInfo,
    stream: impl AsyncBufRead + Unpin,
    database: &DatabaseConnection,
    state: &State,
    existing_nar: NarGuard,
) -> ServerResult<Json<UploadPathResult>> {
    if state.config.require_proof_of_possession {
        let (mut stream, nar_compute) = HashReader::new(stream, Sha256::new());
        tokio::io::copy(&mut stream, &mut tokio::io::sink())
            .await
            .map_err(ServerError::request_error)?;

        // FIXME: errors
        let (nar_hash, nar_size) = nar_compute.get().unwrap();
        let nar_hash = Hash::Sha256((&nar_hash[..]).try_into().unwrap());

        // Confirm that the NAR Hash and Size are correct
        if nar_hash.to_typed_base16() != existing_nar.nar_hash
            || *nar_size != upload_info.nar_size
            || *nar_size != existing_nar.nar_size as usize
        {
            return Err(ErrorKind::RequestError(anyhow!("Bad NAR Hash or Size")).into());
        }
    }

    // Finally...

    // Create a mapping granting the local cache access to the NAR
    Object::insert({
        let mut new_object = upload_info.to_active_model();
        new_object.cache_id = Set(cache.id);
        new_object.nar_id = Set(existing_nar.id);
        new_object.created_at = Set(Utc::now());
        new_object.created_by = Set(username);
        new_object
    })
    .on_conflict_do_update()
    .exec(database)
    .await
    .map_err(ServerError::database_error)?;

    // Ensure it's not unlocked earlier
    drop(existing_nar);

    Ok(Json(UploadPathResult {
        kind: UploadPathResultKind::Deduplicated,
        file_size: None, // TODO: Sum the chunks
        frac_deduplicated: None,
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
    stream: impl AsyncBufRead + Send + Unpin + 'static,
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

async fn load_session(
    database: &DatabaseConnection,
    session_id: Uuid,
) -> ServerResult<upload_session::Model> {
    UploadSession::find_by_id(session_id.to_string())
        .one(database)
        .await
        .map_err(ServerError::database_error)?
        .ok_or(ErrorKind::NotFound.into())
}

fn parse_upload_info(session: &upload_session::Model) -> ServerResult<UploadPathNarInfo> {
    serde_json::from_str(&session.upload_info).map_err(ServerError::database_error)
}

fn stale_before(duration: Duration) -> ServerResult<chrono::DateTime<Utc>> {
    Ok(Utc::now() - chrono::Duration::from_std(duration).map_err(ServerError::request_error)?)
}

fn stream_remote_files(
    state: State,
    remote_files: Vec<RemoteFile>,
) -> impl futures::Stream<Item = Result<Bytes, io::Error>> {
    try_stream! {
        let backend = state
            .storage()
            .await
            .map_err(|e| io::Error::other(e.to_string()))?
            .clone();

        for remote_file in remote_files {
            let download = backend
                .download_file_db(&remote_file, true)
                .await
                .map_err(|e| io::Error::other(e.to_string()))?;
            let reader: Box<dyn AsyncRead + Unpin + Send> = match download {
                Download::AsyncRead(reader) => reader,
                Download::Url(_) => {
                    Err(io::Error::other(
                        "storage backend returned URL for upload session part",
                    ))?
                }
            };

            let mut stream = ReaderStream::new(reader);
            while let Some(chunk) = stream.next().await {
                yield chunk?;
            }
        }
    }
}

/// Uploads a path when there is no matching NAR in the global cache (chunked).
async fn upload_path_new_chunked(
    username: Option<String>,
    cache: cache::Model,
    upload_info: UploadPathNarInfo,
    stream: impl AsyncBufRead + Send + Unpin + 'static,
    database: &DatabaseConnection,
    state: &State,
) -> ServerResult<Json<UploadPathResult>> {
    let chunking_config = &state.config.chunking;
    let compression_config = &state.config.compression;
    let compression_type = compression_config.r#type;
    let compression_level = compression_config.level();
    let compression: Compression = compression_type.into();

    let nar_size_db = i64::try_from(upload_info.nar_size).map_err(ServerError::request_error)?;

    // Create a pending NAR entry
    let nar_id = {
        let model = nar::ActiveModel {
            state: Set(NarState::PendingUpload),
            compression: Set(compression.to_string()),

            nar_hash: Set(upload_info.nar_hash.to_typed_base16()),
            nar_size: Set(nar_size_db),

            num_chunks: Set(0),

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

        async move {
            tracing::warn!("Error occurred - Cleaning up NAR entry");

            if let Err(e) = Nar::delete(nar_model).exec(&database).await {
                tracing::warn!("Failed to unregister failed NAR: {}", e);
            }
        }
    });

    let stream = stream.take(upload_info.nar_size as u64);
    let (stream, nar_compute) = HashReader::new(stream, Sha256::new());
    let mut chunks = chunk_stream(
        stream,
        chunking_config.min_size,
        chunking_config.avg_size,
        chunking_config.max_size,
    );

    let upload_chunk_limit = Arc::new(Semaphore::new(CONCURRENT_CHUNK_UPLOADS));
    let mut futures: Vec<JoinHandle<ServerResult<UploadChunkResult>>> = Vec::new();

    let mut chunk_idx = 0;
    while let Some(bytes) = chunks.next().await {
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(e) => {
                abort_upload_chunk_tasks(futures).await;
                return Err(ServerError::request_error(e));
            }
        };
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
                    database.clone(),
                    state,
                    require_proof_of_possession,
                )
                .await?;

                // Create mapping from the NAR to the chunk
                ChunkRef::insert(chunkref::ActiveModel {
                    nar_id: Set(nar_id),
                    seq: Set(chunk_idx),
                    chunk_id: Set(Some(chunk.guard.id)),
                    ..Default::default()
                })
                .exec(&database)
                .await
                .map_err(ServerError::database_error)?;

                drop(permit);
                Ok(chunk)
            })
        });

        chunk_idx += 1;
    }

    // Confirm that the NAR Hash and Size are correct
    // FIXME: errors
    let (nar_hash, nar_size) = nar_compute.get().unwrap();
    let nar_hash = Hash::Sha256((&nar_hash[..]).try_into().unwrap());

    if nar_hash != upload_info.nar_hash || *nar_size != upload_info.nar_size {
        abort_upload_chunk_tasks(futures).await;
        return Err(ErrorKind::RequestError(anyhow!("Bad NAR Hash or Size")).into());
    }

    // Wait for all uploads to complete
    let chunks: Vec<UploadChunkResult> = join_all(futures)
        .await
        .into_iter()
        .map(|join_result| join_result.unwrap())
        .collect::<ServerResult<Vec<_>>>()?;

    let (file_size, deduplicated_size) =
        chunks
            .iter()
            .fold((0, 0), |(file_size, deduplicated_size), c| {
                (
                    file_size + c.guard.file_size.unwrap() as usize,
                    if c.deduplicated {
                        deduplicated_size + c.guard.chunk_size as usize
                    } else {
                        deduplicated_size
                    },
                )
            });

    // Finally...
    let txn = database
        .begin()
        .await
        .map_err(ServerError::database_error)?;

    // Set num_chunks and mark the NAR as Valid
    Nar::update(nar::ActiveModel {
        id: Set(nar_id),
        state: Set(NarState::Valid),
        num_chunks: Set(chunks.len() as i32),
        ..Default::default()
    })
    .exec(&txn)
    .await
    .map_err(ServerError::database_error)?;

    // Create a mapping granting the local cache access to the NAR
    Object::insert({
        let mut new_object = upload_info.to_active_model();
        new_object.cache_id = Set(cache.id);
        new_object.nar_id = Set(nar_id);
        new_object.created_at = Set(Utc::now());
        new_object.created_by = Set(username);
        new_object
    })
    .on_conflict_do_update()
    .exec(&txn)
    .await
    .map_err(ServerError::database_error)?;

    txn.commit().await.map_err(ServerError::database_error)?;

    cleanup.cancel();

    Ok(Json(UploadPathResult {
        kind: UploadPathResultKind::Uploaded,
        file_size: Some(file_size),

        // Currently, frac_deduplicated is computed from size before compression
        frac_deduplicated: Some(deduplicated_size as f64 / *nar_size as f64),
    }))
}

async fn abort_upload_chunk_tasks(futures: Vec<JoinHandle<ServerResult<UploadChunkResult>>>) {
    for future in &futures {
        future.abort();
    }

    let _ = join_all(futures).await;
}

/// Uploads a path when there is no matching NAR in the global cache (unchunked).
///
/// We upload the entire NAR as a single chunk.
async fn upload_path_new_unchunked(
    username: Option<String>,
    cache: cache::Model,
    upload_info: UploadPathNarInfo,
    stream: impl AsyncBufRead + Send + Unpin + 'static,
    database: &DatabaseConnection,
    state: &State,
) -> ServerResult<Json<UploadPathResult>> {
    let compression_config = &state.config.compression;
    let compression_type = compression_config.r#type;
    let compression: Compression = compression_type.into();

    // Upload the entire NAR as a single chunk
    let stream = stream.take(upload_info.nar_size as u64);
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
    let file_size = chunk.guard.file_size.unwrap() as usize;

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
            nar_size: Set(chunk.guard.chunk_size),

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
        chunk_id: Set(Some(chunk.guard.id)),
        ..Default::default()
    })
    .exec(&txn)
    .await
    .map_err(ServerError::database_error)?;

    // Create a mapping granting the local cache access to the NAR
    Object::insert({
        let mut new_object = upload_info.to_active_model();
        new_object.cache_id = Set(cache.id);
        new_object.nar_id = Set(nar_id);
        new_object.created_at = Set(Utc::now());
        new_object.created_by = Set(username);
        new_object
    })
    .on_conflict_do_update()
    .exec(&txn)
    .await
    .map_err(ServerError::database_error)?;

    txn.commit().await.map_err(ServerError::database_error)?;

    Ok(Json(UploadPathResult {
        kind: UploadPathResultKind::Uploaded,
        file_size: Some(file_size),
        frac_deduplicated: None,
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
) -> ServerResult<UploadChunkResult> {
    let compression: Compression = compression_type.into();

    let given_chunk_hash = data.hash();
    let given_chunk_size = data.size();

    if let Some(existing_chunk) = database
        .find_and_lock_chunk(&given_chunk_hash, compression)
        .await?
    {
        // There's an existing chunk matching the hash
        if require_proof_of_possession && !data.is_hash_trusted() {
            let stream = data.into_async_buf_read();

            let (mut stream, nar_compute) = HashReader::new(stream, Sha256::new());
            tokio::io::copy(&mut stream, &mut tokio::io::sink())
                .await
                .map_err(ServerError::request_error)?;

            // FIXME: errors
            let (nar_hash, nar_size) = nar_compute.get().unwrap();
            let nar_hash = Hash::Sha256((&nar_hash[..]).try_into().unwrap());

            // Confirm that the NAR Hash and Size are correct
            if nar_hash.to_typed_base16() != existing_chunk.chunk_hash
                || *nar_size != given_chunk_size
                || *nar_size != existing_chunk.chunk_size as usize
            {
                return Err(ErrorKind::RequestError(anyhow!("Bad chunk hash or size")).into());
            }
        }

        return Ok(UploadChunkResult {
            guard: existing_chunk,
            deduplicated: true,
        });
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
    let mut stream = CompressionStream::new(data.into_async_buf_read(), compressor);

    backend
        .upload_file(key, stream.stream())
        .await
        .map_err(ServerError::storage_error)?;

    // Confirm that the chunk hash is correct
    let (chunk_hash, chunk_size) = stream.nar_hash_and_size().unwrap();
    let (file_hash, file_size) = stream.file_hash_and_size().unwrap();

    let chunk_hash = Hash::Sha256((&chunk_hash[..]).try_into().unwrap());
    let file_hash = Hash::Sha256((&file_hash[..]).try_into().unwrap());

    if chunk_hash != given_chunk_hash || *chunk_size != given_chunk_size {
        return Err(ErrorKind::RequestError(anyhow!("Bad chunk hash or size")).into());
    }

    // Finally...

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
    .exec(&database)
    .await
    .map_err(ServerError::database_error)?;

    cleanup.cancel();

    let guard = ChunkGuard::from_locked(database.clone(), chunk);

    Ok(UploadChunkResult {
        guard,
        deduplicated: false,
    })
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
                Hash::Sha256((&hash[..]).try_into().unwrap())
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

    /// Turns the data into an AsyncBufRead.
    fn into_async_buf_read(self) -> Box<dyn AsyncBufRead + Unpin + Send> {
        match self {
            Self::Bytes(bytes) => Box::new(Cursor::new(bytes)),
            Self::Stream(stream, _, _) => stream,
        }
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

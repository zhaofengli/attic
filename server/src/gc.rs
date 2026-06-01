//! Garbage collection.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use chrono::{Duration as ChronoDuration, Utc};
use futures::future::join_all;
use sea_orm::entity::prelude::*;
use sea_orm::query::QuerySelect;
use sea_orm::sea_query::{Expr, LockBehavior, LockType, Query};
use sea_orm::{ConnectionTrait, FromQueryResult, TransactionTrait};
use tokio::sync::Semaphore;
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use super::{State, StateInner};
use crate::config::Config;
use crate::database::entity::cache::{self, Entity as Cache};
use crate::database::entity::chunk::{self, ChunkState, Entity as Chunk};
use crate::database::entity::chunkref::{self, Entity as ChunkRef};
use crate::database::entity::nar::{self, Entity as Nar, NarState};
use crate::database::entity::object::{self, Entity as Object};
use crate::database::entity::upload_session::{self, Entity as UploadSession, UploadSessionState};
use crate::database::entity::upload_session_part::{self, Entity as UploadSessionPart};
use crate::storage::RemoteFile;
use crate::storage::StorageBackend;

const UPLOAD_SESSION_ACTIVE_GRACE: Duration = Duration::from_secs(30 * 60);
const UPLOAD_SESSION_FINALIZE_STALE_AFTER: Duration = Duration::from_secs(2 * 60);

#[derive(Debug, FromQueryResult)]
struct CacheIdAndRetentionPeriod {
    id: i64,
    name: String,
    retention_period: i32,
}

/// Runs garbage collection periodically until shutdown is requested.
pub async fn run_garbage_collection(config: Config, shutdown: CancellationToken) {
    let interval = config.garbage_collection.interval;

    if interval == Duration::ZERO {
        // disabled
        return;
    }

    while !shutdown.is_cancelled() {
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!("Garbage collector received shutdown signal");
                break;
            }
            result = run_garbage_collection_once(config.clone()) => {
                // We don't stop even if it errors
                if let Err(e) = result {
                    tracing::warn!("Garbage collection failed: {}", e);
                }
            }
        }

        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!("Garbage collector received shutdown signal");
                break;
            }
            _ = time::sleep(interval) => {}
        }
    }
}

/// Runs garbage collection once.
#[instrument(skip_all)]
pub async fn run_garbage_collection_once(config: Config) -> Result<()> {
    tracing::info!("Running garbage collection...");

    let state = StateInner::new(config).await;
    run_time_based_garbage_collection(&state).await?;
    let (deleted_sessions, deleted_parts) = reap_expired_upload_sessions(&state).await?;
    if deleted_sessions != 0 || deleted_parts != 0 {
        tracing::info!(
            "Deleted {} expired upload sessions and {} uploaded parts",
            deleted_sessions,
            deleted_parts
        );
    }
    run_reap_orphan_nars(&state).await?;
    run_reap_orphan_chunks(&state).await?;

    Ok(())
}

#[instrument(skip_all)]
async fn run_time_based_garbage_collection(state: &State) -> Result<()> {
    let db = state.database().await?;
    let now = Utc::now();

    let default_retention_period = state.config.garbage_collection.default_retention_period;
    let retention_period =
        cache::Column::RetentionPeriod.if_null(default_retention_period.as_secs() as i32);

    // Find caches with retention periods set
    let caches = Cache::find()
        .select_only()
        .column(cache::Column::Id)
        .column(cache::Column::Name)
        .column_as(retention_period.clone(), "retention_period")
        .filter(retention_period.ne(0))
        .into_model::<CacheIdAndRetentionPeriod>()
        .all(db)
        .await?;

    tracing::info!(
        "Found {} caches subject to time-based garbage collection",
        caches.len()
    );

    let mut objects_deleted = 0;

    for cache in caches {
        let period = ChronoDuration::seconds(cache.retention_period.into());
        let cutoff = now.checked_sub_signed(period).ok_or_else(|| {
            anyhow!(
                "Somehow subtracting retention period for cache {} underflowed",
                cache.name
            )
        })?;

        let deletion = Object::delete_many()
            .filter(object::Column::CacheId.eq(cache.id))
            .filter(object::Column::CreatedAt.lt(cutoff))
            .filter(
                object::Column::LastAccessedAt
                    .is_null()
                    .or(object::Column::LastAccessedAt.lt(cutoff)),
            )
            .exec(db)
            .await?;

        tracing::info!(
            "Deleted {} objects from {} (ID {})",
            deletion.rows_affected,
            cache.name,
            cache.id
        );
        objects_deleted += deletion.rows_affected;
    }

    tracing::info!("Deleted {} objects in total", objects_deleted);

    Ok(())
}

/// Reaps expired chunked transport upload sessions.
///
/// The session row is only deleted after every referenced part object is
/// deleted, so failed storage cleanup leaves the DB references available for a
/// future retry.
#[instrument(skip_all)]
pub(crate) async fn reap_expired_upload_sessions(state: &State) -> Result<(u64, u64)> {
    let db = state.database().await?;
    let now = Utc::now();
    let active_after = now - ChronoDuration::from_std(UPLOAD_SESSION_ACTIVE_GRACE)?;
    let stale_finalizing_before =
        now - ChronoDuration::from_std(UPLOAD_SESSION_FINALIZE_STALE_AFTER)?;

    let sessions = UploadSession::find()
        .filter(upload_session::Column::ExpiresAt.lt(now))
        .filter(upload_session::Column::UpdatedAt.lt(active_after))
        .filter(
            upload_session::Column::State
                .ne(UploadSessionState::Finalizing.as_str())
                .or(upload_session::Column::UpdatedAt.lt(stale_finalizing_before)),
        )
        .all(db)
        .await?;

    delete_upload_sessions(state, sessions).await
}

/// Deletes all upload sessions for a cache after their temporary part objects
/// have been cleaned up.
pub(crate) async fn delete_upload_sessions_for_cache(
    state: &State,
    cache_id: i64,
) -> Result<(u64, u64)> {
    let db = state.database().await?;
    let txn = db.begin().await?;

    let sessions = UploadSession::find()
        .filter(upload_session::Column::CacheId.eq(cache_id))
        .all(&txn)
        .await?;

    for session in &sessions {
        if !claim_upload_session_for_cleanup(&txn, session).await? {
            txn.rollback().await?;
            return Err(anyhow!(
                "Cannot delete upload session {} while it is active",
                session.id
            ));
        }
    }

    txn.commit().await?;

    delete_claimed_upload_sessions(state, sessions).await
}

async fn delete_upload_sessions(
    state: &State,
    sessions: Vec<upload_session::Model>,
) -> Result<(u64, u64)> {
    if sessions.is_empty() {
        return Ok((0, 0));
    }

    let db = state.database().await?;
    let mut claimed_sessions = Vec::new();

    for session in sessions {
        if !claim_upload_session_for_cleanup(db, &session).await? {
            continue;
        }

        claimed_sessions.push(session);
    }

    delete_claimed_upload_sessions(state, claimed_sessions).await
}

async fn delete_claimed_upload_sessions(
    state: &State,
    sessions: Vec<upload_session::Model>,
) -> Result<(u64, u64)> {
    let db = state.database().await?;

    if sessions.is_empty() {
        return Ok((0, 0));
    }

    let mut deleted_sessions = 0;
    let mut deleted_parts = 0;

    for session in sessions {
        let deleted_part_count = match delete_upload_session_parts(state, &session.id).await {
            Ok(deleted_part_count) => deleted_part_count,
            Err(e) => {
                tracing::warn!(
                    "Failed to delete upload session parts for session {}: {}",
                    session.id,
                    e
                );
                continue;
            }
        };

        let deletion = UploadSession::delete_many()
            .filter(upload_session::Column::Id.eq(session.id))
            .filter(upload_session::Column::State.eq(UploadSessionState::Reaping.as_str()))
            .exec(db)
            .await?;

        deleted_sessions += deletion.rows_affected;
        deleted_parts += deleted_part_count;
    }

    Ok((deleted_sessions, deleted_parts))
}

async fn claim_upload_session_for_cleanup<C>(
    db: &C,
    session: &upload_session::Model,
) -> Result<bool>
where
    C: ConnectionTrait,
{
    let update = UploadSession::update_many()
        .col_expr(
            upload_session::Column::State,
            Expr::value(UploadSessionState::Reaping.as_str()),
        )
        .col_expr(upload_session::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(upload_session::Column::Id.eq(session.id.clone()));

    let update = if session.state == UploadSessionState::Finalizing.as_str() {
        update
            .filter(upload_session::Column::State.eq(UploadSessionState::Finalizing.as_str()))
            .filter(
                upload_session::Column::UpdatedAt.lt(
                    Utc::now() - ChronoDuration::from_std(UPLOAD_SESSION_FINALIZE_STALE_AFTER)?,
                ),
            )
    } else {
        update.filter(upload_session::Column::State.is_in([
            UploadSessionState::Uploading.as_str(),
            UploadSessionState::Reaping.as_str(),
            UploadSessionState::Completed.as_str(),
            UploadSessionState::Aborted.as_str(),
            UploadSessionState::Failed.as_str(),
        ]))
    };

    let result = update.exec(db).await?;
    Ok(result.rows_affected == 1)
}

/// Deletes all uploaded part objects for a session and then deletes their DB rows.
///
/// Missing storage objects are treated as already deleted so that sessions whose
/// parts were cleaned up by finalize or abort can still be reaped later.
pub(crate) async fn delete_upload_session_parts(state: &State, session_id: &str) -> Result<u64> {
    let db = state.database().await?;
    let storage = state.storage().await?;

    let parts = UploadSessionPart::find()
        .filter(upload_session_part::Column::SessionId.eq(session_id.to_string()))
        .all(db)
        .await?;
    let remote_files = parts
        .iter()
        .map(|part| serde_json::from_str::<RemoteFile>(&part.remote_file))
        .collect::<Result<Vec<_>, _>>()?;

    for remote_file in &remote_files {
        if let Err(e) = storage.delete_file_db(remote_file).await {
            if e.is_storage_not_found() {
                tracing::debug!(
                    "Upload session part for session {} was already deleted",
                    session_id
                );
                continue;
            }
            return Err(e.into());
        }
    }

    let deletion = UploadSessionPart::delete_many()
        .filter(upload_session_part::Column::SessionId.eq(session_id.to_string()))
        .exec(db)
        .await?;

    Ok(deletion.rows_affected)
}

#[instrument(skip_all)]
async fn run_reap_orphan_nars(state: &State) -> Result<()> {
    let db = state.database().await?;

    // find all orphan NARs...
    let orphan_nar_ids = Query::select()
        .from(Nar)
        .expr(nar::Column::Id.into_expr())
        .left_join(
            Object,
            object::Column::NarId
                .into_expr()
                .eq(nar::Column::Id.into_expr()),
        )
        .and_where(object::Column::Id.is_null())
        .and_where(nar::Column::State.eq(NarState::Valid))
        .and_where(nar::Column::HoldersCount.eq(0))
        .lock_with_tables_behavior(LockType::Update, [Nar], LockBehavior::SkipLocked)
        .to_owned();

    // ... and simply delete them
    let deletion = Nar::delete_many()
        .filter(nar::Column::Id.in_subquery(orphan_nar_ids))
        .exec(db)
        .await?;

    tracing::info!("Deleted {} orphan NARs", deletion.rows_affected,);

    Ok(())
}

#[instrument(skip_all)]
async fn run_reap_orphan_chunks(state: &State) -> Result<()> {
    let db = state.database().await?;
    let storage = state.storage().await?;

    let orphan_chunk_limit = match db.get_database_backend() {
        // Arbitrarily chosen sensible value since there's no good default to choose from for MySQL
        sea_orm::DatabaseBackend::MySql => 1000,
        // Panic limit set by sqlx for postgresql: https://github.com/launchbadge/sqlx/issues/671#issuecomment-687043510
        sea_orm::DatabaseBackend::Postgres => u64::from(u16::MAX),
        // Default statement limit imposed by sqlite: https://www.sqlite.org/limits.html#max_variable_number
        sea_orm::DatabaseBackend::Sqlite => 500,
    };

    // find all orphan chunks...
    let orphan_chunk_ids = Query::select()
        .from(Chunk)
        .expr(chunk::Column::Id.into_expr())
        .left_join(
            ChunkRef,
            chunkref::Column::ChunkId
                .into_expr()
                .eq(chunk::Column::Id.into_expr()),
        )
        .and_where(chunkref::Column::Id.is_null())
        .and_where(chunk::Column::State.eq(ChunkState::Valid))
        .and_where(chunk::Column::HoldersCount.eq(0))
        .lock_with_tables_behavior(LockType::Update, [Chunk], LockBehavior::SkipLocked)
        .to_owned();

    // ... and transition their state to Deleted
    //
    // Deleted chunks are essentially invisible from our normal queries
    let transition_statement = {
        let change_state = Query::update()
            .table(Chunk)
            .value(chunk::Column::State, ChunkState::Deleted)
            .and_where(chunk::Column::Id.in_subquery(orphan_chunk_ids))
            .to_owned();
        db.get_database_backend().build(&change_state)
    };

    db.execute(transition_statement).await?;

    let orphan_chunks: Vec<chunk::Model> = Chunk::find()
        .filter(chunk::Column::State.eq(ChunkState::Deleted))
        .limit(orphan_chunk_limit)
        .all(db)
        .await?;

    if orphan_chunks.is_empty() {
        return Ok(());
    }

    // Delete the chunks from remote storage
    let delete_limit = Arc::new(Semaphore::new(20)); // TODO: Make this configurable
    let futures: Vec<_> = orphan_chunks
        .into_iter()
        .map(|chunk| {
            let delete_limit = delete_limit.clone();
            async move {
                let permit = delete_limit.acquire().await?;
                storage.delete_file_db(&chunk.remote_file.0).await?;
                drop(permit);
                Result::<_, anyhow::Error>::Ok(chunk.id)
            }
        })
        .collect();

    // Deletions can result in spurious failures, tolerate them
    //
    // Chunks that failed to be deleted from the remote storage will
    // just be stuck in Deleted state.
    //
    // TODO: Maybe have an interactive command to retry deletions?
    let deleted_chunk_ids: Vec<_> = join_all(futures)
        .await
        .into_iter()
        .filter(|r| {
            if let Err(e) = r {
                tracing::warn!("Deletion failed: {}", e);
            }

            r.is_ok()
        })
        .map(|r| r.unwrap())
        .collect();

    // Finally, delete them from the database
    let deletion = Chunk::delete_many()
        .filter(chunk::Column::Id.is_in(deleted_chunk_ids))
        .exec(db)
        .await?;

    tracing::info!("Deleted {} orphan chunks", deletion.rows_affected);

    Ok(())
}

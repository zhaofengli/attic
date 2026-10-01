//! Garbage collection.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use chrono::{Duration as ChronoDuration, Utc};
use futures::future::join_all;
use sea_orm::entity::prelude::*;
use sea_orm::query::QuerySelect;
use sea_orm::sea_query::{LockBehavior, LockType, Query};
use sea_orm::{ConnectionTrait, DatabaseConnection, ExprTrait, FromQueryResult, Statement};
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
use crate::database::entity::pin::{self, Entity as Pin};
use crate::storage::StorageBackend;

/// Objects deleted per DELETE statement during the sweep.
///
/// Kept well below every backend's bind-parameter limit (see
/// `run_reap_orphan_chunks` for the gory details).
const SWEEP_CHUNK: usize = 500;

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

        // Mark phase: everything reachable from this cache's pins is alive,
        // regardless of age. Reachability follows the `references` list each
        // object already carries.
        let protected = find_pin_protected_object_ids(db, cache.id).await?;

        // Sweep: select the age-based candidates, subtract the protected
        // set, and delete in bounded chunks. Positive IN lists are used on
        // purpose — a NOT IN over the whole protected set would blow the
        // backends' bind-parameter limits, and chunking a NOT IN is unsound
        // (each chunk would need the complete set).
        let candidates: Vec<i64> = Object::find()
            .select_only()
            .column(object::Column::Id)
            .filter(object::Column::CacheId.eq(cache.id))
            .filter(object::Column::CreatedAt.lt(cutoff))
            .filter(
                object::Column::LastAccessedAt
                    .is_null()
                    .or(object::Column::LastAccessedAt.lt(cutoff)),
            )
            .into_tuple()
            .all(db)
            .await?;

        let doomed: Vec<i64> = candidates
            .into_iter()
            .filter(|id| !protected.contains(id))
            .collect();

        let mut rows_affected = 0;
        for chunk in doomed.chunks(SWEEP_CHUNK) {
            let deletion = Object::delete_many()
                .filter(object::Column::Id.is_in(chunk.iter().copied()))
                .exec(db)
                .await?;
            rows_affected += deletion.rows_affected;
        }

        tracing::info!(
            "Deleted {} objects from {} (ID {}), {} objects protected by pins",
            rows_affected,
            cache.name,
            cache.id,
            protected.len(),
        );
        objects_deleted += rows_affected;
    }

    tracing::info!("Deleted {} objects in total", objects_deleted);

    Ok(())
}

/// Collects the IDs of every object reachable from the cache's pins.
///
/// Reachability is computed inside the database with a recursive CTE over
/// the `references` JSON array each object carries. Entries are store path
/// basenames whose first 32 characters are the store path hash, which is
/// indexed. Postgres and SQLite are supported natively; on other backends
/// pins protect only the pinned objects themselves (with a warning), which
/// is safe but not closure-complete.
async fn find_pin_protected_object_ids(
    db: &DatabaseConnection,
    cache_id: i64,
) -> Result<HashSet<i64>> {
    let backend = db.get_database_backend();

    let statement = match backend {
        sea_orm::DatabaseBackend::Postgres => Statement::from_sql_and_values(
            backend,
            r#"
            WITH RECURSIVE reachable(id) AS (
                SELECT p.object_id FROM pin p WHERE p.cache_id = $1
                UNION
                SELECT o2.id
                FROM reachable r
                JOIN object o ON o.id = r.id
                CROSS JOIN LATERAL jsonb_array_elements_text(o."references"::jsonb) AS ref(name)
                JOIN object o2
                  ON o2.cache_id = $1
                 AND o2.store_path_hash = left(ref.name, 32)
            )
            SELECT id FROM reachable
            "#,
            [cache_id.into()],
        ),
        sea_orm::DatabaseBackend::Sqlite => Statement::from_sql_and_values(
            backend,
            r#"
            WITH RECURSIVE reachable(id) AS (
                SELECT p.object_id FROM pin p WHERE p.cache_id = ?
                UNION
                SELECT o2.id
                FROM reachable r
                JOIN object o ON o.id = r.id
                JOIN json_each(o."references") AS ref
                JOIN object o2
                  ON o2.cache_id = ?
                 AND o2.store_path_hash = substr(ref.value, 1, 32)
            )
            SELECT id FROM reachable
            "#,
            [cache_id.into(), cache_id.into()],
        ),
        _ => {
            tracing::warn!(
                "Pin reachability is not implemented for {:?}; \
                 pins protect only the pinned objects themselves",
                backend
            );
            let pins = Pin::find()
                .filter(pin::Column::CacheId.eq(cache_id))
                .all(db)
                .await?;
            return Ok(pins.into_iter().map(|p| p.object_id).collect());
        }
    };

    let rows = db.query_all_raw(statement).await?;
    let mut ids = HashSet::with_capacity(rows.len());
    for row in rows {
        ids.insert(row.try_get::<i64>("", "id")?);
    }
    Ok(ids)
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

    let database_backend = db.get_database_backend();
    let orphan_chunk_limit = match database_backend {
        // Arbitrarily chosen sensible value since there's no good default to choose from for MySQL
        sea_orm::DatabaseBackend::MySql => 1000,
        // Panic limit set by sqlx for postgresql: https://github.com/launchbadge/sqlx/issues/671#issuecomment-687043510
        sea_orm::DatabaseBackend::Postgres => u64::from(u16::MAX),
        // Default statement limit imposed by sqlite: https://www.sqlite.org/limits.html#max_variable_number
        sea_orm::DatabaseBackend::Sqlite => 500,
        _ => {
            return Err(anyhow!(
                "Unsupported database backend: {database_backend:?}"
            ));
        }
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

    db.execute_raw(transition_statement).await?;

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

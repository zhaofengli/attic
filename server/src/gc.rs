//! Garbage collection.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::{Duration as ChronoDuration, Utc};
use futures::future::join_all;
use sea_orm::entity::prelude::*;
use sea_orm::query::QuerySelect;
use sea_orm::sea_query::{LockBehavior, LockType, Query};
use sea_orm::{ConnectionTrait, FromQueryResult};
use tokio::sync::Semaphore;
use tokio::time;
use tracing::instrument;

use super::{State, StateInner};
use crate::config::Config;
use crate::database::entity::cache::{self, Entity as Cache};
use crate::database::entity::nar::{self, Entity as Nar, NarState};
use crate::database::entity::object::{self, Entity as Object};

#[derive(Debug, FromQueryResult)]
struct CacheIdAndRetentionPeriod {
    id: i64,
    name: String,
    retention_period: i32,
}

/// Runs garbage collection periodically.
pub async fn run_garbage_collection(config: Config) {
    let interval = config.garbage_collection.interval;

    if interval == Duration::ZERO {
        // disabled
        return;
    }

    loop {
        // We don't stop even if it errors
        if let Err(e) = run_garbage_collection_once(config.clone()).await {
            tracing::warn!("Garbage collection failed: {}", e);
        }

        time::sleep(interval).await;
    }
}

/// Runs garbage collection once.
#[instrument(skip_all)]
pub async fn run_garbage_collection_once(config: Config) -> Result<()> {
    tracing::info!("Running garbage collection...");

    let state = StateInner::new(config).await;
    run_time_based_garbage_collection(&state).await?;
    run_reap_orphan_nars(&state).await?;

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
        .filter(retention_period.not_equals(0))
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

#[instrument(skip_all)]
async fn run_reap_orphan_nars(state: &State) -> Result<()> {
    let db = state.database().await?;
    let storage = state.storage().await?;

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

    // ... and transition their state to Deleted
    //
    // Deleted NARs are essentially invisible from our normal queries
    let change_state = Query::update()
        .table(Nar)
        .value(nar::Column::State, NarState::Deleted)
        .and_where(nar::Column::Id.in_subquery(orphan_nar_ids))
        .returning_all()
        .to_owned();

    let stmt = db.get_database_backend().build(&change_state);

    let orphan_nars = nar::Model::find_by_statement(stmt).all(db).await?;

    if orphan_nars.is_empty() {
        return Ok(());
    }

    // Delete the NARs from remote storage
    let delete_limit = Arc::new(Semaphore::new(20)); // TODO: Make this configurable
    let futures: Vec<_> = orphan_nars
        .into_iter()
        .map(|nar| {
            let delete_limit = delete_limit.clone();
            async move {
                let permit = delete_limit.acquire().await?;
                storage.delete_file_db(&nar.remote_file.0).await?;
                drop(permit);
                Result::<_, anyhow::Error>::Ok(nar.id)
            }
        })
        .collect();

    // Deletions can result in spurious failures, tolerate them
    //
    // NARs that failed to be deleted from the remote storage will
    // just be stuck in Deleted state.
    //
    // TODO: Maybe have an interactive command to retry deletions?
    let deleted_nar_ids: Vec<_> = join_all(futures)
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
    let deletion = Nar::delete_many()
        .filter(nar::Column::Id.is_in(deleted_nar_ids))
        .exec(db)
        .await?;

    tracing::info!("Deleted {} NARs", deletion.rows_affected);

    Ok(())
}

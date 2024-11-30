//! Cache configuration endpoint.

use anyhow::anyhow;
use axum::extract::{Extension, Json, Path};
use chrono::Utc;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::ActiveValue::Set;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tracing::instrument;

use crate::database::entity::cache::{self, Entity as Cache};
use crate::database::entity::Json as DbJson;
use crate::error::{ErrorKind, ServerError, ServerResult};
use crate::{RequestState, State};
use attic::api::v1::cache_config::{
    CacheConfig, CreateCacheRequest, KeypairConfig, RetentionPeriodConfig,
};
use attic::cache::CacheName;
use attic::signing::NixKeypair;

#[instrument(skip_all, fields(cache_name))]
pub(crate) async fn get_cache_config(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path(cache_name): Path<CacheName>,
) -> ServerResult<Json<CacheConfig>> {
    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_pull()?;
            Ok(cache)
        })
        .await?;

    let public_key = cache.keypair()?.export_public_key();

    let retention_period_config = if let Some(period) = cache.retention_period {
        RetentionPeriodConfig::Period(period as u32)
    } else {
        RetentionPeriodConfig::Global
    };

    Ok(Json(CacheConfig {
        substituter_endpoint: Some(req_state.substituter_endpoint(cache_name)?),
        api_endpoint: Some(req_state.api_endpoint()?),
        keypair: None,
        public_key: Some(public_key),
        is_public: Some(cache.is_public),
        store_dir: Some(cache.store_dir),
        priority: Some(cache.priority),
        upstream_cache_key_names: Some(cache.upstream_cache_key_names.0),
        retention_period: Some(retention_period_config),
    }))
}

#[instrument(skip_all, fields(cache_name, payload))]
pub(crate) async fn configure_cache(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path(cache_name): Path<CacheName>,
    Json(payload): Json<CacheConfig>,
) -> ServerResult<()> {
    let database = state.database().await?;
    let (cache, permission) = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_configure_cache()?;
            Ok((cache, permission.clone()))
        })
        .await?;

    let mut update = cache::ActiveModel {
        id: Set(cache.id),
        ..Default::default()
    };

    let mut modified = false;

    if let Some(keypair_cfg) = payload.keypair {
        let keypair = match keypair_cfg {
            KeypairConfig::Generate => NixKeypair::generate(cache_name.as_str())?,
            KeypairConfig::Keypair(k) => k,
        };
        update.keypair = Set(keypair.export_keypair());
        modified = true;
    }

    if let Some(is_public) = payload.is_public {
        update.is_public = Set(is_public);
        modified = true;
    }

    if let Some(store_dir) = payload.store_dir {
        update.store_dir = Set(store_dir);
        modified = true;
    }

    if let Some(priority) = payload.priority {
        update.priority = Set(priority);
        modified = true;
    }

    if let Some(upstream_cache_key_names) = payload.upstream_cache_key_names {
        update.upstream_cache_key_names = Set(DbJson(upstream_cache_key_names));
        modified = true;
    }

    if let Some(retention_period_config) = payload.retention_period {
        permission.require_configure_cache_retention()?;

        match retention_period_config {
            RetentionPeriodConfig::Global => {
                update.retention_period = Set(None);
            }
            RetentionPeriodConfig::Period(period) => {
                update.retention_period =
                    Set(Some(period.try_into().map_err(|_| {
                        ErrorKind::RequestError(anyhow!("Invalid retention period"))
                    })?));
            }
        }

        modified = true;
    }

    if modified {
        Cache::update(update)
            .exec(database)
            .await
            .map_err(ServerError::database_error)?;

        Ok(())
    } else {
        Err(ErrorKind::RequestError(anyhow!("No modifiable fields were set.")).into())
    }
}

#[instrument(skip_all, fields(cache_name))]
pub(crate) async fn destroy_cache(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path(cache_name): Path<CacheName>,
) -> ServerResult<()> {
    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_destroy_cache()?;
            Ok(cache)
        })
        .await?;

    if state.config.soft_delete_caches {
        // Perform soft deletion
        let deletion = Cache::update_many()
            .col_expr(cache::Column::DeletedAt, Expr::value(Some(Utc::now())))
            .filter(cache::Column::Id.eq(cache.id))
            .filter(cache::Column::DeletedAt.is_null())
            .exec(database)
            .await
            .map_err(ServerError::database_error)?;

        if deletion.rows_affected == 0 {
            // Someone raced to (soft) delete the cache before us
            Err(ErrorKind::NoSuchCache.into())
        } else {
            Ok(())
        }
    } else {
        // Perform hard deletion
        let deletion = Cache::delete_many()
            .filter(cache::Column::Id.eq(cache.id))
            .filter(cache::Column::DeletedAt.is_null()) // don't operate on soft-deleted caches
            .exec(database)
            .await
            .map_err(ServerError::database_error)?;

        if deletion.rows_affected == 0 {
            // Someone raced to (soft) delete the cache before us
            Err(ErrorKind::NoSuchCache.into())
        } else {
            Ok(())
        }
    }
}

#[instrument(skip_all, fields(cache_name, payload))]
pub(crate) async fn create_cache(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path(cache_name): Path<CacheName>,
    Json(payload): Json<CreateCacheRequest>,
) -> ServerResult<()> {
    let permission = req_state.auth.get_permission_for_cache(&cache_name, false);
    permission.require_create_cache()?;
    
    let database = state.database().await?;

    let keypair = match payload.keypair {
        KeypairConfig::Generate => NixKeypair::generate(cache_name.as_str())?,
        KeypairConfig::Keypair(k) => k,
    };

    let num_inserted = Cache::insert(cache::ActiveModel {
        name: Set(cache_name.to_string()),
        keypair: Set(keypair.export_keypair()),
        is_public: Set(payload.is_public),
        store_dir: Set(payload.store_dir),
        priority: Set(payload.priority),
        upstream_cache_key_names: Set(DbJson(payload.upstream_cache_key_names)),
        created_at: Set(Utc::now()),
        ..Default::default()
    })
    .on_conflict(
        OnConflict::column(cache::Column::Name)
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(database)
    .await
    .map_err(ServerError::database_error)?;

    if num_inserted == 0 {
        // The cache already exists
        Err(ErrorKind::CacheAlreadyExists.into())
    } else {
        Ok(())
    }
}

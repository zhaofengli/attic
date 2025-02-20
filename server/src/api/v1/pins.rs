use std::collections::HashMap;

use axum::extract::{Extension, Json, Path};
use sea_orm::entity::prelude::*;
use sea_orm::ActiveValue::Set;
use tracing::instrument;

use crate::error::{ErrorKind, ServerError, ServerResult};
use crate::{RequestState, State};
use attic::cache::CacheName;
use attic::pin::PinName;

use crate::database::entity::pin::{self, Entity as Pin};

#[instrument(skip_all, fields(cache_name))]
pub(crate) async fn get_pins(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path(cache_name): Path<CacheName>,
) -> ServerResult<Json<HashMap<PinName, String>>> {
    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_pull()?;
            Ok(cache)
        })
        .await?;

    // Safety: Already checked
    #[allow(unsafe_code)]
    let pins = HashMap::from_iter(
        Pin::find()
            .filter(pin::Column::CacheId.eq(cache.id))
            .all(database)
            .await
            .map_err(ServerError::database_error)?
            .iter()
            .cloned()
            .map(|pin| unsafe { (PinName::new_unchecked(pin.name), pin.store_path) })
            .collect::<Vec<(PinName, String)>>(),
    );

    Ok(Json(pins))
}

#[instrument(skip_all, fields(cache_name, pin_name))]
pub(crate) async fn get_pin(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path((cache_name, pin_name)): Path<(CacheName, PinName)>,
) -> ServerResult<Json<String>> {
    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_pull()?;
            Ok(cache)
        })
        .await?;

    let pin = Pin::find()
        .filter(pin::Column::CacheId.eq(cache.id))
        .filter(pin::Column::Name.eq(pin_name.as_str()))
        .one(database)
        .await
        .map_err(ServerError::database_error)?
        .ok_or_else(|| Into::<ServerError>::into(ErrorKind::NoSuchPin))?;

    let store_path = pin.store_path;

    Ok(Json(store_path))
}

#[instrument(skip_all, fields(cache_name, pin_name))]
pub(crate) async fn create_pin(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path((cache_name, pin_name)): Path<(CacheName, PinName)>,
    Json(store_path): Json<String>,
) -> ServerResult<()> {
    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_push()?;
            Ok(cache)
        })
        .await?;

    let old_pin = Pin::find()
        .filter(pin::Column::CacheId.eq(cache.id))
        .filter(pin::Column::Name.eq(pin_name.as_str()))
        .one(database)
        .await
        .map_err(ServerError::database_error)?;

    let model = pin::ActiveModel {
        cache_id: Set(cache.id),
        name: Set(pin_name.to_string()),
        store_path: Set(store_path.clone()),
        ..Default::default()
    };
    Pin::insert(model)
        .exec(database)
        .await
        .map_err(ServerError::database_error)?;

    if let Some(old_pin) = old_pin {
        tracing::info!(
            "Updated pin {}/{} ({} -> {})",
            cache.name,
            pin_name,
            old_pin.store_path,
            store_path,
        );
    } else {
        tracing::info!("Created pin {}/{} ({})", cache.name, pin_name, store_path);
    }

    Ok(())
}

#[instrument(skip_all, fields(cache_name, pin_name))]
pub(crate) async fn delete_pin(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path((cache_name, pin_name)): Path<(CacheName, PinName)>,
) -> ServerResult<()> {
    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_push()?;
            Ok(cache)
        })
        .await?;

    if let Some(pin) = Pin::find()
        .filter(pin::Column::CacheId.eq(cache.id))
        .filter(pin::Column::Name.eq(pin_name.as_str()))
        .one(database)
        .await
        .map_err(ServerError::database_error)?
    {
        Pin::delete_by_id(pin.id)
            .exec(database)
            .await
            .map_err(ServerError::database_error)?;
        tracing::info!(
            "Deleted pin {}/{} ({})",
            cache.name,
            pin.name,
            pin.store_path,
        );
    }

    Ok(())
}

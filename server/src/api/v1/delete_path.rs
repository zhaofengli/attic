use std::collections::HashSet;

use axum::extract::{Extension, Json, Path};
use sea_orm::entity::prelude::*;
use sea_orm::{FromQueryResult, QuerySelect};
use tracing::instrument;

use crate::database::entity::cache;
use crate::database::entity::nar;
use crate::database::entity::object::{self, Entity as Object};
use crate::error::{ServerError, ServerResult};
use crate::{RequestState, State};
use attic::api::v1::get_missing_paths::{GetMissingPathsRequest, GetMissingPathsResponse};
use attic::nix_store::StorePathHash;


/// Deletes a path from a cache.
#[instrument(skip_all, fields(cache_name, path))]
pub(crate) async fn delete_path(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path((cache_name, path)): Path<(CacheName, String)>,
) -> ServerResult<()> {
    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_delete()?;
            Ok(cache)
        })
        .await?;

    let object = Object::find()
        .filter(object::Column::StorePathHash.eq(store_path_hash.as_str()))
        .one(database)
        .await
        .map_err(ServerError::database_error)?;

    if let Some(object) = object {
        object.delete(database).await.map_err(ServerError::database_error)?;
    } else {
        return Err(ServerError::not_found("object not found"));
    }

    Ok(())
}

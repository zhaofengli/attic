//! Pin endpoints.
//!
//! Pins are named GC roots: the garbage collector keeps everything reachable
//! from a pinned object, regardless of age (see `crate::gc`). Creating and
//! deleting pins requires push permission; listing requires pull.

use axum::extract::{Extension, Json, Path};
use chrono::Utc;
use sea_orm::ActiveValue::Set;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use tracing::instrument;

use crate::database::entity::object::{self, Entity as Object};
use crate::database::entity::pin::{self, Entity as Pin, InsertExt};
use crate::error::{ErrorKind, ServerError, ServerResult};
use crate::{RequestState, State};
use attic::api::v1::pin::{CreatePinRequest, ListPinsResponse, PinEntry};
use attic::cache::CacheName;

/// Maximum length of a pin name.
const MAX_PIN_NAME_LEN: usize = 200;

fn validate_pin_name(name: &str) -> ServerResult<()> {
    let ok = !name.is_empty()
        && name.len() <= MAX_PIN_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '@' | '+'));
    if ok {
        Ok(())
    } else {
        Err(ErrorKind::RequestError(anyhow::anyhow!(
            "Invalid pin name (allowed: alphanumerics and - _ . : @ +, at most {} characters)",
            MAX_PIN_NAME_LEN
        ))
        .into())
    }
}

/// Extracts the hash part from a full store path.
///
/// We only need the hash to find the object row; the store directory prefix
/// is not interpreted.
fn store_path_hash_part(store_path: &str) -> ServerResult<String> {
    let base = store_path.rsplit('/').next().unwrap_or(store_path);
    let hash: String = base.chars().take_while(|c| *c != '-').collect();
    if hash.len() == 32 && hash.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(hash)
    } else {
        Err(ErrorKind::RequestError(anyhow::anyhow!(
            "Invalid store path: {}",
            store_path
        ))
        .into())
    }
}

/// Creates or re-points a pin.
///
/// `PUT /_api/v1/pins/{cache}/{name}`
#[instrument(skip_all, fields(cache_name, pin_name))]
pub(crate) async fn create_pin(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path((cache_name, pin_name)): Path<(CacheName, String)>,
    Json(payload): Json<CreatePinRequest>,
) -> ServerResult<()> {
    validate_pin_name(&pin_name)?;

    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_push()?;
            Ok(cache)
        })
        .await?;

    let hash = store_path_hash_part(&payload.store_path)?;
    let object = Object::find()
        .filter(object::Column::CacheId.eq(cache.id))
        .filter(object::Column::StorePathHash.eq(hash))
        .one(database)
        .await
        .map_err(ServerError::database_error)?
        .ok_or_else(|| {
            ErrorKind::NoSuchObject
        })?;

    let username = req_state.auth.username().map(str::to_string);

    Pin::insert(pin::ActiveModel {
        cache_id: Set(cache.id),
        name: Set(pin_name),
        object_id: Set(object.id),
        created_at: Set(Utc::now()),
        created_by: Set(username),
        ..Default::default()
    })
    .on_conflict_do_update()
    .exec(database)
    .await
    .map_err(ServerError::database_error)?;

    Ok(())
}

/// Deletes a pin, releasing its closure for garbage collection.
///
/// `DELETE /_api/v1/pins/{cache}/{name}`
#[instrument(skip_all, fields(cache_name, pin_name))]
pub(crate) async fn delete_pin(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path((cache_name, pin_name)): Path<(CacheName, String)>,
) -> ServerResult<()> {
    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_push()?;
            Ok(cache)
        })
        .await?;

    let deletion = Pin::delete_many()
        .filter(pin::Column::CacheId.eq(cache.id))
        .filter(pin::Column::Name.eq(pin_name))
        .exec(database)
        .await
        .map_err(ServerError::database_error)?;

    if deletion.rows_affected == 0 {
        return Err(ErrorKind::NoSuchObject.into());
    }

    Ok(())
}

/// Lists the pins of a cache.
///
/// `GET /_api/v1/pins/{cache}`
#[instrument(skip_all, fields(cache_name))]
pub(crate) async fn list_pins(
    Extension(state): Extension<State>,
    Extension(req_state): Extension<RequestState>,
    Path(cache_name): Path<CacheName>,
) -> ServerResult<Json<ListPinsResponse>> {
    let database = state.database().await?;
    let cache = req_state
        .auth
        .auth_cache(database, &cache_name, |cache, permission| {
            permission.require_pull()?;
            Ok(cache)
        })
        .await?;

    let pins = Pin::find()
        .filter(pin::Column::CacheId.eq(cache.id))
        .find_also_related(Object)
        .order_by_asc(pin::Column::Name)
        .all(database)
        .await
        .map_err(ServerError::database_error)?;

    let pins = pins
        .into_iter()
        .map(|(p, o)| PinEntry {
            name: p.name,
            store_path: o.map(|o| o.store_path).unwrap_or_default(),
            created_at: p.created_at.to_rfc3339(),
            created_by: p.created_by,
        })
        .collect();

    Ok(Json(ListPinsResponse { pins }))
}

#[cfg(test)]
mod tests {
    use super::{store_path_hash_part, validate_pin_name};

    #[test]
    fn hash_part_accepts_full_store_path() {
        assert_eq!(
            store_path_hash_part("/nix/store/w41lkdzpr1277fxgwph46ib1b0srz03c-pin-victim")
                .unwrap(),
            "w41lkdzpr1277fxgwph46ib1b0srz03c"
        );
    }

    #[test]
    fn hash_part_accepts_bare_basename() {
        assert_eq!(
            store_path_hash_part("w41lkdzpr1277fxgwph46ib1b0srz03c-x").unwrap(),
            "w41lkdzpr1277fxgwph46ib1b0srz03c"
        );
    }

    #[test]
    fn hash_part_rejects_short_hash() {
        assert!(store_path_hash_part("/nix/store/tooshort-name").is_err());
    }

    #[test]
    fn hash_part_rejects_missing_dash() {
        // 32 chars but no name component: not a store path basename.
        assert!(store_path_hash_part("w41lkdzpr1277fxgwph46ib1b0srz03c").is_err());
    }

    #[test]
    fn pin_name_accepts_typical_identities() {
        for name in ["job-1051", "release_2.0", "receipt:abc@host+1"] {
            assert!(validate_pin_name(name).is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn pin_name_rejects_empty_and_pathy() {
        assert!(validate_pin_name("").is_err());
        assert!(validate_pin_name("a/b").is_err());
        assert!(validate_pin_name(&"x".repeat(201)).is_err());
    }
}

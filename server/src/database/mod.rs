pub mod entity;
pub mod migration;

use std::ops::Deref;

use async_trait::async_trait;
use chrono::Utc;
use sea_orm::entity::prelude::*;
use sea_orm::entity::Iterable as EnumIterable;
use sea_orm::query::{JoinType, QuerySelect, QueryTrait};
use sea_orm::sea_query::{Expr, LockBehavior, LockType, Query, Value};
use sea_orm::{ActiveValue::Set, ConnectionTrait, DatabaseConnection, FromQueryResult};
use tokio::task;

use crate::error::{ServerError, ServerResult};
use attic::cache::CacheName;
use attic::hash::Hash;
use attic::nix_store::StorePathHash;
use entity::cache::{self, CacheModel, Entity as Cache};
use entity::nar::{self, Entity as Nar, NarModel, NarState};
use entity::object::{self, Entity as Object, ObjectModel};

const SELECT_OBJECT: &str = "O_";
const SELECT_CACHE: &str = "C_";
const SELECT_NAR: &str = "N_";

#[async_trait]
pub trait AtticDatabase: Send + Sync {
    /// Retrieves an object in a binary cache by its store path hash.
    async fn find_object_by_store_path_hash(
        &self,
        cache: &CacheName,
        store_path_hash: &StorePathHash,
    ) -> ServerResult<(ObjectModel, CacheModel, NarModel)>;

    /// Retrieves a binary cache.
    async fn find_cache(&self, cache: &CacheName) -> ServerResult<CacheModel>;

    /// Retrieves and locks a valid NAR matching a NAR Hash.
    async fn find_and_lock_nar(&self, nar_hash: &Hash) -> ServerResult<Option<NarGuard>>;

    /// Bumps the last accessed timestamp of an object.
    async fn bump_object_last_accessed(&self, object_id: i64) -> ServerResult<()>;
}

pub struct NarGuard {
    database: DatabaseConnection,
    nar: NarModel,
}

fn prefix_column<E: EntityTrait, S: QuerySelect>(mut select: S, prefix: &str) -> S {
    for col in <E::Column as EnumIterable>::iter() {
        let alias = format!("{}{}", prefix, Iden::to_string(&col));
        select = select.column_as(col, alias);
    }
    select
}

pub fn build_cache_object_nar_query() -> Select<Object> {
    let mut query = Object::find()
        .select_only()
        .join(JoinType::LeftJoin, object::Relation::Cache.def())
        .join(JoinType::LeftJoin, object::Relation::Nar.def());

    query = prefix_column::<object::Entity, _>(query, SELECT_OBJECT);
    query = prefix_column::<cache::Entity, _>(query, SELECT_CACHE);
    query = prefix_column::<nar::Entity, _>(query, SELECT_NAR);

    query
}

#[async_trait]
impl AtticDatabase for DatabaseConnection {
    async fn find_object_by_store_path_hash(
        &self,
        cache: &CacheName,
        store_path_hash: &StorePathHash,
    ) -> ServerResult<(ObjectModel, CacheModel, NarModel)> {
        let stmt = build_cache_object_nar_query()
            .filter(cache::Column::Name.eq(cache.as_str()))
            .filter(cache::Column::DeletedAt.is_null())
            .filter(object::Column::StorePathHash.eq(store_path_hash.as_str()))
            .filter(nar::Column::State.eq(NarState::Valid))
            .limit(1)
            .build(self.get_database_backend());

        let result = self
            .query_one(stmt)
            .await
            .map_err(ServerError::database_error)?
            .ok_or(ServerError::NoSuchObject)?;

        let object = object::Model::from_query_result(&result, SELECT_OBJECT)
            .map_err(ServerError::database_error)?;
        let cache = cache::Model::from_query_result(&result, SELECT_CACHE)
            .map_err(ServerError::database_error)?;
        let nar = nar::Model::from_query_result(&result, SELECT_NAR)
            .map_err(ServerError::database_error)?;

        Ok((object, cache, nar))
    }

    async fn find_cache(&self, cache: &CacheName) -> ServerResult<CacheModel> {
        Cache::find()
            .filter(cache::Column::Name.eq(cache.as_str()))
            .filter(cache::Column::DeletedAt.is_null())
            .one(self)
            .await
            .map_err(ServerError::database_error)?
            .ok_or(ServerError::NoSuchCache)
    }

    async fn find_and_lock_nar(&self, nar_hash: &Hash) -> ServerResult<Option<NarGuard>> {
        let one = Value::Unsigned(Some(1));
        let matched_ids = Query::select()
            .from(Nar)
            .and_where(nar::Column::NarHash.eq(nar_hash.to_typed_base16()))
            .and_where(nar::Column::State.eq(NarState::Valid))
            .expr(Expr::col(nar::Column::Id))
            .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
            .limit(1)
            .to_owned();
        let incr_holders = Query::update()
            .table(Nar)
            .values([(
                nar::Column::HoldersCount,
                Expr::col(nar::Column::HoldersCount).add(one),
            )])
            .and_where(nar::Column::Id.in_subquery(matched_ids))
            .returning_all()
            .to_owned();
        let stmt = self.get_database_backend().build(&incr_holders);

        let guard = nar::Model::find_by_statement(stmt)
            .one(self)
            .await
            .map_err(ServerError::database_error)?
            .map(|nar| NarGuard {
                database: self.clone(),
                nar,
            });

        Ok(guard)
    }

    async fn bump_object_last_accessed(&self, object_id: i64) -> ServerResult<()> {
        let now = Utc::now();

        Object::update(object::ActiveModel {
            id: Set(object_id),
            last_accessed_at: Set(Some(now)),
            ..Default::default()
        })
        .exec(self)
        .await
        .map_err(ServerError::database_error)?;

        Ok(())
    }
}

impl Deref for NarGuard {
    type Target = NarModel;

    fn deref(&self) -> &Self::Target {
        &self.nar
    }
}

impl Drop for NarGuard {
    fn drop(&mut self) {
        let database = self.database.clone();
        let nar_id = self.nar.id;

        task::spawn(async move {
            tracing::debug!("Unlocking NAR");

            let one = Value::Unsigned(Some(1));
            let decr_holders = Query::update()
                .table(Nar)
                .values([(
                    nar::Column::HoldersCount,
                    Expr::col(nar::Column::HoldersCount).sub(one),
                )])
                .and_where(nar::Column::Id.eq(nar_id))
                .to_owned();
            let stmt = database.get_database_backend().build(&decr_holders);

            if let Err(e) = database.execute(stmt).await {
                tracing::warn!("Failed to decrement holders count: {}", e);
            }
        });
    }
}

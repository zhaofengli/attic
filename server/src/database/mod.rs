pub mod entity;
pub mod migration;

use std::ops::Deref;

use anyhow::anyhow;
use async_trait::async_trait;
use chrono::Utc;
use sea_orm::entity::prelude::*;
use sea_orm::entity::Iterable as EnumIterable;
use sea_orm::query::{JoinType, QueryOrder, QuerySelect, QueryTrait};
use sea_orm::sea_query::{Expr, LockBehavior, LockType, Query, Value};
use sea_orm::{ActiveValue::Set, ConnectionTrait, DatabaseConnection, FromQueryResult};
use tokio::task;

use crate::error::{ErrorKind, ServerError, ServerResult};
use crate::narinfo::Compression;
use attic::cache::CacheName;
use attic::hash::Hash;
use attic::nix_store::StorePathHash;
use entity::cache::{self, CacheModel, Entity as Cache};
use entity::chunk::{self, ChunkModel, ChunkState, Entity as Chunk};
use entity::chunkref;
use entity::nar::{self, Entity as Nar, NarModel, NarState};
use entity::object::{self, Entity as Object, ObjectModel};

// quintuple join time
const SELECT_OBJECT: &str = "O_";
const SELECT_CACHE: &str = "C_";
const SELECT_NAR: &str = "N_";
const SELECT_CHUNK: &str = "CH_";
const SELECT_CHUNKREF: &str = "CHR_";

#[async_trait]
pub trait AtticDatabase: Send + Sync {
    /// Retrieves an object in a binary cache by its store path hash, returning all its
    /// chunks.
    async fn find_object_and_chunks_by_store_path_hash(
        &self,
        cache: &CacheName,
        store_path_hash: &StorePathHash,
    ) -> ServerResult<(ObjectModel, CacheModel, NarModel, Vec<Option<ChunkModel>>)>;

    /// Retrieves a binary cache.
    async fn find_cache(&self, cache: &CacheName) -> ServerResult<CacheModel>;

    /// Retrieves and locks a valid NAR matching a NAR Hash.
    async fn find_and_lock_nar(&self, nar_hash: &Hash) -> ServerResult<Option<NarGuard>>;

    /// Retrieves and locks a valid chunk matching a chunk Hash.
    async fn find_and_lock_chunk(
        &self,
        chunk_hash: &Hash,
        compression: Compression,
    ) -> ServerResult<Option<ChunkGuard>>;

    /// Bumps the last accessed timestamp of an object.
    async fn bump_object_last_accessed(&self, object_id: i64) -> ServerResult<()>;
}

pub struct NarGuard {
    database: DatabaseConnection,
    nar: NarModel,
}

pub struct ChunkGuard {
    database: DatabaseConnection,
    chunk: ChunkModel,
}

fn prefix_column<E: EntityTrait, S: QuerySelect>(mut select: S, prefix: &str) -> S {
    for col in <E::Column as EnumIterable>::iter() {
        let alias = format!("{}{}", prefix, Iden::to_string(&col));
        select = select.column_as(col, alias);
    }
    select
}

pub fn build_cache_object_nar_query() -> Select<Object> {
    /*
        Build something like:

        -- chunkrefs must exist but chunks may not exist

        select * from object
        inner join cache
             on object.cache_id = cache.id
        inner join nar
             on object.nar_id = nar.id
        inner join chunkref
            on chunkref.nar_id = nar.id
        left join chunk
            on chunkref.chunk_id = chunk.id
        where
            object.store_path_hash = 'fiwsv60kgwrfvib2nf9dkq9q8bk1h7qh' and
            nar.state = 'V' and
            cache.name = 'zhaofeng' and
            cache.deleted_at is null

        Returns (CacheModel, ObjectModel, NarModel, Vec<Option<ChunkModel>>)
        where the number of elements in the Vec must be equal to `nar.num_chunks`.

        If any element in the chunk `Vec` is `None`, it means the chunk is missing
        for some reason (e.g., corrupted) and the full NAR cannot be reconstructed.
        In such cases, .narinfo/.nar requests will return HTTP 503 and the affected
        store paths will be treated as non-existent in `get-missing-paths` so they
        can be repaired automatically when any client upload a path containing the
        missing chunk.

        It's a quintuple join and the query plans look reasonable on SQLite
        and Postgres. For each .narinfo/.nar request, we only submit a single query.
    */
    let mut query = Object::find()
        .select_only()
        .join(JoinType::InnerJoin, object::Relation::Cache.def())
        .join(JoinType::InnerJoin, object::Relation::Nar.def())
        .join(JoinType::InnerJoin, nar::Relation::ChunkRef.def())
        .join(JoinType::LeftJoin, chunkref::Relation::Chunk.def())
        .order_by_asc(chunkref::Column::Seq);

    query = prefix_column::<object::Entity, _>(query, SELECT_OBJECT);
    query = prefix_column::<cache::Entity, _>(query, SELECT_CACHE);
    query = prefix_column::<nar::Entity, _>(query, SELECT_NAR);
    query = prefix_column::<chunk::Entity, _>(query, SELECT_CHUNK);
    query = prefix_column::<chunkref::Entity, _>(query, SELECT_CHUNKREF);

    query
}

#[async_trait]
impl AtticDatabase for DatabaseConnection {
    async fn find_object_and_chunks_by_store_path_hash(
        &self,
        cache: &CacheName,
        store_path_hash: &StorePathHash,
    ) -> ServerResult<(ObjectModel, CacheModel, NarModel, Vec<Option<ChunkModel>>)> {
        let stmt = build_cache_object_nar_query()
            .filter(cache::Column::Name.eq(cache.as_str()))
            .filter(cache::Column::DeletedAt.is_null())
            .filter(object::Column::StorePathHash.eq(store_path_hash.as_str()))
            .filter(nar::Column::State.eq(NarState::Valid))
            .filter(
                chunk::Column::State
                    .eq(ChunkState::Valid)
                    .or(chunk::Column::State.is_null()),
            )
            .build(self.get_database_backend());

        let results = self
            .query_all(stmt)
            .await
            .map_err(ServerError::database_error)?;

        if results.is_empty() {
            return Err(ErrorKind::NoSuchObject.into());
        }

        let mut it = results.iter();
        let first = it.next().unwrap();

        let mut chunks = Vec::new();

        let object = object::Model::from_query_result(first, SELECT_OBJECT)
            .map_err(ServerError::database_error)?;
        let cache = cache::Model::from_query_result(first, SELECT_CACHE)
            .map_err(ServerError::database_error)?;
        let nar = nar::Model::from_query_result(first, SELECT_NAR)
            .map_err(ServerError::database_error)?;

        if results.len() != nar.num_chunks as usize {
            // Something went terribly wrong. This means there are a wrong number of `chunkref` rows.
            return Err(ErrorKind::DatabaseError(anyhow!(
                "Database returned the wrong number of chunks: Expected {}, got {}",
                nar.num_chunks,
                results.len()
            ))
            .into());
        }

        chunks.push({
            let chunk_id: Option<i64> = first
                .try_get(SELECT_CHUNK, chunk::Column::Id.as_str())
                .map_err(ServerError::database_error)?;

            if chunk_id.is_some() {
                Some(
                    chunk::Model::from_query_result(first, SELECT_CHUNK)
                        .map_err(ServerError::database_error)?,
                )
            } else {
                None
            }
        });

        for chunk in it {
            chunks.push({
                let chunk_id: Option<i64> = chunk
                    .try_get(SELECT_CHUNK, chunk::Column::Id.as_str())
                    .map_err(ServerError::database_error)?;

                if chunk_id.is_some() {
                    Some(
                        chunk::Model::from_query_result(chunk, SELECT_CHUNK)
                            .map_err(ServerError::database_error)?,
                    )
                } else {
                    None
                }
            });
        }

        Ok((object, cache, nar, chunks))
    }

    async fn find_cache(&self, cache: &CacheName) -> ServerResult<CacheModel> {
        Cache::find()
            .filter(cache::Column::Name.eq(cache.as_str()))
            .filter(cache::Column::DeletedAt.is_null())
            .one(self)
            .await
            .map_err(ServerError::database_error)?
            .ok_or_else(|| ErrorKind::NoSuchCache.into())
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

    // FIXME: Repetition
    async fn find_and_lock_chunk(
        &self,
        chunk_hash: &Hash,
        compression: Compression,
    ) -> ServerResult<Option<ChunkGuard>> {
        let one = Value::Unsigned(Some(1));
        let matched_ids = Query::select()
            .from(Chunk)
            .and_where(chunk::Column::ChunkHash.eq(chunk_hash.to_typed_base16()))
            .and_where(chunk::Column::State.eq(ChunkState::Valid))
            .and_where(chunk::Column::Compression.eq(compression.as_str()))
            .expr(Expr::col(chunk::Column::Id))
            .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
            .limit(1)
            .to_owned();
        let incr_holders = Query::update()
            .table(Chunk)
            .values([(
                chunk::Column::HoldersCount,
                Expr::col(chunk::Column::HoldersCount).add(one),
            )])
            .and_where(chunk::Column::Id.in_subquery(matched_ids))
            .returning_all()
            .to_owned();
        let stmt = self.get_database_backend().build(&incr_holders);

        let guard = chunk::Model::find_by_statement(stmt)
            .one(self)
            .await
            .map_err(ServerError::database_error)?
            .map(|chunk| ChunkGuard {
                database: self.clone(),
                chunk,
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

impl ChunkGuard {
    pub fn from_locked(database: DatabaseConnection, chunk: ChunkModel) -> Self {
        Self { database, chunk }
    }
}

impl Deref for ChunkGuard {
    type Target = ChunkModel;

    fn deref(&self) -> &Self::Target {
        &self.chunk
    }
}

impl Drop for ChunkGuard {
    fn drop(&mut self) {
        let database = self.database.clone();
        let chunk_id = self.chunk.id;

        task::spawn(async move {
            tracing::debug!("Unlocking chunk");

            let one = Value::Unsigned(Some(1));
            let decr_holders = Query::update()
                .table(Chunk)
                .values([(
                    chunk::Column::HoldersCount,
                    Expr::col(chunk::Column::HoldersCount).sub(one),
                )])
                .and_where(chunk::Column::Id.eq(chunk_id))
                .to_owned();
            let stmt = database.get_database_backend().build(&decr_holders);

            if let Err(e) = database.execute(stmt).await {
                tracing::warn!("Failed to decrement holders count: {}", e);
            }
        });
    }
}

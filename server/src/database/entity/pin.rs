//! A pin: a named root that exempts an object's closure from garbage
//! collection.
//!
//! Pins give retention a correctness notion that time-based expiry cannot
//! express: "this store path and everything it references stays until I say
//! otherwise". The garbage collector treats every object reachable from a
//! pinned object (via the `references` each object already carries) as
//! alive, regardless of age; deleting the pin releases the closure — except
//! whatever is still reachable from other pins.

use sea_orm::Insert;
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::OnConflict;

pub type PinModel = Model;

pub trait InsertExt {
    fn on_conflict_do_update(self) -> Self;
}

/// A named GC root in a binary cache.
#[derive(Debug, Clone, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "pin")]
pub struct Model {
    /// Unique numeric ID of the pin.
    #[sea_orm(primary_key)]
    pub id: i64,

    /// ID of the binary cache the pin belongs to.
    #[sea_orm(indexed)]
    pub cache_id: i64,

    /// Human-readable name of the pin, unique per cache.
    ///
    /// Typically the identity of whatever the pinned closure belongs to —
    /// a release tag, a job ID, a receipt.
    pub name: String,

    /// ID of the pinned object (the root of the protected closure).
    #[sea_orm(indexed)]
    pub object_id: i64,

    /// Timestamp when the pin was created (or last re-pointed).
    pub created_at: ChronoDateTimeUtc,

    /// The creator of the pin (the `sub` claim of the client's JWT).
    pub created_by: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::cache::Entity",
        from = "Column::CacheId",
        to = "super::cache::Column::Id"
    )]
    Cache,

    #[sea_orm(
        belongs_to = "super::object::Entity",
        from = "Column::ObjectId",
        to = "super::object::Column::Id"
    )]
    Object,
}

impl InsertExt for Insert<ActiveModel> {
    fn on_conflict_do_update(self) -> Self {
        self.on_conflict(
            OnConflict::columns([Column::CacheId, Column::Name])
                .update_columns([Column::ObjectId, Column::CreatedAt, Column::CreatedBy])
                .to_owned(),
        )
    }
}

impl Related<super::cache::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Cache.def()
    }
}

impl Related<super::object::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Object.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

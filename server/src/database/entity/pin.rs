//! An pinned path in a local cache.

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::OnConflict;
use sea_orm::Insert;

pub type PinModel = Model;

pub trait InsertExt {
    fn on_conflict_do_update(self) -> Self;
}

/// An pinned path in a binary cache.
#[derive(Debug, Clone, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "pin")]
pub struct Model {
    /// Unique numeric ID of the pin.
    #[sea_orm(primary_key)]
    pub id: i64,

    /// Name of the pin.
    #[sea_orm(column_type = "String(Some(50))", indexed)]
    pub name: String,

    /// ID of the binary cache the pin belongs to.
    #[sea_orm(indexed)]
    pub cache_id: i64,

    /// The object store path hash this pin points to
    #[sea_orm(column_type = "String(Some(32))", indexed)]
    pub store_path_hash: String,

    /// The full store path being pinned, including the store directory.
    pub store_path: String,
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
        from = "Column::StorePathHash",
        to = "super::object::Column::StorePathHash"
    )]
    Object,
}

impl InsertExt for Insert<ActiveModel> {
    fn on_conflict_do_update(self) -> Self {
        self.on_conflict(
            OnConflict::columns([Column::Name, Column::CacheId])
                .update_columns([Column::StorePathHash, Column::StorePath])
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

//! A reference binding a NAR and a chunk.
//!
//! A NAR is backed by a sequence of chunks.

use sea_orm::entity::prelude::*;

pub type ChunkRefModel = Model;

/// A reference binding a NAR to a chunk.
#[derive(Debug, Clone, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "chunkref")]
pub struct Model {
    /// Unique numeric ID of the link.
    #[sea_orm(primary_key)]
    pub id: i64,

    /// ID of the NAR.
    #[sea_orm(indexed)]
    pub nar_id: i64,

    /// The zero-indexed sequence number of the chunk.
    pub seq: i32,

    /// ID of the chunk.
    ///
    /// This may be NULL when the chunk is missing from the
    /// database.
    ///
    /// TODO: NOT NULL
    #[sea_orm(indexed)]
    pub chunk_id: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::chunk::Entity",
        from = "Column::ChunkId",
        to = "super::chunk::Column::Id"
    )]
    Chunk,

    #[sea_orm(
        belongs_to = "super::nar::Entity",
        from = "Column::NarId",
        to = "super::nar::Column::Id"
    )]
    Nar,
}

impl Related<super::chunk::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Chunk.def()
    }
}

impl Related<super::nar::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Nar.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

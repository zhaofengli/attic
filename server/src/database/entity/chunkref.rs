//! A reference binding a NAR and a chunk.
//!
//! A NAR is backed by a sequence of chunks.
//!
//! A chunk may become unavailable (e.g., disk corruption) and
//! removed from the database, in which case all dependent NARs
//! will become unavailable.
//!
//! Such scenario can be recovered from by reuploading any object
//! that has the missing chunk. `atticadm` will have the functionality
//! to kill/delete a corrupted chunk from the database and to find
//! objects with missing chunks so they can be repaired.

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
    #[sea_orm(indexed)]
    pub chunk_id: Option<i64>,

    /// The hash of the uncompressed chunk.
    ///
    /// This always begins with "sha256:" with the hash in the
    /// hexadecimal format.
    ///
    /// This is used for recovering from a missing chunk.
    #[sea_orm(indexed)]
    pub chunk_hash: String,

    /// The compression of the compressed chunk.
    ///
    /// This is used for recovering from a missing chunk.
    pub compression: String,
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

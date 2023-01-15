//! A content-addressed chunk in the global chunk store.

use sea_orm::entity::prelude::*;

use super::Json;
use crate::storage::RemoteFile;

pub type ChunkModel = Model;

/// The state of a chunk.
#[derive(EnumIter, DeriveActiveEnum, Debug, Clone, PartialEq, Eq)]
#[sea_orm(rs_type = "String", db_type = "String(Some(1))")]
pub enum ChunkState {
    /// The chunk can be used.
    ///
    /// The raw and compressed hashes are available.
    #[sea_orm(string_value = "V")]
    Valid,

    /// The chunk is a pending upload.
    ///
    /// The raw and compressed hashes may not be available.
    #[sea_orm(string_value = "P")]
    PendingUpload,

    /// The chunk can be deleted because it already exists.
    ///
    /// This state can be transitioned into from `PendingUpload`
    /// if some other client completes uploading the same chunk
    /// faster.
    #[sea_orm(string_value = "C")]
    ConfirmedDeduplicated,

    /// The chunk is being deleted.
    ///
    /// This row will be deleted shortly.
    #[sea_orm(string_value = "D")]
    Deleted,
}

/// A content-addressed chunk in the global cache.
#[derive(Debug, Clone, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "chunk")]
pub struct Model {
    /// Unique numeric ID of the chunk.
    #[sea_orm(primary_key)]
    pub id: i64,

    /// The state of the chunk.
    state: ChunkState,

    /// The hash of the uncompressed chunk.
    ///
    /// This always begins with "sha256:" with the hash in the
    /// hexadecimal format.
    ///
    /// The global chunk store may have several chunks with the same
    /// hash:
    ///
    /// - Racing uploads from different clients
    /// - Different compression methods
    #[sea_orm(indexed)]
    pub chunk_hash: String,

    /// The size of the uncompressed chunk.
    pub chunk_size: i64,

    /// The hash of the compressed chunk.
    ///
    /// This always begins with "sha256:" with the hash in the
    /// hexadecimal format.
    ///
    /// This field may not be available if the file hashes aren't
    /// confirmed.
    pub file_hash: Option<String>,

    /// The size of the compressed chunk.
    ///
    /// This field may not be available if the file hashes aren't
    /// confirmed.
    pub file_size: Option<i64>,

    /// The type of compression in use.
    #[sea_orm(column_type = "String(Some(10))")]
    pub compression: String,

    /// The remote file backing this chunk.
    pub remote_file: Json<RemoteFile>,

    /// Unique string identifying the remote file.
    #[sea_orm(unique)]
    pub remote_file_id: String,

    /// Number of processes holding this chunk.
    ///
    /// This is for preventing garbage collection of chunks when
    /// there is a pending upload that can be deduplicated and
    /// there are no existing NAR references.
    pub holders_count: i32,

    /// Timestamp when the chunk is created.
    pub created_at: ChronoDateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::chunkref::Entity")]
    ChunkRef,
}

impl ActiveModelBehavior for ActiveModel {}

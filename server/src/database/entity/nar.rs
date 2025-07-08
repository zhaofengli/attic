//! A content-addressed NAR in the global cache.

use sea_orm::entity::prelude::*;

pub type NarModel = Model;

/// The state of a NAR.
#[derive(EnumIter, DeriveActiveEnum, Debug, Clone, PartialEq, Eq)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(1))")]
pub enum NarState {
    /// The NAR can be used.
    ///
    /// The NAR and file hashes have been confirmed.
    #[sea_orm(string_value = "V")]
    Valid,

    /// The NAR is a pending upload.
    ///
    /// The NAR and file hashes aren't trusted and may
    /// not be available.
    #[sea_orm(string_value = "P")]
    PendingUpload,

    /// The NAR can be deleted because it already exists.
    ///
    /// This state can be transitioned into from `PendingUpload`
    /// if some other client completes uploading the same NAR
    /// faster.
    #[sea_orm(string_value = "C")]
    ConfirmedDeduplicated,

    /// The NAR is being deleted.
    ///
    /// This row will be deleted shortly.
    /// This variant is no longer used since the actual storage is managed as chunks.
    #[sea_orm(string_value = "D")]
    Deleted,
}

/// A content-addressed NAR in the global cache.
///
/// A NAR without `nix-store --export` metadata is context-free,
/// meaning that it's not associated with a store path and only
/// depends on its contents.
///
/// ## NAR Repair
///
/// After a NAR is transitioned into the `Valid` state, its list
/// of constituent chunks in `chunkref` is immutable. When a client
/// uploads an existing NAR and the NAR has unavailable chunks,
/// a new `nar` entry is created and all dependent `object` rows
/// will have the `nar_id` updated. The old `nar` entry will
/// be garbage-collected.
///
/// Why don't we just fill in the missing chunks in the existing
/// `nar`? Because the NAR stream from the client _might_ be chunked
/// differently. This is not supposed to happen since FastCDC
/// has a deterministic lookup table for cut-point judgment, however
/// we want the system to tolerate different chunking behavior because
/// of table changes, for example.
///
/// However, when a chunk is added, all broken `chunkref`s with
/// the same `chunk_hash` _are_ repaired. In other words, by
/// re-uploading a broken NAR you are helping other NARs with
/// the same broken chunk.
#[derive(Debug, Clone, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "nar")]
pub struct Model {
    /// Unique numeric ID of the NAR.
    #[sea_orm(primary_key)]
    pub id: i64,

    /// The state of the NAR archive.
    state: NarState,

    /// The hash of the NAR archive.
    ///
    /// This always begins with "sha256:" with the hash in the
    /// hexadecimal format.
    ///
    /// The global cache may have several NARs with the same NAR
    /// hash:
    ///
    /// - Unconfirmed uploads from clients
    /// - Global deduplication is turned off
    #[sea_orm(indexed)]
    pub nar_hash: String,

    /// The size of the NAR archive.
    pub nar_size: i64,

    /// The type of compression in use.
    #[sea_orm(column_type = "String(StringLen::N(10))")]
    pub compression: String,

    /// Number of chunks that make up this NAR.
    pub num_chunks: i32,

    /// Hint indicating whether all chunks making up this NAR are available.
    ///
    /// This is used by the `get-missing-paths` endpoint to
    /// also return store paths that are inaccessible due to
    /// missing chunks in the associated NARs. They can then be
    /// repaired by any client uploading.
    ///
    /// This flag may be outdated, but it's okay since when a client
    /// tries to upload the same NAR, it will be immediately deduplicated
    /// if all chunks are present and the flag will be updated.
    pub completeness_hint: bool,

    /// Number of processes holding this NAR.
    ///
    /// This is for preventing garbage collection of NARs when
    /// there is a pending upload that can be deduplicated and
    /// there are no existing object references.
    pub holders_count: i32,

    /// Timestamp when the NAR is created.
    pub created_at: ChronoDateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::object::Entity")]
    Object,

    #[sea_orm(has_many = "super::chunkref::Entity")]
    ChunkRef,
}

impl ActiveModelBehavior for ActiveModel {}

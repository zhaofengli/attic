//! A content-addressed NAR in the global cache.

use sea_orm::entity::prelude::*;

use super::Json;
use crate::storage::RemoteFile;

pub type NarModel = Model;

/// The state of a NAR.
#[derive(EnumIter, DeriveActiveEnum, Debug, Clone, PartialEq, Eq)]
#[sea_orm(rs_type = "String", db_type = "String(Some(1))")]
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
    #[sea_orm(string_value = "D")]
    Deleted,
}

/// A content-addressed NAR in the global cache.
///
/// A NAR without `nix-store --export` metadata is context-free,
/// meaning that it's not associated with a store path and only
/// depends on its contents.
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

    /// The hash of the compressed file.
    ///
    /// This always begins with "sha256:" with the hash in the
    /// hexadecimal format.
    ///
    /// This field may not be available if the file hashes aren't
    /// confirmed.
    pub file_hash: Option<String>,

    /// The size of the compressed file.
    ///
    /// This field may not be available if the file hashes aren't
    /// confirmed.
    pub file_size: Option<i64>,

    /// The type of compression in use.
    #[sea_orm(column_type = "String(Some(10))")]
    pub compression: String,

    /// The remote file backing this NAR.
    pub remote_file: Json<RemoteFile>,

    /// Unique string identifying the remote file.
    #[sea_orm(unique)]
    pub remote_file_id: String,

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
}

impl ActiveModelBehavior for ActiveModel {}

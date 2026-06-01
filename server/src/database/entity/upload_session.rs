//! A chunked transport upload session.

use sea_orm::entity::prelude::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UploadSessionState {
    Uploading,
    Finalizing,
    Reaping,
    Completed,
    Aborted,
    Failed,
}

impl UploadSessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uploading => "uploading",
            Self::Finalizing => "finalizing",
            Self::Reaping => "reaping",
            Self::Completed => "completed",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
        }
    }
}

/// A chunked transport upload session.
#[derive(Debug, Clone, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "upload_session")]
pub struct Model {
    /// UUID of the upload session.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,

    /// ID of the cache this upload targets.
    #[sea_orm(indexed)]
    pub cache_id: i64,

    /// Serialized `UploadPathNarInfo`.
    pub upload_info: String,

    /// Number of expected transport parts.
    pub expected_parts: i32,

    /// Session state.
    pub state: String,

    /// User that created the session.
    pub created_by: Option<String>,

    /// Timestamp when the session is created.
    pub created_at: ChronoDateTimeUtc,

    /// Timestamp when the session is last updated.
    pub updated_at: ChronoDateTimeUtc,

    /// Timestamp after which the session can be garbage-collected.
    pub expires_at: ChronoDateTimeUtc,

    /// Serialized `UploadPathResult` when the upload is completed.
    pub result: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::upload_session_part::Entity")]
    UploadSessionPart,

    #[sea_orm(
        belongs_to = "super::cache::Entity",
        from = "Column::CacheId",
        to = "super::cache::Column::Id"
    )]
    Cache,
}

impl Related<super::cache::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Cache.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

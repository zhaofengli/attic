//! A chunked transport upload session part.

use sea_orm::entity::prelude::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UploadSessionPartState {
    Pending,
    Valid,
}

impl UploadSessionPartState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Valid => "valid",
        }
    }
}

/// A chunked transport upload session part.
#[derive(Debug, Clone, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "upload_session_part")]
pub struct Model {
    /// Unique numeric ID of the part row.
    #[sea_orm(primary_key)]
    pub id: i64,

    /// UUID of the upload session.
    #[sea_orm(indexed)]
    pub session_id: String,

    /// Zero-indexed transport part sequence number.
    pub seq: i32,

    /// Upload state of the transport part.
    pub state: String,

    /// Serialized `RemoteFile` backing this transport part.
    pub remote_file: String,

    /// Timestamp when the part is created.
    pub created_at: ChronoDateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::upload_session::Entity",
        from = "Column::SessionId",
        to = "super::upload_session::Column::Id"
    )]
    UploadSession,
}

impl Related<super::upload_session::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::UploadSession.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

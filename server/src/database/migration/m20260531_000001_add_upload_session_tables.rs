use sea_orm_migration::prelude::*;

use crate::database::entity::upload_session::UploadSessionState;
use crate::database::entity::upload_session_part::UploadSessionPartState;
use crate::database::entity::{cache, upload_session, upload_session_part};

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260531_000001_add_upload_session_tables"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(upload_session::Entity)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(upload_session::Column::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(upload_session::Column::CacheId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(upload_session::Column::UploadInfo)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(upload_session::Column::ExpectedParts)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(upload_session::Column::State)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(upload_session::Column::CreatedBy).string())
                    .col(
                        ColumnDef::new(upload_session::Column::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(upload_session::Column::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(upload_session::Column::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(upload_session::Column::Result).text())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-upload-session-cache")
                            .from(upload_session::Entity, upload_session::Column::CacheId)
                            .to(cache::Entity, cache::Column::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .check(upload_session_state_check())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(upload_session_part::Entity)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(upload_session_part::Column::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(upload_session_part::Column::SessionId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(upload_session_part::Column::Seq)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(upload_session_part::Column::State)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(upload_session_part::Column::RemoteFile)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(upload_session_part::Column::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-upload-session-part-session")
                            .from(
                                upload_session_part::Entity,
                                upload_session_part::Column::SessionId,
                            )
                            .to(upload_session::Entity, upload_session::Column::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .check(upload_session_part_state_check())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-upload-session-cache-id")
                    .table(upload_session::Entity)
                    .col(upload_session::Column::CacheId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-upload-session-part-session-seq")
                    .table(upload_session_part::Entity)
                    .col(upload_session_part::Column::SessionId)
                    .col(upload_session_part::Column::Seq)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

fn upload_session_state_check() -> SimpleExpr {
    Expr::col(upload_session::Column::State).is_in([
        UploadSessionState::Uploading.as_str(),
        UploadSessionState::Finalizing.as_str(),
        UploadSessionState::Reaping.as_str(),
        UploadSessionState::Completed.as_str(),
        UploadSessionState::Aborted.as_str(),
        UploadSessionState::Failed.as_str(),
    ])
}

fn upload_session_part_state_check() -> SimpleExpr {
    Expr::col(upload_session_part::Column::State).is_in([
        UploadSessionPartState::Pending.as_str(),
        UploadSessionPartState::Valid.as_str(),
    ])
}

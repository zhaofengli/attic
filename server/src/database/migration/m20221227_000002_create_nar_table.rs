use sea_orm_migration::prelude::*;

use crate::database::entity::nar::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20221227_000003_create_nar_table"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Entity)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Column::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Column::State)
                            .r#char()
                            .char_len(1)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Column::NarHash).string().not_null())
                    .col(ColumnDef::new(Column::NarSize).big_integer().not_null())
                    .col(ColumnDef::new(Column::FileHash).string().null())
                    .col(ColumnDef::new(Column::FileSize).big_integer().null())
                    .col(ColumnDef::new(Column::Compression).string().not_null())
                    .col(ColumnDef::new(Column::RemoteFile).string().not_null())
                    .col(
                        ColumnDef::new(Column::RemoteFileId)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(Column::HoldersCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Column::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-nar-nar-hash")
                    .table(Entity)
                    .col(Column::NarHash)
                    .to_owned(),
            )
            .await
    }
}

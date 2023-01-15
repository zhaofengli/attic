use sea_orm_migration::prelude::*;

use crate::database::entity::chunk::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20230112_000001_add_chunk_table"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Entity)
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
                    .col(ColumnDef::new(Column::ChunkHash).string().not_null())
                    .col(ColumnDef::new(Column::ChunkSize).big_integer().not_null())
                    .col(ColumnDef::new(Alias::new("file_hash")).string().null())
                    .col(ColumnDef::new(Alias::new("file_size")).big_integer().null())
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
                    .name("idx-chunk-chunk-hash")
                    .table(Entity)
                    .col(Column::ChunkHash)
                    .to_owned(),
            )
            .await
    }
}

use sea_orm_migration::prelude::*;

use crate::database::entity::chunk;
use crate::database::entity::chunkref::*;
use crate::database::entity::nar;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20230112_000002_add_chunkref_table"
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
                    .col(ColumnDef::new(Column::NarId).big_integer().not_null())
                    .col(ColumnDef::new(Column::Seq).integer().not_null())
                    .col(ColumnDef::new(Column::ChunkId).big_integer().null())
                    .col(ColumnDef::new(Column::ChunkHash).string().not_null())
                    .col(ColumnDef::new(Column::Compression).string().not_null())
                    .foreign_key(
                        ForeignKeyCreateStatement::new()
                            .name("fk_chunkref_chunk")
                            .from_tbl(Entity)
                            .from_col(Column::ChunkId)
                            .to_tbl(chunk::Entity)
                            .to_col(chunk::Column::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .foreign_key(
                        ForeignKeyCreateStatement::new()
                            .name("fk_chunkref_nar")
                            .from_tbl(Entity)
                            .from_col(Column::NarId)
                            .to_tbl(nar::Entity)
                            .to_col(nar::Column::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-chunk-nar-id")
                    .table(Entity)
                    .col(Column::NarId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-chunk-chunk-id")
                    .table(Entity)
                    .col(Column::ChunkId)
                    .to_owned(),
            )
            .await
    }
}

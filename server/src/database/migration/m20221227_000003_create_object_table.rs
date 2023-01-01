use sea_orm_migration::prelude::*;

use crate::database::entity::cache;
use crate::database::entity::nar;
use crate::database::entity::object::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20221227_000002_create_object_table"
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
                    .col(ColumnDef::new(Column::CacheId).big_integer().not_null())
                    .col(ColumnDef::new(Column::NarId).big_integer().not_null())
                    .col(
                        ColumnDef::new(Column::StorePathHash)
                            .string_len(32)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Column::StorePath).string().not_null())
                    .col(ColumnDef::new(Column::References).string().not_null())
                    .col(ColumnDef::new(Column::System).string())
                    .col(ColumnDef::new(Column::Deriver).string())
                    .col(ColumnDef::new(Column::Sigs).string().not_null())
                    .col(ColumnDef::new(Column::Ca).string())
                    .col(
                        ColumnDef::new(Column::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKeyCreateStatement::new()
                            .name("fk_object_cache")
                            .from_tbl(Entity)
                            .from_col(Column::CacheId)
                            .to_tbl(cache::Entity)
                            .to_col(cache::Column::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKeyCreateStatement::new()
                            .name("fk_object_nar")
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
                    .name("idx-object-cache-hash")
                    .table(Entity)
                    .col(Column::CacheId)
                    .col(Column::StorePathHash)
                    .unique()
                    .to_owned(),
            )
            .await
    }
}

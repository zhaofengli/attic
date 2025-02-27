use sea_orm_migration::prelude::*;

use crate::database::entity::cache;
use crate::database::entity::pin::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20250219_000001_add_pin_table"
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
                    .col(ColumnDef::new(Column::Name).string_len(50).not_null())
                    .col(ColumnDef::new(Column::CacheId).big_integer().not_null())
                    .col(
                        ColumnDef::new(Column::StorePathHash)
                            .string_len(32)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Column::StorePath).string().not_null())
                    .foreign_key(
                        ForeignKeyCreateStatement::new()
                            .name("fk_pin_cache")
                            .from_tbl(Entity)
                            .from_col(Column::CacheId)
                            .to_tbl(cache::Entity)
                            .to_col(cache::Column::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-pin-cache-name")
                    .table(Entity)
                    .col(Column::CacheId)
                    .col(Column::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-pin-cache-name-path")
                    .table(Entity)
                    .col(Column::CacheId)
                    .col(Column::Name)
                    .col(Column::StorePath)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

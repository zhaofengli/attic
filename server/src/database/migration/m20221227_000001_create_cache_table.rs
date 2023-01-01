use sea_orm_migration::prelude::*;

use crate::database::entity::cache::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20221227_000001_create_cache_table"
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
                        ColumnDef::new(Column::Name)
                            .string_len(50)
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Column::Keypair).string().not_null())
                    .col(ColumnDef::new(Column::IsPublic).boolean().not_null())
                    .col(ColumnDef::new(Column::StoreDir).string().not_null())
                    .col(ColumnDef::new(Column::Priority).integer().not_null())
                    .col(
                        ColumnDef::new(Column::UpstreamCacheKeyNames)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Column::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Column::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-cache-name")
                    .table(Entity)
                    .col(Column::Name)
                    .to_owned(),
            )
            .await
    }
}

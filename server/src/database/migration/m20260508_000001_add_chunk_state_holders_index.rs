use sea_orm_migration::prelude::*;

use crate::database::entity::chunk::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260508_000001_add_chunk_state_holders_index"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx-chunk-state-holders")
                    .table(Entity)
                    .col(Column::State)
                    .col(Column::HoldersCount)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx-chunk-state-holders")
                    .table(Entity)
                    .to_owned(),
            )
            .await
    }
}

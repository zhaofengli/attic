use sea_orm_migration::prelude::*;

use crate::database::entity::{chunkref, nar};

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260624_000001_remove_chunk_recovery"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in ["chunk_hash", "compression"] {
            manager
                .alter_table(
                    Table::alter()
                        .table(chunkref::Entity)
                        .drop_column(Alias::new(column))
                        .to_owned(),
                )
                .await?;
        }

        manager
            .alter_table(
                Table::alter()
                    .table(nar::Entity)
                    .drop_column(Alias::new("completeness_hint"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

use sea_orm_migration::prelude::*;

use crate::database::entity::cache::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20221227_000005_add_cache_retention_period"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Entity)
                    .add_column(ColumnDef::new(Column::RetentionPeriod).integer().null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

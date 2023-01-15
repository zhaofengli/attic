use sea_orm_migration::prelude::*;

use crate::database::entity::nar::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20230112_000003_add_nar_num_chunks"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Entity)
                    .add_column(
                        ColumnDef::new(Column::NumChunks)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

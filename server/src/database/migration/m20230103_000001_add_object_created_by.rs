use sea_orm_migration::prelude::*;

use crate::database::entity::object::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20230103_000001_add_object_created_by"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Entity)
                    .add_column(ColumnDef::new(Column::CreatedBy).string().null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

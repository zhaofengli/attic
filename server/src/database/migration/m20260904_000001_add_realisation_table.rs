use sea_orm_migration::prelude::*;

use crate::database::entity::cache;
use crate::database::entity::realisation::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260904_000001_add_realisation_table"
    }
}

// Nix ca-derivations discover cached results through realisations: a mapping
// from a resolved derivation output id (e.g. `sha256:<hex>!out`) to the
// output path it produced. Substituters are queried at
// `/{cache}/realisations/{id}.doi`. Attic didn't store or serve these
// (upstream issue #188), so CA-derivation outputs could never be reused
// through it. This table is the storage side of fixing that.
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
                    .col(ColumnDef::new(Column::DrvOutputId).string().not_null())
                    .col(ColumnDef::new(Column::Data).text().not_null())
                    .col(
                        ColumnDef::new(Column::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Column::CreatedBy).string().null())
                    .foreign_key(
                        ForeignKeyCreateStatement::new()
                            .name("fk_realisation_cache")
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
                    .name("idx-realisation-cache-drv-output")
                    .table(Entity)
                    .col(Column::CacheId)
                    .col(Column::DrvOutputId)
                    .unique()
                    .to_owned(),
            )
            .await
    }
}

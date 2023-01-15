use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use sea_orm_migration::prelude::*;

use crate::database::entity::nar::{self, *};

pub struct Migration;

const TEMP_NAR_TABLE: &str = "nar_new";

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20230112_000005_drop_old_nar_columns"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        eprintln!("* Migrating NAR schema...");

        if manager.get_database_backend() == DatabaseBackend::Sqlite {
            // Just copy all data to a new table
            manager
                .get_connection()
                .execute(Statement::from_string(
                    manager.get_database_backend(),
                    "PRAGMA foreign_keys = OFF".to_owned(),
                ))
                .await?;

            manager
                .create_table(
                    Table::create()
                        .table(Alias::new(TEMP_NAR_TABLE))
                        .if_not_exists()
                        .col(
                            ColumnDef::new(Column::Id)
                                .big_integer()
                                .not_null()
                                .auto_increment()
                                .primary_key(),
                        )
                        .col(
                            ColumnDef::new(Column::State)
                                .r#char()
                                .char_len(1)
                                .not_null(),
                        )
                        .col(ColumnDef::new(Column::NarHash).string().not_null())
                        .col(ColumnDef::new(Column::NarSize).big_integer().not_null())
                        .col(ColumnDef::new(Column::Compression).string().not_null())
                        .col(
                            ColumnDef::new(Column::NumChunks)
                                .integer()
                                .not_null()
                                .default(1),
                        )
                        .col(
                            ColumnDef::new(Column::HoldersCount)
                                .integer()
                                .not_null()
                                .default(0),
                        )
                        .col(
                            ColumnDef::new(Column::CreatedAt)
                                .timestamp_with_time_zone()
                                .not_null(),
                        )
                        .to_owned(),
                )
                .await?;

            let columns = [
                nar::Column::Id.into_iden(),
                nar::Column::State.into_iden(),
                nar::Column::NarHash.into_iden(),
                nar::Column::NarSize.into_iden(),
                nar::Column::Compression.into_iden(),
                nar::Column::NumChunks.into_iden(),
                nar::Column::HoldersCount.into_iden(),
                nar::Column::CreatedAt.into_iden(),
            ];

            let select_nar = Query::select()
                .from(nar::Entity)
                .columns(columns.clone())
                .to_owned();

            let insertion = Query::insert()
                .into_table(Alias::new(TEMP_NAR_TABLE))
                .columns(columns.clone())
                .select_from(select_nar)
                .unwrap()
                .to_owned();

            let insertion_stmt = manager.get_database_backend().build(&insertion);
            manager.get_connection().execute(insertion_stmt).await?;

            manager
                .drop_table(Table::drop().table(nar::Entity).to_owned())
                .await?;

            manager
                .rename_table(
                    Table::rename()
                        .table(Alias::new(TEMP_NAR_TABLE), nar::Entity)
                        .to_owned(),
                )
                .await?;

            manager
                .get_connection()
                .execute(Statement::from_string(
                    manager.get_database_backend(),
                    "PRAGMA foreign_keys = ON".to_owned(),
                ))
                .await?;
        } else {
            // Just drop the columns
            manager
                .alter_table(
                    Table::alter()
                        .table(nar::Entity)
                        .drop_column(Alias::new("file_hash"))
                        .to_owned(),
                )
                .await?;

            manager
                .alter_table(
                    Table::alter()
                        .table(nar::Entity)
                        .drop_column(Alias::new("file_size"))
                        .to_owned(),
                )
                .await?;

            manager
                .alter_table(
                    Table::alter()
                        .table(nar::Entity)
                        .drop_column(Alias::new("remote_file"))
                        .to_owned(),
                )
                .await?;

            manager
                .alter_table(
                    Table::alter()
                        .table(nar::Entity)
                        .drop_column(Alias::new("remote_file_id"))
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}

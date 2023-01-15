use sea_orm::{ConnectionTrait, TransactionTrait};
use sea_orm_migration::prelude::*;

use crate::database::entity::chunk;
use crate::database::entity::chunkref;
use crate::database::entity::nar;

pub struct Migration;

pub enum TempChunkCols {
    /// The ID of the NAR.
    NarId,
}

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20230112_000004_migrate_nar_remote_files_to_chunks"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // When this migration is run, we assume that there are no
        // preexisting chunks.

        eprintln!("* Migrating NARs to chunks...");

        // Add a temporary column into `chunk` to store the related `nar_id`.
        manager
            .alter_table(
                Table::alter()
                    .table(chunk::Entity)
                    .add_column_if_not_exists(
                        ColumnDef::new(TempChunkCols::NarId).integer().not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Get the original values from NARs
        let select_remote_file = Query::select()
            .from(nar::Entity)
            .columns([
                nar::Column::Id.into_iden(),
                Alias::new("remote_file").into_iden(),
                Alias::new("remote_file_id").into_iden(),
                nar::Column::NarHash.into_iden(),
                nar::Column::NarSize.into_iden(),
                Alias::new("file_hash").into_iden(),
                Alias::new("file_size").into_iden(),
                nar::Column::Compression.into_iden(),
                nar::Column::CreatedAt.into_iden(),
            ])
            .expr_as(chunk::ChunkState::Valid, chunk::Column::State.into_iden())
            .to_owned();

        // ... insert them into the `chunk` table
        let insert_chunk = Query::insert()
            .into_table(chunk::Entity)
            .columns([
                TempChunkCols::NarId.into_iden(),
                chunk::Column::RemoteFile.into_iden(),
                chunk::Column::RemoteFileId.into_iden(),
                chunk::Column::ChunkHash.into_iden(),
                chunk::Column::ChunkSize.into_iden(),
                chunk::Column::FileHash.into_iden(),
                chunk::Column::FileSize.into_iden(),
                chunk::Column::Compression.into_iden(),
                chunk::Column::CreatedAt.into_iden(),
                chunk::Column::State.into_iden(),
            ])
            .select_from(select_remote_file)
            .unwrap()
            .returning(Query::returning().columns([
                chunk::Column::Id.into_column_ref(),
                TempChunkCols::NarId.into_column_ref(),
            ]))
            .to_owned();

        let insert_chunk_stmt = manager.get_database_backend().build(&insert_chunk);

        // ... then create chunkrefs binding the chunks and original NARs
        let select_chunk = Query::select()
            .from(chunk::Entity)
            .columns([
                chunk::Column::Id.into_iden(),
                TempChunkCols::NarId.into_iden(),
                chunk::Column::ChunkHash.into_iden(),
                chunk::Column::Compression.into_iden(),
            ])
            .expr_as(0, chunkref::Column::Seq.into_iden())
            .to_owned();

        let insert_chunkref = Query::insert()
            .into_table(chunkref::Entity)
            .columns([
                chunkref::Column::ChunkId.into_iden(),
                chunkref::Column::NarId.into_iden(),
                chunkref::Column::ChunkHash.into_iden(),
                chunkref::Column::Compression.into_iden(),
                chunkref::Column::Seq.into_iden(),
            ])
            .select_from(select_chunk)
            .unwrap()
            .returning(Query::returning().columns([chunkref::Column::Id.into_column_ref()]))
            .to_owned();

        let insert_chunkref_stmt = manager.get_database_backend().build(&insert_chunkref);

        // Actually run the migration
        let txn = manager.get_connection().begin().await?;
        txn.execute(insert_chunk_stmt).await?;
        txn.execute(insert_chunkref_stmt).await?;
        txn.commit().await?;

        // Finally, drop the temporary column
        manager
            .alter_table(
                Table::alter()
                    .table(chunk::Entity)
                    .drop_column(TempChunkCols::NarId)
                    .to_owned(),
            )
            .await?;

        // We will drop the unused columns in `nar` in the next migration
        Ok(())
    }
}

impl Iden for TempChunkCols {
    fn unquoted(&self, s: &mut dyn std::fmt::Write) {
        write!(
            s,
            "{}",
            match self {
                Self::NarId => "temp_nar_id",
            }
        )
        .unwrap();
    }
}

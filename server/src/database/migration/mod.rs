//! Database migrations.

pub use sea_orm_migration::*;

mod m20221227_000001_create_cache_table;
mod m20221227_000002_create_nar_table;
mod m20221227_000003_create_object_table;
mod m20221227_000004_add_object_last_accessed;
mod m20221227_000005_add_cache_retention_period;
mod m20230103_000001_add_object_created_by;
mod m20230112_000001_add_chunk_table;
mod m20230112_000002_add_chunkref_table;
mod m20230112_000003_add_nar_num_chunks;
mod m20230112_000004_migrate_nar_remote_files_to_chunks;
mod m20230112_000005_drop_old_nar_columns;
mod m20230112_000006_add_nar_completeness_hint;
mod m20260508_000001_add_chunk_state_holders_index;
mod m20260531_000001_add_upload_session_tables;
mod m20260611_000001_add_nar_state_holders_index;
mod m20260624_000001_remove_chunk_recovery;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20221227_000001_create_cache_table::Migration),
            Box::new(m20221227_000002_create_nar_table::Migration),
            Box::new(m20221227_000003_create_object_table::Migration),
            Box::new(m20221227_000004_add_object_last_accessed::Migration),
            Box::new(m20221227_000005_add_cache_retention_period::Migration),
            Box::new(m20230103_000001_add_object_created_by::Migration),
            Box::new(m20230112_000001_add_chunk_table::Migration),
            Box::new(m20230112_000002_add_chunkref_table::Migration),
            Box::new(m20230112_000003_add_nar_num_chunks::Migration),
            Box::new(m20230112_000004_migrate_nar_remote_files_to_chunks::Migration),
            Box::new(m20230112_000005_drop_old_nar_columns::Migration),
            Box::new(m20230112_000006_add_nar_completeness_hint::Migration),
            Box::new(m20260508_000001_add_chunk_state_holders_index::Migration),
            Box::new(m20260611_000001_add_nar_state_holders_index::Migration),
            Box::new(m20260531_000001_add_upload_session_tables::Migration),
            Box::new(m20260624_000001_remove_chunk_recovery::Migration),
        ]
    }
}

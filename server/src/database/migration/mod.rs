//! Database migrations.

pub use sea_orm_migration::*;

mod m20221227_000001_create_cache_table;
mod m20221227_000002_create_nar_table;
mod m20221227_000003_create_object_table;
mod m20221227_000004_add_object_last_accessed;
mod m20221227_000005_add_cache_retention_period;
mod m20230103_000001_add_object_created_by;

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
        ]
    }
}

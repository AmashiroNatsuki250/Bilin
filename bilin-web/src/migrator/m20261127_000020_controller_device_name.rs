use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        let exists = connection
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT 1 FROM pragma_table_info('controller_devices') WHERE name = 'display_name'",
            ))
            .await?
            .is_some();
        if !exists {
            connection
                .execute_unprepared("ALTER TABLE controller_devices ADD COLUMN display_name TEXT")
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE controller_devices DROP COLUMN display_name")
            .await?;
        Ok(())
    }
}

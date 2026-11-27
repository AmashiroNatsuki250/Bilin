use sea_orm::TransactionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        let transaction = connection.begin().await?;
        transaction
            .execute_unprepared(
                "ALTER TABLE controller_devices ADD COLUMN device_code TEXT CHECK (
                length(device_code) = 8 AND device_code NOT GLOB '*[^0-9a-z]*'
                AND device_code GLOB '*[0-9]*' AND device_code GLOB '*[a-z]*');
             ALTER TABLE controller_devices ADD COLUMN deleted_at INTEGER;
             CREATE UNIQUE INDEX controller_devices_code ON controller_devices(device_code);",
            )
            .await?;
        let ids = transaction
            .query_all(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT device_id FROM controller_devices",
            ))
            .await?;
        for row in ids {
            let id: String = row.try_get("", "device_id")?;
            loop {
                let code = crate::db::controller_devices::new_device_code();
                let updated = transaction.execute(sea_orm::Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Sqlite,
                    "UPDATE OR IGNORE controller_devices SET device_code = ? WHERE device_id = ?",
                    [code.into(), id.clone().into()],
                )).await?;
                if updated.rows_affected() != 0 {
                    break;
                }
            }
        }
        transaction.commit().await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP INDEX controller_devices_code;
             ALTER TABLE controller_devices DROP COLUMN device_code;
             ALTER TABLE controller_devices DROP COLUMN deleted_at;",
            )
            .await?;
        Ok(())
    }
}

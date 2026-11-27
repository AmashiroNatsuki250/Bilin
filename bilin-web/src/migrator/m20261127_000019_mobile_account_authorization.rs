use sea_orm::TransactionTrait as _;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let tx = manager.get_connection().begin().await?;
        tx.execute_unprepared(
            "ALTER TABLE controller_devices ADD COLUMN mobile_account_authorized_revision INTEGER;
             ALTER TABLE controller_devices ADD COLUMN mobile_selected_network_id TEXT;",
        )
        .await?;
        tx.commit().await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let tx = manager.get_connection().begin().await?;
        tx.execute_unprepared(
            "ALTER TABLE controller_devices DROP COLUMN mobile_selected_network_id;
             ALTER TABLE controller_devices DROP COLUMN mobile_account_authorized_revision;",
        )
        .await?;
        tx.commit().await
    }
}

use sea_orm::TransactionTrait as _;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let transaction = manager.get_connection().begin().await?;
        transaction
            .execute_unprepared(
                "ALTER TABLE users ADD COLUMN banned INTEGER NOT NULL DEFAULT 0 CHECK (banned IN (0, 1));
                 ALTER TABLE users ADD COLUMN udp_enabled INTEGER NOT NULL DEFAULT 0 CHECK (udp_enabled IN (0, 1));
                 ALTER TABLE users ADD COLUMN force_relay INTEGER NOT NULL DEFAULT 0 CHECK (force_relay IN (0, 1));
                 ALTER TABLE controller_devices ADD COLUMN account_transport_policy_supported INTEGER NOT NULL DEFAULT 0
                     CHECK (account_transport_policy_supported IN (0, 1));",
            )
            .await?;
        transaction.commit().await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let transaction = manager.get_connection().begin().await?;
        transaction
            .execute_unprepared(
                "ALTER TABLE controller_devices DROP COLUMN account_transport_policy_supported;
                 ALTER TABLE users DROP COLUMN force_relay;
                 ALTER TABLE users DROP COLUMN udp_enabled;
                 ALTER TABLE users DROP COLUMN banned;",
            )
            .await?;
        transaction.commit().await
    }
}

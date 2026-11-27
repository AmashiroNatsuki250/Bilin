use sea_orm::TransactionTrait as _;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let transaction = manager.get_connection().begin().await?;
        transaction.execute_unprepared(
            "ALTER TABLE users ADD COLUMN forced_protocol TEXT NOT NULL DEFAULT 'auto'
                 CHECK (forced_protocol IN ('auto', 'udp', 'kcp', 'quic', 'faketcp', 'tcp', 'wss')
                        AND (NOT force_relay OR forced_protocol = 'auto'));
             ALTER TABLE controller_devices ADD COLUMN extended_transports_supported INTEGER NOT NULL DEFAULT 0
                 CHECK (extended_transports_supported IN (0, 1));",
        ).await?;
        transaction.commit().await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let transaction = manager.get_connection().begin().await?;
        transaction
            .execute_unprepared(
                "ALTER TABLE controller_devices DROP COLUMN extended_transports_supported;
             ALTER TABLE users DROP COLUMN forced_protocol;",
            )
            .await?;
        transaction.commit().await
    }
}

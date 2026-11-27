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
                "ALTER TABLE controller_devices ADD COLUMN upstream_subnet_cidr TEXT;
                 ALTER TABLE controller_devices ADD COLUMN upstream_subnet_applied INTEGER NOT NULL DEFAULT 0
                     CHECK (upstream_subnet_applied IN (0, 1));",
            )
            .await?;
        transaction.commit().await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let transaction = manager.get_connection().begin().await?;
        transaction
            .execute_unprepared(
                "ALTER TABLE controller_devices DROP COLUMN upstream_subnet_applied;
                 ALTER TABLE controller_devices DROP COLUMN upstream_subnet_cidr;",
            )
            .await?;
        transaction.commit().await
    }
}

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
                "UPDATE users SET forced_protocol = 'auto' WHERE forced_protocol = 'faketcp';
                 CREATE TRIGGER users_reject_retired_protocol_insert
                 BEFORE INSERT ON users WHEN NEW.forced_protocol = 'faketcp'
                 BEGIN
                     SELECT RAISE(ABORT, 'unsupported forced protocol');
                 END;
                 CREATE TRIGGER users_reject_retired_protocol_update
                 BEFORE UPDATE OF forced_protocol ON users WHEN NEW.forced_protocol = 'faketcp'
                 BEGIN
                     SELECT RAISE(ABORT, 'unsupported forced protocol');
                 END;",
            )
            .await?;
        transaction.commit().await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let transaction = manager.get_connection().begin().await?;
        transaction
            .execute_unprepared(
                "DROP TRIGGER users_reject_retired_protocol_insert;
                 DROP TRIGGER users_reject_retired_protocol_update;",
            )
            .await?;
        transaction.commit().await
    }
}

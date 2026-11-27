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
                r#"
                ALTER TABLE controller_devices ADD COLUMN device_login_revision INTEGER NOT NULL DEFAULT 1;
                CREATE TABLE controller_device_login_tickets (
                    device_id TEXT PRIMARY KEY NOT NULL REFERENCES controller_devices(device_id) ON DELETE CASCADE,
                    ticket_digest TEXT NOT NULL UNIQUE,
                    expires_at INTEGER NOT NULL
                );
                CREATE INDEX controller_device_login_tickets_expiry ON controller_device_login_tickets (expires_at);
                CREATE TRIGGER controller_device_login_registration_changed
                AFTER UPDATE OF owner_id, deleted_at ON controller_devices
                WHEN OLD.owner_id IS NOT NEW.owner_id OR OLD.deleted_at IS NOT NEW.deleted_at
                BEGIN
                    DELETE FROM controller_device_login_tickets WHERE device_id = NEW.device_id;
                    UPDATE controller_devices SET device_login_revision = device_login_revision + 1
                    WHERE device_id = NEW.device_id AND
                        ((OLD.owner_id IS NOT NULL AND NEW.owner_id IS NULL) OR
                         (OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL));
                END;
                "#,
            )
            .await?;
        transaction.commit().await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let transaction = manager.get_connection().begin().await?;
        transaction
            .execute_unprepared(
                r#"
                DROP TRIGGER controller_device_login_registration_changed;
                DROP TABLE controller_device_login_tickets;
                ALTER TABLE controller_devices DROP COLUMN device_login_revision;
                "#,
            )
            .await?;
        transaction.commit().await
    }
}

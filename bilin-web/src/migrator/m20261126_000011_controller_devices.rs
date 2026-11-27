use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            CREATE TABLE controller_devices (
                device_id TEXT PRIMARY KEY NOT NULL,
                identity_digest TEXT NOT NULL,
                config_token_digest TEXT NOT NULL UNIQUE,
                pairing_digest TEXT,
                pairing_expires_at INTEGER,
                owner_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
                hostname TEXT NOT NULL,
                version TEXT NOT NULL,
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL
            );
            CREATE INDEX controller_devices_owner ON controller_devices(owner_id, last_seen);
            "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE controller_devices")
            .await?;
        Ok(())
    }
}

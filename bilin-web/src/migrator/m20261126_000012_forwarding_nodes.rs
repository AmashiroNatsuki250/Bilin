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
            CREATE TABLE forwarding_nodes (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                peer_url TEXT NOT NULL UNIQUE,
                token_digest TEXT NOT NULL UNIQUE,
                last_seen INTEGER,
                ready INTEGER NOT NULL DEFAULT 0
            );
            "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE forwarding_nodes")
            .await?;
        Ok(())
    }
}

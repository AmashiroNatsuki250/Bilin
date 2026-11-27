use std::collections::BTreeMap;

use sea_orm::DbErr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Db, UserIdInDb, sqlx_db_error};

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub(crate) struct AccountPolicy {
    pub banned: bool,
    pub udp_enabled: bool,
    pub force_relay: bool,
    pub forced_protocol: String,
}

impl Default for AccountPolicy {
    fn default() -> Self {
        Self {
            banned: false,
            udp_enabled: false,
            force_relay: false,
            forced_protocol: "auto".into(),
        }
    }
}

impl AccountPolicy {
    pub fn smart_enabled(&self) -> bool {
        !self.force_relay && self.forced_protocol == "auto" && self.udp_enabled
    }

    pub fn selected_protocol(&self) -> Option<&str> {
        (self.forced_protocol != "auto").then_some(self.forced_protocol.as_str())
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ProtocolChoice {
    Auto,
    Udp,
    Kcp,
    Quic,
    Tcp,
    Wss,
}

impl ProtocolChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Udp => "udp",
            Self::Kcp => "kcp",
            Self::Quic => "quic",
            Self::Tcp => "tcp",
            Self::Wss => "wss",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountPolicyPatch {
    pub banned: Option<bool>,
    pub udp_enabled: Option<bool>,
    pub force_relay: Option<bool>,
    pub forced_protocol: Option<ProtocolChoice>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub(crate) struct PlatformUser {
    pub id: UserIdInDb,
    pub username: String,
    pub banned: bool,
    pub udp_enabled: bool,
    pub force_relay: bool,
    pub forced_protocol: String,
}

pub(crate) struct DeletedPlatformUser {
    pub network_ids: Vec<Uuid>,
    pub device_ids: Vec<Uuid>,
    pub managed_instances: BTreeMap<Uuid, Vec<Uuid>>,
}

pub(crate) enum PlatformUserDeletion {
    Forbidden,
    Missing,
    Deleted(DeletedPlatformUser),
}

const IS_PLATFORM_ADMIN: &str = "SELECT EXISTS(SELECT 1 FROM users_groups ug
    JOIN groups g ON g.id = ug.group_id JOIN users u ON u.id = ug.user_id
    WHERE ug.user_id = ? AND g.name = 'admins' AND u.banned = 0)";

impl Db {
    pub(crate) async fn is_platform_admin(&self, user_id: UserIdInDb) -> Result<bool, DbErr> {
        sqlx::query_scalar(IS_PLATFORM_ADMIN)
            .bind(user_id)
            .fetch_one(&self.db)
            .await
            .map_err(sqlx_db_error)
    }

    pub(crate) async fn account_policy(
        &self,
        user_id: UserIdInDb,
    ) -> Result<Option<AccountPolicy>, DbErr> {
        sqlx::query_as(
            "SELECT banned, udp_enabled, force_relay, forced_protocol FROM users WHERE id = ?",
        )
        .bind(user_id)
        .fetch_optional(&self.inner())
        .await
        .map_err(sqlx_db_error)
    }

    pub(crate) async fn list_platform_users(&self) -> Result<Vec<PlatformUser>, DbErr> {
        sqlx::query_as(
            "SELECT id, username, banned, udp_enabled, force_relay, forced_protocol FROM users ORDER BY username",
        )
        .fetch_all(&self.inner())
        .await
        .map_err(sqlx_db_error)
    }

    pub(crate) async fn update_account_policy(
        &self,
        user_id: UserIdInDb,
        patch: AccountPolicyPatch,
    ) -> Result<Option<PlatformUser>, DbErr> {
        sqlx::query_as(
            "UPDATE users
             SET banned = COALESCE(?, banned), udp_enabled = COALESCE(?, udp_enabled),
                 force_relay = COALESCE(?, force_relay), forced_protocol = COALESCE(?, forced_protocol)
             WHERE id = ?
             RETURNING id, username, banned, udp_enabled, force_relay, forced_protocol",
        )
        .bind(patch.banned)
        .bind(patch.udp_enabled)
        .bind(patch.force_relay)
        .bind(patch.forced_protocol.map(ProtocolChoice::as_str))
        .bind(user_id)
        .fetch_optional(&self.inner())
        .await
        .map_err(sqlx_db_error)
    }

    pub(crate) async fn delete_platform_user(
        &self,
        administrator: UserIdInDb,
        user_id: UserIdInDb,
    ) -> Result<PlatformUserDeletion, DbErr> {
        let mut transaction = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;

        let administrator_is_active: bool = sqlx::query_scalar(IS_PLATFORM_ADMIN)
            .bind(administrator)
            .fetch_one(&mut *transaction)
            .await
            .map_err(sqlx_db_error)?;
        if administrator == user_id || !administrator_is_active {
            transaction.rollback().await.map_err(sqlx_db_error)?;
            return Ok(PlatformUserDeletion::Forbidden);
        }

        let network_ids =
            sqlx::query_scalar::<_, String>("SELECT id FROM networks WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&mut *transaction)
                .await
                .map_err(sqlx_db_error)?
                .into_iter()
                .map(|id| stored_uuid(&id))
                .collect::<Result<Vec<_>, _>>()?;
        let device_ids = sqlx::query_scalar::<_, String>(
            "UPDATE controller_devices SET owner_id = NULL, pairing_expires_at = NULL,
                 upstream_subnet_cidr = NULL, upstream_subnet_applied = 0
             WHERE owner_id = ? RETURNING device_id",
        )
        .bind(user_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(sqlx_db_error)?
        .into_iter()
        .map(|id| stored_uuid(&id))
        .collect::<Result<Vec<_>, _>>()?;
        let mut managed_instances = BTreeMap::<_, Vec<_>>::new();
        for (device_id, instance_id) in sqlx::query_as::<_, (String, String)>(
            "SELECT device_id, network_instance_id FROM user_running_network_configs
             WHERE user_id = ? AND source = 'web'
             UNION SELECT device_id, network_id FROM network_members WHERE user_id = ?",
        )
        .bind(user_id)
        .bind(user_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(sqlx_db_error)?
        {
            managed_instances
                .entry(stored_uuid(&device_id)?)
                .or_default()
                .push(stored_uuid(&instance_id)?);
        }

        // Credentials still referenced by a member are protected by ON DELETE RESTRICT.
        sqlx::query("DELETE FROM network_members WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_db_error)?;
        let deleted = sqlx::query("DELETE FROM users WHERE id = ? AND username <> 'admin'")
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_db_error)?;
        if deleted.rows_affected() == 0 {
            transaction.rollback().await.map_err(sqlx_db_error)?;
            return Ok(PlatformUserDeletion::Missing);
        }
        transaction.commit().await.map_err(sqlx_db_error)?;
        Ok(PlatformUserDeletion::Deleted(DeletedPlatformUser {
            network_ids,
            device_ids,
            managed_instances,
        }))
    }
}

fn stored_uuid(value: &str) -> Result<Uuid, DbErr> {
    Uuid::parse_str(value)
        .map_err(|error| DbErr::Custom(format!("Invalid stored identifier: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_transport_patch_rejects_retired_protocol() {
        assert!(
            serde_json::from_str::<AccountPolicyPatch>(r#"{"forced_protocol":"faketcp"}"#).is_err()
        );
    }

    #[tokio::test]
    async fn account_policy_defaults_to_tcp_and_updates_only_supplied_flags() {
        let db = Db::memory_db().await;
        let admin = db.get_user_id("admin").await.unwrap().unwrap();
        let user = db.auto_create_user("account-policy-default").await.unwrap();

        for id in [admin, user.id] {
            assert_eq!(
                db.account_policy(id).await.unwrap(),
                Some(AccountPolicy::default())
            );
        }

        db.update_account_policy(
            user.id,
            AccountPolicyPatch {
                udp_enabled: Some(true),
                force_relay: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
        db.update_account_policy(
            user.id,
            AccountPolicyPatch {
                banned: Some(true),
                udp_enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(
            db.account_policy(user.id).await.unwrap(),
            Some(AccountPolicy {
                banned: true,
                udp_enabled: false,
                force_relay: true,
                ..Default::default()
            })
        );
        assert_eq!(
            db.account_policy(admin).await.unwrap(),
            Some(AccountPolicy::default())
        );
    }
}

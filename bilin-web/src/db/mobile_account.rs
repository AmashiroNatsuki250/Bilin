use std::collections::HashMap;

use sea_orm::DbErr;
use sqlx::{Sqlite, Transaction};
use uuid::Uuid;

use super::{
    Db, UserIdInDb,
    controller_devices::{DEVICE_ONLINE_SECONDS, bind_controller_device_in_transaction, digest},
    sqlx_db_error,
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MobileAccountDevice {
    pub owner_id: UserIdInDb,
    pub username: String,
    pub device_code: String,
    pub selected_network_id: Option<Uuid>,
}

#[derive(sqlx::FromRow)]
pub(crate) struct MobileNetworkMember {
    pub device_id: String,
    pub hostname: String,
    pub virtual_ipv4: Option<String>,
    pub allocated_ipv4: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum MobileAccountError {
    #[error(transparent)]
    Database(#[from] DbErr),
    #[error("device identity does not match")]
    InvalidIdentity,
    #[error("device not registered")]
    MissingDevice,
    #[error("device account authorization is required")]
    SignInRequired,
    #[error("device registration changed")]
    RegistrationChanged,
    #[error("device already bound to a different account")]
    BoundElsewhere,
    #[error("account suspended")]
    AccountUnavailable,
}

#[derive(sqlx::FromRow)]
struct MobileRegistration {
    identity_matches: bool,
    owner_id: Option<UserIdInDb>,
    deleted_at: Option<i64>,
    last_seen: i64,
    device_login_revision: i64,
    device_code: String,
    mobile_account_authorized_revision: Option<i64>,
    selected_network_id: Option<String>,
}

async fn get_registration(
    tx: &mut Transaction<'_, Sqlite>,
    device_id: Uuid,
    identity: &str,
    revision: i64,
) -> Result<MobileRegistration, MobileAccountError> {
    let registration = sqlx::query_as::<_, MobileRegistration>(
        "SELECT d.identity_digest = ? AS identity_matches,
                d.owner_id, d.deleted_at, d.last_seen, d.device_login_revision, d.device_code,
                d.mobile_account_authorized_revision,
                (SELECT n.id FROM network_members m JOIN networks n
                   ON n.user_id = m.user_id AND n.id = m.network_id
                 WHERE m.user_id = d.owner_id AND m.device_id = d.device_id
                   AND n.id = d.mobile_selected_network_id
                   AND n.networking_method = 'Gateway' AND n.secure_mode) AS selected_network_id
         FROM controller_devices d WHERE d.device_id = ?",
    )
    .bind(digest(identity))
    .bind(device_id.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(sqlx_db_error)?
    .ok_or(MobileAccountError::MissingDevice)?;
    if !registration.identity_matches {
        return Err(MobileAccountError::InvalidIdentity);
    }
    if registration.deleted_at.is_some() {
        return Err(MobileAccountError::MissingDevice);
    }
    if registration.device_login_revision != revision {
        return Err(MobileAccountError::RegistrationChanged);
    }
    Ok(registration)
}

async fn account_name(
    tx: &mut Transaction<'_, Sqlite>,
    owner_id: UserIdInDb,
    session_password_hash: Option<&str>,
) -> Result<String, MobileAccountError> {
    sqlx::query_scalar(
        "SELECT username FROM users WHERE id = ? AND NOT banned
           AND (? IS NULL OR password = ?)",
    )
    .bind(owner_id)
    .bind(session_password_hash)
    .bind(session_password_hash)
    .fetch_optional(&mut **tx)
    .await
    .map_err(sqlx_db_error)?
    .ok_or(MobileAccountError::AccountUnavailable)
}

fn optional_network_id(value: Option<&str>) -> Result<Option<Uuid>, DbErr> {
    value
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| DbErr::Custom("invalid stored mobile network identifier".into()))
}

impl Db {
    pub(crate) async fn authorize_mobile_account(
        &self,
        device_id: Uuid,
        identity: &str,
        revision: i64,
        owner_id: UserIdInDb,
        session_password_hash: &str,
    ) -> Result<(MobileAccountDevice, bool), MobileAccountError> {
        let mut tx = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;
        let username = account_name(&mut tx, owner_id, Some(session_password_hash)).await?;
        let registration = get_registration(&mut tx, device_id, identity, revision).await?;
        if registration.owner_id.is_some_and(|owner| owner != owner_id) {
            return Err(MobileAccountError::BoundElsewhere);
        }
        if chrono::Utc::now().timestamp() - registration.last_seen > DEVICE_ONLINE_SECONDS {
            return Err(MobileAccountError::MissingDevice);
        }
        if registration.owner_id.is_none()
            && !bind_controller_device_in_transaction(&mut tx, device_id, owner_id, None).await?
        {
            return Err(MobileAccountError::MissingDevice);
        }
        let previously_authorized =
            registration.mobile_account_authorized_revision == Some(revision);
        if !previously_authorized {
            sqlx::query(
                "UPDATE controller_devices SET mobile_account_authorized_revision = ?,
                    mobile_selected_network_id = NULL WHERE device_id = ?",
            )
            .bind(revision)
            .bind(device_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(sqlx_db_error)?;
        }
        tx.commit().await.map_err(sqlx_db_error)?;
        let selected_network_id = if previously_authorized {
            optional_network_id(registration.selected_network_id.as_deref())?
        } else {
            None
        };
        Ok((
            MobileAccountDevice {
                owner_id,
                username,
                device_code: registration.device_code,
                selected_network_id,
            },
            !previously_authorized,
        ))
    }

    pub(crate) async fn mobile_account_for_device(
        &self,
        device_id: Uuid,
        identity: &str,
        revision: i64,
    ) -> Result<MobileAccountDevice, MobileAccountError> {
        let mut tx = self.db.begin().await.map_err(sqlx_db_error)?;
        let registration = get_registration(&mut tx, device_id, identity, revision).await?;
        let owner_id = registration
            .owner_id
            .ok_or(MobileAccountError::SignInRequired)?;
        if registration.mobile_account_authorized_revision != Some(revision) {
            return Err(MobileAccountError::SignInRequired);
        }
        let username = account_name(&mut tx, owner_id, None).await?;
        tx.commit().await.map_err(sqlx_db_error)?;
        Ok(MobileAccountDevice {
            owner_id,
            username,
            device_code: registration.device_code,
            selected_network_id: optional_network_id(registration.selected_network_id.as_deref())?,
        })
    }

    pub(crate) async fn mobile_network_selections(
        &self,
        user_id: UserIdInDb,
    ) -> Result<HashMap<Uuid, Option<Uuid>>, DbErr> {
        let rows = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT d.device_id, n.id FROM controller_devices d
             LEFT JOIN network_members m ON m.user_id = d.owner_id AND m.device_id = d.device_id
               AND m.network_id = d.mobile_selected_network_id
             LEFT JOIN networks n ON n.user_id = m.user_id AND n.id = m.network_id
               AND n.networking_method = 'Gateway' AND n.secure_mode
             WHERE d.owner_id = ? AND d.deleted_at IS NULL
               AND d.mobile_account_authorized_revision = d.device_login_revision",
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await
        .map_err(sqlx_db_error)?;
        rows.into_iter()
            .map(|(device, selected)| {
                let device_id = Uuid::parse_str(&device)
                    .map_err(|_| DbErr::Custom("invalid stored mobile device identity".into()))?;
                Ok((device_id, optional_network_id(selected.as_deref())?))
            })
            .collect()
    }

    pub(crate) async fn mobile_network_members(
        &self,
        user_id: UserIdInDb,
        network_id: Uuid,
    ) -> Result<Vec<MobileNetworkMember>, DbErr> {
        sqlx::query_as(
            "SELECT m.device_id, m.virtual_ipv4, m.allocated_ipv4,
                    COALESCE(c.display_name, NULLIF(m.hostname_override, ''), NULLIF(d.hostname, ''), c.hostname, '') AS hostname
             FROM network_members m
             LEFT JOIN devices d ON d.user_id = m.user_id AND d.machine_id = m.device_id
             LEFT JOIN controller_devices c ON c.owner_id = m.user_id AND c.device_id = m.device_id
               AND c.deleted_at IS NULL
             WHERE m.user_id = ? AND m.network_id = ?
             ORDER BY m.device_id",
        )
        .bind(user_id)
        .bind(network_id.to_string())
        .fetch_all(&self.db)
        .await
        .map_err(sqlx_db_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::controller_devices::{DeviceEnrollment, DeviceReport};

    const IDENTITY: &str = "native account device identity";

    async fn register(db: &Db, device_id: Uuid) -> DeviceEnrollment {
        db.report_controller_device(DeviceReport {
            device_id,
            identity: IDENTITY,
            config_token: "native device configuration token",
            pairing_code: "native device pairing code",
            hostname: "phone",
            version: "test",
            account_transport_policy_supported: false,
            extended_transports_supported: false,
        })
        .await
        .unwrap()
        .unwrap()
    }

    #[tokio::test]
    async fn mobile_account_requires_explicit_account_proof_and_cannot_transfer() {
        let db = Db::memory_db().await;
        let device = Uuid::new_v4();
        let alice = db.auto_create_user("mobile-account-alice").await.unwrap();
        let bob = db.auto_create_user("mobile-account-bob").await.unwrap();
        let enrollment = register(&db, device).await;
        let revision = enrollment.device_login_revision;
        assert!(
            db.bind_controller_device(device, alice.id, None)
                .await
                .unwrap()
        );
        assert!(matches!(
            db.mobile_account_for_device(device, IDENTITY, revision)
                .await,
            Err(MobileAccountError::SignInRequired)
        ));
        assert!(matches!(
            db.authorize_mobile_account(device, IDENTITY, revision, bob.id, &bob.password)
                .await,
            Err(MobileAccountError::BoundElsewhere)
        ));
        assert!(matches!(
            db.authorize_mobile_account(
                device,
                "another identity",
                revision,
                alice.id,
                &alice.password
            )
            .await,
            Err(MobileAccountError::InvalidIdentity)
        ));
        assert!(matches!(
            db.authorize_mobile_account(
                device,
                IDENTITY,
                revision,
                alice.id,
                "expired session hash"
            )
            .await,
            Err(MobileAccountError::AccountUnavailable)
        ));
        let (account, changed) = db
            .authorize_mobile_account(device, IDENTITY, revision, alice.id, &alice.password)
            .await
            .unwrap();
        assert!(changed);
        assert_eq!(account.username, "mobile-account-alice");
        assert_eq!(account.device_code, enrollment.device_code);
        assert_eq!(account.selected_network_id, None);
        assert_eq!(
            db.mobile_account_for_device(device, IDENTITY, revision)
                .await
                .unwrap(),
            account
        );
        assert_eq!(
            db.authorize_mobile_account(device, IDENTITY, revision, alice.id, &alice.password)
                .await
                .unwrap(),
            (account, false)
        );
        assert_eq!(
            db.mobile_network_selections(alice.id).await.unwrap(),
            HashMap::from([(device, None)])
        );
        assert!(
            db.mobile_network_selections(bob.id)
                .await
                .unwrap()
                .is_empty()
        );
        sqlx::query("UPDATE users SET banned = 1 WHERE id = ?")
            .bind(alice.id)
            .execute(&db.db)
            .await
            .unwrap();
        assert!(matches!(
            db.mobile_account_for_device(device, IDENTITY, revision)
                .await,
            Err(MobileAccountError::AccountUnavailable)
        ));
    }

    #[tokio::test]
    async fn mobile_account_reenrollment_requires_a_new_explicit_sign_in() {
        let db = Db::memory_db().await;
        let device = Uuid::new_v4();
        let owner = db.auto_create_user("mobile-account-owner").await.unwrap();
        let revision = register(&db, device).await.device_login_revision;
        let (authorized, changed) = db
            .authorize_mobile_account(device, IDENTITY, revision, owner.id, &owner.password)
            .await
            .unwrap();
        assert!(changed);
        assert_eq!(authorized.owner_id, owner.id);
        assert!(
            db.delete_bound_device((owner.id, device), false, true)
                .await
                .unwrap()
        );
        assert!(matches!(
            db.mobile_account_for_device(device, IDENTITY, revision)
                .await,
            Err(MobileAccountError::MissingDevice)
        ));
        let renewed = register(&db, device).await;
        assert!(renewed.owner_id.is_none());
        assert_eq!(renewed.device_code, authorized.device_code);
        assert_eq!(renewed.device_login_revision, revision + 1);
        assert!(matches!(
            db.authorize_mobile_account(device, IDENTITY, revision, owner.id, &owner.password)
                .await,
            Err(MobileAccountError::RegistrationChanged)
        ));
        assert!(
            db.bind_controller_device(device, owner.id, None)
                .await
                .unwrap()
        );
        assert!(matches!(
            db.mobile_account_for_device(device, IDENTITY, renewed.device_login_revision)
                .await,
            Err(MobileAccountError::SignInRequired)
        ));
        let (_, changed) = db
            .authorize_mobile_account(
                device,
                IDENTITY,
                renewed.device_login_revision,
                owner.id,
                &owner.password,
            )
            .await
            .unwrap();
        assert!(changed);
    }
}

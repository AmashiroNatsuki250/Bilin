use rand::{RngCore as _, rngs::OsRng};
use sea_orm::DbErr;
use serde::Serialize;
use uuid::Uuid;

use super::{
    Db, UserIdInDb,
    controller_devices::{DEVICE_ONLINE_SECONDS, bind_controller_device_in_transaction, digest},
    sqlx_db_error,
};

const TICKET_VALID_SECONDS: i64 = 10 * 60;

#[derive(Serialize)]
pub(crate) struct DeviceLoginTicket {
    ticket: String,
    device_login_revision: i64,
}

#[derive(Debug, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub(crate) struct DeviceLoginDetails {
    device_code: String,
    hostname: String,
}

pub(crate) enum StartDeviceLogin {
    Issued(DeviceLoginTicket),
    Missing,
    Unauthorized,
    AlreadyBound,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ApproveDeviceLogin {
    Bound,
    Unavailable,
    AccountUnavailable,
}

#[derive(sqlx::FromRow)]
struct DeviceRegistration {
    identity_matches: bool,
    owner_id: Option<UserIdInDb>,
    deleted_at: Option<i64>,
    last_seen: i64,
    device_login_revision: i64,
}

impl Db {
    pub(crate) async fn start_controller_device_login(
        &self,
        device_id: Uuid,
        identity: &str,
    ) -> Result<StartDeviceLogin, DbErr> {
        let now = chrono::Utc::now().timestamp();
        let mut tx = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;
        let registration = sqlx::query_as::<_, DeviceRegistration>(
            "SELECT identity_digest = ? AS identity_matches, owner_id, deleted_at,
                    last_seen, device_login_revision
             FROM controller_devices WHERE device_id = ?",
        )
        .bind(digest(identity))
        .bind(device_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(sqlx_db_error)?;
        let Some(registration) = registration else {
            return Ok(StartDeviceLogin::Missing);
        };
        if !registration.identity_matches {
            return Ok(StartDeviceLogin::Unauthorized);
        }
        if registration.deleted_at.is_some() || now - registration.last_seen > DEVICE_ONLINE_SECONDS
        {
            return Ok(StartDeviceLogin::Missing);
        }
        if registration.owner_id.is_some() {
            return Ok(StartDeviceLogin::AlreadyBound);
        }

        let mut random = [0u8; 32];
        OsRng.fill_bytes(&mut random);
        let ticket: String = random.iter().map(|byte| format!("{byte:02x}")).collect();

        sqlx::query("DELETE FROM controller_device_login_tickets WHERE expires_at <= ?")
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(sqlx_db_error)?;
        sqlx::query(
            "INSERT INTO controller_device_login_tickets (device_id, ticket_digest, expires_at)
             VALUES (?, ?, ?)
             ON CONFLICT(device_id) DO UPDATE SET
                ticket_digest = excluded.ticket_digest, expires_at = excluded.expires_at",
        )
        .bind(device_id.to_string())
        .bind(digest(&ticket))
        .bind(now + TICKET_VALID_SECONDS)
        .execute(&mut *tx)
        .await
        .map_err(sqlx_db_error)?;
        tx.commit().await.map_err(sqlx_db_error)?;
        Ok(StartDeviceLogin::Issued(DeviceLoginTicket {
            ticket,
            device_login_revision: registration.device_login_revision,
        }))
    }

    pub(crate) async fn controller_device_login_details(
        &self,
        ticket: &str,
    ) -> Result<Option<DeviceLoginDetails>, DbErr> {
        sqlx::query_as(
            "SELECT d.device_code, COALESCE(d.display_name, d.hostname) AS hostname
             FROM controller_device_login_tickets t JOIN controller_devices d USING (device_id)
             WHERE t.ticket_digest = ? AND t.expires_at > ?
                AND d.owner_id IS NULL AND d.deleted_at IS NULL",
        )
        .bind(digest(ticket))
        .bind(chrono::Utc::now().timestamp())
        .fetch_optional(&self.db)
        .await
        .map_err(sqlx_db_error)
    }

    pub(crate) async fn approve_controller_device_login(
        &self,
        ticket: &str,
        owner_id: UserIdInDb,
    ) -> Result<ApproveDeviceLogin, DbErr> {
        let mut tx = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;
        let account_active: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ? AND NOT banned)")
                .bind(owner_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(sqlx_db_error)?;
        if !account_active {
            return Ok(ApproveDeviceLogin::AccountUnavailable);
        }

        let device_id: Option<String> = sqlx::query_scalar(
            "DELETE FROM controller_device_login_tickets
             WHERE ticket_digest = ? AND expires_at > ? RETURNING device_id",
        )
        .bind(digest(ticket))
        .bind(chrono::Utc::now().timestamp())
        .fetch_optional(&mut *tx)
        .await
        .map_err(sqlx_db_error)?;
        let Some(device_id) = device_id else {
            return Ok(ApproveDeviceLogin::Unavailable);
        };
        let device_id =
            Uuid::parse_str(&device_id).map_err(|error| DbErr::Custom(error.to_string()))?;
        let bound =
            bind_controller_device_in_transaction(&mut tx, device_id, owner_id, None).await?;
        tx.commit().await.map_err(sqlx_db_error)?;
        Ok(if bound {
            ApproveDeviceLogin::Bound
        } else {
            ApproveDeviceLogin::Unavailable
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::controller_devices::{DeviceEnrollment, DeviceReport};

    const IDENTITY: &str = "device enrollment identity";

    async fn register(db: &Db, id: Uuid) -> DeviceEnrollment {
        db.report_controller_device(DeviceReport {
            device_id: id,
            identity: IDENTITY,
            config_token: "device configuration token",
            pairing_code: "device pairing code",
            hostname: "laptop",
            version: "test",
            account_transport_policy_supported: false,
            extended_transports_supported: false,
        })
        .await
        .unwrap()
        .unwrap()
    }

    async fn ticket(db: &Db, id: Uuid) -> DeviceLoginTicket {
        match db
            .start_controller_device_login(id, IDENTITY)
            .await
            .unwrap()
        {
            StartDeviceLogin::Issued(ticket) => ticket,
            _ => panic!("device login should start"),
        }
    }

    #[tokio::test]
    async fn browser_login_uses_a_single_ticket_and_binds_without_network_membership() {
        let db = Db::memory_db().await;
        let id = Uuid::new_v4();
        let default_owner = db.auto_create_user("device-login-owner").await.unwrap();
        let wrong_owner = db.auto_create_user("device-login-other").await.unwrap();

        assert!(matches!(
            db.start_controller_device_login(id, IDENTITY)
                .await
                .unwrap(),
            StartDeviceLogin::Missing
        ));
        let registration = register(&db, id).await;
        assert_eq!(registration.device_login_revision, 1);
        assert!(registration.bound_account.is_none());
        assert!(matches!(
            db.start_controller_device_login(id, "different identity")
                .await
                .unwrap(),
            StartDeviceLogin::Unauthorized
        ));

        let first = ticket(&db, id).await;
        let second = ticket(&db, id).await;
        assert_ne!(first.ticket, second.ticket);
        assert_eq!(first.ticket.len(), 64);
        assert_eq!(
            second.device_login_revision,
            registration.device_login_revision
        );
        assert_eq!(
            db.controller_device_login_details(&first.ticket)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            db.controller_device_login_details(&second.ticket)
                .await
                .unwrap(),
            Some(DeviceLoginDetails {
                device_code: registration.device_code,
                hostname: "laptop".to_owned(),
            }),
        );

        let now = chrono::Utc::now().timestamp();
        sqlx::query("UPDATE controller_device_login_tickets SET expires_at = ?")
            .bind(now - 1)
            .execute(&db.db)
            .await
            .unwrap();
        assert_eq!(
            db.approve_controller_device_login(&second.ticket, wrong_owner.id)
                .await
                .unwrap(),
            ApproveDeviceLogin::Unavailable
        );
        let third = ticket(&db, id).await;
        assert_eq!(
            db.approve_controller_device_login(&third.ticket, -1)
                .await
                .unwrap(),
            ApproveDeviceLogin::AccountUnavailable,
        );
        assert_eq!(
            db.approve_controller_device_login(&third.ticket, default_owner.id)
                .await
                .unwrap(),
            ApproveDeviceLogin::Bound
        );
        assert_eq!(
            db.approve_controller_device_login(&third.ticket, wrong_owner.id)
                .await
                .unwrap(),
            ApproveDeviceLogin::Unavailable
        );
        assert!(matches!(
            db.start_controller_device_login(id, IDENTITY)
                .await
                .unwrap(),
            StartDeviceLogin::AlreadyBound
        ));
        let enrollment = register(&db, id).await;
        assert_eq!(
            enrollment.bound_account.as_deref(),
            Some("device-login-owner")
        );
        assert_eq!(enrollment.device_login_revision, 1);
        let members = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM network_members WHERE device_id = ?",
        )
        .bind(id.to_string())
        .fetch_one(&db.db)
        .await
        .unwrap();
        assert_eq!(members, 0);
    }

    #[tokio::test]
    async fn retirement_and_manual_binding_revoke_pending_tickets() {
        let db = Db::memory_db().await;
        let id = Uuid::new_v4();
        let owner = db.auto_create_user("device-login-owner").await.unwrap();
        register(&db, id).await;
        let before_delete = ticket(&db, id).await;
        assert!(db.retire_unbound_controller_device(id).await.unwrap());
        assert!(matches!(
            db.start_controller_device_login(id, IDENTITY)
                .await
                .unwrap(),
            StartDeviceLogin::Missing
        ));
        let registration = register(&db, id).await;
        assert_eq!(
            registration.device_login_revision,
            before_delete.device_login_revision + 1
        );
        assert_eq!(
            db.approve_controller_device_login(&before_delete.ticket, owner.id)
                .await
                .unwrap(),
            ApproveDeviceLogin::Unavailable
        );

        let before_manual_bind = ticket(&db, id).await;
        assert!(db.bind_controller_device(id, owner.id, None).await.unwrap());
        assert!(
            db.delete_bound_device((owner.id, id), false, false)
                .await
                .unwrap()
        );
        let registration = register(&db, id).await;
        assert_eq!(
            registration.device_login_revision,
            before_manual_bind.device_login_revision + 1
        );
        assert!(registration.bound_account.is_none());
        assert_eq!(
            db.approve_controller_device_login(&before_manual_bind.ticket, owner.id)
                .await
                .unwrap(),
            ApproveDeviceLogin::Unavailable
        );
    }
}

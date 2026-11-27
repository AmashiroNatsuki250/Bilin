use std::collections::HashMap;

use bilin::web_client::enrollment::DEVICE_TOKEN_PREFIX;
use rand::{Rng, rngs::OsRng};
use sea_orm::DbErr;
use sha2::{Digest as _, Sha256};
use sqlx::{Sqlite, Transaction};
use uuid::Uuid;

use super::{Db, UserIdInDb, sqlx_db_error};

pub const DEVICE_ONLINE_SECONDS: i64 = 35;
const PAIRING_SECONDS: i64 = 90;

#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct ControllerDevice {
    pub device_id: String,
    pub device_code: String,
    pub owner_id: Option<UserIdInDb>,
    pub hostname: String,
    #[serde(skip)]
    pub display_name: Option<String>,
    pub version: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub upstream_subnet_cidr: Option<String>,
    #[serde(skip)]
    pub upstream_subnet_applied: bool,
}

pub struct DeviceReport<'a> {
    pub device_id: Uuid,
    pub identity: &'a str,
    pub config_token: &'a str,
    pub pairing_code: &'a str,
    pub hostname: &'a str,
    pub version: &'a str,
    pub account_transport_policy_supported: bool,
    pub extended_transports_supported: bool,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct DeviceEnrollment {
    pub owner_id: Option<UserIdInDb>,
    pub bound_account: Option<String>,
    pub device_login_revision: i64,
    pub device_code: String,
    pub device_name: String,
    pub renew_pairing_code: bool,
    pub upstream_subnet_cidr: Option<String>,
}

#[derive(sqlx::FromRow)]
pub(crate) struct UpstreamSubnet {
    pub cidr: Option<String>,
    pub applied: bool,
}

pub(super) fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

pub(crate) fn new_device_code() -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut rng = OsRng;
    loop {
        let code: String = (0..8)
            .map(|_| char::from(ALPHABET[rng.gen_range(0..ALPHABET.len())]))
            .collect();
        if code.bytes().any(|byte| byte.is_ascii_digit())
            && code.bytes().any(|byte| byte.is_ascii_lowercase())
        {
            return code;
        }
    }
}

const DEVICE_FIELDS: &str =
    "device_id, device_code, owner_id, COALESCE(display_name, hostname) AS hostname,
     display_name, version, first_seen, last_seen,
     upstream_subnet_cidr, upstream_subnet_applied";

#[derive(Debug, serde::Serialize)]
pub(crate) struct DeviceName {
    pub device_name: String,
    pub device_login_revision: i64,
}

pub(crate) enum DeviceNameActor<'a> {
    Administrator,
    Owner(UserIdInDb),
    Device { identity: &'a str, revision: i64 },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DeviceNameError {
    #[error(transparent)]
    Database(#[from] DbErr),
    #[error("device not found")]
    NotFound,
    #[error("device identity does not match")]
    InvalidIdentity,
    #[error("device registration changed")]
    RegistrationChanged,
    #[error("account suspended")]
    AccountSuspended,
}

#[derive(sqlx::FromRow)]
struct NamedRegistration {
    device_id: String,
    owner_id: Option<UserIdInDb>,
    identity_digest: String,
    device_login_revision: i64,
    device_name: String,
    display_name: Option<String>,
    deleted_at: Option<i64>,
    suspended: bool,
}

impl NamedRegistration {
    fn verify_identity(&self, identity: &str) -> Result<(), DeviceNameError> {
        if self.identity_digest != digest(identity) {
            return Err(DeviceNameError::InvalidIdentity);
        }
        if self.deleted_at.is_some() {
            return Err(DeviceNameError::NotFound);
        }
        if self.suspended {
            return Err(DeviceNameError::AccountSuspended);
        }
        Ok(())
    }
}

async fn named_registration(
    tx: &mut Transaction<'_, Sqlite>,
    identifier: &str,
) -> Result<NamedRegistration, DeviceNameError> {
    let (column, value) = match Uuid::parse_str(identifier) {
        Ok(device_id) => ("d.device_id", device_id.to_string()),
        Err(_) => ("d.device_code", identifier.to_ascii_lowercase()),
    };
    sqlx::query_as(&format!(
        "SELECT d.device_id, d.owner_id, d.identity_digest, d.device_login_revision,
                COALESCE(d.display_name, d.hostname) AS device_name,
                d.display_name, d.deleted_at, COALESCE(u.banned, 0) AS suspended
         FROM controller_devices d LEFT JOIN users u ON u.id = d.owner_id
         WHERE {column} = ?"
    ))
    .bind(value)
    .fetch_optional(&mut **tx)
    .await
    .map_err(sqlx_db_error)?
    .ok_or(DeviceNameError::NotFound)
}

pub(super) async fn bind_controller_device_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    device_id: Uuid,
    owner_id: UserIdInDb,
    pairing_code: Option<&str>,
) -> Result<bool, DbErr> {
    let now = chrono::Utc::now();
    let device_id = device_id.to_string();
    let code = pairing_code.map(digest);
    let updated = sqlx::query(
        r#"
        UPDATE controller_devices
        SET owner_id = ?, pairing_expires_at = NULL,
            upstream_subnet_cidr = NULL, upstream_subnet_applied = 0
        WHERE device_id = ? AND owner_id IS NULL AND deleted_at IS NULL
            AND (? IS NULL OR (pairing_digest = ? AND pairing_expires_at >= ?))
        "#,
    )
    .bind(owner_id)
    .bind(&device_id)
    .bind(&code)
    .bind(&code)
    .bind(now.timestamp())
    .execute(&mut **transaction)
    .await
    .map_err(sqlx_db_error)?;
    if updated.rows_affected() == 0 {
        return Ok(false);
    }

    sqlx::query(
        r#"
        INSERT INTO devices (
            user_id, machine_id, hostname, bilin_version, first_seen_time, last_seen_time
        ) SELECT ?, device_id, hostname, version, ?, ? FROM controller_devices
          WHERE device_id = ?
        ON CONFLICT(user_id, machine_id) DO UPDATE
        SET hostname = excluded.hostname, bilin_version = excluded.bilin_version
        "#,
    )
    .bind(owner_id)
    .bind(now.fixed_offset())
    .bind(now.fixed_offset())
    .bind(&device_id)
    .execute(&mut **transaction)
    .await
    .map_err(sqlx_db_error)?;
    Ok(true)
}

impl Db {
    pub async fn config_session_user(
        &self,
        machine_id: Uuid,
        token: &str,
    ) -> Result<Option<UserIdInDb>, DbErr> {
        if token.starts_with(DEVICE_TOKEN_PREFIX) {
            self.controller_device_owner(machine_id, token).await
        } else {
            self.get_user_id_by_token(token).await
        }
    }

    /// None means that the identifier already belongs to a different identity.
    pub async fn report_controller_device(
        &self,
        report: DeviceReport<'_>,
    ) -> Result<Option<DeviceEnrollment>, DbErr> {
        self.report_controller_device_with(report, new_device_code)
            .await
    }

    async fn report_controller_device_with(
        &self,
        report: DeviceReport<'_>,
        mut next_code: impl FnMut() -> String,
    ) -> Result<Option<DeviceEnrollment>, DbErr> {
        let now = chrono::Utc::now().timestamp();
        loop {
            let result = sqlx::query_as::<_, DeviceEnrollment>(
                r#"
            INSERT INTO controller_devices (
                device_id, identity_digest, config_token_digest, pairing_digest,
                pairing_expires_at, hostname, version, first_seen, last_seen, device_code,
                account_transport_policy_supported, extended_transports_supported
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(device_id) DO UPDATE SET
                hostname = excluded.hostname, version = excluded.version,
                last_seen = excluded.last_seen,
                account_transport_policy_supported = excluded.account_transport_policy_supported,
                extended_transports_supported = excluded.extended_transports_supported,
                deleted_at = NULL,
                pairing_digest = excluded.pairing_digest,
                pairing_expires_at = CASE
                    WHEN owner_id IS NOT NULL THEN NULL
                    -- Require a fresh code after deletion. Older tombstones have
                    -- no saved digest, so their first report establishes the baseline.
                    WHEN deleted_at IS NOT NULL AND pairing_digest IS NULL THEN NULL
                    WHEN pairing_expires_at IS NULL AND pairing_digest = excluded.pairing_digest THEN NULL
                    ELSE excluded.pairing_expires_at END
            WHERE identity_digest = excluded.identity_digest
                AND config_token_digest = excluded.config_token_digest
            ON CONFLICT(device_code) DO NOTHING
            RETURNING owner_id, device_code, upstream_subnet_cidr, device_login_revision,
                COALESCE(display_name, hostname) AS device_name,
                (SELECT username FROM users WHERE id = controller_devices.owner_id) AS bound_account,
                owner_id IS NULL AND pairing_expires_at IS NULL AS renew_pairing_code
            "#,
            )
            .bind(report.device_id.to_string())
            .bind(digest(report.identity))
            .bind(digest(report.config_token))
            .bind(digest(report.pairing_code))
            .bind(now + PAIRING_SECONDS)
            .bind(report.hostname)
            .bind(report.version)
            .bind(now)
            .bind(now)
            .bind(next_code())
            .bind(report.account_transport_policy_supported)
            .bind(report.extended_transports_supported)
            .fetch_optional(&self.db)
            .await
            .map_err(sqlx_db_error)?;
            if result.is_some() || self.has_controller_identity(report.device_id).await? {
                return Ok(result);
            }
        }
    }

    pub async fn find_controller_device(
        &self,
        identifier: &str,
    ) -> Result<Option<ControllerDevice>, DbErr> {
        sqlx::query_as(&format!(
            "SELECT {DEVICE_FIELDS} FROM controller_devices WHERE deleted_at IS NULL AND {} = ?",
            if Uuid::parse_str(identifier).is_ok() {
                "device_id"
            } else {
                "device_code"
            }
        ))
        .bind(identifier)
        .fetch_optional(&self.db)
        .await
        .map_err(sqlx_db_error)
    }

    pub(crate) async fn read_controller_device_name(
        &self,
        device_id: Uuid,
        identity: &str,
    ) -> Result<DeviceName, DeviceNameError> {
        let mut tx = self.db.begin().await.map_err(sqlx_db_error)?;
        let device = named_registration(&mut tx, &device_id.to_string()).await?;
        device.verify_identity(identity)?;
        tx.commit().await.map_err(sqlx_db_error)?;
        Ok(DeviceName {
            device_name: device.device_name,
            device_login_revision: device.device_login_revision,
        })
    }

    pub(crate) async fn rename_controller_device(
        &self,
        identifier: &str,
        actor: DeviceNameActor<'_>,
        name: &str,
    ) -> Result<(DeviceName, Option<UserIdInDb>), DeviceNameError> {
        let mut tx = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;
        let device = named_registration(&mut tx, identifier).await?;
        match actor {
            DeviceNameActor::Device { identity, revision } => {
                device.verify_identity(identity)?;
                if revision != device.device_login_revision {
                    return Err(DeviceNameError::RegistrationChanged);
                }
            }
            DeviceNameActor::Owner(user) => {
                if device.deleted_at.is_some() || device.owner_id != Some(user) {
                    return Err(DeviceNameError::NotFound);
                }
                if device.suspended {
                    return Err(DeviceNameError::AccountSuspended);
                }
            }
            DeviceNameActor::Administrator => {
                if device.deleted_at.is_some() {
                    return Err(DeviceNameError::NotFound);
                }
            }
        }
        let changed = device.display_name.as_deref() != Some(name);
        if changed {
            sqlx::query("UPDATE controller_devices SET display_name = ? WHERE device_id = ?")
                .bind(name)
                .bind(&device.device_id)
                .execute(&mut *tx)
                .await
                .map_err(sqlx_db_error)?;
        }
        tx.commit().await.map_err(sqlx_db_error)?;
        Ok((
            DeviceName {
                device_name: name.to_owned(),
                device_login_revision: device.device_login_revision,
            },
            device.owner_id.filter(|_| changed),
        ))
    }

    pub(crate) async fn controller_display_names(
        &self,
        owner: UserIdInDb,
    ) -> Result<HashMap<String, String>, DbErr> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT device_id, display_name FROM controller_devices
             WHERE owner_id = ? AND deleted_at IS NULL AND display_name IS NOT NULL",
        )
        .bind(owner)
        .fetch_all(&self.db)
        .await
        .map_err(sqlx_db_error)?;
        Ok(rows.into_iter().collect())
    }

    pub(crate) async fn controller_upstream_subnet(
        &self,
        owner: UserIdInDb,
        device: Uuid,
    ) -> Result<Option<UpstreamSubnet>, DbErr> {
        sqlx::query_as(
            "SELECT upstream_subnet_cidr AS cidr, upstream_subnet_applied AS applied
             FROM controller_devices WHERE owner_id = ? AND device_id = ? AND deleted_at IS NULL",
        )
        .bind(owner)
        .bind(device.to_string())
        .fetch_optional(&self.db)
        .await
        .map_err(sqlx_db_error)
    }

    pub async fn retire_unbound_controller_device(&self, device: Uuid) -> Result<bool, DbErr> {
        let result = sqlx::query("UPDATE controller_devices SET deleted_at = ?, pairing_expires_at = NULL WHERE device_id = ? AND owner_id IS NULL AND deleted_at IS NULL")
            .bind(chrono::Utc::now().timestamp()).bind(device.to_string()).execute(&self.db).await.map_err(sqlx_db_error)?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn controller_device_owner(
        &self,
        machine_id: Uuid,
        config_token: &str,
    ) -> Result<Option<UserIdInDb>, DbErr> {
        sqlx::query_scalar(
            "SELECT owner_id FROM controller_devices WHERE device_id = ? AND config_token_digest = ? AND owner_id IS NOT NULL AND deleted_at IS NULL",
        )
        .bind(machine_id.to_string())
        .bind(digest(config_token))
        .fetch_optional(&self.db)
        .await
        .map_err(sqlx_db_error)
    }

    pub async fn has_controller_identity(&self, machine_id: Uuid) -> Result<bool, DbErr> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM controller_devices WHERE device_id = ?)")
            .bind(machine_id.to_string())
            .fetch_one(&self.db)
            .await
            .map_err(sqlx_db_error)
    }

    pub async fn owns_controller_device(
        &self,
        owner: UserIdInDb,
        device: Uuid,
    ) -> Result<bool, DbErr> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM controller_devices WHERE device_id = ? AND owner_id = ?)",
        )
        .bind(device.to_string())
        .bind(owner)
        .fetch_one(&self.db)
        .await
        .map_err(sqlx_db_error)
    }

    /// Assign an unbound registration to an owner. The caller has authenticated
    /// the user and resolved the device number, or authorized administrator access.
    /// Older requests identified by UUID can also require a legacy pairing proof.
    pub async fn bind_controller_device(
        &self,
        device_id: Uuid,
        owner_id: UserIdInDb,
        pairing_code: Option<&str>,
    ) -> Result<bool, DbErr> {
        let mut tx = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;
        if !bind_controller_device_in_transaction(&mut tx, device_id, owner_id, pairing_code)
            .await?
        {
            tx.rollback().await.map_err(sqlx_db_error)?;
            return Ok(false);
        }

        tx.commit().await.map_err(sqlx_db_error)?;
        Ok(true)
    }

    pub async fn list_controller_devices(
        &self,
        owner_id: Option<UserIdInDb>,
    ) -> Result<Vec<ControllerDevice>, DbErr> {
        sqlx::query_as(&format!(
            "SELECT {DEVICE_FIELDS} FROM controller_devices WHERE deleted_at IS NULL AND (? IS NULL OR owner_id = ?) ORDER BY last_seen DESC, device_id",
        ))
        .bind(owner_id)
        .bind(owner_id)
        .fetch_all(&self.db)
        .await
        .map_err(sqlx_db_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn device_name_is_shared_across_actors_and_survives_heartbeat_and_revocation() {
        let db = Db::memory_db().await;
        let device = Uuid::new_v4();
        let identity = "verified device identity";
        let owner = db.auto_create_user("device-name-owner").await.unwrap().id;
        let stranger = db.auto_create_user("device-name-other").await.unwrap().id;
        let report = async |hostname| {
            db.report_controller_device(DeviceReport {
                device_id: device,
                identity,
                config_token: "device-name-config",
                pairing_code: "device-name-pairing",
                hostname,
                version: "test",
                account_transport_policy_supported: false,
                extended_transports_supported: false,
            })
            .await
            .unwrap()
            .unwrap()
        };
        let initial = report("system-host").await;
        let revision = initial.device_login_revision;
        let identifier = device.to_string();
        let code = initial.device_code;
        assert_eq!(initial.device_name, "system-host");
        assert!(matches!(
            db.read_controller_device_name(device, "wrong credential")
                .await,
            Err(DeviceNameError::InvalidIdentity)
        ));
        assert_eq!(
            db.read_controller_device_name(device, identity)
                .await
                .unwrap()
                .device_name,
            "system-host"
        );
        assert_eq!(
            db.rename_controller_device(&code, DeviceNameActor::Administrator, "管理员命名")
                .await
                .unwrap()
                .1,
            None
        );
        assert_eq!(report("new-system-host").await.device_name, "管理员命名");
        assert!(
            db.bind_controller_device(device, owner, None)
                .await
                .unwrap()
        );
        assert!(matches!(
            db.rename_controller_device(&code, DeviceNameActor::Owner(stranger), "unauthorized")
                .await,
            Err(DeviceNameError::NotFound)
        ));
        assert_eq!(
            db.rename_controller_device(&identifier, DeviceNameActor::Owner(owner), "来自 Web")
                .await
                .unwrap()
                .1,
            Some(owner)
        );
        assert_eq!(
            db.rename_controller_device(&identifier, DeviceNameActor::Owner(owner), "来自 Web")
                .await
                .unwrap()
                .1,
            None
        );
        assert_eq!(
            db.read_controller_device_name(device, identity)
                .await
                .unwrap()
                .device_name,
            "来自 Web"
        );
        assert!(matches!(
            db.rename_controller_device(
                &identifier,
                DeviceNameActor::Device {
                    identity: "wrong credential",
                    revision
                },
                "unauthorized"
            )
            .await,
            Err(DeviceNameError::InvalidIdentity)
        ));
        assert_eq!(
            db.rename_controller_device(
                &identifier,
                DeviceNameActor::Device { identity, revision },
                "来自客户端"
            )
            .await
            .unwrap()
            .1,
            Some(owner)
        );
        assert_eq!(
            db.find_controller_device(&code)
                .await
                .unwrap()
                .unwrap()
                .hostname,
            "来自客户端"
        );
        assert_eq!(
            db.controller_display_names(owner).await.unwrap()[&identifier],
            "来自客户端"
        );
        assert!(
            db.controller_display_names(stranger)
                .await
                .unwrap()
                .is_empty()
        );

        assert!(
            db.delete_bound_device((owner, device), false, true)
                .await
                .unwrap()
        );
        assert!(matches!(
            db.read_controller_device_name(device, identity).await,
            Err(DeviceNameError::NotFound)
        ));
        let renewed = report("last-system-host").await;
        assert_eq!(renewed.device_login_revision, revision + 1);
        assert_eq!(renewed.device_name, "来自客户端");
        assert!(matches!(
            db.rename_controller_device(
                &identifier,
                DeviceNameActor::Device { identity, revision },
                "late old-owner request"
            )
            .await,
            Err(DeviceNameError::RegistrationChanged)
        ));
        assert!(matches!(
            db.rename_controller_device(
                &identifier,
                DeviceNameActor::Owner(owner),
                "late Web owner request"
            )
            .await,
            Err(DeviceNameError::NotFound)
        ));
        assert_eq!(
            db.read_controller_device_name(device, identity)
                .await
                .unwrap()
                .device_name,
            "来自客户端"
        );
    }

    #[tokio::test]
    async fn retries_short_code_collisions_and_requires_fresh_pairing_after_deletion() {
        let db = Db::memory_db().await;
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let report = |device_id, token, pairing_code| DeviceReport {
            device_id,
            identity: token,
            config_token: token,
            pairing_code,
            hostname: "host",
            version: "test",
            account_transport_policy_supported: false,
            extended_transports_supported: false,
        };
        db.report_controller_device_with(report(first, "first", "pair"), || "aa000000".into())
            .await
            .unwrap();
        let mut codes = ["aa000000", "zz000001"].into_iter();
        db.report_controller_device_with(report(second, "second", "pair"), || {
            codes.next().unwrap().into()
        })
        .await
        .unwrap();
        db.report_controller_device_with(report(second, "second", "pair"), || "other123".into())
            .await
            .unwrap();
        assert_eq!(
            db.find_controller_device("zz000001")
                .await
                .unwrap()
                .unwrap()
                .device_id,
            second.to_string()
        );
        assert!(db.retire_unbound_controller_device(second).await.unwrap());
        assert!(
            db.find_controller_device("zz000001")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            db.report_controller_device(report(second, "impostor", "pair"))
                .await
                .unwrap()
                .is_none()
        );
        let owner = db
            .auto_create_user("reregistration-owner")
            .await
            .unwrap()
            .id;
        let enrolled = db
            .report_controller_device(report(second, "second", "pair"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(enrolled.device_code, "zz000001");
        assert_eq!(enrolled.owner_id, None);
        assert!(enrolled.renew_pairing_code);
        assert!(
            !db.bind_controller_device(second, owner, Some("pair"))
                .await
                .unwrap()
        );
        let enrolled = db
            .report_controller_device(report(second, "second", "fresh"))
            .await
            .unwrap()
            .unwrap();
        assert!(!enrolled.renew_pairing_code);
        assert!(
            db.bind_controller_device(second, owner, Some("fresh"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn device_identity_and_pairing_cannot_be_replaced_or_reused() {
        let db = Db::memory_db().await;
        let first = db.auto_create_user("pairing-first").await.unwrap().id;
        let second = db.auto_create_user("pairing-second").await.unwrap().id;
        let device_id = Uuid::new_v4();
        let report = |identity| DeviceReport {
            device_id,
            identity,
            config_token: "device-config",
            pairing_code: "pair-code",
            hostname: "device",
            version: "test",
            account_transport_policy_supported: false,
            extended_transports_supported: false,
        };
        assert_eq!(
            db.report_controller_device(report("original"))
                .await
                .unwrap()
                .map(|enrollment| enrollment.owner_id),
            Some(None)
        );
        assert_eq!(
            db.report_controller_device(report("impostor"))
                .await
                .unwrap(),
            None
        );
        assert!(
            !db.bind_controller_device(device_id, first, Some("wrong"))
                .await
                .unwrap()
        );
        assert_eq!(
            db.controller_device_owner(device_id, "device-config")
                .await
                .unwrap(),
            None
        );

        let (first_claim, second_claim) = tokio::join!(
            db.bind_controller_device(device_id, first, Some("pair-code")),
            db.bind_controller_device(device_id, second, Some("pair-code"))
        );
        let winner = match (first_claim.unwrap(), second_claim.unwrap()) {
            (true, false) => first,
            (false, true) => second,
            _ => panic!("exactly one user should own the device"),
        };
        assert_eq!(
            db.controller_device_owner(device_id, "device-config")
                .await
                .unwrap(),
            Some(winner)
        );
        assert_eq!(
            db.controller_device_owner(device_id, "wrong")
                .await
                .unwrap(),
            None
        );
        assert!(
            !db.bind_controller_device(device_id, winner, Some("pair-code"))
                .await
                .unwrap()
        );
        assert!(db.get_device((winner, device_id)).await.unwrap().is_some());

        assert!(
            db.delete_device_with_optional_block((winner, device_id), false)
                .await
                .unwrap()
        );
        assert_eq!(
            db.controller_device_owner(device_id, "device-config")
                .await
                .unwrap(),
            None
        );
        assert!(
            !db.bind_controller_device(device_id, winner, Some("pair-code"))
                .await
                .unwrap()
        );
    }
}

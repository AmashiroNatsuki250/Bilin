use std::net::Ipv4Addr;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use bilin::proto::web::HeartbeatRequest;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Service, device_login::browser_origin_is_local, enrollment_identity, failure};
use crate::{
    central_network::service::{CentralNetworkService, NetworkSummary},
    db::mobile_account::{MobileAccountDevice, MobileAccountError, MobileNetworkMember},
    restful::{
        AppStateInner, HttpHandleError, authed_user_id, central_network::convert_error,
        convert_db_error, users::AuthSession,
    },
};

#[derive(Deserialize)]
pub(super) struct DeviceProof {
    device_id: Uuid,
    device_login_revision: i64,
}

impl DeviceProof {
    fn identity<'a>(&self, headers: &'a HeaderMap) -> Result<&'a str, HttpHandleError> {
        let identity = enrollment_identity(headers)?;
        if self.device_id.get_version_num() != 4 || self.device_login_revision < 1 {
            return Err(failure(
                StatusCode::BAD_REQUEST,
                "invalid device registration",
            ));
        }
        Ok(identity)
    }
}

#[derive(Deserialize)]
pub(super) struct JoinNetwork {
    #[serde(flatten)]
    device: DeviceProof,
    network_id: Uuid,
}

#[derive(Serialize)]
struct MobileAccountResponse {
    username: String,
    device_code: String,
    networks: Vec<NetworkSummary>,
    selected_network_id: Option<Uuid>,
}

#[derive(Serialize)]
struct MobileDevicesResponse {
    network_id: Uuid,
    devices: Vec<MobileNetworkDevice>,
}

#[derive(Serialize)]
struct MobileNetworkDevice {
    device_id: String,
    hostname: String,
    virtual_ipv4: Option<Ipv4Addr>,
    online: bool,
}

fn parse_ipv4(address: &str) -> Option<Ipv4Addr> {
    let ip = address.parse::<cidr::Ipv4Inet>().ok()?.address();
    (!ip.is_unspecified()).then_some(ip)
}

impl MobileNetworkDevice {
    fn from_member(member: MobileNetworkMember, online: bool) -> Self {
        let virtual_ipv4 = [
            member.virtual_ipv4.as_deref(),
            member.allocated_ipv4.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find_map(parse_ipv4);
        Self {
            device_id: member.device_id,
            hostname: member.hostname,
            virtual_ipv4,
            online,
        }
    }
}

fn heartbeat_reports_network(heartbeat: &HeartbeatRequest, network_id: Uuid) -> bool {
    let instance = network_id.into();
    heartbeat.running_network_instances.contains(&instance)
        && !heartbeat.failed_network_instances.contains(&instance)
}

fn account_error(error: MobileAccountError) -> HttpHandleError {
    let status = match error {
        MobileAccountError::Database(error) => return convert_db_error(error),
        MobileAccountError::InvalidIdentity | MobileAccountError::SignInRequired => {
            StatusCode::UNAUTHORIZED
        }
        MobileAccountError::MissingDevice => StatusCode::NOT_FOUND,
        MobileAccountError::RegistrationChanged | MobileAccountError::BoundElsewhere => {
            StatusCode::CONFLICT
        }
        MobileAccountError::AccountUnavailable => StatusCode::FORBIDDEN,
    };
    failure(status, &error.to_string())
}

async fn response(
    service: &CentralNetworkService,
    account: MobileAccountDevice,
) -> Result<Json<MobileAccountResponse>, HttpHandleError> {
    let networks = service
        .list_networks(account.owner_id)
        .await
        .map_err(convert_error)?
        .into_iter()
        .filter(|network| network.networking_method == "Gateway" && network.secure_mode)
        .collect();
    Ok(Json(MobileAccountResponse {
        username: account.username,
        device_code: account.device_code,
        networks,
        selected_network_id: account.selected_network_id,
    }))
}

pub(super) async fn bind(
    auth: AuthSession,
    Extension(service): Service,
    headers: HeaderMap,
    Json(proof): Json<DeviceProof>,
) -> Result<impl IntoResponse, HttpHandleError> {
    let owner_id = authed_user_id(&auth)?;
    let user = auth.user.as_ref().unwrap();
    if !browser_origin_is_local(&headers) {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "cross-origin mobile sign-in is not allowed",
        ));
    }
    if user.db_user.password == crate::migrator::INITIAL_ADMIN_PASSWORD_HASH {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "change the default administrator password first",
        ));
    }
    let identity = proof.identity(&headers)?;
    let _mutation = service.lock_mutations().await;
    let (account, changed) = service
        .db
        .authorize_mobile_account(
            proof.device_id,
            identity,
            proof.device_login_revision,
            owner_id,
            &user.db_user.password,
        )
        .await
        .map_err(account_error)?;
    if changed {
        service.publish_user(owner_id).await;
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        response(&service, account).await?,
    ))
}

pub(super) async fn status(
    Extension(service): Service,
    headers: HeaderMap,
    Json(proof): Json<DeviceProof>,
) -> Result<impl IntoResponse, HttpHandleError> {
    let identity = proof.identity(&headers)?;
    let _mutation = service.lock_mutations().await;
    let account = service
        .db
        .mobile_account_for_device(proof.device_id, identity, proof.device_login_revision)
        .await
        .map_err(account_error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        response(&service, account).await?,
    ))
}

pub(super) async fn devices(
    Extension(service): Service,
    State(manager): State<AppStateInner>,
    headers: HeaderMap,
    Json(proof): Json<DeviceProof>,
) -> Result<impl IntoResponse, HttpHandleError> {
    let identity = proof.identity(&headers)?;
    let account = service
        .db
        .mobile_account_for_device(proof.device_id, identity, proof.device_login_revision)
        .await
        .map_err(account_error)?;
    let network_id = account
        .selected_network_id
        .ok_or_else(|| failure(StatusCode::CONFLICT, "no mobile network is selected"))?;
    let members = service
        .db
        .mobile_network_members(account.owner_id, network_id)
        .await
        .map_err(convert_db_error)?;
    let mut devices = Vec::with_capacity(members.len());
    for member in members {
        let device_id = Uuid::parse_str(&member.device_id).map_err(|_| {
            failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "invalid stored device identifier",
            )
        })?;
        let online = match manager.get_session_by_machine_id(account.owner_id, &device_id) {
            Some(session) => session
                .get_heartbeat_req()
                .await
                .is_some_and(|heartbeat| heartbeat_reports_network(&heartbeat, network_id)),
            None => false,
        };
        devices.push(MobileNetworkDevice::from_member(member, online));
    }
    // A selection or registration can change while the roster is being read.
    let current = service
        .db
        .mobile_account_for_device(proof.device_id, identity, proof.device_login_revision)
        .await
        .map_err(account_error)?;
    if current.owner_id != account.owner_id || current.selected_network_id != Some(network_id) {
        return Err(failure(
            StatusCode::CONFLICT,
            "mobile selected network changed",
        ));
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(MobileDevicesResponse {
            network_id,
            devices,
        }),
    ))
}

pub(super) async fn join(
    Extension(service): Service,
    headers: HeaderMap,
    Json(request): Json<JoinNetwork>,
) -> Result<StatusCode, HttpHandleError> {
    let proof = request.device;
    let identity = proof.identity(&headers)?;
    let _mutation = service.lock_mutations().await;
    let account = service
        .db
        .mobile_account_for_device(proof.device_id, identity, proof.device_login_revision)
        .await
        .map_err(account_error)?;
    service
        .join_mobile_controller_network_locked(
            account.owner_id,
            proof.device_id,
            proof.device_login_revision,
            request.network_id,
            account.selected_network_id,
        )
        .await
        .map_err(convert_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn member(explicit: Option<&str>, allocated: Option<&str>) -> MobileNetworkMember {
        MobileNetworkMember {
            device_id: "device-id".into(),
            hostname: "workstation".into(),
            virtual_ipv4: explicit.map(str::to_owned),
            allocated_ipv4: allocated.map(str::to_owned),
        }
    }

    #[test]
    fn device_list_only_exposes_selected_network_presence_and_address() {
        let row = MobileNetworkDevice::from_member(
            member(Some("10.23.2.9/24"), Some("10.23.2.11/24")),
            true,
        );
        assert_eq!(
            serde_json::to_value(row).unwrap(),
            json!({
                "device_id": "device-id",
                "hostname": "workstation",
                "virtual_ipv4": "10.23.2.9",
                "online": true,
            }),
        );
    }

    #[test]
    fn device_list_keeps_offline_assigned_address() {
        let offline = MobileNetworkDevice::from_member(
            member(Some("10.23.2.10"), Some("10.23.2.11/24")),
            false,
        );
        assert_eq!(offline.virtual_ipv4, Some(Ipv4Addr::new(10, 23, 2, 10)));
        assert!(!offline.online);
        let fallback = MobileNetworkDevice::from_member(
            member(Some("0.0.0.0/24"), Some("10.23.2.11/24")),
            false,
        );
        assert_eq!(fallback.virtual_ipv4, Some(Ipv4Addr::new(10, 23, 2, 11)));
        assert!(!fallback.online);
        let unassigned = MobileNetworkDevice::from_member(member(None, None), false);
        assert!(unassigned.virtual_ipv4.is_none());
    }

    #[test]
    fn device_list_online_requires_selected_running_network_without_failure() {
        let selected = Uuid::new_v4();
        let other = Uuid::new_v4();
        let mut heartbeat = HeartbeatRequest {
            running_network_instances: vec![other.into()],
            ..Default::default()
        };
        assert!(!heartbeat_reports_network(&heartbeat, selected));
        heartbeat.running_network_instances.push(selected.into());
        assert!(heartbeat_reports_network(&heartbeat, selected));
        heartbeat.failed_network_instances.push(selected.into());
        assert!(!heartbeat_reports_network(&heartbeat, selected));
        assert!(heartbeat_reports_network(&heartbeat, other));
    }
}

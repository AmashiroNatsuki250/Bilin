use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    routing::{delete, get, patch, post},
};
use bilin::web_client::enrollment::{
    AssignedNetwork, DEVICE_TOKEN_PREFIX, NodeCandidate, NodeMeasurement, ObservedClientPath,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod device_login;
mod device_name;
mod mobile_account;

use super::{
    AppStateInner, ConsoleInfoConfig, HttpHandleError, authed_user_id,
    central_network::convert_error, convert_db_error, other_error, users::AuthSession,
};
use crate::{
    central_network::forwarding::{self, ForwardingNode, NodeReport, NodeSnapshot},
    central_network::service::{
        CentralNetworkService, MembersView, NetworkSettings, NetworkSummary, ipv4_cidrs_overlap,
        parse_upstream_subnet,
    },
    db::{
        account_policy::{AccountPolicyPatch, PlatformUser},
        controller_devices::{ControllerDevice, DEVICE_ONLINE_SECONDS, DeviceReport},
    },
};

type Service = Extension<Arc<CentralNetworkService>>;

fn failure(status: StatusCode, message: &str) -> HttpHandleError {
    (status, Json(other_error(message)))
}

#[derive(Deserialize)]
struct Heartbeat {
    device_id: Uuid,
    config_token: String,
    pairing_code: String,
    hostname: String,
    version: String,
    #[serde(default)]
    node_measurements: Option<Vec<NodeMeasurement>>,
    #[serde(default)]
    observed_paths: Vec<ObservedClientPath>,
    #[serde(default)]
    account_transport_policy_supported: bool,
    #[serde(default)]
    extended_transports_supported: bool,
    #[serde(default)]
    subnet_proxy_cidrs: Option<Vec<String>>,
    #[serde(default)]
    subnet_proxy_fallback_cidr: Option<String>,
}

#[derive(Serialize)]
struct HeartbeatReply {
    bound: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    bound_account: Option<String>,
    device_login_supported: bool,
    device_login_revision: i64,
    device_code: String,
    device_name: String,
    device_code_claim_supported: bool,
    renew_pairing_code: bool,
    config_protocol: String,
    config_host: Option<String>,
    config_port: u16,
    candidates: Vec<NodeCandidate>,
    udp_transport_enabled: bool,
    // Retained for clients released before the account-level policy.
    udp_transport_pilot_enabled: bool,
    networks: Vec<AssignedNetwork>,
    upstream_subnet_supported: bool,
    upstream_subnet_cidr: Option<String>,
}

fn lowercase_hex(input: &str, length: usize) -> bool {
    input.len() == length
        && input
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn enrollment_identity(headers: &HeaderMap) -> Result<&str, HttpHandleError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "))
        .filter(|value| lowercase_hex(value, 64))
        .ok_or_else(|| failure(StatusCode::UNAUTHORIZED, "device identity is required"))
}

fn reported_subnet_proxy(
    cidrs: Option<&[String]>,
) -> Result<Option<Vec<cidr::Ipv4Cidr>>, HttpHandleError> {
    let (local, upstream) = match cidrs {
        None => return Ok(None),
        Some([]) => return Ok(Some(Vec::new())),
        Some([local]) => (local, None),
        Some([local, upstream]) => (local, Some(upstream.as_str())),
        Some(_) => return Err(invalid_automatic_subnet()),
    };
    let local = local
        .parse::<cidr::Ipv4Cidr>()
        .ok()
        .filter(|subnet| {
            let address = subnet.first_address();
            subnet.network_length() == 24
                && !matches!(address.octets()[0], 0 | 224..=255)
                && !address.is_loopback()
                && !address.is_link_local()
        })
        .ok_or_else(invalid_automatic_subnet)?;
    let mut routes = vec![local];
    if let Some(upstream) = upstream {
        let upstream = parse_upstream_subnet(upstream).map_err(convert_error)?;
        if ipv4_cidrs_overlap(&local, &upstream) {
            return Err(invalid_automatic_subnet());
        }
        routes.push(upstream);
    }
    Ok(Some(routes))
}

fn invalid_automatic_subnet() -> HttpHandleError {
    failure(
        StatusCode::BAD_REQUEST,
        "invalid automatic subnet proxy CIDR",
    )
}

async fn heartbeat(
    Extension(service): Service,
    Extension(config): Extension<ConsoleInfoConfig>,
    headers: HeaderMap,
    Json(request): Json<Heartbeat>,
) -> Result<Json<HeartbeatReply>, HttpHandleError> {
    let identity = enrollment_identity(&headers)?;
    if request.device_id.get_version_num() != 4
        || !lowercase_hex(&request.pairing_code, 16)
        || !request
            .config_token
            .strip_prefix(DEVICE_TOKEN_PREFIX)
            .is_some_and(|token| lowercase_hex(token, 64))
        || request.hostname.len() > 255
        || request.version.len() > 120
    {
        return Err(failure(StatusCode::BAD_REQUEST, "invalid device report"));
    }
    let subnet_proxy = reported_subnet_proxy(request.subnet_proxy_cidrs.as_deref())?;
    let fallback_confirmation = request
        .subnet_proxy_fallback_cidr
        .as_deref()
        .map(parse_upstream_subnet)
        .transpose()
        .map_err(convert_error)?;
    if fallback_confirmation.is_some() && !matches!(subnet_proxy.as_deref(), Some([_])) {
        return Err(invalid_automatic_subnet());
    }

    let enrollment = service
        .db
        .report_controller_device(DeviceReport {
            device_id: request.device_id,
            identity,
            config_token: &request.config_token,
            pairing_code: &request.pairing_code,
            hostname: &request.hostname,
            version: &request.version,
            account_transport_policy_supported: request.account_transport_policy_supported,
            extended_transports_supported: request.extended_transports_supported,
        })
        .await
        .map_err(convert_db_error)?;
    let Some(enrollment) = enrollment else {
        return Err(failure(
            StatusCode::UNAUTHORIZED,
            "device identity does not match",
        ));
    };
    let owner = enrollment.owner_id;
    let policy = if let Some(owner) = owner {
        service
            .db
            .account_policy(owner)
            .await
            .map_err(convert_db_error)?
            .unwrap_or_default()
    } else {
        Default::default()
    };
    if policy.banned {
        return Err(failure(StatusCode::FORBIDDEN, "account suspended"));
    }

    let upstream_subnet_cidr = match (owner, subnet_proxy) {
        (Some(owner), Some(routes)) => service
            .update_controller_subnet_proxy(
                owner,
                request.device_id,
                &routes,
                fallback_confirmation,
            )
            .await
            .map_err(convert_error)?,
        (Some(_), None) => enrollment.upstream_subnet_cidr,
        (None, _) => None,
    };

    let candidates = if owner.is_some() {
        service.scheduler.observe(
            request.device_id,
            request.node_measurements,
            request.observed_paths,
        );
        service.node_candidates().await.map_err(convert_error)?
    } else {
        Vec::new()
    };
    let networks = if let Some(owner) = owner {
        sqlx::query_as::<_, (String, String)>(
            "SELECT n.id, n.display_name FROM network_members m
             JOIN networks n ON n.user_id = m.user_id AND n.id = m.network_id
             WHERE m.user_id = ? AND m.device_id = ? ORDER BY n.display_name, n.id",
        )
        .bind(owner)
        .bind(request.device_id.to_string())
        .fetch_all(&service.db.inner())
        .await
        .map_err(crate::db::sqlx_db_error)
        .map_err(convert_db_error)?
        .into_iter()
        .map(|(network_id, display_name)| AssignedNetwork {
            network_id,
            display_name,
        })
        .collect()
    } else {
        Vec::new()
    };

    Ok(Json(HeartbeatReply {
        bound: owner.is_some(),
        bound_account: enrollment.bound_account,
        device_login_supported: true,
        device_login_revision: enrollment.device_login_revision,
        device_code: enrollment.device_code,
        device_name: enrollment.device_name,
        device_code_claim_supported: true,
        renew_pairing_code: enrollment.renew_pairing_code,
        config_protocol: config.config_server_protocol,
        config_host: config.config_server_host,
        config_port: config.config_server_port,
        candidates,
        udp_transport_enabled: policy.smart_enabled(),
        udp_transport_pilot_enabled: policy.smart_enabled(),
        networks,
        upstream_subnet_supported: true,
        upstream_subnet_cidr,
    }))
}

#[derive(Serialize, sqlx::FromRow)]
struct Assignment {
    #[serde(skip)]
    device_id: String,
    network_id: String,
    display_name: String,
    gateway: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip)]
    managed: bool,
}

#[derive(Serialize)]
struct DeviceView {
    #[serde(flatten)]
    device: ControllerDevice,
    online: bool,
    config_connected: bool,
    assignments: Vec<Assignment>,
}

async fn devices(
    service: &CentralNetworkService,
    manager: &AppStateInner,
    owner_id: Option<i32>,
) -> Result<Vec<DeviceView>, HttpHandleError> {
    let assignments = sqlx::query_as::<_, Assignment>(
        r#"
        SELECT m.device_id, n.id AS network_id, n.display_name,
            CASE WHEN n.networking_method = 'Gateway' THEN json_extract(n.peer_urls, '$[0]') END AS gateway,
            NULL AS path,
            COALESCE(c.expiry_unix = ?, 0) AS managed
        FROM controller_devices d
        JOIN network_members m ON m.user_id = d.owner_id AND m.device_id = d.device_id
        JOIN networks n ON n.user_id = m.user_id AND n.id = m.network_id
        LEFT JOIN network_credentials c
            ON c.user_id = m.user_id AND c.network_id = m.network_id AND c.credential_id = m.credential_id
        WHERE (? IS NULL OR d.owner_id = ?) ORDER BY n.display_name, n.id
        "#,
    )
    .bind(crate::central_network::service::CONTROLLER_MEMBERSHIP_EXPIRATION)
    .bind(owner_id)
    .bind(owner_id)
    .fetch_all(&service.db.inner())
    .await
    .map_err(crate::db::sqlx_db_error)
    .map_err(convert_db_error)?;
    let mut by_device = HashMap::<String, Vec<Assignment>>::new();
    let nodes = service.node_candidates().await.map_err(convert_error)?;
    for mut assignment in assignments {
        if assignment.managed
            && assignment.gateway.is_some()
            && let (Ok(network), Ok(device)) = (
                Uuid::parse_str(&assignment.network_id),
                Uuid::parse_str(&assignment.device_id),
            )
        {
            assignment.gateway = None;
            if let Some((id, path)) = service.scheduler.assignment(network, device)
                && let Some(node) = nodes.iter().find(|node| node.id == id)
            {
                assignment.gateway = Some(node.peer_url.clone());
                assignment.path = Some(path.to_owned());
            }
        }
        by_device
            .entry(assignment.device_id.clone())
            .or_default()
            .push(assignment);
    }
    let cutoff = chrono::Utc::now().timestamp() - DEVICE_ONLINE_SECONDS;
    let banned_owners: HashSet<_> =
        sqlx::query_scalar::<_, i32>("SELECT id FROM users WHERE banned")
            .fetch_all(&service.db.inner())
            .await
            .map_err(crate::db::sqlx_db_error)
            .map_err(convert_db_error)?
            .into_iter()
            .collect();
    Ok(service
        .db
        .list_controller_devices(owner_id)
        .await
        .map_err(convert_db_error)?
        .into_iter()
        .map(|device| {
            let banned = device
                .owner_id
                .is_some_and(|owner| banned_owners.contains(&owner));
            let config_connected = device
                .owner_id
                .zip(Uuid::parse_str(&device.device_id).ok())
                .is_some_and(|(owner, id)| manager.get_session_by_machine_id(owner, &id).is_some());
            DeviceView {
                online: !banned && device.last_seen >= cutoff,
                config_connected: !banned && config_connected,
                assignments: by_device.remove(&device.device_id).unwrap_or_default(),
                device,
            }
        })
        .collect())
}

async fn user_devices(
    auth: AuthSession,
    Extension(service): Service,
    State(manager): State<AppStateInner>,
) -> Result<Json<Vec<DeviceView>>, HttpHandleError> {
    let owner = authed_user_id(&auth)?;
    Ok(Json(devices(&service, &manager, Some(owner)).await?))
}

#[derive(Deserialize)]
struct Claim {
    device_id: String,
    pairing_code: Option<String>,
}

fn is_short_device_code(identifier: &str) -> bool {
    identifier.len() == 8 && identifier.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

impl Claim {
    fn credentials(self) -> Result<(String, Option<String>), HttpHandleError> {
        let identifier = self.device_id.trim().to_ascii_lowercase();
        if is_short_device_code(&identifier) {
            return Ok((identifier, None));
        }

        // Older clients that still submit a device UUID must also submit the legacy proof.
        if let Ok(device_id) = Uuid::parse_str(&identifier) {
            let code = self
                .pairing_code
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if lowercase_hex(&code, 16) {
                return Ok((device_id.to_string(), Some(code)));
            }
            return Err(failure(
                StatusCode::CONFLICT,
                "device is already bound or the pairing code expired",
            ));
        }

        Err(failure(StatusCode::BAD_REQUEST, "invalid device number"))
    }
}

async fn claim_device(
    auth: AuthSession,
    Extension(service): Service,
    Json(claim): Json<Claim>,
) -> Result<StatusCode, HttpHandleError> {
    let owner = authed_user_id(&auth)?;
    let (identifier, pairing_code) = claim.credentials()?;
    let _mutation = service.lock_mutations().await;
    let device_id = lookup_device(&service, &identifier).await?;
    if !service
        .db
        .bind_controller_device(device_id, owner, pairing_code.as_deref())
        .await
        .map_err(convert_db_error)?
    {
        return Err(failure(
            StatusCode::CONFLICT,
            if pairing_code.is_some() {
                "device is already bound or the pairing code expired"
            } else {
                "device is already bound or not registered"
            },
        ));
    }
    tracing::info!(%owner, %device_id, "controller device claimed");
    Ok(StatusCode::NO_CONTENT)
}

async fn unbind_device(
    auth: AuthSession,
    Extension(service): Service,
    Path(device_id): Path<Uuid>,
) -> Result<StatusCode, HttpHandleError> {
    let owner = authed_user_id(&auth)?;
    if !service
        .db
        .owns_controller_device(owner, device_id)
        .await
        .map_err(convert_db_error)?
        || !service
            .delete_device(owner, device_id, false)
            .await
            .map_err(convert_error)?
    {
        return Err(failure(StatusCode::NOT_FOUND, "bound device not found"));
    }
    tracing::info!(%owner, %device_id, "controller device unbound");
    Ok(StatusCode::NO_CONTENT)
}

async fn require_platform_admin(auth: &AuthSession) -> Result<i32, HttpHandleError> {
    let id = authed_user_id(auth)?;
    if !auth
        .backend
        .is_platform_admin(id)
        .await
        .map_err(convert_db_error)?
    {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "administrator access required",
        ));
    }
    if auth
        .user
        .as_ref()
        .is_some_and(|user| user.db_user.password == crate::migrator::INITIAL_ADMIN_PASSWORD_HASH)
    {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "change the default administrator password first",
        ));
    }
    Ok(id)
}

#[derive(Serialize)]
struct PlatformUsers {
    users: Vec<PlatformUser>,
}

async fn admin_users(
    auth: AuthSession,
    Extension(service): Service,
) -> Result<Json<PlatformUsers>, HttpHandleError> {
    require_platform_admin(&auth).await?;
    Ok(Json(PlatformUsers {
        users: service
            .db
            .list_platform_users()
            .await
            .map_err(convert_db_error)?,
    }))
}

async fn admin_delete_user(
    auth: AuthSession,
    Extension(service): Service,
    Path(user_id): Path<i32>,
    Query(input): Query<ConfirmDeletion>,
) -> Result<StatusCode, HttpHandleError> {
    let admin = require_platform_admin(&auth).await?;
    if !input.confirm {
        return Err(failure(StatusCode::CONFLICT, "confirm account deletion"));
    }
    let deleted =
        tokio::spawn(async move { service.delete_platform_account(admin, user_id).await })
            .await
            .map_err(|error| {
                tracing::error!(%error, %user_id, "account deletion task stopped");
                failure(StatusCode::INTERNAL_SERVER_ERROR, "account deletion failed")
            })?
            .map_err(convert_error)?;
    if !deleted {
        return Err(failure(StatusCode::NOT_FOUND, "user does not exist"));
    }
    tracing::info!(%admin, %user_id, "administrator deleted account");
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_update_account_policy(
    auth: AuthSession,
    Extension(service): Service,
    Path(user_id): Path<i32>,
    Json(patch): Json<AccountPolicyPatch>,
) -> Result<Json<PlatformUser>, HttpHandleError> {
    let admin = require_platform_admin(&auth).await?;
    if patch.banned == Some(true) && user_id == admin {
        return Err(failure(StatusCode::CONFLICT, "cannot ban your own account"));
    }
    if patch.banned.is_none()
        && patch.udp_enabled.is_none()
        && patch.force_relay.is_none()
        && patch.forced_protocol.is_none()
    {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "no account policy changes supplied",
        ));
    }
    let user = service
        .apply_account_policy(admin, user_id, patch)
        .await
        .map_err(convert_error)?
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "user does not exist"))?;
    tracing::info!(%admin, %user_id, ?patch, "administrator updated account connection policy");
    Ok(Json(user))
}

#[derive(Serialize)]
struct PlatformNetwork {
    owner_id: i32,
    #[serde(flatten)]
    network: NetworkSummary,
}

#[derive(Serialize)]
struct Overview {
    users: Vec<PlatformUser>,
    networks: Vec<PlatformNetwork>,
    devices: Vec<DeviceView>,
    gateway: Option<String>,
    gateway_forwarding: bool,
    forwarding_nodes: Vec<NodeOverview>,
}

#[derive(Serialize)]
struct NodeOverview {
    #[serde(flatten)]
    node: ForwardingNode,
    online: bool,
}

async fn overview(
    auth: AuthSession,
    Extension(service): Service,
    State(manager): State<AppStateInner>,
) -> Result<Json<Overview>, HttpHandleError> {
    require_platform_admin(&auth).await?;
    let users = service
        .db
        .list_platform_users()
        .await
        .map_err(convert_db_error)?;
    let mut networks = Vec::new();
    for user in &users {
        for network in service
            .list_networks(user.id)
            .await
            .map_err(convert_error)?
        {
            networks.push(PlatformNetwork {
                owner_id: user.id,
                network,
            });
        }
    }
    let gateway = service.gateway_config();
    let forwarding_nodes = forwarding::list_nodes(&service.db)
        .await
        .map_err(crate::db::sqlx_db_error)
        .map_err(convert_db_error)?
        .into_iter()
        .map(|node| NodeOverview {
            online: node.online(),
            node,
        })
        .collect();
    Ok(Json(Overview {
        users,
        networks,
        devices: devices(&service, &manager, None).await?,
        gateway: gateway.map(|gateway| gateway.peer_url.clone()),
        gateway_forwarding: gateway.is_some_and(|gateway| gateway.relay_data),
        forwarding_nodes,
    }))
}

#[derive(Deserialize)]
struct AdminBind {
    owner_id: i32,
}

async fn admin_bind(
    auth: AuthSession,
    Extension(service): Service,
    Path(identifier): Path<String>,
    Json(input): Json<AdminBind>,
) -> Result<StatusCode, HttpHandleError> {
    let admin = require_platform_admin(&auth).await?;
    let _mutation = service.lock_mutations().await;
    let device_id = lookup_device(&service, &identifier).await?;
    let exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM users WHERE id = ?)")
        .bind(input.owner_id)
        .fetch_one(&service.db.inner())
        .await
        .map_err(crate::db::sqlx_db_error)
        .map_err(convert_db_error)?;
    if !exists {
        return Err(failure(StatusCode::NOT_FOUND, "user does not exist"));
    }
    if !service
        .db
        .bind_controller_device(device_id, input.owner_id, None)
        .await
        .map_err(convert_db_error)?
    {
        return Err(failure(
            StatusCode::CONFLICT,
            "device is already bound or not registered",
        ));
    }
    tracing::info!(%admin, owner = input.owner_id, %device_id, "administrator bound controller device");
    Ok(StatusCode::NO_CONTENT)
}

async fn lookup_device(
    service: &CentralNetworkService,
    identifier: &str,
) -> Result<Uuid, HttpHandleError> {
    let device = service
        .db
        .find_controller_device(identifier.trim())
        .await
        .map_err(convert_db_error)?
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "device not found"))?;
    Uuid::parse_str(&device.device_id).map_err(|_| {
        failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid stored device identity",
        )
    })
}

#[derive(Deserialize)]
struct UpdateUpstreamSubnet {
    #[serde(deserialize_with = "Deserialize::deserialize")]
    upstream_subnet_cidr: Option<String>,
}

async fn admin_update_upstream_subnet(
    auth: AuthSession,
    Extension(service): Service,
    Path(identifier): Path<String>,
    Json(input): Json<UpdateUpstreamSubnet>,
) -> Result<StatusCode, HttpHandleError> {
    let admin = require_platform_admin(&auth).await?;
    let cidr = input
        .upstream_subnet_cidr
        .as_deref()
        .map(str::trim)
        .map(parse_upstream_subnet)
        .transpose()
        .map_err(convert_error)?;
    if !service
        .set_controller_upstream_subnet(identifier.trim(), cidr)
        .await
        .map_err(convert_error)?
    {
        return Err(failure(StatusCode::NOT_FOUND, "bound device not found"));
    }
    tracing::info!(%admin, device = %identifier, "administrator updated controller device upstream subnet");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ConfirmDeletion {
    #[serde(default)]
    confirm: bool,
}

async fn admin_delete_device(
    auth: AuthSession,
    Extension(service): Service,
    Path(identifier): Path<String>,
    Query(input): Query<ConfirmDeletion>,
) -> Result<StatusCode, HttpHandleError> {
    let admin = require_platform_admin(&auth).await?;
    if !service
        .retire_controller_device(identifier.trim(), input.confirm, None)
        .await
        .map_err(convert_error)?
    {
        return Err(failure(StatusCode::NOT_FOUND, "device not found"));
    }
    tracing::info!(%admin, device = %identifier, "controller device deleted");
    Ok(StatusCode::NO_CONTENT)
}

async fn user_delete_device(
    auth: AuthSession,
    Extension(service): Service,
    Path(identifier): Path<String>,
    Query(input): Query<ConfirmDeletion>,
) -> Result<StatusCode, HttpHandleError> {
    let owner = authed_user_id(&auth)?;
    if !service
        .retire_controller_device(identifier.trim(), input.confirm, Some(owner))
        .await
        .map_err(convert_error)?
    {
        return Err(failure(StatusCode::NOT_FOUND, "bound device not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct CreateNetwork {
    display_name: String,
    virtual_cidr: Option<String>,
}

#[derive(Serialize)]
struct CreatedNetwork {
    network_id: String,
}

async fn create_network(
    auth: AuthSession,
    Extension(service): Service,
    Json(input): Json<CreateNetwork>,
) -> Result<Json<CreatedNetwork>, HttpHandleError> {
    let owner = authed_user_id(&auth)?;
    create_managed_network(&service, owner, input).await
}

async fn admin_create_network(
    auth: AuthSession,
    Extension(service): Service,
    Path(owner): Path<i32>,
    Json(input): Json<CreateNetwork>,
) -> Result<Json<CreatedNetwork>, HttpHandleError> {
    let admin = require_platform_admin(&auth).await?;
    let result = create_managed_network(&service, owner, input).await?;
    tracing::info!(%admin, %owner, network_id = %result.network_id, "administrator created managed network");
    Ok(result)
}

async fn create_managed_network(
    service: &CentralNetworkService,
    owner: i32,
    input: CreateNetwork,
) -> Result<Json<CreatedNetwork>, HttpHandleError> {
    if service
        .node_candidates()
        .await
        .map_err(convert_error)?
        .is_empty()
    {
        return Err(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "there is no forwarding node configured",
        ));
    }
    let display_name = input.display_name.trim();
    if display_name.is_empty() || display_name.len() > 120 {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "network name is required and must be at most 120 bytes",
        ));
    }
    let settings = NetworkSettings {
        display_name: display_name.to_owned(),
        network_name: Some(format!("bilin-{}", Uuid::new_v4())),
        networking_method: "Gateway".to_owned(),
        public_server_url: None,
        peer_urls: Vec::new(),
        virtual_cidr: input.virtual_cidr,
        secure_mode: true,
    };
    let detail = service
        .create_controller_network(owner, settings)
        .await
        .map_err(convert_error)?;
    Ok(Json(CreatedNetwork {
        network_id: detail.network_id,
    }))
}

async fn admin_members(
    auth: AuthSession,
    Extension(service): Service,
    Path((owner, network_id)): Path<(i32, Uuid)>,
) -> Result<Json<MembersView>, HttpHandleError> {
    require_platform_admin(&auth).await?;
    Ok(Json(
        service
            .list_members(owner, network_id)
            .await
            .map_err(convert_error)?,
    ))
}

async fn admin_remove_device(
    auth: AuthSession,
    Extension(service): Service,
    Path((owner, network_id, device_id)): Path<(i32, Uuid, Uuid)>,
) -> Result<StatusCode, HttpHandleError> {
    let admin = require_platform_admin(&auth).await?;
    service
        .remove_member(owner, network_id, device_id)
        .await
        .map_err(convert_error)?;
    tracing::info!(%admin, %owner, %network_id, %device_id, "administrator removed network member");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct MemberSubnets {
    proxy_cidrs: Vec<String>,
}

async fn admin_update_member_subnets(
    auth: AuthSession,
    Extension(service): Service,
    Path((owner, network_id, device_id)): Path<(i32, Uuid, Uuid)>,
    Json(input): Json<MemberSubnets>,
) -> Result<StatusCode, HttpHandleError> {
    let admin = require_platform_admin(&auth).await?;
    service
        .update_legacy_admin_member_subnets(owner, network_id, device_id, input.proxy_cidrs)
        .await
        .map_err(convert_error)?;
    tracing::info!(%admin, %owner, %network_id, %device_id, "administrator updated member subnet proxy");
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_delete_network(
    auth: AuthSession,
    Extension(service): Service,
    Path((owner, network_id)): Path<(i32, Uuid)>,
) -> Result<StatusCode, HttpHandleError> {
    let admin = require_platform_admin(&auth).await?;
    service
        .delete_network(owner, network_id)
        .await
        .map_err(convert_error)?;
    tracing::info!(%admin, %owner, %network_id, "administrator deleted network");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct AddDevice {
    device_id: String,
}

async fn add_device(
    auth: AuthSession,
    Extension(service): Service,
    Path(network_id): Path<Uuid>,
    Json(input): Json<AddDevice>,
) -> Result<StatusCode, HttpHandleError> {
    let owner = authed_user_id(&auth)?;
    let device_id = lookup_device(&service, &input.device_id).await?;
    add_member(&service, owner, network_id, device_id).await
}

async fn add_device_by_code(
    auth: AuthSession,
    Extension(service): Service,
    Path(network_id): Path<Uuid>,
    Json(input): Json<AddDevice>,
) -> Result<StatusCode, HttpHandleError> {
    let owner = authed_user_id(&auth)?;
    let device_code = input.device_id.trim().to_ascii_lowercase();
    if !is_short_device_code(&device_code) {
        return Err(failure(StatusCode::BAD_REQUEST, "invalid device number"));
    }
    if !service
        .add_controller_member_by_code(owner, network_id, &device_code)
        .await
        .map_err(convert_error)?
    {
        return Err(failure(StatusCode::NOT_FOUND, "device not found"));
    }
    tracing::info!(%owner, %network_id, "user added network member by device code");
    Ok(StatusCode::NO_CONTENT)
}

async fn admin_add_device(
    auth: AuthSession,
    Extension(service): Service,
    Path((owner, network_id)): Path<(i32, Uuid)>,
    Json(input): Json<AddDevice>,
) -> Result<StatusCode, HttpHandleError> {
    let admin = require_platform_admin(&auth).await?;
    let device_id = lookup_device(&service, &input.device_id).await?;
    let result = add_member(&service, owner, network_id, device_id).await?;
    tracing::info!(%admin, %owner, %network_id, device_id = %input.device_id, "administrator added network member");
    Ok(result)
}

async fn add_member(
    service: &CentralNetworkService,
    owner: i32,
    network_id: Uuid,
    device_id: Uuid,
) -> Result<StatusCode, HttpHandleError> {
    service
        .add_controller_member(owner, network_id, device_id)
        .await
        .map_err(convert_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct NewNode {
    name: String,
    peer_url: String,
}

#[derive(Serialize)]
struct NewNodeResult {
    id: Uuid,
    registration_token: String,
}

async fn create_forwarding_node(
    auth: AuthSession,
    Extension(service): Service,
    Json(request): Json<NewNode>,
) -> Result<Json<NewNodeResult>, HttpHandleError> {
    let admin = require_platform_admin(&auth).await?;
    let name = request.name.trim();
    let url = forwarding::validate_node_url(request.peer_url.trim())
        .map_err(|_| failure(StatusCode::BAD_REQUEST, "a TCP host and port are required"))?;
    if name.is_empty()
        || name.len() > 100
        || service
            .gateway_config()
            .is_some_and(|node| node.peer_url == url.as_str())
    {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "invalid node name or endpoint",
        ));
    }
    let (id, registration_token) = forwarding::create_node(&service.db, name, url.as_str())
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to create forwarding node");
            failure(
                StatusCode::CONFLICT,
                "could not create node, check for a duplicate endpoint",
            )
        })?;
    tracing::info!(%admin, node_id = %id, "administrator registered a forwarding node");
    Ok(Json(NewNodeResult {
        id,
        registration_token,
    }))
}

async fn forwarding_heartbeat(
    Extension(service): Service,
    headers: HeaderMap,
    Json(mut report): Json<NodeReport>,
) -> Result<Json<NodeSnapshot>, HttpHandleError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| {
            token
                .strip_prefix("bln1_")
                .is_some_and(|value| lowercase_hex(value, 64))
        })
        .ok_or_else(|| failure(StatusCode::UNAUTHORIZED, "invalid node token"))?;
    let own = forwarding::node_heartbeat(&service.db, token, report.ready)
        .await
        .map_err(crate::db::sqlx_db_error)
        .map_err(convert_db_error)?
        .ok_or_else(|| failure(StatusCode::UNAUTHORIZED, "invalid node token"))?;
    let node_id = Uuid::parse_str(&own.id)
        .map_err(|_| failure(StatusCode::INTERNAL_SERVER_ERROR, "invalid stored node ID"))?;
    let node_url = forwarding::validate_node_url(&own.peer_url).map_err(|_| {
        failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid stored node address",
        )
    })?;
    report.wss_url = report.wss_url.take().filter(|endpoint| {
        endpoint
            .parse::<url::Url>()
            .is_ok_and(|endpoint| forwarding::validate_node_wss_url(&node_url, &endpoint).is_ok())
    });
    service.scheduler.observe_node(node_id, report);
    let mut mesh_peers: Vec<_> = service
        .node_candidates()
        .await
        .map_err(convert_error)?
        .into_iter()
        .filter_map(|candidate| (candidate.peer_url != own.peer_url).then_some(candidate.peer_url))
        .collect();
    mesh_peers.sort();
    mesh_peers.dedup();
    let udp_enabled_accounts = sqlx::query_scalar::<_, i32>(
        "SELECT id FROM users WHERE NOT banned AND NOT force_relay AND
          ((forced_protocol = 'auto' AND udp_enabled) OR forced_protocol NOT IN ('auto', 'tcp')) ORDER BY id",
    )
    .fetch_all(&service.db.inner())
    .await
    .map_err(crate::db::sqlx_db_error)
    .map_err(convert_db_error)?;
    Ok(Json(NodeSnapshot {
        peer_url: own.peer_url,
        mesh_peers,
        networks: service.forwarding_networks().await.map_err(convert_error)?,
        udp_enabled_accounts: Some(udp_enabled_accounts),
    }))
}

pub(super) fn authenticated_routes() -> Router<AppStateInner> {
    Router::new()
        .route(
            "/api/v1/workspace/device-login",
            post(device_login::approve),
        )
        .route(
            "/api/v1/workspace/mobile/device",
            post(mobile_account::bind),
        )
        .route("/api/v1/workspace/devices", get(user_devices))
        .route("/api/v1/workspace/devices/claim", post(claim_device))
        .route(
            "/api/v1/workspace/devices/{device_id}",
            delete(unbind_device),
        )
        .route(
            "/api/v1/workspace/devices/{device_id}/registration",
            delete(user_delete_device),
        )
        .route(
            "/api/v1/workspace/devices/{device_id}/name",
            patch(device_name::rename_owner_device),
        )
        .route("/api/v1/workspace/networks", post(create_network))
        .route(
            "/api/v1/workspace/networks/{network_id}/devices",
            post(add_device),
        )
        .route(
            "/api/v1/workspace/networks/{network_id}/devices/by-code",
            post(add_device_by_code),
        )
        .route("/api/v1/admin/overview", get(overview))
        .route("/api/v1/admin/users", get(admin_users))
        .route("/api/v1/admin/users/{owner}", delete(admin_delete_user))
        .route(
            "/api/v1/admin/users/{owner}/policy",
            patch(admin_update_account_policy),
        )
        .route("/api/v1/admin/nodes", post(create_forwarding_node))
        .route(
            "/api/v1/admin/devices/{device_id}",
            delete(admin_delete_device),
        )
        .route("/api/v1/admin/devices/{device_id}/bind", post(admin_bind))
        .route(
            "/api/v1/admin/devices/{device_id}/upstream-subnet",
            patch(admin_update_upstream_subnet),
        )
        .route(
            "/api/v1/admin/devices/{device_id}/name",
            patch(device_name::rename_admin_device),
        )
        .route(
            "/api/v1/admin/users/{owner}/networks",
            post(admin_create_network),
        )
        .route(
            "/api/v1/admin/users/{owner}/networks/{network_id}",
            delete(admin_delete_network),
        )
        .route(
            "/api/v1/admin/users/{owner}/networks/{network_id}/devices",
            get(admin_members).post(admin_add_device),
        )
        .route(
            "/api/v1/admin/users/{owner}/networks/{network_id}/devices/{device_id}",
            delete(admin_remove_device).patch(admin_update_member_subnets),
        )
}

pub(super) fn enrollment_routes() -> Router<AppStateInner> {
    Router::new()
        .route("/api/v1/enrollment/heartbeat", post(heartbeat))
        .route(
            "/api/v1/enrollment/device-name",
            get(device_name::read_local_device).post(device_name::rename_local_device),
        )
        .route(
            "/api/v1/enrollment/mobile/status",
            post(mobile_account::status),
        )
        .route("/api/v1/enrollment/mobile/join", post(mobile_account::join))
        .route(
            "/api/v1/enrollment/mobile/devices",
            post(mobile_account::devices),
        )
        .route("/api/v1/enrollment/login/start", post(device_login::start))
        .route(
            "/api/v1/enrollment/login/details",
            post(device_login::details),
        )
        .route(
            "/api/v1/forwarding-nodes/heartbeat",
            post(forwarding_heartbeat),
        )
}

#[cfg(test)]
mod tests {
    use super::{
        Claim, StatusCode, UpdateUpstreamSubnet, parse_upstream_subnet, reported_subnet_proxy,
    };

    #[test]
    fn claim_accepts_device_number_alone_and_keeps_legacy_uuid_proof() {
        let current: Claim = serde_json::from_str(r#"{"device_id":" AbCd1234 "}"#).unwrap();
        assert_eq!(current.credentials().unwrap(), ("abcd1234".into(), None));

        let old: Claim =
            serde_json::from_str(r#"{"device_id":"abcd1234","pairing_code":"old"}"#).unwrap();
        assert_eq!(old.credentials().unwrap(), ("abcd1234".into(), None));

        let uuid = "9ffb192f-3961-4db2-9bc3-1ac07a7aa892";
        let by_uuid = Claim {
            device_id: uuid.into(),
            pairing_code: None,
        };
        assert_eq!(by_uuid.credentials().unwrap_err().0, StatusCode::CONFLICT);

        let legacy = Claim {
            device_id: uuid.into(),
            pairing_code: Some("0123456789ABCDEF".into()),
        };
        assert_eq!(
            legacy.credentials().unwrap(),
            (uuid.into(), Some("0123456789abcdef".into()))
        );

        let invalid = Claim {
            device_id: "abcd-123".into(),
            pairing_code: None,
        };
        assert_eq!(
            invalid.credentials().unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn automatic_subnet_report_distinguishes_absence_and_clear() {
        assert!(reported_subnet_proxy(None).unwrap().is_none());
        assert!(
            reported_subnet_proxy(Some(&[]))
                .unwrap()
                .unwrap()
                .is_empty()
        );
        let subnets = reported_subnet_proxy(Some(&["192.179.1.0/24".to_owned()]))
            .unwrap()
            .unwrap();
        assert_eq!(subnets, ["192.179.1.0/24".parse().unwrap()]);
        for invalid in [
            "192.179.1.0/23",
            "2001:db8::/24",
            "127.0.0.0/24",
            "224.0.0.0/24",
        ] {
            assert!(reported_subnet_proxy(Some(&[invalid.to_owned()])).is_err());
        }
        let multiple = vec!["192.168.1.0/24".to_owned(); 2];
        assert!(reported_subnet_proxy(Some(&multiple)).is_err());
    }

    #[test]
    fn upstream_subnet_requires_an_explicit_disjoint_private_prefix() {
        let local = "192.168.100.0/24".to_owned();
        let upper = "10.168.10.0/23".to_owned();
        let reported = reported_subnet_proxy(Some(&[local.clone(), upper.clone()]))
            .unwrap()
            .unwrap();
        assert_eq!(reported.len(), 2);
        assert_eq!(reported[1].to_string(), upper);
        assert!(reported_subnet_proxy(Some(&[local.clone(), "192.168.0.0/16".into()])).is_err());
        assert!(reported_subnet_proxy(Some(&[local, upper, "10.168.11.0/24".into()])).is_err());
        for cidr in [
            "0.0.0.0/0",
            "10.0.0.0/8",
            "10.168.10.1/24",
            "100.64.1.0/24",
            "8.8.8.0/24",
        ] {
            assert!(parse_upstream_subnet(cidr).is_err(), "{cidr}");
        }
        assert!(parse_upstream_subnet("172.20.1.128/25").is_ok());
        assert!(serde_json::from_str::<UpdateUpstreamSubnet>("{}").is_err());
        assert!(
            serde_json::from_str::<UpdateUpstreamSubnet>(r#"{"upstream_subnet_cidr":null}"#)
                .unwrap()
                .upstream_subnet_cidr
                .is_none()
        );
    }
}

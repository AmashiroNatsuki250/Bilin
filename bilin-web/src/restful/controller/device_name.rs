use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    Service, device_login::browser_origin_is_local, enrollment_identity, failure,
    require_platform_admin,
};
use crate::{
    central_network::service::CentralNetworkService,
    db::controller_devices::{DeviceName, DeviceNameActor, DeviceNameError},
    restful::{HttpHandleError, authed_user_id, convert_db_error, users::AuthSession},
};

#[derive(Deserialize)]
pub(super) struct NameInput {
    name: String,
}

#[derive(Deserialize)]
pub(super) struct ReadLocalDevice {
    device_id: Uuid,
}

#[derive(Deserialize)]
pub(super) struct RenameLocalDevice {
    device_id: Uuid,
    device_login_revision: i64,
    name: String,
}

fn normalize_name(input: &str) -> Result<&str, HttpHandleError> {
    let name = input.trim();
    if name.is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control) {
        return Err(failure(StatusCode::BAD_REQUEST, "invalid device name"));
    }
    Ok(name)
}

fn name_error(error: DeviceNameError) -> HttpHandleError {
    let status = match error {
        DeviceNameError::Database(error) => return convert_db_error(error),
        DeviceNameError::NotFound => StatusCode::NOT_FOUND,
        DeviceNameError::InvalidIdentity => StatusCode::UNAUTHORIZED,
        DeviceNameError::RegistrationChanged => StatusCode::CONFLICT,
        DeviceNameError::AccountSuspended => StatusCode::FORBIDDEN,
    };
    failure(status, &error.to_string())
}

async fn rename(
    service: &CentralNetworkService,
    identifier: &str,
    actor: DeviceNameActor<'_>,
    input: &str,
) -> Result<DeviceName, HttpHandleError> {
    let name = normalize_name(input)?;
    let _mutation = service.lock_mutations().await;
    let (device, changed_owner) = service
        .db
        .rename_controller_device(identifier.trim(), actor, name)
        .await
        .map_err(name_error)?;
    if let Some(owner) = changed_owner {
        service.publish_user(owner).await;
    }
    Ok(device)
}

pub(super) async fn rename_owner_device(
    auth: AuthSession,
    Extension(service): Service,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(input): Json<NameInput>,
) -> Result<StatusCode, HttpHandleError> {
    let owner = authed_user_id(&auth)?;
    if !browser_origin_is_local(&headers) {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "cross-origin device rename is not allowed",
        ));
    }
    rename(
        &service,
        &identifier,
        DeviceNameActor::Owner(owner),
        &input.name,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn rename_admin_device(
    auth: AuthSession,
    Extension(service): Service,
    headers: HeaderMap,
    Path(identifier): Path<String>,
    Json(input): Json<NameInput>,
) -> Result<StatusCode, HttpHandleError> {
    require_platform_admin(&auth).await?;
    if !browser_origin_is_local(&headers) {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "cross-origin device rename is not allowed",
        ));
    }
    rename(
        &service,
        &identifier,
        DeviceNameActor::Administrator,
        &input.name,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn read_local_device(
    Extension(service): Service,
    headers: HeaderMap,
    Query(input): Query<ReadLocalDevice>,
) -> Result<impl IntoResponse, HttpHandleError> {
    let identity = enrollment_identity(&headers)?;
    if input.device_id.get_version_num() != 4 {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "invalid device registration",
        ));
    }
    let device = service
        .db
        .read_controller_device_name(input.device_id, identity)
        .await
        .map_err(name_error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(device)))
}

pub(super) async fn rename_local_device(
    Extension(service): Service,
    headers: HeaderMap,
    Json(input): Json<RenameLocalDevice>,
) -> Result<impl IntoResponse, HttpHandleError> {
    let identity = enrollment_identity(&headers)?;
    if input.device_id.get_version_num() != 4 || input.device_login_revision < 1 {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "invalid device registration",
        ));
    }
    let device = rename(
        &service,
        &input.device_id.to_string(),
        DeviceNameActor::Device {
            identity,
            revision: input.device_login_revision,
        },
        &input.name,
    )
    .await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(device)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_name_validation_matches_runtime_scalar_limit() {
        assert_eq!(normalize_name(" \t 客厅路由 \n").unwrap(), "客厅路由");
        assert!(normalize_name(&"中".repeat(32)).is_ok());
        for name in ["中".repeat(33), "   ".into(), "host\u{7}name".into()] {
            assert_eq!(
                normalize_name(&name).unwrap_err().0,
                StatusCode::BAD_REQUEST
            );
        }
    }
}

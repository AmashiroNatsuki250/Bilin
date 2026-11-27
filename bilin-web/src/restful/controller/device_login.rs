use axum::{
    Extension, Json,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

use super::{Service, enrollment_identity, failure, lowercase_hex};
use crate::{
    db::device_login::{ApproveDeviceLogin, StartDeviceLogin},
    restful::{HttpHandleError, authed_user_id, convert_db_error, users::AuthSession},
};

#[derive(Deserialize)]
pub(super) struct StartLogin {
    device_id: Uuid,
}

#[derive(Deserialize)]
pub(super) struct LoginTicket {
    ticket: String,
}

pub(super) async fn start(
    Extension(service): Service,
    headers: HeaderMap,
    Json(input): Json<StartLogin>,
) -> Result<impl IntoResponse, HttpHandleError> {
    let identity = enrollment_identity(&headers)?;
    if input.device_id.get_version_num() != 4 {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "invalid device identifier",
        ));
    }

    match service
        .db
        .start_controller_device_login(input.device_id, identity)
        .await
        .map_err(convert_db_error)?
    {
        StartDeviceLogin::Issued(ticket) => {
            Ok(([(header::CACHE_CONTROL, "no-store")], Json(ticket)))
        }
        StartDeviceLogin::Missing => Err(failure(StatusCode::NOT_FOUND, "device not registered")),
        StartDeviceLogin::Unauthorized => Err(failure(
            StatusCode::UNAUTHORIZED,
            "device identity does not match",
        )),
        StartDeviceLogin::AlreadyBound => {
            Err(failure(StatusCode::CONFLICT, "device already bound"))
        }
    }
}

fn unavailable() -> HttpHandleError {
    failure(
        StatusCode::NOT_FOUND,
        "device login ticket expired or unavailable",
    )
}

pub(super) async fn details(
    Extension(service): Service,
    Json(input): Json<LoginTicket>,
) -> Result<impl IntoResponse, HttpHandleError> {
    if !lowercase_hex(&input.ticket, 64) {
        return Err(unavailable());
    }
    let details = service
        .db
        .controller_device_login_details(&input.ticket)
        .await
        .map_err(convert_db_error)?
        .ok_or_else(unavailable)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(details)))
}

pub(super) async fn approve(
    auth: AuthSession,
    Extension(service): Service,
    headers: HeaderMap,
    Json(input): Json<LoginTicket>,
) -> Result<StatusCode, HttpHandleError> {
    let owner = authed_user_id(&auth)?;
    if !browser_origin_is_local(&headers) {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "cross-origin device login approval is not allowed",
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
    if !lowercase_hex(&input.ticket, 64) {
        return Err(unavailable());
    }

    let _mutation = service.lock_mutations().await;
    match service
        .db
        .approve_controller_device_login(&input.ticket, owner)
        .await
        .map_err(convert_db_error)?
    {
        ApproveDeviceLogin::Bound => {
            tracing::info!(%owner, "controller device bound through Web login");
            Ok(StatusCode::NO_CONTENT)
        }
        ApproveDeviceLogin::Unavailable => Err(unavailable()),
        ApproveDeviceLogin::AccountUnavailable => {
            Err(failure(StatusCode::FORBIDDEN, "account suspended"))
        }
    }
}

pub(super) fn browser_origin_is_local(headers: &HeaderMap) -> bool {
    if headers
        .get("sec-fetch-site")
        .is_some_and(|site| site != "same-origin")
    {
        return false;
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        // Non-browser API clients and older same-origin browsers may omit Origin.
        // Cross-origin browser POST requests always send it.
        return true;
    };
    let Some(origin) = origin
        .to_str()
        .ok()
        .and_then(|value| Url::parse(value).ok())
    else {
        return false;
    };
    if !matches!(origin.scheme(), "http" | "https")
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return false;
    }

    // The controller can sit behind TLS termination. Its HTTP Host retains the
    // browser's authority, while Fetch Metadata also checks the browser's scheme.
    headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .and_then(|host| Url::parse(&format!("{}://{host}", origin.scheme())).ok())
        .is_some_and(|request| request.origin() == origin.origin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approves_only_local_browser_origins() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "203.0.113.10".parse().unwrap());
        headers.insert(header::ORIGIN, "https://203.0.113.10".parse().unwrap());
        headers.insert("sec-fetch-site", "same-origin".parse().unwrap());
        assert!(browser_origin_is_local(&headers));

        headers.insert("sec-fetch-site", "same-site".parse().unwrap());
        assert!(!browser_origin_is_local(&headers));
        headers.remove("sec-fetch-site");
        headers.insert(
            header::ORIGIN,
            "https://203.0.113.10:8443".parse().unwrap(),
        );
        assert!(!browser_origin_is_local(&headers));
        headers.insert(header::ORIGIN, "null".parse().unwrap());
        assert!(!browser_origin_is_local(&headers));
    }
}

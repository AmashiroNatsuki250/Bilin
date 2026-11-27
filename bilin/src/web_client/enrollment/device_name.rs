use std::path::Path;

use anyhow::{Context as _, Result};
use serde::Deserialize;
use serde_json::json;
use url::Url;

use super::{
    DeviceEnrollment, Identity, enrollment_http_client, parse_controller_origin, read_optional_file,
};

#[derive(Debug, thiserror::Error)]
pub enum DeviceNameError {
    #[error("device name must contain 1–32 characters and no control characters")]
    InvalidName,
    #[error("device name request failed")]
    Request(#[from] reqwest::Error),
    #[error("the controller returned an invalid device name reply")]
    InvalidReply,
}

#[derive(Deserialize)]
struct NameReply {
    device_name: String,
    device_login_revision: u64,
}

fn name_endpoint(controller: &Url) -> Url {
    let mut endpoint = controller.clone();
    endpoint.set_path("/api/v1/enrollment/device-name");
    endpoint
}

async fn request_name(
    http: &reqwest::Client,
    controller: &Url,
    identity: &Identity,
    new_name: Option<&str>,
) -> Result<String, DeviceNameError> {
    let new_name = new_name.map(str::trim);
    if new_name.is_some_and(|name| {
        name.is_empty() || name.chars().any(char::is_control) || name.chars().count() > 32
    }) {
        return Err(DeviceNameError::InvalidName);
    }
    let endpoint = name_endpoint(controller);
    let mut lookup = endpoint.clone();
    lookup
        .query_pairs_mut()
        .append_pair("device_id", &identity.device_id.to_string());
    let current = http
        .get(lookup)
        .bearer_auth(&identity.secret)
        .send()
        .await?
        .error_for_status()?
        .json::<NameReply>()
        .await
        .map_err(|_| DeviceNameError::InvalidReply)?;
    let Some(name) = new_name else {
        return Ok(current.device_name);
    };

    let saved = http
        .post(endpoint)
        .bearer_auth(&identity.secret)
        .json(&json!({
            "device_id": identity.device_id,
            "device_login_revision": current.device_login_revision,
            "name": name,
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<NameReply>()
        .await
        .map_err(|_| DeviceNameError::InvalidReply)?;
    Ok(saved.device_name)
}

impl DeviceEnrollment {
    pub async fn device_name(&self) -> Result<String, DeviceNameError> {
        request_name(&self.http, &self.controller, &self.identity, None).await
    }

    pub async fn rename_device(&self, name: &str) -> Result<String, DeviceNameError> {
        let saved = request_name(&self.http, &self.controller, &self.identity, Some(name)).await?;
        self.refresh.notify_one();
        Ok(saved)
    }
}

/// Read or update the name of an already-enrolled command-line client.
pub async fn managed_device_name(
    controller: &str,
    state_dir: Option<&Path>,
    new_name: Option<&str>,
) -> Result<String> {
    let path = Identity::path(state_dir)?;
    let stored = read_optional_file(&path)?.context("请先启动比邻核心完成设备登记。")?;
    let identity = serde_json::from_str(&stored).context("无法读取本机设备身份。")?;
    let controller = parse_controller_origin(controller)?;
    let http = enrollment_http_client()?;
    Ok(request_name(&http, &controller, &identity, new_name).await?)
}

use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{
    Response, StatusCode,
    header::{COOKIE, HeaderValue, SET_COOKIE},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use url::Url;
use uuid::Uuid;

use super::{
    AssignedNetwork, DeviceEnrollment, Identity, REPORT_INTERVAL, parse_controller_origin,
};
use crate::common::machine_id::{read_optional_file, write_private_state_file};

const MOBILE_PRESENTATION_FILE: &str = "android_account_presentation.json";

#[derive(Deserialize, Serialize)]
struct MobilePresentation {
    controller_origin: Url,
    identity_fingerprint: String,
    account: MobileAccount,
}

fn identity_fingerprint(identity: &Identity) -> String {
    let mut digest = Sha256::new();
    digest.update(b"bilin/android-account-presentation/v1:");
    digest.update(identity.device_id.as_bytes());
    digest.update(identity.secret.as_bytes());
    format!("{:x}", digest.finalize())
}

/// A local presentation snapshot. It has no authority over an enrollment or tunnel.
pub fn restore_mobile_account(controller: &str, state_dir: &Path) -> Option<MobileAccount> {
    let controller = parse_controller_origin(controller).ok()?;
    if controller.scheme() != "https" {
        return None;
    }
    let identity: Identity =
        serde_json::from_str(&read_optional_file(&Identity::path(Some(state_dir)).ok()?).ok()??)
            .ok()?;
    let json = read_optional_file(&state_dir.join(MOBILE_PRESENTATION_FILE)).ok()??;
    let cached: MobilePresentation = serde_json::from_str(&json).ok()?;
    (cached.controller_origin == controller
        && cached.identity_fingerprint == identity_fingerprint(&identity)
        && cached.account.device_login_revision > 0
        && !cached.account.username.is_empty())
    .then_some(cached.account)
}

#[derive(Clone, Deserialize, Serialize)]
pub struct MobileAccount {
    pub username: String,
    pub networks: Vec<AssignedNetwork>,
    pub selected_network_id: Option<Uuid>,
    #[serde(default)]
    pub device_login_revision: u64,
}

#[derive(Deserialize, Serialize)]
pub struct MobileDeviceRoster {
    network_id: Uuid,
    devices: Vec<MobileDevice>,
}

#[derive(Deserialize, Serialize)]
struct MobileDevice {
    device_id: Uuid,
    hostname: String,
    virtual_ipv4: Option<std::net::Ipv4Addr>,
    online: bool,
}

#[derive(Clone, Serialize)]
struct DeviceProof {
    device_id: Uuid,
    device_login_revision: u64,
}

#[derive(Deserialize)]
struct MobileStatusError {
    message: String,
}

fn session_cookie(response: &Response) -> Result<HeaderValue, String> {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .filter_map(|header| header.split(';').next())
        .find(|cookie| cookie.starts_with("id=") && cookie.len() > 3)
        .and_then(|cookie| HeaderValue::from_str(cookie).ok())
        .ok_or_else(|| "控制台没有返回有效的登录会话。".into())
}

fn require_success(
    response: Response,
    message: impl FnOnce(StatusCode) -> &'static str,
) -> Result<Response, String> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(message(response.status()).into())
    }
}

fn connection_error(_: reqwest::Error) -> String {
    "无法连接控制台，请检查网络后重试。".into()
}

impl DeviceEnrollment {
    pub fn persist_mobile_account(
        &self,
        state_dir: &Path,
        account: Option<&MobileAccount>,
    ) -> anyhow::Result<()> {
        let path = state_dir.join(MOBILE_PRESENTATION_FILE);
        let Some(account) = account else {
            return match std::fs::remove_file(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                result => result.map_err(Into::into),
            };
        };
        let cached = MobilePresentation {
            controller_origin: self.controller.clone(),
            identity_fingerprint: identity_fingerprint(&self.identity),
            account: account.clone(),
        };
        let data = serde_json::to_string(&cached)?;
        if read_optional_file(&path)?.as_deref() != Some(&data) {
            write_private_state_file(&path, data.as_bytes())?;
        }
        Ok(())
    }

    fn mobile_url(&self, path: &str) -> Result<url::Url, String> {
        if self.controller.scheme() != "https" {
            return Err("安卓账号登录需要 HTTPS 控制台。".into());
        }
        self.controller
            .join(path)
            .map_err(|_| "控制台地址无效。".into())
    }

    fn device_proof(&self) -> Option<DeviceProof> {
        let status = self.status.borrow();
        status.registered.then_some(DeviceProof {
            device_id: self.identity.device_id,
            device_login_revision: status.device_login_revision?,
        })
    }

    async fn wait_device_proof(&self) -> Result<DeviceProof, String> {
        let mut status = self.subscribe();
        tokio::time::timeout(REPORT_INTERVAL, async {
            loop {
                if let Some(proof) = self.device_proof() {
                    return Ok(proof);
                }
                status.changed().await.map_err(|_| "设备登记已停止。")?;
            }
        })
        .await
        .map_err(|_| "设备还未登记，请稍后重试。")?
        .map_err(str::to_owned)
    }

    pub fn set_mobile_tunnel_revision(&self, revision: Option<u64>) {
        self.mobile_tunnel.send_if_modified(|current| {
            let changed = *current != revision;
            *current = revision;
            changed
        });
    }

    pub async fn request_mobile_captcha(&self) -> Result<String, String> {
        let response = self
            .http
            .get(self.mobile_url("/api/v1/auth/captcha")?)
            .send()
            .await
            .map_err(connection_error)?;
        let response = require_success(response, |_| "暂时无法获取验证码，请重试。")?;
        let cookie = session_cookie(&response)?;
        let png = response.bytes().await.map_err(connection_error)?;
        if !png.starts_with(b"\x89PNG\r\n\x1a\n") || png.len() > 256 * 1024 {
            return Err("控制台返回的验证码无效，请刷新。".into());
        }
        *self.mobile_captcha.lock().await = Some(cookie);
        Ok(format!("data:image/png;base64,{}", STANDARD.encode(png)))
    }

    pub async fn authenticate_mobile_account(
        &self,
        username: &str,
        password: &str,
        captcha: Option<&str>,
    ) -> Result<MobileAccount, String> {
        let username = username.trim();
        if username.is_empty() || password.is_empty() {
            return Err("请输入账号和密码。".into());
        }
        let proof = self.wait_device_proof().await?;
        let credentials =
            json!({"username": username, "password": format!("{:x}", md5::compute(password))});
        if let Some(captcha) = captcha {
            let cookie = self
                .mobile_captcha
                .lock()
                .await
                .take()
                .ok_or("请刷新验证码。")?;
            let response = self
                .http
                .post(self.mobile_url("/api/v1/auth/register")?)
                .header(COOKIE, cookie)
                .json(&json!({"credentials": credentials, "captcha": captcha.trim()}))
                .send()
                .await
                .map_err(connection_error)?;
            require_success(response, |status| match status {
                StatusCode::FORBIDDEN => "当前控制台未开放注册。",
                StatusCode::BAD_REQUEST => "注册失败，请检查账号是否已使用，或刷新验证码后重试。",
                StatusCode::TOO_MANY_REQUESTS => "操作过于频繁，请稍后重试。",
                _ => "暂时无法注册，请稍后重试。",
            })?;
        }
        let response = self
            .http
            .post(self.mobile_url("/api/v1/auth/login")?)
            .json(&credentials)
            .send()
            .await
            .map_err(connection_error)?;
        let response = require_success(response, |status| match status {
            StatusCode::UNAUTHORIZED => "账号或密码错误。",
            StatusCode::TOO_MANY_REQUESTS => "登录过于频繁，请稍后重试。",
            _ => "暂时无法登录，请稍后重试。",
        })?;
        let cookie = session_cookie(&response)?;
        let response = self
            .http
            .post(self.mobile_url("/api/v1/workspace/mobile/device")?)
            .header(COOKIE, cookie)
            .bearer_auth(&self.identity.secret)
            .json(&proof)
            .send()
            .await
            .map_err(connection_error)?;
        let response = require_success(response, |status| match status {
            StatusCode::CONFLICT => {
                "本机已绑定其他账号，或绑定状态已更新。请在控制台删除设备后重试。"
            }
            StatusCode::FORBIDDEN => "当前账号无法绑定设备，请在控制台检查账号状态。",
            StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND => {
                "设备登记状态已变化，请稍后重新登录。"
            }
            _ => "登录后绑定设备失败，请重试。",
        })?;
        Self::read_mobile_account(response, &proof).await
    }

    pub async fn mobile_account_status(&self) -> Result<Option<MobileAccount>, String> {
        let proof = self
            .device_proof()
            .ok_or("正在确认设备登记，请稍后重试。")?;
        let response = self
            .http
            .post(self.mobile_url("/api/v1/enrollment/mobile/status")?)
            .bearer_auth(&self.identity.secret)
            .json(&proof)
            .send()
            .await
            .map_err(connection_error)?;
        let account = if response.status().is_success() {
            Some(Self::read_mobile_account(response, &proof).await?)
        } else {
            let status = response.status();
            let message = response
                .json::<MobileStatusError>()
                .await
                .ok()
                .map(|body| body.message);
            if !matches!(
                (status, message.as_deref()),
                (
                    StatusCode::UNAUTHORIZED,
                    Some(
                        "device account authorization is required"
                            | "device identity does not match"
                    )
                ) | (StatusCode::FORBIDDEN, Some("account suspended"))
                    | (StatusCode::NOT_FOUND, Some("device not registered"))
            ) {
                return Err("暂时无法确认账号状态，请稍后重试。".into());
            }
            None
        };
        let latest = self.status.borrow();
        if latest.revoked || latest.account_suspended {
            Ok(None)
        } else if latest
            .device_login_revision
            .is_some_and(|revision| revision != proof.device_login_revision)
        {
            Err("设备登记状态正在更新，请稍后重试。".into())
        } else {
            Ok(account)
        }
    }

    pub async fn join_mobile_network(&self, network_id: Uuid, revision: u64) -> Result<(), String> {
        let proof = self.wait_device_proof().await?;
        if proof.device_login_revision != revision {
            return Err("设备绑定已变化，请重新登录。".into());
        }
        let response = self.http.post(self.mobile_url("/api/v1/enrollment/mobile/join")?)
            .bearer_auth(&self.identity.secret)
            .json(&json!({"device_id": proof.device_id, "device_login_revision": revision, "network_id": network_id}))
            .send().await.map_err(connection_error)?;
        require_success(response, |status| match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "设备登录已失效，请重新登录。",
            StatusCode::NOT_FOUND => "该网络已不可用，请刷新网络列表。",
            StatusCode::CONFLICT => "设备或网络状态已更新，请重试。",
            _ => "暂时无法加入网络，请重试。",
        })?;
        Ok(())
    }

    pub async fn mobile_device_roster(
        &self,
        revision: u64,
    ) -> Result<Option<MobileDeviceRoster>, String> {
        let proof = self
            .device_proof()
            .ok_or("正在确认设备登记，请稍后重试。")?;
        if proof.device_login_revision != revision {
            return Ok(None);
        }
        let response = self
            .http
            .post(self.mobile_url("/api/v1/enrollment/mobile/devices")?)
            .bearer_auth(&self.identity.secret)
            .json(&proof)
            .send()
            .await
            .map_err(connection_error)?;
        match response.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::CONFLICT => Ok(None),
            status if status.is_success() => response
                .json()
                .await
                .map(Some)
                .map_err(|_| "控制台返回的设备列表不完整。".into()),
            _ => Err("暂时无法读取设备列表，请重试。".into()),
        }
    }

    async fn read_mobile_account(
        response: Response,
        proof: &DeviceProof,
    ) -> Result<MobileAccount, String> {
        let mut account = response
            .json::<MobileAccount>()
            .await
            .map_err(|_| "控制台返回的账号信息不完整。")?;
        account.device_login_revision = proof.device_login_revision;
        Ok(account)
    }
}

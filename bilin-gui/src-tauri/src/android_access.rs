use bilin::web_client::enrollment::{
    DEFAULT_CONTROLLER_URL, DeviceEnrollment, MobileAccount, MobileDeviceRoster,
    restore_mobile_account,
};
use tauri::{AppHandle, Manager as _};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::DEVICE_ENROLLMENT;

static ACTION: Mutex<()> = Mutex::const_new(());
// Only serialize disk updates. A delayed background status request must neither
// block a sign-in nor overwrite the newer presentation it establishes.
static CACHE_GENERATION: Mutex<u64> = Mutex::const_new(0);
const NOT_READY: &str = "正在初始化控制端，请稍后重试。";

fn remember_account(app: &AppHandle, client: &DeviceEnrollment, account: Option<&MobileAccount>) {
    let result = app
        .path()
        .app_data_dir()
        .map_err(anyhow::Error::from)
        .and_then(|path| client.persist_mobile_account(&path, account));
    if let Err(error) = result {
        eprintln!("could not update Android account presentation: {error}");
    }
}

#[tauri::command]
pub fn android_access_restore(app: AppHandle) -> Option<MobileAccount> {
    let path = app.path().app_data_dir().ok()?;
    restore_mobile_account(DEFAULT_CONTROLLER_URL?, &path)
}

#[tauri::command]
pub async fn android_access_status(app: AppHandle) -> Result<Option<MobileAccount>, String> {
    let enrollment = DEVICE_ENROLLMENT.read().await;
    let client = enrollment.as_ref().ok_or(NOT_READY)?;
    let generation = *CACHE_GENERATION.lock().await;
    let account = client.mobile_account_status().await?;
    let current = CACHE_GENERATION.lock().await;
    if generation == *current {
        remember_account(&app, client, account.as_ref());
    }
    Ok(account)
}

#[tauri::command]
pub async fn android_access_forget(app: AppHandle) {
    let enrollment = DEVICE_ENROLLMENT.read().await;
    let mut generation = CACHE_GENERATION.lock().await;
    *generation += 1;
    if let Some(client) = enrollment.as_ref() {
        remember_account(&app, client, None);
    }
}

#[tauri::command]
pub async fn android_access_devices(revision: u64) -> Result<Option<MobileDeviceRoster>, String> {
    let enrollment = DEVICE_ENROLLMENT.read().await;
    enrollment
        .as_ref()
        .ok_or(NOT_READY)?
        .mobile_device_roster(revision)
        .await
}

#[tauri::command]
pub async fn android_access_captcha() -> Result<String, String> {
    let enrollment = DEVICE_ENROLLMENT.read().await;
    enrollment
        .as_ref()
        .ok_or(NOT_READY)?
        .request_mobile_captcha()
        .await
}

#[tauri::command]
pub async fn android_access_sign_in(
    app: AppHandle,
    username: String,
    password: String,
    captcha: Option<String>,
) -> Result<MobileAccount, String> {
    let _action = ACTION.lock().await;
    let enrollment = DEVICE_ENROLLMENT.read().await;
    let client = enrollment.as_ref().ok_or(NOT_READY)?;
    let generation = {
        let mut current = CACHE_GENERATION.lock().await;
        *current += 1;
        *current
    };
    let account = client
        .authenticate_mobile_account(&username, &password, captcha.as_deref())
        .await?;
    let mut current = CACHE_GENERATION.lock().await;
    if generation == *current {
        *current += 1;
        remember_account(&app, client, Some(&account));
    }
    Ok(account)
}

#[tauri::command]
pub async fn android_access_connect(network_id: Uuid, revision: u64) -> Result<(), String> {
    let _action = ACTION.lock().await;
    let enrollment = DEVICE_ENROLLMENT.read().await;
    let client = enrollment.as_ref().ok_or(NOT_READY)?;
    client.join_mobile_network(network_id, revision).await?;
    client.set_mobile_tunnel_revision(Some(revision));
    Ok(())
}

#[tauri::command]
pub async fn android_access_disconnect() -> Result<(), String> {
    let _action = ACTION.lock().await;
    if let Some(client) = DEVICE_ENROLLMENT.read().await.as_ref() {
        client.set_mobile_tunnel_revision(None);
    }
    Ok(())
}

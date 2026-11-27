use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, ensure};
use bilin_core::config::toml::ConfigLoader;
use cidr::Ipv4Cidr;
use futures::{StreamExt as _, stream};
use rand::{RngCore as _, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::{
    sync::{Notify, watch},
    task::JoinHandle,
};
use url::{Host, Url};
use uuid::Uuid;

use super::{WebClient, WebClientHooks, connect_with_identity};
use crate::{
    common::machine_id::{
        MachineIdWriteLock, default_machine_id_state_dir, read_optional_file,
        write_private_state_file,
    },
    instance::factory::NativeInstanceManager,
};

#[cfg(any(
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
mod upstream_routes;

mod device_name;
pub use device_name::{DeviceNameError, managed_device_name};

pub const DEVICE_TOKEN_PREFIX: &str = "bld1_";
pub const DEFAULT_CONTROLLER_URL: Option<&str> = option_env!("BILIN_DEFAULT_CONTROLLER_URL");
const REPORT_INTERVAL: Duration = Duration::from_secs(10);
const PENDING_REPORT_INTERVAL: Duration = Duration::from_secs(2);
const NODE_MEASUREMENT_INTERVAL: Duration = Duration::from_secs(40);
const UDP_NODE_MEASUREMENT_INTERVAL: Duration = Duration::from_secs(15);
const EARLIEST_MEASUREMENT_WAKE: Duration = Duration::from_millis(250);
const CONTROLLER_OFFLINE_LIMIT: Duration = Duration::from_secs(30);
const PAIRING_INTERVAL: Duration = Duration::from_secs(10 * 60);
const SUPPORTS_SUBNET_PROXY: bool = cfg!(any(
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
));

#[derive(Deserialize, Serialize)]
struct Identity {
    device_id: Uuid,
    secret: String,
}

impl Identity {
    fn path(state_dir: Option<&Path>) -> Result<PathBuf> {
        let state_dir = match state_dir {
            Some(path) => path.to_path_buf(),
            None => default_machine_id_state_dir()?,
        };
        Ok(state_dir.join("controller_identity.json"))
    }

    fn load(state_dir: Option<&Path>) -> Result<Self> {
        let path = Self::path(state_dir)?;
        if let Some(existing) = read_optional_file(&path)? {
            return serde_json::from_str(&existing).context("invalid controller identity");
        }

        let _lock = MachineIdWriteLock::acquire(&path)?;
        if let Some(existing) = read_optional_file(&path)? {
            return serde_json::from_str(&existing).context("invalid controller identity");
        }
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        let identity = Self {
            device_id: Uuid::new_v4(),
            secret: format!("{:x}", Sha256::digest(seed)),
        };
        write_private_state_file(&path, &serde_json::to_vec(&identity)?)?;
        Ok(identity)
    }

    fn config_token(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"bilin/config-token/v1:");
        hash.update(self.secret.as_bytes());
        format!("{DEVICE_TOKEN_PREFIX}{:x}", hash.finalize())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct EnrollmentStatus {
    pub device_id: Uuid,
    pub controller_origin: String,
    pub device_code: Option<String>,
    pub device_name: Option<String>,
    pub device_code_claim_supported: bool,
    pub device_login_supported: bool,
    pub device_login_revision: Option<u64>,
    pub revoked: bool,
    pub account_suspended: bool,
    pub registered: bool,
    pub bound: bool,
    pub bound_account: Option<String>,
    pub connected: bool,
    pub desktop_tunnel_enabled: bool,
    pub pairing_code: Option<String>,
    pub networks: Vec<AssignedNetwork>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AssignedNetwork {
    pub network_id: String,
    pub display_name: String,
}

#[derive(Serialize)]
struct DeviceReport<'a> {
    device_id: Uuid,
    config_token: &'a str,
    pairing_code: &'a str,
    hostname: &'a str,
    version: &'a str,
    udp_transport_pilot_supported: bool,
    account_transport_policy_supported: bool,
    extended_transports_supported: bool,
    node_measurements: Option<&'a [NodeMeasurement]>,
    observed_paths: &'a [ObservedClientPath],
    #[serde(skip_serializing_if = "Option::is_none")]
    subnet_proxy_cidrs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subnet_proxy_fallback_cidr: Option<String>,
}

#[derive(Deserialize)]
struct EnrollmentReply {
    bound: bool,
    #[serde(default)]
    bound_account: Option<String>,
    #[serde(default)]
    device_code: Option<String>,
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default)]
    device_code_claim_supported: bool,
    #[serde(default)]
    device_login_supported: bool,
    #[serde(default)]
    device_login_revision: Option<u64>,
    #[serde(default)]
    renew_pairing_code: bool,
    config_protocol: String,
    config_host: Option<String>,
    config_port: u16,
    #[serde(default)]
    candidates: Vec<NodeCandidate>,
    #[serde(default)]
    udp_transport_enabled: Option<bool>,
    #[serde(default, rename = "udp_transport_pilot_enabled")]
    previous_udp_transport_enabled: bool,
    #[serde(default)]
    networks: Option<Vec<AssignedNetwork>>,
    #[serde(default)]
    upstream_subnet_supported: bool,
    #[serde(default)]
    upstream_subnet_cidr: Option<Ipv4Cidr>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct NodeCandidate {
    pub id: Uuid,
    pub peer_url: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct NodeMeasurement {
    pub id: Uuid,
    pub latency_us: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ObservedClientPath {
    pub network_id: Uuid,
    pub destination: Ipv4Addr,
    pub direct: bool,
}

pub struct DeviceEnrollment {
    controller: Url,
    identity: Arc<Identity>,
    http: reqwest::Client,
    status: watch::Receiver<EnrollmentStatus>,
    refresh: Arc<Notify>,
    subnet_proxy_enabled: watch::Sender<bool>,
    local_tunnel_enabled: watch::Sender<bool>,
    #[cfg(target_os = "android")]
    mobile_tunnel: watch::Sender<Option<u64>>,
    #[cfg(target_os = "android")]
    mobile_captcha: tokio::sync::Mutex<Option<reqwest::header::HeaderValue>>,
    task: JoinHandle<()>,
}

#[cfg(target_os = "android")]
mod mobile_access;
#[cfg(target_os = "android")]
pub use mobile_access::{MobileAccount, MobileDeviceRoster, restore_mobile_account};

impl DeviceEnrollment {
    pub fn start(
        controller: &str,
        state_dir: Option<&Path>,
        hostname: Option<String>,
        manager: Arc<NativeInstanceManager>,
        hooks: Option<Arc<dyn WebClientHooks>>,
    ) -> Result<Self> {
        Self::start_with_subnet_proxy(controller, state_dir, hostname, manager, hooks, true)
    }

    pub fn start_with_subnet_proxy(
        controller: &str,
        state_dir: Option<&Path>,
        hostname: Option<String>,
        manager: Arc<NativeInstanceManager>,
        hooks: Option<Arc<dyn WebClientHooks>>,
        enabled: bool,
    ) -> Result<Self> {
        Self::start_with_network_preferences(
            controller, state_dir, hostname, manager, hooks, enabled, true,
        )
    }

    pub fn start_with_network_preferences(
        controller: &str,
        state_dir: Option<&Path>,
        hostname: Option<String>,
        manager: Arc<NativeInstanceManager>,
        hooks: Option<Arc<dyn WebClientHooks>>,
        subnet_proxy_enabled: bool,
        tunnel_enabled: bool,
    ) -> Result<Self> {
        let controller = parse_controller_origin(controller)?;
        let http = enrollment_http_client()?;
        let identity = Arc::new(Identity::load(state_dir)?);
        let initial = EnrollmentStatus {
            device_id: identity.device_id,
            controller_origin: controller.to_string(),
            device_code: None,
            device_name: None,
            device_code_claim_supported: false,
            device_login_supported: false,
            device_login_revision: None,
            revoked: false,
            account_suspended: false,
            registered: false,
            bound: false,
            bound_account: None,
            connected: false,
            // Publish a pause only after runtime cleanup, including on restart.
            desktop_tunnel_enabled: true,
            pairing_code: None,
            networks: Vec::new(),
        };
        let (status_writer, status) = watch::channel(initial);
        let refresh = Arc::new(Notify::new());
        let (subnet_proxy_enabled, subnet_proxy_setting) =
            watch::channel(subnet_proxy_enabled && SUPPORTS_SUBNET_PROXY);
        let (local_tunnel_enabled, local_tunnel_setting) = watch::channel(tunnel_enabled);
        #[cfg(target_os = "android")]
        let (mobile_tunnel, mobile_tunnel_enabled) = watch::channel(None);
        let task = tokio::spawn(report_loop(
            controller.clone(),
            identity.clone(),
            hostname,
            http.clone(),
            manager,
            hooks,
            status_writer,
            refresh.clone(),
            subnet_proxy_setting,
            local_tunnel_setting,
            #[cfg(target_os = "android")]
            mobile_tunnel_enabled,
        ));
        Ok(Self {
            controller,
            identity,
            http,
            status,
            refresh,
            subnet_proxy_enabled,
            local_tunnel_enabled,
            #[cfg(target_os = "android")]
            mobile_tunnel,
            #[cfg(target_os = "android")]
            mobile_captcha: Default::default(),
            task,
        })
    }

    pub fn set_subnet_proxy_enabled(&self, enabled: bool) {
        let enabled = enabled && SUPPORTS_SUBNET_PROXY;
        self.subnet_proxy_enabled.send_if_modified(|current| {
            let changed = *current != enabled;
            *current = enabled;
            changed
        });
    }

    pub async fn set_local_tunnel_enabled(&self, enabled: bool) -> Result<()> {
        let mut status = self.subscribe();
        self.local_tunnel_enabled.send_if_modified(|current| {
            let changed = *current != enabled;
            *current = enabled;
            changed
        });
        tokio::time::timeout(REPORT_INTERVAL + PENDING_REPORT_INTERVAL, async {
            status
                .wait_for(|status| status.desktop_tunnel_enabled == enabled)
                .await
                .map(|_| ())
        })
        .await
        .context("local tunnel change timed out")?
        .context("device enrollment stopped before applying the local tunnel change")
    }

    pub fn status(&self) -> EnrollmentStatus {
        self.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<EnrollmentStatus> {
        self.status.clone()
    }

    pub async fn request_device_login_url(
        &self,
    ) -> std::result::Result<(Url, u64), DeviceLoginError> {
        {
            let status = self.status.borrow();
            if !status.device_login_supported || !status.registered || status.bound {
                return Err(DeviceLoginError::NotReady);
            }
        }
        let mut endpoint = self.controller.clone();
        endpoint.set_path("/api/v1/enrollment/login/start");
        let reply = self
            .http
            .post(endpoint)
            .bearer_auth(&self.identity.secret)
            .json(&DeviceLoginRequest {
                device_id: self.identity.device_id,
            })
            .send()
            .await?
            .error_for_status()?
            .json::<DeviceLoginReply>()
            .await
            .map_err(|_| DeviceLoginError::InvalidReply)?;
        if reply.device_login_revision == 0 {
            return Err(DeviceLoginError::InvalidReply);
        }
        let url = device_login_browser_url(&self.controller, &reply.ticket)
            .map_err(|_| DeviceLoginError::InvalidReply)?;
        Ok((url, reply.device_login_revision))
    }
}

fn enrollment_http_client() -> Result<reqwest::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::builder()
        .timeout(REPORT_INTERVAL)
        .redirect(reqwest::redirect::Policy::none());
    #[cfg(target_os = "android")]
    let http = {
        // Enrollment runs in Rust without the Android JVM verifier bridge.
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        })
        .with_no_client_auth();
        http.tls_backend_preconfigured(tls)
    };
    Ok(http.build()?)
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceLoginError {
    #[error("device enrollment is not ready for account binding")]
    NotReady,
    #[error("device login authorization request failed")]
    Request(#[from] reqwest::Error),
    #[error("the controller returned an invalid device login reply")]
    InvalidReply,
}

#[derive(Serialize)]
struct DeviceLoginRequest {
    device_id: Uuid,
}

#[derive(Deserialize)]
struct DeviceLoginReply {
    ticket: String,
    device_login_revision: u64,
}

fn device_login_browser_url(controller: &Url, ticket: &str) -> Result<Url> {
    ensure!(
        ticket.len() == 64
            && ticket
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid device login ticket"
    );
    let mut url = controller.clone();
    url.set_fragment(Some(&format!("/device-login?ticket={ticket}")));
    Ok(url)
}

pub fn parse_controller_origin(controller: &str) -> Result<Url> {
    let controller = Url::parse(controller).context("invalid controller URL")?;
    let local = match controller.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain(domain)) => domain == "localhost",
        None => false,
    };
    ensure!(
        controller.host().is_some()
            && (controller.scheme() == "https" || (controller.scheme() == "http" && local))
            && controller.username().is_empty()
            && controller.password().is_none()
            && controller.query().is_none()
            && controller.fragment().is_none()
            && controller.path() == "/",
        "use an HTTPS controller origin, or a local HTTP origin for testing"
    );
    Ok(controller)
}

impl Drop for DeviceEnrollment {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn report_loop(
    controller: Url,
    identity: Arc<Identity>,
    hostname: Option<String>,
    http: reqwest::Client,
    manager: Arc<NativeInstanceManager>,
    hooks: Option<Arc<dyn WebClientHooks>>,
    status: watch::Sender<EnrollmentStatus>,
    refresh: Arc<Notify>,
    mut subnet_proxy_setting: watch::Receiver<bool>,
    mut local_tunnel_setting: watch::Receiver<bool>,
    #[cfg(target_os = "android")] mut mobile_tunnel: watch::Receiver<Option<u64>>,
) {
    let endpoint = controller.join("/api/v1/enrollment/heartbeat").unwrap();
    let controller_origin = controller.to_string();
    let hostname =
        hostname.unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());
    let config_token = identity.config_token();
    let mut pairing_code = new_pairing_code();
    let mut code_created = Instant::now();
    let mut session: Option<(Url, WebClient)> = None;
    let mut candidates = Vec::new();
    let mut measure_after = Instant::now();
    let mut udp_transport_enabled = false;
    let mut last_controller_contact = Instant::now();
    let mut retried_pairing = false;
    let mut upstream_supported = false;
    let mut configured_upstream = None;
    let mut binding_recheck = false;
    loop {
        let local_tunnel_enabled = *local_tunnel_setting.borrow_and_update();
        #[cfg(target_os = "android")]
        let tunnel_revision = *mobile_tunnel.borrow_and_update();
        #[cfg(target_os = "android")]
        let tunnel_enabled = local_tunnel_enabled && tunnel_revision.is_some();
        #[cfg(not(target_os = "android"))]
        let tunnel_enabled = local_tunnel_enabled;
        let mut applied_local_change = local_tunnel_enabled;
        if !tunnel_enabled {
            drop(session.take());
            match remove_unassigned_managed_networks(&manager, hooks.as_deref(), &[]).await {
                Ok(()) => applied_local_change = true,
                Err(error) => {
                    tracing::warn!(%error, "cleanup after disabling the tunnel will retry");
                }
            }
        }
        if applied_local_change {
            status.send_if_modified(|state| {
                if state.desktop_tunnel_enabled == local_tunnel_enabled {
                    return false;
                }
                state.desktop_tunnel_enabled = local_tunnel_enabled;
                if !local_tunnel_enabled {
                    state.connected = false;
                }
                true
            });
        }
        let rechecking_binding = std::mem::take(&mut binding_recheck);
        let subnet_proxy_enabled = *subnet_proxy_setting.borrow_and_update();
        let previously_bound = status.borrow().bound;
        if code_created.elapsed() >= PAIRING_INTERVAL {
            pairing_code = new_pairing_code();
            code_created = Instant::now();
        }
        let measurements =
            if !rechecking_binding && candidates.len() > 1 && Instant::now() >= measure_after {
                measure_after = Instant::now()
                    + if udp_transport_enabled {
                        UDP_NODE_MEASUREMENT_INTERVAL
                    } else {
                        NODE_MEASUREMENT_INTERVAL
                    };
                Some(measure_nodes(&candidates).await)
            } else {
                None
            };
        let paths = if !rechecking_binding && !candidates.is_empty() {
            observed_paths(&manager).await
        } else {
            Vec::new()
        };
        let (subnet_proxy_cidrs, subnet_proxy_fallback_cidr) = if rechecking_binding {
            (None, None)
        } else if cfg!(target_os = "android") {
            (Some(Vec::new()), None)
        } else if !SUPPORTS_SUBNET_PROXY {
            (None, None)
        } else if !subnet_proxy_enabled {
            (Some(Vec::new()), None)
        } else if previously_bound {
            let (routes, fallback) =
                detect_proxy_subnets(&manager, upstream_supported, configured_upstream).await;
            (Some(routes), fallback)
        } else {
            (None, None)
        };
        let report = DeviceReport {
            device_id: identity.device_id,
            config_token: &config_token,
            pairing_code: &pairing_code,
            hostname: &hostname,
            version: crate::VERSION,
            udp_transport_pilot_supported: true,
            account_transport_policy_supported: true,
            extended_transports_supported: true,
            node_measurements: measurements.as_deref(),
            observed_paths: &paths,
            subnet_proxy_cidrs,
            subnet_proxy_fallback_cidr,
        };
        let heartbeat = async {
            http.post(endpoint.clone())
                .bearer_auth(&identity.secret)
                .json(&report)
                .send()
                .await?
                .error_for_status()?
                .json::<EnrollmentReply>()
                .await
        };
        let reply = tokio::select! {
            biased;
            _ = local_tunnel_setting.changed() => {
                binding_recheck = true;
                continue;
            }
            reply = heartbeat => reply,
        };
        let result = async {
            let response = reply?;
            #[cfg(target_os = "android")]
            let runtime_allowed =
                tunnel_enabled && response.device_login_revision == tunnel_revision;
            #[cfg(not(target_os = "android"))]
            let runtime_allowed = tunnel_enabled;
            if response.bound && runtime_allowed {
                // An authenticated membership report can withdraw local routes even
                // when the separate configuration transport is unavailable.
                if !session
                    .as_ref()
                    .is_some_and(|(_, client)| client.is_connected())
                    && let Some(assigned) = response.networks.as_deref()
                    && !unassigned_managed_networks(&manager, assigned).is_empty()
                {
                    drop(session.take());
                    remove_unassigned_managed_networks(&manager, hooks.as_deref(), assigned)
                        .await?;
                }
                let config_url = config_url(&controller, &response)?;
                if session
                    .as_ref()
                    .is_none_or(|(current, _)| *current != config_url)
                {
                    session = Some((
                        config_url.clone(),
                        connect_with_identity(
                            config_url,
                            &config_token,
                            identity.device_id,
                            Some(hostname.clone()),
                            true,
                            manager.clone(),
                            hooks.clone(),
                        )?,
                    ));
                }
            } else {
                drop(session.take());
            }
            Ok::<_, anyhow::Error>((response, runtime_allowed))
        }
        .await;

        match result {
            Ok((response, runtime_allowed)) => {
                last_controller_contact = Instant::now();
                let udp_enabled = response.bound
                    && response
                        .udp_transport_enabled
                        .unwrap_or(response.previous_udp_transport_enabled);
                if candidates != response.candidates || udp_transport_enabled != udp_enabled {
                    measure_after = Instant::now();
                }
                candidates = response.candidates;
                udp_transport_enabled = udp_enabled;
                let renew = !response.bound && response.renew_pairing_code;
                let new_upstream = if response.bound && response.upstream_subnet_supported {
                    response.upstream_subnet_cidr
                } else {
                    None
                };
                let refresh_subnets = subnet_proxy_enabled
                    && response.bound
                    && (upstream_supported != response.upstream_subnet_supported
                        || configured_upstream != new_upstream);
                upstream_supported = response.upstream_subnet_supported;
                configured_upstream = new_upstream;
                let networks = if response.bound {
                    response
                        .networks
                        .unwrap_or_else(|| status.borrow().networks.clone())
                } else {
                    Vec::new()
                };
                let desktop_tunnel_enabled = status.borrow().desktop_tunnel_enabled;
                status.send_replace(EnrollmentStatus {
                    device_id: identity.device_id,
                    controller_origin: controller_origin.clone(),
                    device_code: response.device_code,
                    device_name: response.device_name,
                    device_code_claim_supported: response.device_code_claim_supported,
                    device_login_supported: response.device_login_supported,
                    device_login_revision: response.device_login_revision,
                    revoked: false,
                    account_suspended: false,
                    registered: true,
                    bound: response.bound,
                    bound_account: response.bound.then_some(response.bound_account).flatten(),
                    connected: session
                        .as_ref()
                        .is_some_and(|(_, client)| client.is_connected()),
                    desktop_tunnel_enabled,
                    pairing_code: (!response.bound && !renew).then(|| pairing_code.clone()),
                    networks,
                });
                if (!response.bound || !runtime_allowed)
                    && let Err(error) =
                        remove_unassigned_managed_networks(&manager, hooks.as_deref(), &[]).await
                {
                    tracing::warn!(%error, "cleanup of unbound device networks will retry");
                }
                if renew && !retried_pairing {
                    retried_pairing = true;
                    pairing_code = new_pairing_code();
                    code_created = Instant::now();
                    continue;
                }
                if refresh_subnets {
                    continue;
                }
            }
            Err(error)
                if matches!(
                    error
                        .downcast_ref::<reqwest::Error>()
                        .and_then(reqwest::Error::status),
                    Some(reqwest::StatusCode::GONE | reqwest::StatusCode::FORBIDDEN)
                ) =>
            {
                let suspended = error
                    .downcast_ref::<reqwest::Error>()
                    .and_then(reqwest::Error::status)
                    == Some(reqwest::StatusCode::FORBIDDEN);
                configured_upstream = None;
                udp_transport_enabled = false;
                candidates.clear();
                drop(session.take());
                status.send_modify(|state| {
                    state.revoked = !suspended;
                    state.account_suspended = suspended;
                    state.registered = false;
                    state.bound = false;
                    state.device_name = None;
                    state.bound_account = None;
                    state.connected = false;
                    state.pairing_code = None;
                    state.networks.clear();
                });
                if let Err(error) =
                    remove_unassigned_managed_networks(&manager, hooks.as_deref(), &[]).await
                {
                    tracing::warn!(%error, "cleanup of revoked device networks will retry");
                }
            }
            Err(error) => {
                // An older controller may be restored while this client is already connected.
                // Fall back to the original single-CIDR report if that controller rejects a list.
                if report
                    .subnet_proxy_cidrs
                    .as_ref()
                    .is_some_and(|cidrs| cidrs.len() > 1)
                    && error
                        .downcast_ref::<reqwest::Error>()
                        .and_then(reqwest::Error::status)
                        == Some(reqwest::StatusCode::BAD_REQUEST)
                {
                    upstream_supported = false;
                    configured_upstream = None;
                }
                tracing::debug!(%error, "controller registration will retry");
                if last_controller_contact.elapsed() >= CONTROLLER_OFFLINE_LIMIT
                    && !session
                        .as_ref()
                        .is_some_and(|(_, client)| client.is_connected())
                {
                    drop(session.take());
                    if let Err(error) =
                        remove_unassigned_managed_networks(&manager, hooks.as_deref(), &[]).await
                    {
                        tracing::warn!(%error, "cleanup after controller connection loss will retry");
                    }
                }
                status.send_modify(|state| {
                    state.registered = false;
                    state.connected = session
                        .as_ref()
                        .is_some_and(|(_, client)| client.is_connected());
                    state.pairing_code = None;
                });
            }
        }
        retried_pairing = false;
        let (interval, was_connected) = {
            let current = status.borrow();
            let interval = if (current.registered
                && !current.connected
                && (local_tunnel_enabled || !current.bound))
                || current.revoked
            {
                PENDING_REPORT_INTERVAL
            } else {
                REPORT_INTERVAL
            };
            let interval = if current.registered && udp_transport_enabled && candidates.len() > 1 {
                interval.min(
                    measure_after
                        .saturating_duration_since(Instant::now())
                        .max(EARLIEST_MEASUREMENT_WAKE),
                )
            } else {
                interval
            };
            (interval, current.connected)
        };
        let mut connection = session
            .as_ref()
            .map(|(_, client)| client.subscribe_connection());
        #[cfg(target_os = "android")]
        let tunnel_changed = mobile_tunnel.changed();
        #[cfg(not(target_os = "android"))]
        let tunnel_changed = std::future::pending::<()>();
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = refresh.notified() => { binding_recheck = true; }
            _ = subnet_proxy_setting.changed() => {}
            _ = local_tunnel_setting.changed() => { binding_recheck = true; }
            _ = tunnel_changed => { binding_recheck = true; }
            // The controller closes this channel after revoking the current binding.
            // Recheck enrollment before assuming a transport interruption was a deletion.
            _ = async {
                if let Some(connection) = &mut connection {
                    let _ = connection.wait_for(|connected| !*connected).await;
                }
            }, if was_connected && connection.is_some() => { binding_recheck = true; }
        }
    }
}

async fn detect_proxy_subnets(
    manager: &NativeInstanceManager,
    upstream_supported: bool,
    configured_upstream: Option<Ipv4Cidr>,
) -> (Vec<String>, Option<String>) {
    use crate::common::{netns::NetNS, network};
    use std::net::IpAddr;

    let overlay_ips: std::collections::HashSet<_> = manager
        .instance_ids()
        .into_iter()
        .filter_map(|id| manager.config(id)?.get_ipv4().map(|ip| ip.address()))
        .collect();
    let preferred = network::local_ipv4().await.ok();
    let candidates = network::collect_interfaces(NetNS::new(None), true)
        .await
        .into_iter()
        .filter(|iface| is_lan_interface_name(&iface.name))
        .flat_map(|iface| {
            iface
                .addr
                .into_iter()
                .filter_map(move |address| match address.ip() {
                    IpAddr::V4(ip) => Some((iface.index, ip)),
                    IpAddr::V6(_) => None,
                })
        })
        .filter(|(_, ip)| !overlay_ips.contains(ip));
    let Some((interface_index, address)) = select_lan_proxy_address(candidates, preferred) else {
        return (Vec::new(), None);
    };
    let [a, b, c, _] = address.octets();
    let lan = Ipv4Cidr::new(Ipv4Addr::new(a, b, c, 0), 24).expect("masked LAN address is valid");

    #[cfg(any(
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    ))]
    if upstream_supported {
        let (detected, fallback) =
            upstream_routes::discover(interface_index, address, lan, configured_upstream).await;
        if let Some(detected) = detected {
            return (vec![lan.to_string(), detected.to_string()], None);
        }
        return (vec![lan.to_string()], fallback.map(|cidr| cidr.to_string()));
    }
    #[cfg(not(any(
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos"))
    )))]
    let _ = (interface_index, upstream_supported, configured_upstream);
    (vec![lan.to_string()], None)
}

fn is_lan_interface_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ![
        "bilin",
        "wintun",
        "wireguard",
        "tailscale",
        "zerotier",
        "tun",
        "tap",
        "docker",
        "veth",
        "virbr",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

fn select_lan_proxy_address(
    candidates: impl IntoIterator<Item = (u32, Ipv4Addr)>,
    preferred: Option<Ipv4Addr>,
) -> Option<(u32, Ipv4Addr)> {
    candidates
        .into_iter()
        .filter(|(_, ip)| {
            let first = ip.octets()[0];
            (1..224).contains(&first) && !ip.is_loopback() && !ip.is_link_local()
        })
        .min_by_key(|(index, ip)| (Some(*ip) != preferred, *index, *ip))
}

async fn remove_unassigned_managed_networks(
    manager: &NativeInstanceManager,
    hooks: Option<&dyn WebClientHooks>,
    assigned: &[AssignedNetwork],
) -> anyhow::Result<()> {
    // Share the mutation lock with the config channel so only the path that
    // actually removes the instance notifies the application.
    let mutation_lock = manager.mutation_lock();
    let _mutation = mutation_lock.lock().await;
    let removed = unassigned_managed_networks(manager, assigned);
    if removed.is_empty() {
        return Ok(());
    }
    manager
        .delete_network_instances(removed.iter().copied())
        .await?;
    if let Some(hooks) = hooks
        && let Err(error) = hooks.post_remove_network_instances(&removed).await
    {
        tracing::warn!(%error, "post-remove hook failed after clearing managed networks");
    }
    Ok(())
}

fn unassigned_managed_networks(
    manager: &NativeInstanceManager,
    assigned: &[AssignedNetwork],
) -> Vec<Uuid> {
    let assigned: std::collections::HashSet<_> = assigned
        .iter()
        .filter_map(|network| Uuid::parse_str(&network.network_id).ok())
        .collect();
    manager
        .instance_ids()
        .into_iter()
        .filter(|id| {
            !assigned.contains(id)
                && manager.config_source(*id) == Some(bilin_core::config::toml::ConfigSource::Web)
        })
        .collect()
}

async fn measure_nodes(candidates: &[NodeCandidate]) -> Vec<NodeMeasurement> {
    let probes = candidates.to_vec();
    stream::iter(probes)
        .map(|candidate| async move {
            let endpoint = Url::parse(&candidate.peer_url).ok()?;
            if endpoint.scheme() != "tcp" {
                return None;
            }
            let host = match endpoint.host()? {
                Host::Ipv4(ip) => ip.to_string(),
                Host::Ipv6(ip) => ip.to_string(),
                Host::Domain(domain) => domain.to_owned(),
            };
            let port = endpoint.port()?;
            let mut samples = Vec::new();
            for _ in 0..3 {
                let start = Instant::now();
                if tokio::time::timeout(
                    Duration::from_millis(900),
                    tokio::net::TcpStream::connect((host.as_str(), port)),
                )
                .await
                .is_ok_and(|result| result.is_ok())
                {
                    samples.push(start.elapsed().as_micros().max(1) as u64);
                }
            }
            samples.sort_unstable();
            (samples.len() >= 2).then(|| NodeMeasurement {
                id: candidate.id,
                latency_us: samples[samples.len() / 2],
            })
        })
        .buffer_unordered(16)
        .filter_map(std::future::ready)
        .collect()
        .await
}

async fn observed_paths(manager: &NativeInstanceManager) -> Vec<ObservedClientPath> {
    let Ok(instances) = manager.collect_network_infos().await else {
        return Vec::new();
    };
    instances
        .into_iter()
        .filter(|(network, info)| {
            info.running
                && manager.config_source(*network)
                    == Some(bilin_core::config::toml::ConfigSource::Web)
        })
        .flat_map(|(network_id, info)| {
            let direct_peers: std::collections::HashSet<_> = info
                .peers
                .into_iter()
                .filter(|peer| !peer.directly_connected_conns.is_empty())
                .map(|peer| peer.peer_id)
                .collect();
            info.routes.into_iter().filter_map(move |route| {
                let destination = route.ipv4_addr?.address?.into();
                Some(ObservedClientPath {
                    network_id,
                    destination,
                    direct: route.next_hop_peer_id == route.peer_id
                        && direct_peers.contains(&route.peer_id),
                })
            })
        })
        .take(256)
        .collect()
}

fn new_pairing_code() -> String {
    let code = OsRng.next_u64();
    format!("{code:016x}")
}

fn config_url(controller: &Url, reply: &EnrollmentReply) -> Result<Url> {
    ensure!(
        matches!(reply.config_protocol.as_str(), "tcp" | "udp"),
        "unsupported config-server protocol"
    );
    let host = reply
        .config_host
        .as_deref()
        .or_else(|| controller.host_str())
        .context("missing config-server host")?;
    let host = Host::parse(host).context("invalid config-server host")?;
    Ok(Url::parse(&format!(
        "{}://{}:{}",
        reply.config_protocol, host, reply.config_port
    ))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_tunnel_pause_removes_managed_runtime_without_waiting_for_heartbeat() {
        use bilin_core::{
            config::toml::{ConfigSource, TomlConfig},
            instance::manager::{ConfigFileControl, ConfigFilePermission},
        };
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let manager = Arc::new(crate::instance::factory::native_instance_manager());
        let start = |source| {
            let config = TomlConfig::default();
            config.set_network_config_source(Some(source));
            let mut flags = config.get_flags();
            flags.no_tun = true;
            config.set_flags(flags);
            manager
                .run_network_instance(
                    config,
                    ConfigFileControl::new(None, ConfigFilePermission::default()),
                )
                .unwrap()
        };
        let managed = start(ConfigSource::Web);
        let local = start(ConfigSource::User);
        let controller = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let origin = format!("http://{}", controller.local_addr().unwrap());
        let received_heartbeat = Arc::new(Notify::new());
        let on_request = received_heartbeat.clone();
        let server = tokio::spawn(async move {
            // Leave the first request stalled to exercise prompt cancellation.
            let (first, _) = controller.accept().await.unwrap();
            on_request.notify_one();
            let assigned = serde_json::json!({
                "bound": true,
                "bound_account": "test-owner",
                "device_login_revision": 1,
                "config_protocol": "tcp",
                "config_port": 1,
                "networks": [{"network_id": managed, "display_name": "test network"}],
            })
            .to_string();
            loop {
                let (mut stream, _) = controller.accept().await.unwrap();
                let mut request = [0; 4096];
                if stream.read(&mut request).await.unwrap_or_default() != 0 {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        assigned.len(),
                        assigned
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
                let _ = &first;
            }
        });
        let state_dir = tempfile::tempdir().unwrap();
        let enrollment = DeviceEnrollment::start_with_network_preferences(
            &origin,
            Some(state_dir.path()),
            None,
            manager.clone(),
            None,
            false,
            true,
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(3), received_heartbeat.notified())
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(3),
            enrollment.set_local_tunnel_enabled(false),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(manager.instance_ids(), vec![local]);
        let mut status = enrollment.subscribe();
        let paused = tokio::time::timeout(
            Duration::from_secs(3),
            status.wait_for(|status| status.registered),
        )
        .await
        .unwrap()
        .unwrap()
        .clone();
        assert!(paused.bound);
        assert!(!paused.desktop_tunnel_enabled);
        assert!(!paused.connected);
        assert_eq!(paused.networks[0].network_id, managed.to_string());
        enrollment.set_local_tunnel_enabled(true).await.unwrap();
        assert!(enrollment.status().desktop_tunnel_enabled);
        server.abort();
    }

    #[test]
    fn device_login_browser_url_uses_the_enrolled_controller_and_an_opaque_ticket() {
        let controller = parse_controller_origin("https://control.example:8443").unwrap();
        let ticket = "a5".repeat(32);
        let url = device_login_browser_url(&controller, &ticket).unwrap();
        assert_eq!(
            url.as_str(),
            format!("https://control.example:8443/#/device-login?ticket={ticket}")
        );
        assert!(url.query().is_none());
        assert!(device_login_browser_url(&controller, &format!("{}&x", "a".repeat(62))).is_err());
    }

    #[test]
    fn automatic_proxy_uses_the_primary_lan_even_with_publicly_numbered_addresses() {
        let primary = "192.179.1.23".parse().unwrap();
        let candidates = [(2, "10.0.0.8".parse().unwrap()), (4, primary)];
        assert_eq!(
            select_lan_proxy_address(candidates, Some(primary)),
            Some((4, primary))
        );
    }

    #[test]
    fn automatic_proxy_ignores_unusable_addresses_and_uses_a_consistent_fallback() {
        let candidates = [
            (1, "169.254.1.2".parse().unwrap()),
            (2, "127.0.0.1".parse().unwrap()),
            (3, "192.168.2.5".parse().unwrap()),
            (4, "192.168.3.6".parse().unwrap()),
        ];
        assert_eq!(
            select_lan_proxy_address(candidates, Some(candidates[0].1)),
            Some(candidates[2])
        );
        assert_eq!(
            select_lan_proxy_address(candidates[..2].iter().copied(), None),
            None
        );
        assert!(!is_lan_interface_name("Bilin-TUN"));
        assert!(!is_lan_interface_name("docker0"));
        assert!(is_lan_interface_name("Ethernet"));
    }
}

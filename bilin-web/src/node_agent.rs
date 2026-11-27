use std::{
    collections::HashSet,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context as _, Result};
use bilin::{
    proto::rpc::standalone::{runtime_rpc_listener, runtime_udp_tunnel_listener},
    tunnel::{
        kcp::runtime_kcp_tunnel_listener, quic::runtime_quic_tunnel_listener,
        websocket::runtime_websocket_relay_listener,
    },
    web_client::enrollment::parse_controller_origin,
};
use bilin_core::{socket::SocketListener, tunnel::Tunnel};

use crate::central_network::{
    forwarding::{NodeReport, NodeSnapshot, validate_node_url, validate_node_wss_url},
    gateway::{GatewayConfig, NetworkInstanceManager, listener::GatewayListener},
};

const REPORT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct NodeTransports {
    pub udp: bool,
    pub quic: bool,
    pub kcp: bool,
    pub wss_bind: Option<SocketAddr>,
    pub wss_url: Option<url::Url>,
}

pub(crate) async fn run(
    controller: &str,
    token: &str,
    bind: Option<SocketAddr>,
    transports: NodeTransports,
) -> Result<()> {
    let endpoint =
        parse_controller_origin(controller)?.join("/api/v1/forwarding-nodes/heartbeat")?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let snapshot = loop {
        match heartbeat(&client, &endpoint, token, &NodeReport::default()).await {
            Ok(snapshot) => break snapshot,
            Err(error) if registration_rejected(&error) => {
                return Err(error).context("forwarding node registration rejected");
            }
            Err(error) => tracing::warn!(%error, "forwarding node registration will retry"),
        }
        tokio::select! {
            _ = tokio::time::sleep(REPORT_INTERVAL) => {}
            signal = tokio::signal::ctrl_c() => return Ok(signal?),
        }
    };
    let peer = validate_node_url(&snapshot.peer_url)?;
    let bind = bind.unwrap_or_else(|| ([0, 0, 0, 0], peer.port().unwrap()).into());
    let wss = transports.wss_bind.zip(transports.wss_url);
    if let Some((bind, url)) = &wss {
        anyhow::ensure!(
            bind.ip().is_loopback(),
            "forwarding WebSocket bind must be loopback"
        );
        validate_node_wss_url(&peer, url)?;
    }
    let instances = Arc::new(NetworkInstanceManager::new_with_udp_preference(
        GatewayConfig {
            peer_url: peer.to_string(),
            relay_data: true,
        },
        transports.udp || transports.quic || transports.kcp || wss.is_some(),
    ));
    let mut listener = GatewayListener::new(runtime_rpc_listener(bind), instances.clone());
    listener.listen().await?;
    let tcp_port = listener
        .local_url()
        .port()
        .context("forwarding TCP listener port missing")?;
    let derived_bind = |enabled: bool, protocol: &str, offset: u16| {
        if !enabled {
            return None;
        }
        let port = peer
            .port()
            .and_then(|port| port.checked_add(offset))
            .and_then(|_| tcp_port.checked_add(offset));
        if port.is_none() {
            tracing::warn!(%peer, %bind, protocol, "forwarding transport port is out of range");
        }
        port.map(|port| SocketAddr::new(bind.ip(), port))
    };
    tracing::info!(bind = %listener.local_url(), %peer, "forwarding node listening on TCP");

    let udp_ready = AtomicBool::new(false);
    let quic_ready = AtomicBool::new(false);
    let kcp_ready = AtomicBool::new(false);
    let wss_ready = AtomicBool::new(false);
    let accepting = async {
        loop {
            let error = accept_gateway_tunnels(&mut listener).await;
            tracing::warn!(%error, "forwarding node TCP accept failed");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let synchronizing = async {
        let mut ready = apply_snapshot(&instances, snapshot).await?;
        loop {
            let ready_on = |transport: &AtomicBool| ready && transport.load(Ordering::Relaxed);
            let report = NodeReport {
                ready,
                udp_ready: ready_on(&udp_ready),
                quic_ready: ready_on(&quic_ready),
                kcp_ready: ready_on(&kcp_ready),
                wss_url: wss
                    .as_ref()
                    .filter(|_| ready_on(&wss_ready))
                    .map(|(_, url)| url.to_string()),
            };
            match heartbeat(&client, &endpoint, token, &report).await {
                Ok(snapshot) => {
                    anyhow::ensure!(
                        snapshot.peer_url == peer.as_str(),
                        "forwarding node address changed, restart the service"
                    );
                    ready = apply_snapshot(&instances, snapshot).await?;
                }
                Err(error) if registration_rejected(&error) => {
                    return Err(error).context("forwarding node registration revoked");
                }
                Err(error) => tracing::warn!(%error, "forwarding node heartbeat will retry"),
            }
            tokio::time::sleep(REPORT_INTERVAL).await;
        }
    };
    let udp = accept_optional_tunnels(
        "UDP",
        derived_bind(transports.udp, "UDP", 0),
        &instances,
        &udp_ready,
        |bind| {
            Ok(runtime_udp_tunnel_listener(
                format!("udp://{bind}").parse()?,
                bind,
            ))
        },
    );
    let quic = accept_optional_tunnels(
        "QUIC",
        derived_bind(transports.quic, "QUIC", 1),
        &instances,
        &quic_ready,
        |bind| Ok(runtime_quic_tunnel_listener(bind)),
    );
    let kcp = accept_optional_tunnels(
        "KCP",
        derived_bind(transports.kcp, "KCP", 2),
        &instances,
        &kcp_ready,
        |bind| Ok(runtime_kcp_tunnel_listener(bind)),
    );
    let websocket = accept_optional_tunnels(
        "WSS",
        wss.as_ref().map(|(bind, _)| *bind),
        &instances,
        &wss_ready,
        runtime_websocket_relay_listener,
    );
    tokio::select! {
        _ = accepting => Ok(()),
        _ = udp => Ok(()),
        _ = quic => Ok(()),
        _ = kcp => Ok(()),
        _ = websocket => Ok(()),
        result = synchronizing => result,
        signal = tokio::signal::ctrl_c() => Ok(signal?),
    }
}

async fn accept_gateway_tunnels(
    listener: &mut impl SocketListener<Accepted = Box<dyn Tunnel>>,
) -> anyhow::Error {
    loop {
        match listener.accept().await {
            Ok(web_tunnel) => drop(web_tunnel),
            Err(error) => return error,
        }
    }
}

async fn accept_optional_tunnels<L: SocketListener<Accepted = Box<dyn Tunnel>> + 'static>(
    protocol: &str,
    bind: Option<SocketAddr>,
    instances: &Arc<NetworkInstanceManager>,
    ready: &AtomicBool,
    create_listener: impl Fn(SocketAddr) -> Result<L>,
) {
    let Some(bind) = bind else {
        return std::future::pending().await;
    };
    loop {
        let error = match create_listener(bind) {
            Ok(inner) => {
                let mut listener = GatewayListener::new(inner, instances.clone());
                match listener.listen().await {
                    Ok(()) => {
                        ready.store(true, Ordering::Relaxed);
                        tracing::info!(%protocol, bind = %listener.local_url(), "forwarding node transport listening");
                        accept_gateway_tunnels(&mut listener).await
                    }
                    Err(error) => error,
                }
            }
            Err(error) => error,
        };
        ready.store(false, Ordering::Relaxed);
        tracing::warn!(%protocol, %bind, %error, "forwarding node transport unavailable, will retry");
        tokio::time::sleep(REPORT_INTERVAL).await;
    }
}

fn registration_rejected(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<reqwest::Error>()
        .and_then(reqwest::Error::status)
        .is_some_and(|status| {
            status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN
        })
}

async fn heartbeat(
    client: &reqwest::Client,
    endpoint: &url::Url,
    token: &str,
    report: &NodeReport,
) -> Result<NodeSnapshot> {
    Ok(client
        .post(endpoint.clone())
        .bearer_auth(token)
        .json(report)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn apply_snapshot(
    instances: &NetworkInstanceManager,
    snapshot: NodeSnapshot,
) -> Result<bool> {
    let mesh_peers: Vec<url::Url> = snapshot
        .mesh_peers
        .into_iter()
        .map(|url| url.parse())
        .collect::<Result<_, _>>()?;
    let configured: HashSet<_> = snapshot
        .networks
        .iter()
        .map(|network| (network.user_id, network.id))
        .collect();
    let udp_accounts = snapshot
        .udp_enabled_accounts
        .map(|accounts| accounts.into_iter().collect::<HashSet<_>>());
    for (owner, id) in instances.network_ids().await {
        if !configured.contains(&(owner, id)) {
            instances.remove(owner, id).await;
        }
    }
    for network in &snapshot.networks {
        let allow_udp = udp_accounts
            .as_ref()
            .is_none_or(|accounts| accounts.contains(&network.user_id));
        instances
            .reconcile_with_peer_policy(network, &mesh_peers, allow_udp)
            .await;
    }
    let active: HashSet<_> = instances.network_ids().await.into_iter().collect();
    Ok(configured == active)
}

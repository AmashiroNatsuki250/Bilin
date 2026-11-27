use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

use bilin::web_client::enrollment::{NodeCandidate, NodeMeasurement, ObservedClientPath};
use rand::{RngCore as _, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::db::Db;

use super::model::CentralNetworkIntent;

const NODE_TIMEOUT: i64 = 25;
const MEASUREMENT_TTL: Duration = Duration::from_secs(90);
const OBSERVED_PATH_TTL: Duration = Duration::from_secs(35);
const DIRECT_ATTEMPT: Duration = Duration::from_secs(25);
const RERANK_HOLD: Duration = Duration::from_secs(25);
const MIN_RERANK_GAIN_US: u64 = 3_000;
const MIN_RERANK_GAIN_PERCENT: u64 = 15;
const BUILTIN_ID: Uuid = Uuid::nil();

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ForwardingNode {
    pub id: String,
    pub name: String,
    pub peer_url: String,
    pub last_seen: Option<i64>,
    pub ready: bool,
}

impl ForwardingNode {
    pub fn online(&self) -> bool {
        self.last_seen
            .is_some_and(|seen| seen >= chrono::Utc::now().timestamp() - NODE_TIMEOUT)
    }

    pub fn candidate(&self) -> Option<NodeCandidate> {
        (self.ready && self.online()).then(|| NodeCandidate {
            id: Uuid::parse_str(&self.id).expect("stored forwarding node UUID"),
            peer_url: self.peer_url.clone(),
        })
    }
}

#[derive(Default, Deserialize, Serialize)]
pub struct NodeReport {
    pub ready: bool,
    #[serde(default)]
    pub udp_ready: bool,
    #[serde(default)]
    pub quic_ready: bool,
    #[serde(default)]
    pub kcp_ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wss_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub peer_url: String,
    pub mesh_peers: Vec<String>,
    pub networks: Vec<CentralNetworkIntent>,
    #[serde(default)]
    pub udp_enabled_accounts: Option<Vec<i32>>,
}

pub fn validate_node_url(input: &str) -> anyhow::Result<url::Url> {
    let mut url: url::Url = input.parse()?;
    anyhow::ensure!(
        url.scheme() == "tcp"
            && url.host().is_some()
            && url.port().is_some_and(|port| port != 0)
            && url.username().is_empty()
            && url.password().is_none()
            && matches!(url.path(), "" | "/")
            && url.query().is_none()
            && url.fragment().is_none(),
        "use a TCP node address with a host and port"
    );
    if let Some(host) = url.host_str()
        && host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_unspecified() || ip.is_multicast())
    {
        anyhow::bail!("node endpoint is not a routable address");
    }
    url.set_path("");
    Ok(url)
}

pub fn validate_node_wss_url(node: &url::Url, endpoint: &url::Url) -> anyhow::Result<()> {
    bilin::tunnel::websocket::validate_websocket_relay_url(endpoint)?;
    // TCP URLs can expose a numeric host as a domain, unlike HTTPS and WSS.
    let host = url::Host::parse(
        node.host_str()
            .ok_or_else(|| anyhow::anyhow!("node has no host"))?,
    )?;
    anyhow::ensure!(
        Some(host) == endpoint.host().map(|host| host.to_owned()),
        "the WSS URL must use the registered node host"
    );
    Ok(())
}

fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

pub async fn create_node(db: &Db, name: &str, peer_url: &str) -> anyhow::Result<(Uuid, String)> {
    let id = Uuid::new_v4();
    let mut random = [0; 32];
    OsRng.fill_bytes(&mut random);
    let token = format!("bln1_{:x}", Sha256::digest(random));
    sqlx::query(
        "INSERT INTO forwarding_nodes(id, name, peer_url, token_digest) VALUES (?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(peer_url)
    .bind(digest(&token))
    .execute(&db.inner())
    .await?;
    Ok((id, token))
}

pub async fn list_nodes(db: &Db) -> Result<Vec<ForwardingNode>, sqlx::Error> {
    sqlx::query_as("SELECT id, name, peer_url, last_seen, ready FROM forwarding_nodes ORDER BY id")
        .fetch_all(&db.inner())
        .await
}

pub async fn node_heartbeat(
    db: &Db,
    token: &str,
    ready: bool,
) -> Result<Option<ForwardingNode>, sqlx::Error> {
    sqlx::query_as(
        r#"
        UPDATE forwarding_nodes SET last_seen = ?, ready = ? WHERE token_digest = ?
        RETURNING id, name, peer_url, last_seen, ready
    "#,
    )
    .bind(chrono::Utc::now().timestamp())
    .bind(ready)
    .bind(digest(token))
    .fetch_optional(&db.inner())
    .await
}

#[derive(Default)]
pub struct Scheduler(Mutex<State>);

#[derive(Default)]
struct State {
    devices: HashMap<Uuid, Observation>,
    udp_ready_nodes: HashMap<Uuid, Instant>,
    quic_ready_nodes: HashMap<Uuid, Instant>,
    kcp_ready_nodes: HashMap<Uuid, Instant>,
    wss_nodes: HashMap<Uuid, (Instant, String)>,
    routes: HashMap<(Uuid, Uuid), Placement>,
    memberships: HashMap<Uuid, (usize, HashMap<std::net::Ipv4Addr, Uuid>)>,
}

struct Observation {
    measurements: HashMap<Uuid, u64>,
    paths: Vec<ObservedClientPath>,
    measured_at: Instant,
    reported_at: Instant,
}

impl Observation {
    fn measured_delay(&self, node: Uuid) -> Option<u64> {
        if self.measured_at.elapsed() > MEASUREMENT_TTL {
            return None;
        }
        self.measurements.get(&node).copied()
    }
}

struct Placement {
    primary: Uuid,
    started: Instant,
    stage: Stage,
}

impl Placement {
    fn set_primary(&mut self, node: Uuid) {
        if self.primary != node {
            self.primary = node;
            self.started = Instant::now();
        }
    }
}

fn worthwhile_rerank(current: u64, best: u64) -> bool {
    let gain = current.saturating_sub(best);
    gain >= MIN_RERANK_GAIN_US && gain * 100 >= current * MIN_RERANK_GAIN_PERCENT
}

fn best_settled_pair<'a>(
    network: Uuid,
    destinations: &HashMap<std::net::Ipv4Addr, Uuid>,
    devices: &HashMap<Uuid, Observation>,
    routes: &HashMap<(Uuid, Uuid), Placement>,
    nodes: &'a [NodeCandidate],
    forced: bool,
    force_relay: bool,
) -> Option<([Uuid; 2], &'a NodeCandidate)> {
    let pair: [Uuid; 2] = destinations
        .values()
        .copied()
        .collect::<Vec<_>>()
        .try_into()
        .ok()?;
    let [left, right] = pair;
    let left_placement = routes.get(&(network, left))?;
    let right_placement = routes.get(&(network, right))?;
    let left_observation = devices.get(&left)?;
    let right_observation = devices.get(&right)?;
    if !matches!(left_placement.stage, Stage::Relay)
        && !matches!(right_placement.stage, Stage::Relay)
    {
        return None;
    }
    for (device, placement, observation) in [
        (left, left_placement, left_observation),
        (right, right_placement, right_observation),
    ] {
        if observation.reported_at.elapsed() >= OBSERVED_PATH_TTL {
            return None;
        }
        let peer_paths = observation.paths.iter().filter(|path| {
            path.network_id == network
                && destinations
                    .get(&path.destination)
                    .is_some_and(|peer| *peer != device)
        });
        if !force_relay && peer_paths.clone().any(|path| path.direct) {
            return None;
        }
        match placement.stage {
            Stage::Relay => {}
            // The other peer settled because its probe found only one usable
            // endpoint. An observed relay may now join its settled placement.
            Stage::Direct
                if placement.started.elapsed() >= DIRECT_ATTEMPT && peer_paths.count() > 0 => {}
            _ => return None,
        }
    }
    let combined_delay = |node| {
        Some(left_observation.measured_delay(node)? + right_observation.measured_delay(node)?)
    };
    let (best_delay, best) = nodes
        .iter()
        .filter_map(|node| combined_delay(node.id).map(|delay| (delay, node)))
        .min_by_key(|(delay, node)| (*delay, node.id))?;
    let held = [left_placement, right_placement]
        .iter()
        .all(|placement| placement.started.elapsed() >= RERANK_HOLD);
    let same_node = left_placement.primary == right_placement.primary;
    let current = same_node
        .then(|| combined_delay(left_placement.primary))
        .flatten();
    let should_switch = forced
        || (held
            && (!same_node || current.is_some_and(|delay| worthwhile_rerank(delay, best_delay))));
    should_switch.then_some((pair, best))
}

#[derive(Clone, Copy)]
enum Stage {
    Direct,
    Retry { peer: Uuid },
    Relay,
}

pub fn candidates(nodes: &[ForwardingNode], embedded: Option<&str>) -> Vec<NodeCandidate> {
    let mut result: Vec<_> = nodes.iter().filter_map(ForwardingNode::candidate).collect();
    if let Some(peer_url) = embedded {
        result.push(NodeCandidate {
            id: BUILTIN_ID,
            peer_url: peer_url.to_owned(),
        });
    }
    result
}

impl Scheduler {
    pub fn observe(
        &self,
        device: Uuid,
        samples: Option<Vec<NodeMeasurement>>,
        paths: Vec<ObservedClientPath>,
    ) {
        let mut state = self.0.lock().unwrap();
        let now = Instant::now();
        let observation = state.devices.entry(device).or_insert_with(|| Observation {
            measurements: HashMap::new(),
            paths: Vec::new(),
            measured_at: now,
            reported_at: now,
        });
        if let Some(samples) = samples {
            observation.measurements = samples
                .into_iter()
                .filter(|sample| (1..=10_000_000).contains(&sample.latency_us))
                .map(|sample| (sample.id, sample.latency_us))
                .collect();
            observation.measured_at = now;
        }
        observation.paths = paths.into_iter().take(256).collect();
        observation.reported_at = now;
    }

    #[cfg(test)]
    fn observe_node_udp(&self, node: Uuid, ready: bool) {
        Self::observe_transport(&mut self.0.lock().unwrap().udp_ready_nodes, node, ready);
    }

    #[cfg(test)]
    fn observe_node_quic(&self, node: Uuid, ready: bool) {
        Self::observe_transport(&mut self.0.lock().unwrap().quic_ready_nodes, node, ready);
    }

    pub fn observe_node(&self, node: Uuid, report: NodeReport) {
        let mut state = self.0.lock().unwrap();
        Self::observe_transport(
            &mut state.udp_ready_nodes,
            node,
            report.ready && report.udp_ready,
        );
        Self::observe_transport(
            &mut state.quic_ready_nodes,
            node,
            report.ready && report.quic_ready,
        );
        Self::observe_transport(
            &mut state.kcp_ready_nodes,
            node,
            report.ready && report.kcp_ready,
        );
        state
            .wss_nodes
            .retain(|_, (seen, _)| seen.elapsed() < Duration::from_secs(NODE_TIMEOUT as u64));
        if let Some(url) = report.wss_url.filter(|_| report.ready) {
            state.wss_nodes.insert(node, (Instant::now(), url));
        } else {
            state.wss_nodes.remove(&node);
        }
    }

    fn observe_transport(nodes: &mut HashMap<Uuid, Instant>, node: Uuid, ready: bool) {
        nodes.retain(|_, seen| seen.elapsed() < Duration::from_secs(NODE_TIMEOUT as u64));
        if ready {
            nodes.insert(node, Instant::now());
        } else {
            nodes.remove(&node);
        }
    }

    pub fn transport_config(
        &self,
        node: Option<&NodeCandidate>,
        udp_enabled: bool,
        smart_transport: bool,
        extended: bool,
    ) -> Vec<String> {
        let state = self.0.lock().unwrap();
        let mut peer_urls = Vec::new();
        if let Some(node) = node {
            if udp_enabled {
                let mut add = |active: &HashMap<Uuid, Instant>, protocol, offset| {
                    if active.get(&node.id).is_some_and(|seen| {
                        seen.elapsed() < Duration::from_secs(NODE_TIMEOUT as u64)
                    }) && let Ok(mut url) = validate_node_url(&node.peer_url)
                        && let Some(port) = url.port().and_then(|port| port.checked_add(offset))
                        && url.set_scheme(protocol).is_ok()
                        && url.set_port(Some(port)).is_ok()
                    {
                        peer_urls.push(url.to_string());
                    }
                };
                add(&state.udp_ready_nodes, "udp", 0);
                if smart_transport {
                    add(&state.quic_ready_nodes, "quic", 1);
                }
                if extended {
                    add(&state.kcp_ready_nodes, "kcp", 2);
                }
            }
            peer_urls.push(node.peer_url.clone());
            if udp_enabled
                && extended
                && let Some((seen, url)) = state.wss_nodes.get(&node.id)
                && seen.elapsed() < Duration::from_secs(NODE_TIMEOUT as u64)
            {
                peer_urls.push(url.clone());
            }
        }
        peer_urls
    }

    pub fn reconcile<'a>(
        &self,
        intent: &CentralNetworkIntent,
        nodes: &'a [NodeCandidate],
        force_relay: bool,
    ) -> HashMap<Uuid, &'a NodeCandidate> {
        let mut state = self.0.lock().unwrap();
        let State {
            devices,
            routes,
            memberships,
            ..
        } = &mut *state;
        let member_ids: HashSet<_> = intent
            .members
            .iter()
            .filter_map(|member| Uuid::parse_str(&member.device_id).ok())
            .collect();
        routes.retain(|(network, device), _| *network != intent.id || member_ids.contains(device));
        devices.retain(|_, observation| {
            observation.reported_at.elapsed() < MEASUREMENT_TTL + DIRECT_ATTEMPT
        });
        let membership: HashMap<_, _> = intent
            .members
            .iter()
            .filter_map(|member| {
                let id = Uuid::parse_str(&member.device_id).ok()?;
                let address = member
                    .virtual_ipv4
                    .as_ref()
                    .or(member.allocated_ipv4.as_ref())?
                    .parse::<std::net::Ipv4Addr>()
                    .ok()?;
                Some((address, id))
            })
            .collect();
        let mut result = HashMap::new();
        let member_count = member_ids.len();

        let mut pair_placements = Vec::new();
        let match_pairs = member_ids.len() == 2;
        let mut relay_failover = false;
        for &device in &member_ids {
            let key = (intent.id, device);
            let Some(observation) = devices.get(&device) else {
                if let [only] = nodes
                    && routes
                        .get(&key)
                        .is_none_or(|placement| placement.primary != only.id)
                {
                    routes.insert(
                        key,
                        Placement {
                            primary: only.id,
                            started: Instant::now(),
                            stage: if force_relay {
                                Stage::Relay
                            } else {
                                Stage::Direct
                            },
                        },
                    );
                }
                if let Some(node) = routes
                    .get(&key)
                    .and_then(|placement| nodes.iter().find(|node| node.id == placement.primary))
                {
                    result.insert(device, node);
                }
                continue;
            };
            let mut ranked: Vec<_> = nodes
                .iter()
                .filter_map(|node| {
                    if nodes.len() == 1 {
                        return Some((0, node));
                    }
                    observation
                        .measured_delay(node.id)
                        .map(|delay| (delay, node))
                })
                .collect();
            ranked.sort_unstable_by_key(|(delay, node)| (*delay, node.id));

            if !routes
                .get(&key)
                .is_some_and(|placement| nodes.iter().any(|node| node.id == placement.primary))
            {
                let previous = routes.remove(&key);
                if let Some((_, best)) = ranked.first() {
                    let stage = if force_relay
                        || previous.is_some_and(|old| matches!(old.stage, Stage::Relay))
                    {
                        relay_failover = true;
                        Stage::Relay
                    } else {
                        Stage::Direct
                    };
                    routes.insert(
                        key,
                        Placement {
                            primary: best.id,
                            started: Instant::now(),
                            stage,
                        },
                    );
                }
            }
            let Some(placement) = routes.get_mut(&key) else {
                continue;
            };
            if force_relay {
                placement.stage = Stage::Relay;
            }

            // A successful batch for another node makes a missing measurement
            // of the current node actionable even while it is globally online.
            // An empty or expired ranking may be missing evidence for every node.
            if let Some((_, best)) = ranked.first()
                && observation.measured_at >= placement.started
                && !ranked.iter().any(|(_, node)| node.id == placement.primary)
            {
                relay_failover |= matches!(placement.stage, Stage::Relay);
                placement.set_primary(best.id);
            }

            if placement.started.elapsed() >= DIRECT_ATTEMPT && !ranked.is_empty() {
                let observed = observation
                    .paths
                    .iter()
                    .filter(|path| path.network_id == intent.id);
                let has_direct = !force_relay
                    && observed
                        .clone()
                        .any(|path| path.direct && membership.contains_key(&path.destination));
                let relay_peer = observed
                    .filter_map(|path| {
                        if path.direct {
                            return None;
                        }
                        membership
                            .get(&path.destination)
                            .copied()
                            .filter(|remote| *remote != device)
                    })
                    .min();
                if !has_direct {
                    match placement.stage {
                        Stage::Direct => {
                            if ranked.len() == 1 && relay_peer.is_some() {
                                placement.stage = Stage::Relay;
                            }
                            if let Some(remote) = relay_peer.filter(|remote| device < *remote)
                                && let Some((_, alternate)) =
                                    ranked.iter().find(|(_, node)| node.id != placement.primary)
                            {
                                placement.set_primary(alternate.id);
                                placement.stage = Stage::Retry { peer: remote };
                            }
                        }
                        Stage::Retry { peer: remote } => {
                            let peer_measurements = devices
                                .get(&remote)
                                .filter(|peer| peer.measured_at.elapsed() <= MEASUREMENT_TTL);
                            let best = ranked
                                .iter()
                                .filter_map(|(delay, node)| {
                                    peer_measurements?
                                        .measurements
                                        .get(&node.id)
                                        .map(|peer_delay| (delay + peer_delay, *node))
                                })
                                .min_by_key(|(delay, node)| (*delay, node.id))
                                .map(|(_, node)| node)
                                .unwrap_or(ranked[0].1);
                            placement.set_primary(best.id);
                            placement.stage = Stage::Relay;
                            if match_pairs
                                && peer_measurements
                                    .is_some_and(|peer| peer.measurements.contains_key(&best.id))
                            {
                                pair_placements.push((remote, best.id));
                            }
                        }
                        _ => {}
                    }
                }
                if !has_direct
                    && (!match_pairs || !matches!(placement.stage, Stage::Relay))
                    && (member_count == 1 || matches!(placement.stage, Stage::Relay))
                    && placement.started.elapsed() >= RERANK_HOLD
                    && let Some((current, _)) =
                        ranked.iter().find(|(_, node)| node.id == placement.primary)
                    && worthwhile_rerank(*current, ranked[0].0)
                {
                    placement.set_primary(ranked[0].1.id);
                }
            }
            if let Some(node) = nodes.iter().find(|node| node.id == placement.primary) {
                result.insert(device, node);
            }
        }
        for (device, node_id) in pair_placements {
            if let Some(placement) = routes.get_mut(&(intent.id, device))
                && let Some(node) = nodes.iter().find(|node| node.id == node_id)
            {
                placement.set_primary(node_id);
                placement.stage = Stage::Relay;
                result.insert(device, node);
            }
        }
        if match_pairs
            && let Some((pair, node)) = best_settled_pair(
                intent.id,
                &membership,
                devices,
                routes,
                nodes,
                relay_failover,
                force_relay,
            )
        {
            for device in pair {
                if let Some(placement) = routes.get_mut(&(intent.id, device)) {
                    placement.set_primary(node.id);
                    placement.stage = Stage::Relay;
                    result.insert(device, node);
                }
            }
        }
        memberships.insert(intent.id, (member_count, membership));
        result
    }

    pub fn assignment(&self, network: Uuid, device: Uuid) -> Option<(Uuid, &'static str)> {
        let state = self.0.lock().unwrap();
        let placement = state.routes.get(&(network, device))?;
        let (member_count, destinations) = state.memberships.get(&network)?;
        let mode = if *member_count == 1 {
            "single_device"
        } else if let Some(observation) = state
            .devices
            .get(&device)
            .filter(|observation| observation.reported_at.elapsed() < OBSERVED_PATH_TTL)
        {
            let (mut direct, mut relay) = (false, false);
            for path in &observation.paths {
                if path.network_id == network
                    && destinations
                        .get(&path.destination)
                        .is_some_and(|peer| *peer != device)
                {
                    direct |= path.direct;
                    relay |= !path.direct;
                }
            }
            match (direct, relay) {
                (true, true) => "mixed",
                (true, false) => "direct",
                (false, true) => "relay",
                (false, false) => "unobserved",
            }
        } else {
            "unobserved"
        };
        Some((placement.primary, mode))
    }

    pub fn forget_network(&self, network: Uuid) {
        let mut state = self.0.lock().unwrap();
        state.routes.retain(|(id, _), _| *id != network);
        state.memberships.remove(&network);
    }

    pub fn forget_device(&self, device: Uuid) {
        let mut state = self.0.lock().unwrap();
        state.devices.remove(&device);
        state.routes.retain(|(_, id), _| *id != device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::central_network::model::{NetworkMemberIntent, NetworkMode};

    fn test_nodes() -> [NodeCandidate; 2] {
        [1, 2].map(|node| NodeCandidate {
            id: Uuid::from_u128(node),
            peer_url: format!("tcp://node{node}:22021"),
        })
    }

    fn test_network(nodes: &[NodeCandidate], devices: &[Uuid]) -> CentralNetworkIntent {
        CentralNetworkIntent {
            id: Uuid::new_v4(),
            user_id: 1,
            display_name: "network".into(),
            network_name: "mesh".into(),
            network_secret: "test".into(),
            mode: NetworkMode::Gateway {
                peer_url: nodes[0].peer_url.clone(),
            },
            virtual_cidr: None,
            secure_mode: true,
            credentials: Vec::new(),
            acl_policy: None,
            members: devices
                .iter()
                .enumerate()
                .map(|(index, &device)| NetworkMemberIntent {
                    id: device,
                    device_id: device.to_string(),
                    hostname: None,
                    virtual_ipv4: Some(format!("10.0.0.{}", index + 1)),
                    allocated_ipv4: None,
                    config_override: None,
                    credential_id: None,
                    acl_group_secret: None,
                })
                .collect(),
        }
    }

    fn test_measurements(nodes: &[NodeCandidate], delays: &[Option<u64>]) -> Vec<NodeMeasurement> {
        nodes
            .iter()
            .zip(delays)
            .filter_map(|(node, delay)| {
                delay.map(|latency_us| NodeMeasurement {
                    id: node.id,
                    latency_us,
                })
            })
            .collect()
    }

    fn age_placements(scheduler: &Scheduler, network: Uuid, elapsed: Duration) {
        for ((id, _), placement) in &mut scheduler.0.lock().unwrap().routes {
            if *id == network {
                placement.started -= elapsed;
            }
        }
    }

    #[test]
    fn reranks_after_a_second_node_appears_and_recovers_from_a_client_only_failure() {
        let scheduler = Scheduler::default();
        let nodes = test_nodes();
        let device = Uuid::from_u128(10);
        let intent = test_network(&nodes, &[device]);
        let report = |measurements| scheduler.observe(device, measurements, Vec::new());
        report(None);
        assert_eq!(
            scheduler.reconcile(&intent, &nodes[..1], false)[&device].id,
            nodes[0].id
        );

        report(Some(test_measurements(
            &nodes,
            &[Some(30_000), Some(5_000)],
        )));
        assert_eq!(
            scheduler.reconcile(&intent, &nodes, false)[&device].id,
            nodes[0].id
        );
        age_placements(&scheduler, intent.id, RERANK_HOLD);
        assert_eq!(
            scheduler.reconcile(&intent, &nodes, false)[&device].id,
            nodes[1].id
        );

        age_placements(&scheduler, intent.id, RERANK_HOLD);
        report(Some(test_measurements(&nodes, &[Some(3_000), Some(5_000)])));
        assert_eq!(
            scheduler.reconcile(&intent, &nodes, false)[&device].id,
            nodes[1].id
        );
        report(Some(test_measurements(
            &nodes,
            &[Some(20_000), Some(23_100)],
        )));
        assert_eq!(
            scheduler.reconcile(&intent, &nodes, false)[&device].id,
            nodes[1].id
        );

        // No successful probes gives us no provably better fallback.
        report(Some(Vec::new()));
        assert_eq!(
            scheduler.reconcile(&intent, &nodes, false)[&device].id,
            nodes[1].id
        );

        // The second node still heartbeats and is present in nodes. The next
        // complete client batch reaches only the first one, before the dwell.
        scheduler
            .0
            .lock()
            .unwrap()
            .routes
            .get_mut(&(intent.id, device))
            .unwrap()
            .started = Instant::now();
        report(Some(test_measurements(&nodes, &[Some(30_000), None])));
        assert_eq!(
            scheduler.reconcile(&intent, &nodes, false)[&device].id,
            nodes[0].id
        );
    }

    #[test]
    fn settled_pair_reranks_together_and_uses_the_latency_sum() {
        let scheduler = Scheduler::default();
        let nodes = test_nodes();
        let pair = [Uuid::from_u128(10), Uuid::from_u128(20)];
        let intent = test_network(&nodes, &pair);
        let report = |index: usize, delays: &[Option<u64>], direct| {
            scheduler.observe(
                pair[index],
                Some(test_measurements(&nodes, delays)),
                vec![ObservedClientPath {
                    network_id: intent.id,
                    destination: format!("10.0.0.{}", 2 - index).parse().unwrap(),
                    direct,
                }],
            );
        };
        let assert_pair = |current: HashMap<Uuid, &NodeCandidate>, expected: Uuid| {
            assert!(pair.iter().all(|device| current[device].id == expected));
        };
        report(0, &[Some(20_000), None], false);
        report(1, &[Some(10_000), None], false);
        assert_pair(
            scheduler.reconcile(&intent, &nodes[..1], false),
            nodes[0].id,
        );
        age_placements(&scheduler, intent.id, DIRECT_ATTEMPT);
        assert_pair(
            scheduler.reconcile(&intent, &nodes[..1], false),
            nodes[0].id,
        );

        // Right individually prefers node 1, but their combined RTT improves on
        // node 2. Do not move while either device reports direct traffic.
        report(0, &[Some(20_000), Some(4_000)], true);
        report(1, &[Some(10_000), Some(12_000)], false);
        assert_pair(scheduler.reconcile(&intent, &nodes, false), nodes[0].id);
        report(0, &[Some(20_000), Some(4_000)], false);
        assert_pair(scheduler.reconcile(&intent, &nodes, false), nodes[1].id);

        // One member losing the shared node forces both to the surviving common
        // node even though the other member can still reach the previous node.
        report(0, &[Some(20_000), None], false);
        assert_pair(scheduler.reconcile(&intent, &nodes, false), nodes[0].id);
    }

    #[test]
    fn chooses_by_measurement_tries_one_alternate_then_uses_best_common_relay() {
        let scheduler = Scheduler::default();
        let network = Uuid::new_v4();
        let left = Uuid::from_u128(10);
        let right = Uuid::from_u128(20);
        let first = NodeCandidate {
            id: Uuid::from_u128(1),
            peer_url: "tcp://first:1".into(),
        };
        let second = NodeCandidate {
            id: Uuid::from_u128(2),
            peer_url: "tcp://second:2".into(),
        };
        let nodes = [first.clone(), second.clone()];
        let intent = CentralNetworkIntent {
            id: network,
            user_id: 1,
            display_name: "network".into(),
            network_name: "mesh".into(),
            network_secret: "test".into(),
            mode: NetworkMode::Gateway {
                peer_url: first.peer_url.clone(),
            },
            virtual_cidr: None,
            secure_mode: true,
            credentials: Vec::new(),
            acl_policy: None,
            members: [left, right]
                .into_iter()
                .enumerate()
                .map(|(index, device)| NetworkMemberIntent {
                    id: device,
                    device_id: device.to_string(),
                    hostname: None,
                    virtual_ipv4: Some(format!("10.0.0.{}", index + 1)),
                    allocated_ipv4: None,
                    config_override: None,
                    credential_id: None,
                    acl_group_secret: None,
                })
                .collect(),
        };
        let observation = |target: &str, direct| ObservedClientPath {
            network_id: network,
            destination: target.parse().unwrap(),
            direct,
        };
        scheduler.observe(
            left,
            Some(vec![
                NodeMeasurement {
                    id: first.id,
                    latency_us: 4_000,
                },
                NodeMeasurement {
                    id: second.id,
                    latency_us: 9_000,
                },
            ]),
            vec![observation("10.0.0.2", true)],
        );
        scheduler.observe(
            right,
            Some(vec![
                NodeMeasurement {
                    id: first.id,
                    latency_us: 20_000,
                },
                NodeMeasurement {
                    id: second.id,
                    latency_us: 3_000,
                },
            ]),
            vec![observation("10.0.0.1", true)],
        );

        let assigned = scheduler.reconcile(&intent, &nodes, false);
        assert_eq!(
            scheduler.assignment(network, left),
            Some((first.id, "direct"))
        );
        assert_eq!(assigned[&left].peer_url, first.peer_url);
        assert_eq!(assigned[&right].peer_url, second.peer_url);
        let expire_trial = || {
            scheduler
                .0
                .lock()
                .unwrap()
                .routes
                .get_mut(&(network, left))
                .unwrap()
                .started -= DIRECT_ATTEMPT
        };
        expire_trial();
        assert_eq!(
            scheduler.reconcile(&intent, &nodes, false)[&left].peer_url,
            first.peer_url
        );

        scheduler.observe(left, None, vec![observation("10.0.0.2", false)]);
        scheduler.observe(right, None, vec![observation("10.0.0.1", false)]);
        assert_eq!(
            scheduler.reconcile(&intent, &nodes, false)[&left].peer_url,
            second.peer_url
        );
        assert_eq!(
            scheduler.assignment(network, left),
            Some((second.id, "relay"))
        );
        assert!(
            matches!(scheduler.0.lock().unwrap().routes[&(network, left)].stage, Stage::Retry { peer } if peer == right)
        );
        expire_trial();
        scheduler.observe(left, None, Vec::new());
        let relayed = scheduler.reconcile(&intent, &nodes, false);
        assert_eq!(
            scheduler.assignment(network, left),
            Some((second.id, "unobserved"))
        );
        assert_eq!(relayed[&left].peer_url, second.peer_url);
        assert_eq!(relayed[&right].peer_url, second.peer_url);

        assert_eq!(
            scheduler.reconcile(&intent, std::slice::from_ref(&first), false)[&left].peer_url,
            first.peer_url
        );
    }

    #[test]
    fn wss_node_endpoint_matches_the_tcp_host_after_normalization() {
        for host in ["192.0.2.10", "[2001:db8::1]", "NODE.example"] {
            let tcp = validate_node_url(&format!("tcp://{host}:22021")).unwrap();
            let wss = format!("wss://{host}/_bilin/relay").parse().unwrap();
            assert!(validate_node_wss_url(&tcp, &wss).is_ok());
        }
        let tcp = validate_node_url("tcp://192.0.2.10:22021").unwrap();
        for endpoint in [
            "wss://192.0.2.11/_bilin/relay",
            "ws://192.0.2.10/_bilin/relay",
            "wss://192.0.2.10/another-path",
        ] {
            assert!(validate_node_wss_url(&tcp, &endpoint.parse().unwrap()).is_err());
        }
    }

    #[test]
    fn older_nodes_can_report_retired_transport_without_advertising_it() {
        let scheduler = Scheduler::default();
        let node = NodeCandidate {
            id: Uuid::new_v4(),
            peer_url: "tcp://192.0.2.10:22021".into(),
        };
        let wss_url = "wss://192.0.2.10/_bilin/relay";
        let report: NodeReport = serde_json::from_value(serde_json::json!({
            "ready": true,
            "udp_ready": true,
            "quic_ready": true,
            "kcp_ready": true,
            "faketcp_ready": true,
            "wss_url": wss_url,
        }))
        .unwrap();
        assert!(!serde_json::to_string(&report).unwrap().contains("faketcp"));
        scheduler.observe_node(node.id, report);

        assert_eq!(
            scheduler.transport_config(Some(&node), true, true, true),
            [
                "udp://192.0.2.10:22021",
                "quic://192.0.2.10:22022",
                "kcp://192.0.2.10:22023",
                "tcp://192.0.2.10:22021",
                wss_url,
            ]
        );
    }

    #[test]
    fn account_udp_publishes_dual_endpoints_only_while_ready() {
        let scheduler = Scheduler::default();
        let node = NodeCandidate {
            id: Uuid::new_v4(),
            peer_url: "tcp://[2001:db8::1]:22021".into(),
        };
        let tcp = vec![node.peer_url.clone()];
        let dual = vec!["udp://[2001:db8::1]:22021".into(), node.peer_url.clone()];
        let old_report: NodeReport = serde_json::from_str(r#"{"ready":true}"#).unwrap();
        assert!(!old_report.udp_ready);
        assert!(!old_report.quic_ready);
        assert_eq!(
            scheduler.transport_config(Some(&node), true, false, false),
            tcp
        );
        scheduler.observe_node_udp(node.id, true);
        scheduler.observe_node_quic(node.id, true);
        assert_eq!(
            scheduler.transport_config(Some(&node), true, false, false),
            dual
        );
        assert_eq!(
            scheduler.transport_config(Some(&node), false, false, false),
            tcp
        );
        assert_eq!(
            scheduler.transport_config(Some(&node), false, true, false),
            tcp
        );
        assert_eq!(
            scheduler.transport_config(Some(&node), true, true, false),
            [
                dual[0].clone(),
                "quic://[2001:db8::1]:22022".into(),
                tcp[0].clone()
            ]
        );
        *scheduler
            .0
            .lock()
            .unwrap()
            .udp_ready_nodes
            .get_mut(&node.id)
            .unwrap() -= Duration::from_secs(NODE_TIMEOUT as u64);
        assert_eq!(
            scheduler.transport_config(Some(&node), true, false, false),
            tcp
        );
        scheduler.observe_node_udp(node.id, true);
        scheduler.observe_node_udp(node.id, false);
        assert_eq!(
            scheduler.transport_config(Some(&node), true, false, false),
            tcp
        );
    }
}

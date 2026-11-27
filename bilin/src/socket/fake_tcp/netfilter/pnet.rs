use std::{
    io,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Weak,
        atomic::{AtomicU32, Ordering},
    },
};

use bytes::{Bytes, BytesMut};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use pnet_datalink::{self as datalink, DataLinkSender, NetworkInterface};
use smoltcp::wire::{
    EthernetFrame, EthernetProtocol, IpProtocol, Ipv4Packet, Ipv6Packet, TcpPacket,
};
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
use tokio::sync::Mutex;

use crate::socket::fake_tcp::stack;

type PacketFilter = Box<dyn Fn(&[u8]) -> bool + Send + Sync>;

fn filter_tcp_packet(
    packet: &[u8],
    src_addr: Option<&SocketAddr>,
    dst_addr: Option<&SocketAddr>,
) -> bool {
    let Ok(ethernet) = EthernetFrame::new_checked(packet) else {
        return false;
    };

    match ethernet.ethertype() {
        EthernetProtocol::Ipv4 => {
            let Ok(ipv4) = Ipv4Packet::new_checked(ethernet.payload()) else {
                return false;
            };

            if ipv4.next_header() != IpProtocol::Tcp {
                return false;
            }

            let Ok(tcp) = TcpPacket::new_checked(ipv4.payload()) else {
                return false;
            };

            if let Some(src_addr) = src_addr {
                if IpAddr::V4(ipv4.src_addr()) != src_addr.ip() {
                    return false;
                }
                if tcp.src_port() != src_addr.port() {
                    return false;
                }
            }

            if let Some(dst_addr) = dst_addr {
                if IpAddr::V4(ipv4.dst_addr()) != dst_addr.ip() {
                    return false;
                }
                if tcp.dst_port() != dst_addr.port() {
                    return false;
                }
            }

            tracing::trace!(
                ?tcp,
                "FakeTcpSocketListener packet matched filter, dispatching, src_addr: {:?}, dst_addr: {:?}, packet_src_ip: {:?}, packet_dst_ip: {:?}, packet_src_port: {:?}, packet_dst_port: {:?}",
                src_addr,
                dst_addr,
                ipv4.src_addr(),
                ipv4.dst_addr(),
                tcp.src_port(),
                tcp.dst_port(),
            );
        }
        EthernetProtocol::Ipv6 => {
            let Ok(ipv6) = Ipv6Packet::new_checked(ethernet.payload()) else {
                return false;
            };

            if ipv6.next_header() != IpProtocol::Tcp {
                return false;
            }

            let Ok(tcp) = TcpPacket::new_checked(ipv6.payload()) else {
                return false;
            };

            if let Some(src_addr) = src_addr {
                if IpAddr::V6(ipv6.src_addr()) != src_addr.ip() {
                    return false;
                }
                if tcp.src_port() != src_addr.port() {
                    return false;
                }
            }

            if let Some(dst_addr) = dst_addr {
                if IpAddr::V6(ipv6.dst_addr()) != dst_addr.ip() {
                    return false;
                }
                if tcp.dst_port() != dst_addr.port() {
                    return false;
                }
            }

            tracing::trace!(
                ?tcp,
                "FakeTcpSocketListener packet matched filter, dispatching"
            );
        }
        _ => return false,
    }

    true
}

pub fn create_packet_filter(src_addr: Option<SocketAddr>, dst_addr: SocketAddr) -> PacketFilter {
    Box::new(move |packet: &[u8]| -> bool {
        filter_tcp_packet(packet, src_addr.as_ref(), Some(&dst_addr))
    })
}

struct Subscriber {
    filter: PacketFilter,
    sender: tokio::sync::mpsc::Sender<Vec<u8>>,
}

struct InterfaceWorker {
    tx: Mutex<Box<dyn DataLinkSender>>,
    subscribers: Arc<DashMap<u32, Subscriber>>,
}

impl InterfaceWorker {
    fn new(interface: NetworkInterface) -> io::Result<Arc<Self>> {
        let (tx, mut rx) = match datalink::channel(&interface, Default::default()) {
            Ok(datalink::Channel::Ethernet(tx, rx)) => (tx, rx),
            Ok(_) => return Err(io::Error::other("Unhandled channel type")),
            Err(e) => return Err(io::Error::other(e)),
        };

        let subscribers = Arc::new(DashMap::<u32, Subscriber>::new());
        let subscribers_clone = subscribers.clone();

        std::thread::spawn(move || {
            loop {
                match rx.next() {
                    Ok(packet) => {
                        // Iterate over subscribers and send packet if filter matches
                        // Note: DashMap iteration might be slow if many subscribers, but usually few per interface.
                        // For high performance we might need a better structure or read-copy-update.
                        for r in subscribers_clone.iter() {
                            let subscriber = r.value();
                            if (subscriber.filter)(packet) {
                                tracing::trace!(
                                    ?packet,
                                    "InterfaceWorker packet matched filter, dispatching"
                                );
                                // Try send, ignore errors (best effort)
                                let _ = subscriber.sender.try_send(packet.to_vec());
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("InterfaceWorker read error: {}", e);
                        // If interface goes down, we might need to handle it.
                        // For now just break and maybe the worker is dead.
                        break;
                    }
                }
            }
        });

        Ok(Arc::new(Self {
            tx: Mutex::new(tx),
            subscribers,
        }))
    }

    fn subscribe(&self, filter: PacketFilter, sender: tokio::sync::mpsc::Sender<Vec<u8>>) -> u32 {
        static ID_GEN: AtomicU32 = AtomicU32::new(0);
        let id = ID_GEN.fetch_add(1, Ordering::Relaxed);
        self.subscribers.insert(id, Subscriber { filter, sender });
        id
    }

    fn unsubscribe(&self, id: u32) {
        self.subscribers.remove(&id);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum NetworkNamespaceId {
    #[cfg(target_os = "linux")]
    LinuxInode(u64),
    #[cfg(not(target_os = "linux"))]
    Unsupported,
}

type InterfaceWorkerKey = (NetworkNamespaceId, String);

static INTERFACE_MANAGERS: Lazy<DashMap<InterfaceWorkerKey, Weak<InterfaceWorker>>> =
    Lazy::new(DashMap::new);

fn current_network_namespace_id() -> Option<NetworkNamespaceId> {
    #[cfg(target_os = "linux")]
    {
        match std::fs::metadata("/proc/thread-self/ns/net") {
            Ok(metadata) => Some(NetworkNamespaceId::LinuxInode(metadata.ino())),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "failed to identify current network namespace; disabling FakeTCP pnet worker sharing"
                );
                None
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    Some(NetworkNamespaceId::Unsupported)
}

fn get_or_create_worker(interface_name: &str) -> io::Result<Arc<InterfaceWorker>> {
    let key = current_network_namespace_id()
        .map(|namespace_id| (namespace_id, interface_name.to_owned()));
    if let Some(worker) = cached_worker(key.as_ref()) {
        return Ok(worker);
    }

    let interface = interfaces()?
        .into_iter()
        .find(|iface| iface.name == interface_name)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("Network interface '{}' not found", interface_name),
            )
        })?;

    create_worker(interface, key)
}

fn interfaces() -> io::Result<Vec<NetworkInterface>> {
    std::panic::catch_unwind(datalink::interfaces)
        .map_err(|_| io::Error::other("failed to enumerate network interfaces: pnet panicked"))
}

fn cached_worker(key: Option<&InterfaceWorkerKey>) -> Option<Arc<InterfaceWorker>> {
    key.and_then(|key| INTERFACE_MANAGERS.get(key))
        .and_then(|worker| worker.upgrade())
}

fn create_worker(
    interface: NetworkInterface,
    key: Option<InterfaceWorkerKey>,
) -> io::Result<Arc<InterfaceWorker>> {
    let worker = InterfaceWorker::new(interface)?;
    if let Some(key) = key {
        INTERFACE_MANAGERS.insert(key, Arc::downgrade(&worker));
    }
    Ok(worker)
}

#[cfg(any(windows, test))]
fn pnet_interface_for_local_ip(
    interfaces: Vec<NetworkInterface>,
    local_ip: IpAddr,
) -> io::Result<NetworkInterface> {
    let mut matching = interfaces
        .into_iter()
        .filter(|interface| interface.ips.iter().any(|network| network.ip() == local_ip));
    let interface = matching.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no packet capture adapter is available for local address {local_ip}"),
        )
    })?;
    if matching.next().is_some() {
        return Err(io::Error::other(format!(
            "multiple packet capture adapters have local address {local_ip}"
        )));
    }
    Ok(interface)
}

pub struct PnetTun {
    worker: Arc<InterfaceWorker>,
    subscription_id: u32,
    recv_queue: Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>,
}

impl PnetTun {
    pub fn new(interface_name: &str, filter: PacketFilter) -> io::Result<Self> {
        tracing::debug!(interface_name, "Creating new PnetTun");
        Ok(Self::subscribe(
            get_or_create_worker(interface_name)?,
            filter,
        ))
    }

    #[cfg(windows)]
    pub fn new_for_local_ip(local_ip: IpAddr, filter: PacketFilter) -> io::Result<Self> {
        // WinPcap/Npcap exposes a capture device ID, not the Windows friendly name.
        let interface = pnet_interface_for_local_ip(interfaces()?, local_ip)?;
        tracing::debug!(%local_ip, adapter = interface.name, "Creating new PnetTun");
        let key = current_network_namespace_id()
            .map(|namespace_id| (namespace_id, interface.name.clone()));
        let worker = match cached_worker(key.as_ref()) {
            Some(worker) => worker,
            None => create_worker(interface, key)?,
        };
        Ok(Self::subscribe(worker, filter))
    }

    fn subscribe(worker: Arc<InterfaceWorker>, filter: PacketFilter) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(1024);
        let id = worker.subscribe(filter, tx);

        Self {
            worker,
            subscription_id: id,
            recv_queue: Mutex::new(rx),
        }
    }
}

impl Drop for PnetTun {
    fn drop(&mut self) {
        tracing::debug!(subscription_id = self.subscription_id, "Dropping PnetTun");
        self.worker.unsubscribe(self.subscription_id);
    }
}

#[async_trait::async_trait]
impl stack::Tun for PnetTun {
    async fn recv(&self, packet: &mut BytesMut) -> Result<usize, std::io::Error> {
        let mut rx = self.recv_queue.lock().await;
        match rx.recv().await {
            Some(data) => {
                tracing::trace!(?data, "PnetTun received packet");
                packet.extend_from_slice(&data);
                Ok(data.len())
            }
            None => {
                tracing::warn!("PnetTun recv channel closed");
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "PnetTun channel closed",
                ))
            }
        }
    }

    fn try_send(&self, packet: &Bytes) -> Result<(), std::io::Error> {
        tracing::trace!(len = packet.len(), "PnetTun try_sending packet");
        // We need async lock for tx.
        // try_send is sync. We can use try_lock if available or blocking lock.
        // tokio::sync::Mutex::try_lock is available.
        if let Ok(mut tx) = self.worker.tx.try_lock() {
            tx.send_to(packet, None)
                .ok_or(std::io::Error::other("send_to failed"))?
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "PnetTun tx lock busy",
            ))
        }
    }

    fn driver_type(&self) -> &'static str {
        "pnet"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_the_capture_id_by_exact_local_address() {
        let adapter = |name: &str, address: &str| NetworkInterface {
            name: name.to_owned(),
            description: "以太网 2".to_owned(),
            index: 0,
            mac: None,
            ips: vec![address.parse().unwrap()],
            flags: 0,
        };
        let local_ip = "192.168.100.124".parse().unwrap();
        let interfaces = vec![
            adapter(r"\Device\NPF_{OTHER}", "192.168.100.125/24"),
            adapter(r"\Device\NPF_{SELECTED}", "192.168.100.124/24"),
        ];
        let selected = pnet_interface_for_local_ip(interfaces.clone(), local_ip).unwrap();
        assert_eq!(selected.name, r"\Device\NPF_{SELECTED}");
        let missing = pnet_interface_for_local_ip(interfaces, "192.168.100.126".parse().unwrap());
        assert_eq!(missing.unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}

mod netfilter;
mod packet;
mod stack;

use bytes::BytesMut;
use network_interface::NetworkInterfaceConfig;
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    net::TcpStream,
};

use bilin_core::{
    socket::tcp::VirtualTcpSocket,
    tunnel::{IpVersion, TunnelError},
};

use crate::{common::netns::NetNS, tunnel::FromUrl};

use self::netfilter::create_tun;
use self::packet::MacAddr;

use futures::Future;
use tokio_util::task::AbortOnDropHandle;

use dashmap::DashMap;

struct IpToIfNameCache {
    ip_to_ifname: DashMap<IpAddr, (String, Option<MacAddr>)>,
}

fn parse_mac_addr(value: &str) -> Option<MacAddr> {
    let mut bytes = [0; 6];
    let mut octets = value.split([':', '-']);
    for byte in &mut bytes {
        *byte = u8::from_str_radix(octets.next()?, 16).ok()?;
    }
    octets.next().is_none().then(|| MacAddr::from_bytes(&bytes))
}

impl IpToIfNameCache {
    fn new() -> Self {
        Self {
            ip_to_ifname: DashMap::new(),
        }
    }

    fn reload_ip_to_ifname(&self) {
        self.ip_to_ifname.clear();
        let Ok(interfaces) = network_interface::NetworkInterface::show() else {
            tracing::warn!("failed to enumerate interfaces when reloading faketcp ip cache");
            return;
        };
        for iface in interfaces {
            let mac = iface.mac_addr.as_deref().and_then(|mac| {
                parse_mac_addr(mac).or_else(|| {
                    tracing::debug!(iface = %iface.name, mac, "failed to parse interface mac");
                    None
                })
            });
            for ip in iface.addr.iter() {
                self.ip_to_ifname.insert(ip.ip(), (iface.name.clone(), mac));
            }
        }
    }

    fn get_ifname(&self, ip: &IpAddr) -> Option<(String, Option<MacAddr>)> {
        if let Some(ifname) = self.ip_to_ifname.get(ip) {
            Some(ifname.clone())
        } else {
            self.reload_ip_to_ifname();
            self.ip_to_ifname.get(ip).map(|s| s.clone())
        }
    }
}

fn faketcp_transport_label(driver_type: &str) -> String {
    format!("faketcp_{}", driver_type)
}

async fn create_tun_off_runtime(
    interface_name: String,
    src_addr: Option<SocketAddr>,
    dst_addr: SocketAddr,
    net_ns: NetNS,
) -> Result<Arc<dyn stack::Tun>, TunnelError> {
    tokio::task::spawn_blocking(move || {
        net_ns.run(|| create_tun(&interface_name, src_addr, dst_addr))
    })
    .await
    .map_err(|e| TunnelError::InternalError(format!("faketcp create_tun task failed: {e}")))?
    .map_err(Into::into)
}

pub(crate) struct FakeTcpSocketListener {
    addr: url::Url,
    os_listener: Option<tokio::net::TcpListener>,
    // Each capture filters one local IP and port, even on a multi-address interface.
    stack_map: DashMap<SocketAddr, Arc<stack::Stack>>,
    // a cache from ip addr to interface name
    ip_to_ifname: IpToIfNameCache,
}

impl std::fmt::Debug for FakeTcpSocketListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FakeTcpSocketListener")
            .field("addr", &self.addr)
            .field("listening", &self.os_listener.is_some())
            .finish()
    }
}

impl FakeTcpSocketListener {
    pub(crate) fn new(addr: url::Url) -> Self {
        FakeTcpSocketListener {
            addr,
            os_listener: None,
            stack_map: DashMap::new(),
            ip_to_ifname: IpToIfNameCache::new(),
        }
    }

    async fn do_accept(&mut self) -> Result<AcceptResult, TunnelError> {
        loop {
            match self.os_listener.as_mut().unwrap().accept().await {
                Ok((s, remote_addr)) => {
                    let Ok(local_addr) = s.local_addr() else {
                        tracing::warn!("accept fail with local_addr error");
                        continue;
                    };
                    let Some((interface_name, mac)) =
                        self.ip_to_ifname.get_ifname(&local_addr.ip())
                    else {
                        tracing::warn!("accept fail with interface_name error");
                        continue;
                    };
                    return Ok(AcceptResult {
                        socket: s,
                        local_addr,
                        remote_addr,
                        interface_name,
                        mac,
                    });
                }
                Err(e) => {
                    use std::io::ErrorKind::*;
                    if matches!(
                        e.kind(),
                        NotConnected | ConnectionAborted | ConnectionRefused | ConnectionReset
                    ) {
                        tracing::warn!(?e, "accept fail with retryable error: {:?}", e);
                        continue;
                    }
                    tracing::warn!(?e, "accept fail");
                    return Err(e.into());
                }
            }
        }
    }

    async fn get_stack(
        &self,
        local_addr: SocketAddr,
        interface_name: &str,
        mac: Option<MacAddr>,
    ) -> Result<Arc<stack::Stack>, TunnelError> {
        if let Some(entry) = self.stack_map.get(&local_addr) {
            let stack = entry.clone();
            drop(entry);

            if !stack.is_closed() {
                return Ok(stack);
            }

            tracing::warn!(
                interface_name,
                "fake_tcp stack reader_task finished, recreating stack"
            );
            self.stack_map.remove(&local_addr);
        }

        let tun = create_tun_off_runtime(
            interface_name.to_string(),
            None,
            local_addr,
            NetNS::new(None),
        )
        .await?;
        tracing::info!(
            ?local_addr,
            "create new stack with interface_name: {:?}",
            interface_name
        );
        let stack = Arc::new(stack::Stack::new(tun, mac, true));
        self.stack_map.insert(local_addr, stack.clone());

        Ok(stack)
    }
}

fn build_os_socket_reader_task(mut socket: TcpStream) -> AbortOnDropHandle<()> {
    AbortOnDropHandle::new(tokio::spawn(async move {
        // read the os socket until it's closed
        let mut buf = [0u8; 1024];
        while let Ok(size) = socket.read(&mut buf).await {
            tracing::trace!("read {} bytes from os socket", size);
            if size == 0 {
                break;
            }
        }
        tracing::info!("FakeTcpSocketListener os socket closed");
    }))
}

type FakeTcpReadFuture = Pin<Box<dyn Future<Output = Option<BytesMut>> + Send + Sync + 'static>>;

enum FakeTcpReadState {
    Buffered(BytesMut),
    Receiving(FakeTcpReadFuture),
    Closed,
}

pub(crate) struct FakeTcpSocket {
    socket: Arc<stack::Socket>,
    read_state: FakeTcpReadState,
    transport_label: String,
    _lifetime_guard: Box<dyn Send + Sync>,
}

impl FakeTcpSocket {
    fn new<T>(socket: stack::Socket, transport_label: String, lifetime_guard: T) -> Self
    where
        T: Send + Sync + 'static,
    {
        Self {
            socket: Arc::new(socket),
            read_state: FakeTcpReadState::Buffered(BytesMut::new()),
            transport_label,
            _lifetime_guard: Box::new(lifetime_guard),
        }
    }
}

impl AsyncRead for FakeTcpSocket {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            let state = std::mem::replace(&mut this.read_state, FakeTcpReadState::Closed);
            match state {
                FakeTcpReadState::Buffered(mut buffer) if !buffer.is_empty() => {
                    let length = buffer.len().min(output.remaining());
                    output.put_slice(&buffer.split_to(length));
                    this.read_state = FakeTcpReadState::Buffered(buffer);
                    return Poll::Ready(Ok(()));
                }
                FakeTcpReadState::Buffered(_) => {
                    let socket = this.socket.clone();
                    this.read_state = FakeTcpReadState::Receiving(Box::pin(async move {
                        let mut buffer = BytesMut::new();
                        socket.recv(&mut buffer).await.map(|_| buffer)
                    }));
                }
                FakeTcpReadState::Receiving(mut receive) => match receive.as_mut().poll(context) {
                    Poll::Ready(Some(buffer)) => {
                        this.read_state = FakeTcpReadState::Buffered(buffer);
                    }
                    Poll::Ready(None) => {
                        this.read_state = FakeTcpReadState::Closed;
                        return Poll::Ready(Ok(()));
                    }
                    Poll::Pending => {
                        this.read_state = FakeTcpReadState::Receiving(receive);
                        return Poll::Pending;
                    }
                },
                FakeTcpReadState::Closed => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl AsyncWrite for FakeTcpSocket {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut TaskContext<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.socket.try_send(buffer).is_none() {
            // Preserve FakeTCP's existing lossy send behavior. A temporary
            // driver lock conflict is indistinguishable from a closed stack
            // here, and the former must not tear down the peer connection.
            tracing::trace!(
                len = buffer.len(),
                "FakeTCP socket dropped an outgoing frame"
            );
        }
        Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        self.socket.close();
        Poll::Ready(Ok(()))
    }
}

impl VirtualTcpSocket for FakeTcpSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.socket.local_addr())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.socket.remote_addr())
    }

    fn transport_label(&self) -> Option<&str> {
        Some(&self.transport_label)
    }
}

#[derive(Debug)]
struct AcceptResult {
    socket: TcpStream,
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
    interface_name: String,
    mac: Option<MacAddr>,
}

impl FakeTcpSocketListener {
    pub(crate) async fn accept_socket(&mut self) -> Result<FakeTcpSocket, TunnelError> {
        tracing::debug!("FakeTcpSocketListener waiting for accept");
        let (res, stack, socket) = loop {
            let res = self.do_accept().await?;
            let stack = self
                .get_stack(res.local_addr, &res.interface_name, res.mac)
                .await?;
            let socket = stack.try_alloc_established_socket(
                res.local_addr,
                res.remote_addr,
                stack::State::Established,
            );
            let Some(socket) = socket else {
                tracing::warn!(
                    interface_name = res.interface_name,
                    "fake_tcp stack closed while accepting connection, dropping accepted socket"
                );
                self.stack_map.remove(&res.local_addr);
                continue;
            };
            break (res, stack, socket);
        };

        tracing::info!(
            ?res,
            remote = socket.remote_addr().to_string(),
            "FakeTcpSocketListener accepted connection"
        );

        let transport_label = faketcp_transport_label(stack.driver_type());
        Ok(FakeTcpSocket::new(
            socket,
            transport_label,
            (build_os_socket_reader_task(res.socket), stack),
        ))
    }

    async fn listen_socket(&mut self) -> Result<(), TunnelError> {
        if self.os_listener.is_some() {
            return Ok(());
        }
        let bind_addr = SocketAddr::from_url(self.addr.clone(), IpVersion::Both).await?;
        let os_listener = tokio::net::TcpListener::bind(bind_addr).await?;
        let port = os_listener.local_addr()?.port();
        self.addr.set_port(Some(port)).map_err(|()| {
            TunnelError::InvalidAddr(format!("FakeTCP listener URL cannot store port {port}"))
        })?;
        self.ip_to_ifname.reload_ip_to_ifname();
        let interfaces: Vec<_> = self
            .ip_to_ifname
            .ip_to_ifname
            .iter()
            .filter(|entry| {
                let ip = *entry.key();
                ip == bind_addr.ip()
                    || (bind_addr.ip().is_unspecified() && ip.is_ipv4() == bind_addr.is_ipv4())
            })
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();
        // Capture the first payload as soon as the kernel TCP handshake completes.
        for (ip, (interface, mac)) in interfaces {
            let local_addr = SocketAddr::new(ip, port);
            if let Err(error) = self.get_stack(local_addr, &interface, mac).await {
                tracing::debug!(%local_addr, %interface, %error, "FakeTCP capture will retry on accept");
            }
        }
        tracing::info!(port, "FakeTcpSocketListener listening");
        self.os_listener = Some(os_listener);
        Ok(())
    }
}

#[async_trait::async_trait]
impl bilin_core::socket::SocketListener for FakeTcpSocketListener {
    type Accepted = FakeTcpSocket;

    async fn listen(&mut self) -> anyhow::Result<()> {
        Ok(self.listen_socket().await?)
    }

    async fn accept(&mut self) -> anyhow::Result<Self::Accepted> {
        Ok(self.accept_socket().await?)
    }

    fn local_url(&self) -> url::Url {
        self.addr.clone()
    }
}

fn get_local_ip_for_destination(destination: SocketAddr) -> Option<IpAddr> {
    // 使用一个不可路由的、私有的、或回环地址创建一个临时的 socket，让内核自动选择源接口。
    // 对于 IPv4，使用 0.0.0.0; 对于 IPv6，使用 ::
    let bind_addr = if destination.is_ipv4() {
        IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))
    } else {
        IpAddr::V6(std::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0))
    };

    // 绑定到一个临时端口 (0)
    let socket = UdpSocket::bind((bind_addr, 0)).ok()?;

    // 尝试连接到目标地址。这不会真正发送数据包，只是让内核确定路由。
    socket.connect(destination).ok()?;

    // 获取 socket 的本地地址信息
    socket.local_addr().map(|addr| addr.ip()).ok()
}

async fn connect_socket_with_cache(
    remote_addr: SocketAddr,
    socket_mark: Option<u32>,
    ip_to_if_name: &IpToIfNameCache,
    net_ns: NetNS,
) -> Result<FakeTcpSocket, TunnelError> {
    let (local_addr, interface_name, mac, os_socket) = net_ns.run(|| {
        let mut route_probe = remote_addr;
        if !cfg!(all(
            windows,
            any(target_arch = "x86_64", target_arch = "x86")
        )) {
            route_probe.set_port(80);
        }
        let local_ip = get_local_ip_for_destination(route_probe)
            .ok_or(TunnelError::InternalError("Failed to get local ip".into()))?;
        let os_socket = tokio::net::TcpSocket::new_v4()?;
        // SO_MARK applies only to the kernel-visible "decoy" socket below.
        // The actual FakeTCP payload travels via crafted segments written
        // straight to the TUN device, which the kernel doesn't tag with
        // SO_MARK. Operators relying on fwmark for FakeTCP must mark the
        // TUN device's traffic with a separate nftables/iptables rule.
        crate::tunnel::common::apply_socket_mark(&socket2::SockRef::from(&os_socket), socket_mark)?;
        let mut bind_ip = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        let capture_by_local_ip = cfg!(all(
            windows,
            any(target_arch = "x86_64", target_arch = "x86")
        ));
        if capture_by_local_ip {
            // The capture filters by local IP and port. Give TCP the same
            // source IP so Windows cannot choose a different capture tuple.
            bind_ip = local_ip;
        }
        os_socket.bind(SocketAddr::new(bind_ip, 0))?;
        let local_addr = SocketAddr::new(local_ip, os_socket.local_addr()?.port());
        let (interface_name, mac) = match ip_to_if_name.get_ifname(&local_ip) {
            Some(interface) => interface,
            None if capture_by_local_ip => (String::new(), None),
            None => {
                return Err(TunnelError::InternalError(
                    "Failed to get interface name".into(),
                ));
            }
        };
        Ok::<_, TunnelError>((local_addr, interface_name, mac, os_socket))
    })?;

    let tun = create_tun_off_runtime(interface_name, Some(remote_addr), local_addr, net_ns).await?;
    let stack = stack::Stack::new(tun, mac, false);
    let socket = stack
        .try_alloc_established_socket(local_addr, remote_addr, stack::State::SynSent)
        .ok_or(TunnelError::InternalError(
            "FakeTCP stack closed while allocating socket".into(),
        ))?;
    let driver = stack.driver_type();
    let transport_label = faketcp_transport_label(driver);
    let os_stream = os_socket.connect(remote_addr).await.map_err(|error| {
        TunnelError::ProtocolError(format!("FakeTCP TCP connection to {remote_addr}: {error}"))
    })?;
    tracing::debug!(%local_addr, %remote_addr, driver, "FakeTCP TCP handshake finished; waiting for captured SYN/ACK");
    let mut buf = BytesMut::new();
    socket.recv(&mut buf).await.ok_or_else(|| {
        TunnelError::ProtocolError(format!(
            "FakeTCP TCP handshake succeeded but {driver} did not capture SYN/ACK for {local_addr} to {remote_addr}"
        ))
    })?;

    tracing::info!(%local_addr, %remote_addr, driver, "FakeTCP socket connected");

    Ok(FakeTcpSocket::new(
        socket,
        transport_label,
        (build_os_socket_reader_task(os_stream), stack),
    ))
}

pub(crate) async fn connect_socket(
    remote_addr: SocketAddr,
    socket_mark: Option<u32>,
    net_ns: NetNS,
) -> Result<FakeTcpSocket, TunnelError> {
    connect_socket_with_cache(remote_addr, socket_mark, &IpToIfNameCache::new(), net_ns).await
}

#[cfg(test)]
mod tests {
    use bilin_core::socket::SocketListener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn faketcp_socket_pingpong() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            #[cfg(target_family = "unix")]
            {
                if unsafe { nix::libc::geteuid() } != 0 {
                    return;
                }
            }

            let mut listener =
                FakeTcpSocketListener::new("faketcp://0.0.0.0:31011".parse().unwrap());
            listener.listen().await.unwrap();
            let (server_ready_tx, server_ready_rx) = tokio::sync::oneshot::channel();

            let server = tokio::spawn(async move {
                let mut socket = listener.accept().await.unwrap();
                server_ready_tx.send(()).unwrap();
                let mut request = [0; 4];
                socket.read_exact(&mut request).await.unwrap();
                assert_eq!(&request, b"ping");
                socket.write_all(b"pong").await.unwrap();
            });

            let mut socket =
                connect_socket("127.0.0.1:31011".parse().unwrap(), None, NetNS::new(None))
                    .await
                    .unwrap();
            server_ready_rx.await.unwrap();
            socket.write_all(b"ping").await.unwrap();
            let mut response = [0; 4];
            socket.read_exact(&mut response).await.unwrap();
            assert_eq!(&response, b"pong");

            server.await.unwrap();
        })
        .await
        .expect("FakeTCP socket ping-pong timed out");
    }
}

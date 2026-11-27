use std::net::SocketAddr;

#[cfg(target_os = "linux")]
use anyhow::Context as _;
use bilin_core::{
    connectivity::protocol::{
        CoreServerProtocolConfig, CoreServerProtocolUpgrader, ServerProtocolUpgrade,
        ServerProtocolUpgrader,
    },
    socket::SocketListener,
    tunnel::Tunnel,
};

use crate::socket::fake_tcp::{FakeTcpSocket, FakeTcpSocketListener};

/// A FakeTCP packet tunnel listener for a standalone forwarding node.
pub fn runtime_faketcp_tunnel_listener(
    bind: SocketAddr,
) -> impl SocketListener<Accepted = Box<dyn Tunnel>> {
    let url = format!("faketcp://{bind}")
        .parse()
        .expect("socket address must be a valid FakeTCP URL");
    StandaloneFakeTcpTunnelListener {
        bind,
        socket: FakeTcpSocketListener::new(url),
    }
}

#[derive(Debug)]
struct StandaloneFakeTcpTunnelListener {
    bind: SocketAddr,
    socket: FakeTcpSocketListener,
}

#[async_trait::async_trait]
impl SocketListener for StandaloneFakeTcpTunnelListener {
    type Accepted = Box<dyn Tunnel>;

    async fn listen(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(self.bind.is_ipv4(), "FakeTCP requires an IPv4 binding");

        #[cfg(target_os = "linux")]
        {
            // The native stack can fall back to Pnet when eBPF is unavailable.
            // A raw packet socket still has to be possible before advertising
            // this optional listener as ready.
            let _raw_socket = socket2::Socket::new(
                socket2::Domain::from(nix::libc::AF_PACKET),
                socket2::Type::RAW,
                Some(socket2::Protocol::from(i32::from(
                    (nix::libc::ETH_P_ALL as u16).to_be(),
                ))),
            )
            .context("FakeTCP requires access to Linux raw packet sockets (CAP_NET_RAW)")?;
        }

        self.socket.listen().await
    }

    async fn accept(&mut self) -> anyhow::Result<Self::Accepted> {
        let socket = self.socket.accept().await?;
        let protocol = CoreServerProtocolUpgrader::<FakeTcpSocket>::new(CoreServerProtocolConfig {
            faketcp: true,
            ..Default::default()
        });
        match protocol
            .upgrade_tcp(socket, self.socket.local_url())
            .await?
        {
            ServerProtocolUpgrade::Tunnel(tunnel) => Ok(tunnel),
            ServerProtocolUpgrade::Acceptor(_) => {
                anyhow::bail!("FakeTCP upgrade unexpectedly returned an acceptor")
            }
        }
    }

    fn local_url(&self) -> url::Url {
        self.socket.local_url()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::time::Duration;

    use bilin_core::{
        connectivity::{
            protocol::{
                ClientProtocolUpgrader, CoreClientProtocolConfig, CoreClientProtocolUpgrader,
            },
            transport::ConnectedTransport,
        },
        packet::ZCPacket,
    };
    use futures::{SinkExt, StreamExt};

    use crate::{common::netns::NetNS, socket::fake_tcp::connect_socket};

    use super::*;

    #[tokio::test]
    async fn standalone_faketcp_listener_transfers_packets_both_ways() {
        if unsafe { nix::libc::geteuid() } != 0 {
            return;
        }

        tokio::time::timeout(Duration::from_secs(8), async {
            let mut listener = runtime_faketcp_tunnel_listener("127.0.0.1:0".parse().unwrap());
            listener.listen().await.unwrap();
            let url = listener.local_url();
            let port = url.port().expect("bound FakeTCP port");
            assert_ne!(port, 0);
            let peer = SocketAddr::from(([127, 0, 0, 1], port));
            let (accepted, connected) = tokio::join!(
                listener.accept(),
                connect_socket(peer, None, NetNS::new(None))
            );
            let server = accepted.unwrap();
            let client =
                CoreClientProtocolUpgrader::<FakeTcpSocket>::new(CoreClientProtocolConfig {
                    faketcp: true,
                    ..Default::default()
                })
                .upgrade_client(ConnectedTransport::Tcp(connected.unwrap()), url.clone())
                .await
                .unwrap();
            for tunnel in [&server, &client] {
                assert!(tunnel.info().unwrap().tunnel_type.starts_with("faketcp_"));
            }
            assert_eq!(server.info().unwrap().local_addr.unwrap().url, url.as_str());

            let (mut from_client, mut to_client) = server.split();
            let (mut from_server, mut to_server) = client.split();
            to_server
                .send(ZCPacket::new_with_payload(b"Bilin FakeTCP request"))
                .await
                .unwrap();
            assert_eq!(
                from_client.next().await.unwrap().unwrap().payload(),
                b"Bilin FakeTCP request"
            );
            to_client
                .send(ZCPacket::new_with_payload(b"Bilin FakeTCP response"))
                .await
                .unwrap();
            assert_eq!(
                from_server.next().await.unwrap().unwrap().payload(),
                b"Bilin FakeTCP response"
            );
        })
        .await
        .expect("standalone FakeTCP packet transfer timed out");
    }
}

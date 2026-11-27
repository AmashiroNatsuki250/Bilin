//! Reliable KCP carrier for full Bilin peer packets over UDP.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use bytes::{Bytes, BytesMut};
use kcp_sys::{
    endpoint::{KcpEndpoint, KcpPacketReceiver, KcpPakcetSender},
    ffi_safe::KcpConfig,
    packet_def::KcpPacketHeader,
    stream::KcpStream,
};
use tokio::{sync::OwnedSemaphorePermit, task::JoinSet, time::timeout};
use tokio_util::task::AbortOnDropHandle;

use bilin_core::{
    connectivity::{
        protocol::{ServerProtocolAdmission, ServerProtocolAdmissionController},
        transport::ConnectedUdpSession,
    },
    socket::{
        ListenerConnectionCounter, SocketListener,
        udp::{
            KCP_TUNNEL_MAGIC, UdpBindOptions, UdpSession, UdpSessionAcceptKind,
            UdpSessionListenRequest, UdpSessionProtocol, UdpSessionSocket,
            UdpSessionSocketListener,
        },
    },
    tunnel::{
        Tunnel,
        framed::{FramedReader, FramedWriter},
        wrapper::TunnelWrapper,
    },
};

use crate::{
    host_runtime::{NativeHostRuntime, native_host_runtime},
    proto::common::TunnelInfo,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(4);
// Includes room for the KCP header, scheme marker and UDP/IPv6 headers
// below the 1280-byte IPv6 minimum path MTU.
const KCP_MTU: i32 = 1_180;
const MAX_DATAGRAM: usize =
    KCP_MTU as usize + size_of::<KcpPacketHeader>() + KCP_TUNNEL_MAGIC.len();
const MAX_FRAMED_PACKET: usize = 4_500;

fn endpoint() -> KcpEndpoint {
    let mut endpoint = KcpEndpoint::new();
    endpoint.set_kcp_config_factory(Box::new(|conversation| KcpConfig {
        mtu: Some(KCP_MTU),
        sndwnd: Some(128),
        rcvwnd: Some(128),
        nc: Some(0),
        ..KcpConfig::new_turbo(conversation)
    }));
    endpoint
}

async fn drive_udp(
    session: UdpSession,
    input: KcpPakcetSender,
    mut output: KcpPacketReceiver,
    accepting: bool,
) -> Result<()> {
    let conversation = AtomicU64::new(0);
    let incoming = async {
        // One extra byte distinguishes oversized datagrams from full buffers.
        let mut buffer = [0u8; MAX_DATAGRAM + 1];
        loop {
            let size = session.recv(&mut buffer).await?;
            if size > MAX_DATAGRAM {
                continue;
            }
            let Some(payload) = buffer[..size].strip_prefix(&KCP_TUNNEL_MAGIC) else {
                continue;
            };
            if payload.len() < size_of::<KcpPacketHeader>()
                || payload[4..12] != [0; 8]
                || payload[13] != 1
            {
                continue;
            }
            let identifier = u64::from(u32::from_le_bytes(payload[..4].try_into()?)) + 1;
            // A UDP tuple carries one conversation. Permit only its initial
            // control frame to reserve it, then ignore other conversations.
            if accepting && payload.len() == size_of::<KcpPacketHeader>() && payload[12] == 1 {
                let _ = conversation.compare_exchange(
                    0,
                    identifier,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
            if conversation.load(Ordering::Relaxed) == identifier {
                input
                    .send(BytesMut::from(payload).into())
                    .await
                    .context("KCP input closed")?;
            }
        }
    };
    let outgoing = async {
        let mut buffer = Vec::with_capacity(MAX_DATAGRAM);
        while let Some(packet) = output.recv().await {
            let header = packet.header();
            let identifier = u64::from(header.conv()) + 1;
            if !accepting && header.is_syn() && !header.is_ack() {
                let _ = conversation.compare_exchange(
                    0,
                    identifier,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
            buffer.clear();
            buffer.extend_from_slice(&KCP_TUNNEL_MAGIC);
            buffer.extend_from_slice(&packet.inner());
            let written = session.send(&buffer).await?;
            if written != buffer.len() {
                bail!("short KCP UDP send");
            }
        }
        Ok(())
    };
    tokio::select! {
        result = incoming => result,
        result = outgoing => result,
    }
}

struct KcpResources {
    _endpoint: KcpEndpoint,
    _layer: Option<Box<dyn Send + Sync>>,
    _active_session: Option<OwnedSemaphorePermit>,
}

async fn establish(
    session: UdpSession,
    local_url: Option<url::Url>,
    requested_url: Option<url::Url>,
    keep_layer: Option<Box<dyn Send + Sync>>,
    admission: Option<ServerProtocolAdmission>,
) -> Result<Box<dyn Tunnel>> {
    let local_addr = session.local_addr()?;
    let remote_addr = session.peer_addr()?;
    let accepting = admission.is_some();
    let (active_session, handshake) = if let Some(admission) = admission {
        let (active_session, available_handshakes) = admission.into_parts();
        let handshake = available_handshakes
            .try_acquire_owned()
            .context("KCP handshake capacity reached")?;
        (Some(active_session), Some(handshake))
    } else {
        (None, None)
    };
    let mut endpoint = endpoint();
    endpoint.run().await;
    let input = endpoint.input_sender();
    let output = endpoint.output_receiver().context("missing KCP output")?;
    let mut driver =
        AbortOnDropHandle::new(tokio::spawn(drive_udp(session, input, output, accepting)));
    let handshake_result = async {
        if accepting {
            anyhow::Ok(
                timeout(HANDSHAKE_TIMEOUT, endpoint.accept())
                    .await
                    .context("timed out accepting KCP carrier")??,
            )
        } else {
            Ok(endpoint
                .connect(HANDSHAKE_TIMEOUT, 0, 0, Bytes::new())
                .await?)
        }
    };
    let connection = tokio::select! {
        result = handshake_result => result,
        result = &mut driver => {
            result??;
            bail!("KCP UDP transport closed during the handshake");
        }
    }?;
    let stream = KcpStream::new(&endpoint, connection).context("missing established KCP stream")?;
    drop(handshake);
    let resources = KcpResources {
        _endpoint: endpoint,
        _layer: keep_layer,
        _active_session: active_session,
    };
    let lifetime = Arc::new(AbortOnDropHandle::new(tokio::spawn(async move {
        if let Ok(Err(error)) = driver.await {
            tracing::debug!(%error, "KCP UDP transport closed");
        }
        drop(resources);
    })));
    let local_url = local_url
        .unwrap_or_else(|| super::build_url_from_socket_addr(&local_addr.to_string(), "kcp"));
    let resolved_remote = super::build_url_from_socket_addr(&remote_addr.to_string(), "kcp");
    let info = TunnelInfo {
        tunnel_type: "kcp".to_owned(),
        local_addr: Some(local_url.into()),
        remote_addr: Some(
            requested_url
                .unwrap_or_else(|| resolved_remote.clone())
                .into(),
        ),
        resolved_remote_addr: Some(resolved_remote.into()),
    };
    let (read, write) = tokio::io::split(stream);
    Ok(Box::new(TunnelWrapper::new(
        FramedReader::new_with_associate_data(
            read,
            MAX_FRAMED_PACKET,
            Some(Box::new(lifetime.clone())),
        ),
        FramedWriter::new_with_associate_data(write, Some(Box::new(lifetime))),
        Some(info),
    )))
}

pub(crate) async fn upgrade_connected(
    connected: ConnectedUdpSession,
    requested_url: url::Url,
) -> Result<Box<dyn Tunnel>> {
    let (session, keep_layer) = connected.into_parts();
    establish(session, None, Some(requested_url), Some(keep_layer), None).await
}

pub(crate) async fn upgrade_accepted(
    session: UdpSession,
    local_url: url::Url,
    admission: ServerProtocolAdmission,
) -> Result<Box<dyn Tunnel>> {
    establish(session, Some(local_url), None, None, Some(admission)).await
}

/// Listen for KCP carriers outside a Bilin instance, for example on a gateway.
pub fn runtime_kcp_tunnel_listener(
    bind: SocketAddr,
) -> impl SocketListener<Accepted = Box<dyn Tunnel>> {
    StandaloneKcpTunnelListener {
        inner: UdpSessionSocketListener::new_with_request(
            super::build_url_from_socket_addr(&bind.to_string(), "kcp"),
            UdpSessionListenRequest::new(
                UdpBindOptions::port_bound_listener(bind).with_only_v6(bind.is_ipv6()),
            ),
            UdpSessionAcceptKind::Classified(UdpSessionProtocol::Kcp),
            native_host_runtime(),
        ),
        admission: ServerProtocolAdmissionController::kcp(),
        handshakes: JoinSet::new(),
    }
}

struct StandaloneKcpTunnelListener {
    inner: UdpSessionSocketListener<NativeHostRuntime>,
    admission: ServerProtocolAdmissionController,
    handshakes: JoinSet<Result<Box<dyn Tunnel>>>,
}

impl std::fmt::Debug for StandaloneKcpTunnelListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StandaloneKcpTunnelListener")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl SocketListener for StandaloneKcpTunnelListener {
    type Accepted = Box<dyn Tunnel>;

    async fn listen(&mut self) -> Result<()> {
        self.inner.listen().await
    }

    async fn accept(&mut self) -> Result<Self::Accepted> {
        loop {
            tokio::select! {
                biased;
                completion = self.handshakes.join_next(), if !self.handshakes.is_empty() => {
                    match completion {
                        Some(Ok(Ok(tunnel))) => return Ok(tunnel),
                        Some(Ok(Err(error))) => tracing::debug!(%error, "KCP client handshake stopped"),
                        Some(Err(error)) => tracing::warn!(%error, "KCP handshake task stopped"),
                        None => {}
                    }
                }
                session = self.inner.accept() => {
                    let session = session?;
                    if let Some(admission) = self.admission.try_admit() {
                        self.handshakes.spawn(upgrade_accepted(session, self.inner.local_url(), admission));
                    }
                }
            }
        }
    }

    fn local_url(&self) -> url::Url {
        self.inner.local_url()
    }

    fn connection_counter(&self) -> Arc<dyn ListenerConnectionCounter> {
        self.inner.connection_counter()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use bilin_core::{
        connectivity::transport::{UdpSessionMode, connect_udp},
        packet::ZCPacket,
    };
    use futures::{SinkExt as _, StreamExt as _};
    use tokio::net::UdpSocket;

    use super::*;

    #[tokio::test]
    async fn standalone_kcp_carries_peer_packets_across_a_dropped_datagram() -> Result<()> {
        let mut listener = runtime_kcp_tunnel_listener("127.0.0.1:0".parse()?);
        listener.listen().await?;
        let target = SocketAddr::from(([127, 0, 0, 1], listener.local_url().port().unwrap()));
        let edge = UdpSocket::bind("127.0.0.1:0").await?;
        let upstream = UdpSocket::bind("127.0.0.1:0").await?;
        upstream.connect(target).await?;
        let proxy_addr = edge.local_addr()?;
        let dropped = Arc::new(AtomicBool::new(false));
        let proxy_dropped = dropped.clone();
        let _proxy = AbortOnDropHandle::new(tokio::spawn(async move {
            let mut edge_buffer = [0u8; MAX_DATAGRAM + 1];
            let mut upstream_buffer = [0u8; MAX_DATAGRAM + 1];
            let mut client = None;
            loop {
                tokio::select! {
                    result = edge.recv_from(&mut edge_buffer) => {
                        let (size, addr) = result?;
                        client = Some(addr);
                        let data = size > KCP_TUNNEL_MAGIC.len() + size_of::<KcpPacketHeader>()
                            && edge_buffer[KCP_TUNNEL_MAGIC.len() + 12] & 8 != 0;
                        if !data || proxy_dropped.swap(true, Ordering::Relaxed) {
                            upstream.send(&edge_buffer[..size]).await?;
                        }
                    }
                    result = upstream.recv(&mut upstream_buffer), if client.is_some() => {
                        let size = result?;
                        edge.send_to(&upstream_buffer[..size], client.unwrap()).await?;
                    }
                }
            }
            #[allow(unreachable_code)]
            anyhow::Ok(())
        }));
        let _echo = AbortOnDropHandle::new(tokio::spawn(async move {
            let tunnel = listener.accept().await?;
            assert_eq!(
                tunnel
                    .info()
                    .context("missing KCP tunnel metadata")?
                    .tunnel_type,
                "kcp"
            );
            let (mut stream, mut sink) = tunnel.split();
            sink.send(
                stream
                    .next()
                    .await
                    .context("KCP payload was not received")??,
            )
            .await?;
            // Keep the writer alive until the client acknowledges the echoed data.
            let _ = stream.next().await;
            anyhow::Ok(())
        }));
        timeout(Duration::from_secs(12), async {
            let udp = connect_udp(
                native_host_runtime(),
                proxy_addr,
                Vec::new(),
                UdpBindOptions::direct_connect(),
                UdpSessionMode::Classified(UdpSessionProtocol::Kcp),
            )
            .await?;
            let tunnel = upgrade_connected(udp, format!("kcp://{proxy_addr}").parse()?).await?;
            assert_eq!(
                tunnel
                    .info()
                    .context("missing KCP tunnel metadata")?
                    .tunnel_type,
                "kcp"
            );
            let payload = [0x6bu8; 3_000];
            let (mut stream, mut sink) = tunnel.split();
            sink.send(ZCPacket::new_with_payload(&payload)).await?;
            let result = stream.next().await.context("KCP echo was not received")??;
            assert_eq!(result.payload(), payload);
            anyhow::Ok(())
        })
        .await??;
        assert!(dropped.load(Ordering::Relaxed));
        Ok(())
    }
}

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use bilin_core::{
    connectivity::{
        protocol::{
            ClientProtocolUpgrader, ServerProtocolAdmission, ServerProtocolUpgrade,
            ServerProtocolUpgrader,
        },
        transport::ConnectedTransport,
    },
    socket::udp::UdpSession,
    tunnel::Tunnel,
};

use crate::{
    socket::tcp::RuntimeTcpSocket,
    tunnel::kcp::{upgrade_accepted, upgrade_connected},
};

use super::{ClientAdapter, ServerAdapter};

struct KcpAdapter;

pub(super) fn client_adapter() -> ClientAdapter {
    Arc::new(KcpAdapter)
}

pub(super) fn server_adapter() -> ServerAdapter {
    Arc::new(KcpAdapter)
}

#[async_trait]
impl ClientProtocolUpgrader<RuntimeTcpSocket> for KcpAdapter {
    fn supports_scheme(&self, scheme: &str) -> bool {
        scheme == "kcp"
    }

    fn connect_timeout(&self, scheme: &str) -> Option<Duration> {
        // Leave time for KCP's handshake retransmission under congestion.
        (scheme == "kcp").then_some(Duration::from_secs(5))
    }

    async fn upgrade_client(
        &self,
        connected: ConnectedTransport<RuntimeTcpSocket>,
        requested_url: url::Url,
    ) -> anyhow::Result<Box<dyn Tunnel>> {
        let ConnectedTransport::Udp(session) = connected else {
            anyhow::bail!("KCP protocol requires a UDP session");
        };
        upgrade_connected(session, requested_url).await
    }
}

#[async_trait]
impl ServerProtocolUpgrader<RuntimeTcpSocket> for KcpAdapter {
    fn supports_scheme(&self, scheme: &str) -> bool {
        scheme == "kcp"
    }

    async fn upgrade_tcp(
        &self,
        _socket: RuntimeTcpSocket,
        _local_url: url::Url,
    ) -> anyhow::Result<ServerProtocolUpgrade> {
        anyhow::bail!("KCP protocol requires a UDP session");
    }

    async fn upgrade_udp(
        &self,
        session: UdpSession,
        local_url: url::Url,
        admission: Option<ServerProtocolAdmission>,
    ) -> anyhow::Result<ServerProtocolUpgrade> {
        let admission =
            admission.ok_or_else(|| anyhow::anyhow!("KCP server admission permit is missing"))?;
        Ok(ServerProtocolUpgrade::Tunnel(
            upgrade_accepted(session, local_url, admission).await?,
        ))
    }

    async fn upgrade_byte_stream(
        &self,
        _socket: RuntimeTcpSocket,
        _local_url: url::Url,
        _remote_url: Option<url::Url>,
    ) -> anyhow::Result<ServerProtocolUpgrade> {
        anyhow::bail!("KCP protocol requires a UDP session");
    }
}

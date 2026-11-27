use super::FromUrl;
use crate::tunnel::common::bind;
use crate::{proto::common::TunnelInfo, socket::tcp::RuntimeTcpSocket};
use anyhow::Context as _;
use bilin_core::{
    packet::{ZCPacket, ZCPacketType},
    socket::tcp::VirtualTcpSocket,
    tunnel::{IpVersion, Tunnel, TunnelError, wrapper::TunnelWrapper},
};
use bytes::BytesMut;
use cidr::IpCidr;
use forwarded_header_value::ForwardedHeaderValue;
use futures::{Sink, StreamExt};
use std::{
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, LazyLock},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{net::TcpListener, time::timeout};
use tokio_rustls::TlsAcceptor;
use tokio_util::either::Either;
use tokio_websockets::{ClientBuilder, Connector, Limits, MaybeTlsStream, Message, ServerBuilder};
use zerocopy::AsBytes as _;

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const SERVER_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
pub const WEBSOCKET_RELAY_PATH: &str = "/_bilin/relay";
const RELAY_PAYLOAD_LIMIT: usize = bilin_core::host::tunnel::MAX_HOST_TUNNEL_PAYLOAD_LEN;

static VERIFIED_RELAY_CONNECTOR: LazyLock<Result<Connector, String>> = LazyLock::new(|| {
    crate::utils::init_crypto_provider();
    match Connector::new().map_err(|error| error.to_string())? {
        connector @ Connector::Rustls(_) => Ok(connector),
        _ => Err("verified WebSocket relay TLS is unavailable".into()),
    }
});

static TRUSTED_PROXIES: LazyLock<Vec<IpCidr>> = LazyLock::new(|| {
    [
        "127.0.0.0/8",
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "::1/128",
        "fc00::/7",
    ]
    .into_iter()
    .map(|cidr| cidr.parse().unwrap())
    .collect()
});

fn trusted_proxy_contains(ip: IpAddr) -> bool {
    TRUSTED_PROXIES.iter().any(|cidr| match (cidr, ip) {
        (IpCidr::V4(cidr), IpAddr::V4(ip)) => cidr.contains(&ip),
        (IpCidr::V6(cidr), IpAddr::V6(ip)) => cidr.contains(&ip),
        _ => false,
    })
}

fn websocket_error(error: impl std::fmt::Display) -> TunnelError {
    TunnelError::ProtocolError(format!("websocket error: {error}"))
}

fn is_wss(url: &url::Url) -> Result<bool, TunnelError> {
    match url.scheme() {
        "ws" => Ok(false),
        "wss" => Ok(true),
        scheme => Err(TunnelError::InvalidProtocol(scheme.to_owned())),
    }
}

pub fn validate_websocket_relay_url(url: &url::Url) -> Result<(), TunnelError> {
    if url.scheme() != "wss"
        || url.host().is_none()
        || url.path() != WEBSOCKET_RELAY_PATH
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(TunnelError::InvalidProtocol(
            "invalid WSS relay endpoint".into(),
        ));
    }
    Ok(())
}

fn verified_relay_server_name(url: &url::Url) -> Result<String, TunnelError> {
    validate_websocket_relay_url(url)?;
    match url.host() {
        Some(url::Host::Domain(host)) => Ok(host.to_owned()),
        Some(url::Host::Ipv4(host)) => Ok(host.to_string()),
        Some(url::Host::Ipv6(host)) => Ok(host.to_string()),
        None => Err(TunnelError::InvalidProtocol("WSS relay has no host".into())),
    }
}

struct WebSocketPacketSink<S> {
    inner: S,
}

impl<S> WebSocketPacketSink<S> {
    fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S, E> Sink<ZCPacket> for WebSocketPacketSink<S>
where
    S: Sink<Message, Error = E> + Unpin,
    E: std::fmt::Display,
{
    type Error = TunnelError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner)
            .poll_ready(cx)
            .map(|result| result.map_err(websocket_error))
    }

    fn start_send(mut self: Pin<&mut Self>, packet: ZCPacket) -> Result<(), Self::Error> {
        Pin::new(&mut self.inner)
            .start_send(Message::binary(packet.tunnel_payload_bytes().freeze()))
            .map_err(websocket_error)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner)
            .poll_flush(cx)
            .map(|result| result.map_err(websocket_error))
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner)
            .poll_close(cx)
            .map(|result| result.map_err(websocket_error))
    }
}

async fn map_from_ws_message(
    message: Result<Message, tokio_websockets::Error>,
) -> Option<Result<ZCPacket, TunnelError>> {
    let message = match message {
        Ok(message) => message,
        Err(error) => {
            tracing::error!(?error, "recv from websocket error");
            return Some(Err(websocket_error(error)));
        }
    };
    if message.is_close() {
        tracing::warn!("recv close message from websocket");
        return None;
    }
    if !message.is_binary() {
        let message = format!("{message:?}");
        tracing::error!(?message, "Invalid packet");
        return Some(Err(TunnelError::InvalidPacket(message)));
    }
    Some(Ok(ZCPacket::new_from_buf(
        BytesMut::from(message.into_payload().as_bytes()),
        ZCPacketType::DummyTunnel,
    )))
}

#[derive(Debug)]
struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl SkipServerVerification {
    fn new(provider: Arc<rustls::crypto::CryptoProvider>) -> Arc<Self> {
        Arc::new(Self(provider))
    }
}

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn get_insecure_tls_client_config() -> rustls::ClientConfig {
    // Library callers may not have initialized a process-wide provider.
    // Use the same explicit backend for TLS and signature verification.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(SkipServerVerification::new(provider))
        .with_no_client_auth();
    config.enable_sni = true;
    config.enable_early_data = false;
    config
}

fn get_insecure_tls_cert<'a>() -> (
    Vec<rustls::pki_types::CertificateDer<'a>>,
    rustls::pki_types::PrivateKeyDer<'a>,
) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_der = cert.cert.der().clone();
    let private_key = cert.signing_key.serialize_der();
    let private_key = rustls::pki_types::PrivatePkcs8KeyDer::from(private_key);
    (vec![cert_der], private_key.into())
}

pub(crate) async fn upgrade_accepted<S>(
    stream: S,
    local_url: url::Url,
) -> Result<Box<dyn Tunnel>, TunnelError>
where
    S: VirtualTcpSocket,
{
    upgrade_accepted_with_mode(stream, local_url, false).await
}

async fn upgrade_accepted_with_mode<S>(
    stream: S,
    mut local_url: url::Url,
    edge_relay: bool,
) -> Result<Box<dyn Tunnel>, TunnelError>
where
    S: VirtualTcpSocket,
{
    let peer_addr = stream.peer_addr()?;
    let stream = if !edge_relay && is_wss(&local_url)? {
        let (certificates, private_key) = get_insecure_tls_cert();
        // Do not rely on another tunnel initializing the global provider.
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .with_context(|| "Failed to select TLS protocol versions")?
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)
        .with_context(|| "Failed to create server config")?;
        Either::Left(TlsAcceptor::from(Arc::new(config)).accept(stream).await?)
    } else {
        Either::Right(stream)
    };

    let limits = if edge_relay {
        Limits::unlimited().max_payload_len(Some(RELAY_PAYLOAD_LIMIT))
    } else {
        Limits::unlimited()
    };
    let (request, stream) = ServerBuilder::new()
        .limits(limits)
        .max_headers(128)
        .accept(stream)
        .await
        .map_err(websocket_error)?;

    if edge_relay {
        if !peer_addr.ip().is_loopback() || request.uri().path() != WEBSOCKET_RELAY_PATH {
            return Err(TunnelError::InvalidProtocol(
                "invalid edge relay upgrade".into(),
            ));
        }
        local_url.set_scheme("wss").map_err(|_| {
            TunnelError::InvalidProtocol("invalid edge relay transport scheme".into())
        })?;
    }
    let mut remote_url = socket_url(local_url.scheme(), peer_addr);
    if trusted_proxy_contains(peer_addr.ip())
        && let Some(forwarded) = request
            .headers()
            .get("Forwarded")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| ForwardedHeaderValue::from_forwarded(value).ok())
            .or_else(|| {
                request
                    .headers()
                    .get("X-Forwarded-For")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| ForwardedHeaderValue::from_x_forwarded_for(value).ok())
            })
        && let Some(ip) = forwarded.remotest_forwarded_for_ip()
    {
        remote_url
            .set_ip_host(ip)
            .map_err(|_| TunnelError::InvalidAddr(format!("invalid forwarded ip {ip}")))?;
        remote_url
            .query_pairs_mut()
            .append_pair("proxy", &peer_addr.to_string());
    }

    let (write, read) = stream.split();
    let remote_url: crate::proto::common::Url = remote_url.into();
    let info = TunnelInfo {
        tunnel_type: local_url.scheme().to_owned(),
        local_addr: Some(local_url.into()),
        remote_addr: Some(remote_url.clone()),
        resolved_remote_addr: Some(remote_url),
    };
    Ok(Box::new(TunnelWrapper::new(
        read.filter_map(map_from_ws_message),
        WebSocketPacketSink::new(write),
        Some(info),
    )))
}

fn socket_url(scheme: &str, addr: SocketAddr) -> url::Url {
    let mut url = url::Url::parse(&format!("{scheme}://0.0.0.0"))
        .expect("WebSocket transport scheme should be a valid URL scheme");
    url.set_ip_host(addr.ip()).unwrap();
    url.set_port(Some(addr.port())).unwrap();
    url
}

#[derive(Debug)]
pub struct WsTunnelListener {
    addr: url::Url,
    listener: Option<TcpListener>,
    socket_mark: Option<u32>,
    edge_relay: bool,
}

impl WsTunnelListener {
    pub fn new(addr: url::Url) -> Self {
        WsTunnelListener {
            addr,
            listener: None,
            socket_mark: None,
            edge_relay: false,
        }
    }

    pub fn set_socket_mark(&mut self, socket_mark: Option<u32>) {
        self.socket_mark = socket_mark;
    }

    async fn listen_tunnel(&mut self) -> Result<(), TunnelError> {
        self.listener = None;

        let addr = SocketAddr::from_url(self.addr.clone(), IpVersion::Both).await?;
        let listener = bind::<TcpListener>()
            .addr(addr)
            .only_v6(true)
            .maybe_socket_mark(self.socket_mark)
            .call()
            .await?;

        self.addr
            .set_port(Some(listener.local_addr()?.port()))
            .unwrap();
        self.listener = Some(listener);

        Ok(())
    }

    async fn accept_tunnel(&mut self) -> Result<Box<dyn Tunnel>, TunnelError> {
        loop {
            let listener = self.listener.as_ref().unwrap();
            // only fail on tcp accept error
            let (stream, _) = listener.accept().await?;
            stream.set_nodelay(true).unwrap();
            match timeout(
                SERVER_HANDSHAKE_TIMEOUT,
                upgrade_accepted_with_mode(
                    RuntimeTcpSocket::new(stream),
                    self.addr.clone(),
                    self.edge_relay,
                ),
            )
            .await
            {
                Ok(Ok(tunnel)) => return Ok(tunnel),
                e => {
                    tracing::error!(?e, ?self, "Failed to accept ws/wss tunnel");
                    continue;
                }
            }
        }
    }
}

/// The public edge terminates verified TLS before forwarding this upgrade.
/// The plain upstream must remain private to the local TLS edge.
pub fn runtime_websocket_relay_listener(bind: SocketAddr) -> anyhow::Result<WsTunnelListener> {
    anyhow::ensure!(
        bind.ip().is_loopback(),
        "WebSocket relay must bind to loopback"
    );
    let mut addr = socket_url("ws", bind);
    addr.set_path(WEBSOCKET_RELAY_PATH);
    let mut listener = WsTunnelListener::new(addr);
    listener.edge_relay = true;
    Ok(listener)
}

#[async_trait::async_trait]
impl bilin_core::socket::SocketListener for WsTunnelListener {
    type Accepted = Box<dyn Tunnel>;

    async fn listen(&mut self) -> anyhow::Result<()> {
        Ok(self.listen_tunnel().await?)
    }

    async fn accept(&mut self) -> anyhow::Result<Self::Accepted> {
        Ok(self.accept_tunnel().await?)
    }

    fn local_url(&self) -> url::Url {
        self.addr.clone()
    }
}

pub(crate) async fn upgrade_connected<S>(
    stream: S,
    remote_url: url::Url,
) -> Result<Box<dyn Tunnel>, TunnelError>
where
    S: VirtualTcpSocket,
{
    let is_wss = is_wss(&remote_url)?;
    let relay_host = if remote_url.path() == WEBSOCKET_RELAY_PATH {
        Some(verified_relay_server_name(&remote_url)?)
    } else {
        None
    };
    let local_addr = stream.local_addr()?;
    let resolved_remote_addr = stream.peer_addr()?;
    let info = TunnelInfo {
        tunnel_type: remote_url.scheme().to_owned(),
        local_addr: Some(
            super::build_url_from_socket_addr(&local_addr.to_string(), remote_url.scheme()).into(),
        ),
        remote_addr: Some(remote_url.clone().into()),
        resolved_remote_addr: Some(
            super::build_url_from_socket_addr(
                &resolved_remote_addr.to_string(),
                remote_url.scheme(),
            )
            .into(),
        ),
    };

    let mut client = ClientBuilder::from_uri(
        http::Uri::try_from(remote_url.to_string()).map_err(websocket_error)?,
    )
    .max_headers(128);
    let stream: MaybeTlsStream<S> = if let Some(host) = relay_host {
        client = client.limits(Limits::unlimited().max_payload_len(Some(RELAY_PAYLOAD_LIMIT)));
        VERIFIED_RELAY_CONNECTOR
            .as_ref()
            .map_err(websocket_error)?
            .wrap(&host, stream)
            .await
            .map_err(websocket_error)?
    } else if is_wss {
        let tls = tokio_rustls::TlsConnector::from(Arc::new(get_insecure_tls_client_config()));
        let sni = remote_url.domain().unwrap_or("localhost").to_owned();
        let server_name = rustls::pki_types::ServerName::try_from(sni)
            .map_err(|_| TunnelError::InvalidProtocol("Invalid SNI".to_owned()))?;
        MaybeTlsStream::Rustls(tls.connect(server_name, stream).await?)
    } else {
        MaybeTlsStream::Plain(stream)
    };

    let (client, _) = client.connect_on(stream).await.map_err(websocket_error)?;
    let (write, read) = client.split();
    Ok(Box::new(TunnelWrapper::new(
        read.filter_map(map_from_ws_message),
        WebSocketPacketSink::new(write),
        Some(info),
    )))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use bilin_core::socket::SocketListener;
    use futures::SinkExt;
    use std::io;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpSocket,
    };

    #[test]
    fn wss_relay_requires_tls_and_preserves_actual_ipv4_and_ipv6_identity() {
        let ipv4 = format!("wss://192.0.2.10{WEBSOCKET_RELAY_PATH}")
            .parse()
            .unwrap();
        assert_eq!(verified_relay_server_name(&ipv4).unwrap(), "192.0.2.10");
        let ipv6 = format!("wss://[2001:db8::10]{WEBSOCKET_RELAY_PATH}")
            .parse()
            .unwrap();
        assert_eq!(verified_relay_server_name(&ipv6).unwrap(), "2001:db8::10");
        for invalid in [
            format!("ws://example.com{WEBSOCKET_RELAY_PATH}"),
            format!("wss://account@example.com{WEBSOCKET_RELAY_PATH}"),
            format!("wss://example.com{WEBSOCKET_RELAY_PATH}?unexpected=1"),
            format!("wss://example.com{WEBSOCKET_RELAY_PATH}#fragment"),
            "wss://example.com/other-path".into(),
        ] {
            assert!(validate_websocket_relay_url(&invalid.parse().unwrap()).is_err());
        }
        assert!(runtime_websocket_relay_listener("0.0.0.0:0".parse().unwrap()).is_err());
    }

    #[tokio::test]
    async fn managed_wss_rejects_an_untrusted_certificate() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            assert!(
                upgrade_accepted(RuntimeTcpSocket::new(stream), socket_url("wss", address))
                    .await
                    .is_err()
            );
        });
        let local = RuntimeTcpSocket::new(tokio::net::TcpStream::connect(address).await.unwrap());
        let url = format!("wss://{address}{WEBSOCKET_RELAY_PATH}")
            .parse()
            .unwrap();
        let Err(error) = upgrade_connected(local, url).await else {
            panic!("WSS relay accepted a self-signed server certificate");
        };
        assert!(error.to_string().contains("UnknownIssuer"), "{error}");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn private_edge_relay_upstream_carries_packets_and_reports_wss() {
        let mut listener =
            runtime_websocket_relay_listener("127.0.0.1:0".parse().unwrap()).unwrap();
        listener.listen().await.unwrap();
        let url = listener.local_url();
        let address = format!("127.0.0.1:{}", url.port().unwrap());
        let server = tokio::spawn(async move {
            let tunnel = listener.accept().await.unwrap();
            assert_eq!(tunnel.info().unwrap().tunnel_type, "wss");
            let (mut receiver, mut sender) = tunnel.split();
            let packet = receiver.next().await.unwrap().unwrap();
            assert_eq!(packet.payload(), b"relay request");
            sender
                .send(ZCPacket::new_with_payload(b"relay response"))
                .await
                .unwrap();
        });

        timeout(Duration::from_secs(3), async {
            // The reverse proxy already terminated TLS on the edge.
            let stream = tokio::net::TcpStream::connect(address).await.unwrap();
            let (mut proxy_side, _) = ClientBuilder::from_uri(url.as_str().parse().unwrap())
                .connect_on(stream)
                .await
                .unwrap();
            let request = ZCPacket::new_with_payload(b"relay request");
            proxy_side
                .send(Message::binary(request.tunnel_payload_bytes().freeze()))
                .await
                .unwrap();
            let response = proxy_side.next().await.unwrap().unwrap().into_payload();
            let response = ZCPacket::new_from_buf(
                BytesMut::from(response.as_bytes()),
                ZCPacketType::DummyTunnel,
            );
            assert_eq!(response.payload(), b"relay response");
            server.await.unwrap();
        })
        .await
        .unwrap();
    }

    struct FailingWebSocketSink {
        close_called: bool,
    }

    impl Sink<Message> for FailingWebSocketSink {
        type Error = io::Error;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(self: Pin<&mut Self>, _item: Message) -> Result<(), Self::Error> {
            Ok(())
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "send failed",
            )))
        }

        fn poll_close(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            self.close_called = true;
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "close failed",
            )))
        }
    }

    #[tokio::test]
    async fn packet_sink_maps_send_and_close_errors_independently() {
        let mut sink = WebSocketPacketSink::new(FailingWebSocketSink {
            close_called: false,
        });

        sink.send(ZCPacket::new_with_payload(b"packet"))
            .await
            .expect_err("send should fail");
        sink.close().await.expect_err("close should fail");
        assert!(sink.inner.close_called);
    }

    #[rstest::rstest]
    #[case::xff_ipv4("X-Forwarded-For: 203.0.113.5, 192.168.1.1", "203.0.113.5")]
    #[case::xff_ipv6("X-Forwarded-For: 2409:8a21:0:1::e87b", "2409:8a21:0:1::e87b")]
    #[case::xff_ipv6_private("X-Forwarded-For: fd00::1", "fd00::1")]
    #[case::xff_ipv6_loopback("X-Forwarded-For: ::1", "::1")]
    #[case::forwarded_ipv4("Forwarded: for=203.0.113.5", "203.0.113.5")]
    #[case::forwarded_ipv6(
        "Forwarded: for=\"[2409:8a21:0:1::e87b]\";proto=https",
        "2409:8a21:0:1::e87b"
    )]
    #[case::forwarded_ipv6_loopback("Forwarded: for=\"[::1]\"", "::1")]
    #[case::forwarded_preferred(
        "Forwarded: for=\"[2409:8a21:0:1::e87b]\"\r\nX-Forwarded-For: 203.0.113.5",
        "2409:8a21:0:1::e87b"
    )]
    #[case::invalid_forwarded_falls_back(
        "Forwarded: for=invalid\r\nX-Forwarded-For: 2409:8a21:0:1::e87b",
        "2409:8a21:0:1::e87b"
    )]
    #[tokio::test]
    async fn ws_forwarded(#[case] forwarded_headers: &str, #[case] expected_ip: &str) {
        let mut listener = WsTunnelListener::new("ws://127.0.0.1:0".parse().unwrap());
        listener.listen().await.unwrap();
        let listener_addr =
            SocketAddr::from(([127, 0, 0, 1], listener.local_url().port().unwrap()));
        let server_task = tokio::spawn(async move { listener.accept().await.unwrap() });

        let socket = TcpSocket::new_v4().unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut stream = socket.connect(listener_addr).await.unwrap();
        let peer_addr = stream.local_addr().unwrap();

        let handshake = format!(
            "GET / HTTP/1.1\r\n\
                         Host: {listener_addr}\r\n\
                         Upgrade: websocket\r\n\
                         Connection: Upgrade\r\n\
                         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                         Sec-WebSocket-Version: 13\r\n\
                         {forwarded_headers}\r\n\
                         \r\n"
        );

        stream.write_all(handshake.as_bytes()).await.unwrap();

        let mut buf = [0u8; 1024];
        let bytes_read = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..bytes_read]);

        assert!(response.contains("101 Switching Protocols"));

        let tunnel = timeout(Duration::from_secs(3), server_task)
            .await
            .expect("forwarded WebSocket connection should be accepted")
            .unwrap();
        let info = tunnel.info().unwrap();
        assert_eq!(info.remote_addr, info.resolved_remote_addr);
        let remote_addr: url::Url = info.remote_addr.unwrap().into();
        let expected_host = match expected_ip.parse::<IpAddr>().unwrap() {
            IpAddr::V4(ip) => url::Host::Ipv4(ip),
            IpAddr::V6(ip) => url::Host::Ipv6(ip),
        };
        assert_eq!(remote_addr.host(), Some(expected_host));
        assert_eq!(remote_addr.port(), Some(peer_addr.port()));
        let proxy_addr = remote_addr
            .query_pairs()
            .find(|(k, _)| k == "proxy")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        assert_eq!(proxy_addr, peer_addr.to_string());
    }
}

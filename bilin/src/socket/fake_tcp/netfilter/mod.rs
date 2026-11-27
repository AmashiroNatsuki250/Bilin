pub mod pnet;

use std::{io, net::SocketAddr, sync::Arc};

cfg_select! {
    target_os = "linux" => {
        pub mod linux_bpf;

        pub fn create_tun(
            interface_name: &str,
            src_addr: Option<SocketAddr>,
            dst_addr: SocketAddr,
        ) -> io::Result<Arc<dyn super::stack::Tun>> {
            match linux_bpf::LinuxBpfTun::new(interface_name, src_addr, dst_addr) {
                Ok(tun) => Ok(Arc::new(tun)),
                Err(e) => {
                    tracing::warn!(
                        ?e,
                        interface_name,
                        "LinuxBpfTun init failed, falling back to PnetTun"
                    );
                    Ok(Arc::new(pnet::PnetTun::new(
                        interface_name,
                        pnet::create_packet_filter(src_addr, dst_addr),
                    )?))
                }
            }
        }
    }

    all(target_os = "macos", not(feature = "macos-ne")) => {
        pub mod macos_bpf;

        pub fn create_tun(
            interface_name: &str,
            src_addr: Option<SocketAddr>,
            dst_addr: SocketAddr,
        ) -> io::Result<Arc<dyn super::stack::Tun>> {
            match macos_bpf::MacosBpfTun::new(interface_name, src_addr, dst_addr) {
                Ok(tun) => Ok(Arc::new(tun)),
                Err(e) => {
                    tracing::warn!(
                        ?e,
                        interface_name,
                        "MacosBpfTun init failed, falling back to PnetTun"
                    );
                    Ok(Arc::new(pnet::PnetTun::new(
                        interface_name,
                        pnet::create_packet_filter(src_addr, dst_addr),
                    )?))
                }
            }
        }
    }

    all(windows, any(target_arch = "x86_64", target_arch = "x86")) => {
        pub mod windivert;

        pub fn create_tun(
            _interface_name: &str,
            src_addr: Option<SocketAddr>,
            dst_addr: SocketAddr,
        ) -> io::Result<Arc<dyn super::stack::Tun>> {
            match windivert::WinDivertTun::new(src_addr, dst_addr) {
                Ok(tun) => Ok(Arc::new(tun)),
                Err(win_error) => create_windows_pnet_tun(src_addr, dst_addr, win_error),
            }
        }

        fn create_windows_pnet_tun(
            src_addr: Option<SocketAddr>,
            dst_addr: SocketAddr,
            win_error: io::Error,
        ) -> io::Result<Arc<dyn super::stack::Tun>> {
            tracing::warn!(error = ?win_error, %dst_addr, "WinDivert unavailable, trying Pnet by local IP");
            pnet::PnetTun::new_for_local_ip(dst_addr.ip(), pnet::create_packet_filter(src_addr, dst_addr))
                .map(|tun| Arc::new(tun) as Arc<dyn super::stack::Tun>)
                .map_err(|pnet_error| io::Error::other(format!(
                    "FakeTCP capture initialization failed: WinDivert {win_error:?}; Pnet {pnet_error}"
                )))
        }
    }

    _ => {
        pub fn create_tun(
            interface_name: &str,
            src_addr: Option<SocketAddr>,
            dst_addr: SocketAddr,
        ) -> io::Result<Arc<dyn super::stack::Tun>> {
            Ok(Arc::new(pnet::PnetTun::new(
                interface_name,
                pnet::create_packet_filter(src_addr, dst_addr),
            )?))
        }
    }
}

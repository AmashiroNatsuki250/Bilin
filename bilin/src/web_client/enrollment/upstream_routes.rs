use std::net::Ipv4Addr;

use cidr::Ipv4Cidr;

#[cfg(any(
    target_os = "windows",
    all(target_os = "linux", feature = "linux-netlink"),
    test
))]
const EGRESS_DESTINATION: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);

#[cfg(any(
    target_os = "windows",
    all(target_os = "linux", feature = "linux-netlink"),
    test
))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GatewayPath {
    interface_index: u32,
    gateway: Ipv4Addr,
}

#[cfg(any(
    target_os = "windows",
    all(target_os = "linux", feature = "linux-netlink"),
    test
))]
struct Route {
    prefix: Ipv4Cidr,
    path: Option<GatewayPath>,
    explicit: bool,
}

pub(super) async fn discover(
    interface_index: u32,
    source: Ipv4Addr,
    lan: Ipv4Cidr,
    fallback: Option<Ipv4Cidr>,
) -> (Option<Ipv4Cidr>, Option<Ipv4Cidr>) {
    #[cfg(any(
        target_os = "windows",
        all(target_os = "linux", feature = "linux-netlink")
    ))]
    {
        tokio::task::spawn_blocking(move || {
            select_upstreams(
                interface_index,
                lan,
                fallback,
                &platform::routes(),
                |destination| platform::best_path(destination, source),
            )
        })
        .await
        .unwrap_or_default()
    }
    #[cfg(not(any(
        target_os = "windows",
        all(target_os = "linux", feature = "linux-netlink")
    )))]
    {
        let _ = (interface_index, source, lan, fallback);
        (None, None)
    }
}

#[cfg(any(
    target_os = "windows",
    all(target_os = "linux", feature = "linux-netlink"),
    test
))]
fn select_upstreams(
    interface_index: u32,
    lan: Ipv4Cidr,
    fallback: Option<Ipv4Cidr>,
    routes: &[Route],
    best_path: impl Fn(Ipv4Addr) -> Option<GatewayPath>,
) -> (Option<Ipv4Cidr>, Option<Ipv4Cidr>) {
    // The same local routing lookup is used when selecting the primary IPv4
    // interface. Looking up an address sends no traffic to it.
    let Some(default) = best_path(EGRESS_DESTINATION).filter(|path| {
        path.interface_index == interface_index
            && routes
                .iter()
                .any(|route| route.prefix.network_length() == 0 && route.path == Some(*path))
    }) else {
        return (None, None);
    };

    let usable = |prefix: &Ipv4Cidr| {
        if !(16..=30).contains(&prefix.network_length())
            || !prefix.first_address().is_private()
            || !prefix.last_address().is_private()
            || overlaps(*prefix, lan)
            || prefix.contains(&default.gateway)
        {
            return false;
        }
        // Reject ranges a more-specific route sends through another network.
        // The kernel lookup also resolves route metrics and policy tables.
        !routes.iter().any(|route| {
            route.prefix.network_length() > prefix.network_length()
                && route.path != Some(default)
                && overlaps(*prefix, route.prefix)
        }) && [
            Ipv4Addr::from_bits(prefix.first_address().to_bits() + 1),
            Ipv4Addr::from_bits(prefix.last_address().to_bits() - 1),
        ]
        .into_iter()
        .all(|address| best_path(address) == Some(default))
    };

    let mut candidates = routes
        .iter()
        .filter(|route| route.explicit && route.path == Some(default))
        .map(|route| route.prefix)
        .filter(usable);
    let automatic = candidates.next();
    let automatic = automatic.filter(|first| candidates.all(|other| other == *first));
    (automatic, fallback.filter(usable))
}

#[cfg(any(
    target_os = "windows",
    all(target_os = "linux", feature = "linux-netlink"),
    test
))]
fn overlaps(first: Ipv4Cidr, second: Ipv4Cidr) -> bool {
    first.contains(&second.first_address()) || second.contains(&first.first_address())
}

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
mod platform {
    use super::*;
    use crate::common::ifcfg::{IfConfiger, RouteMessage, RouteType};
    use nix::libc::{RTPROT_BOOT, RTPROT_STATIC};
    use std::net::IpAddr;

    const RTPROT_DHCP: u8 = 16; // linux/rtnetlink.h

    pub(super) fn routes() -> Vec<Route> {
        IfConfiger::list_routes()
            .unwrap_or_default()
            .iter()
            .filter_map(|route| {
                let destination = match route.destination() {
                    Some(IpAddr::V4(ip)) => *ip,
                    None if route.dst_len() == 0 => Ipv4Addr::UNSPECIFIED,
                    _ => return None,
                };
                Some(Route {
                    prefix: Ipv4Cidr::new(destination, route.dst_len()).ok()?,
                    path: gateway_path(route),
                    explicit: matches!(route.protocol(), RTPROT_BOOT | RTPROT_DHCP | RTPROT_STATIC),
                })
            })
            .collect()
    }

    pub(super) fn best_path(destination: Ipv4Addr, _source: Ipv4Addr) -> Option<GatewayPath> {
        gateway_path(&IfConfiger::lookup_ipv4_route(destination).ok()??)
    }

    fn gateway_path(route: &RouteMessage) -> Option<GatewayPath> {
        if route.route_type() != RouteType::Unicast {
            return None;
        }
        let IpAddr::V4(gateway) = route.gateway()? else {
            return None;
        };
        Some(GatewayPath {
            interface_index: route.oif()?,
            gateway,
        })
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use std::{ptr, slice};
    use windows::Win32::{
        Foundation::NO_ERROR,
        NetworkManagement::IpHelper::{
            FreeMibTable, GetBestRoute, GetIpForwardTable2, MIB_IPFORWARD_TABLE2, MIB_IPFORWARDROW,
        },
        Networking::WinSock::{
            AF_INET, MIB_IPPROTO_DHCP, MIB_IPPROTO_NETMGMT, MIB_IPPROTO_NT_STATIC,
            MIB_IPPROTO_NT_STATIC_NON_DOD, NlroDHCP, SOCKADDR_INET,
        },
    };

    pub(super) fn routes() -> Vec<Route> {
        let mut table = ptr::null_mut::<MIB_IPFORWARD_TABLE2>();
        // The API allocates a table with NumEntries rows starting at Table.
        if unsafe { GetIpForwardTable2(AF_INET, &mut table) } != NO_ERROR || table.is_null() {
            return Vec::new();
        }
        let entries = unsafe {
            slice::from_raw_parts(
                ptr::addr_of!((*table).Table).cast(),
                (*table).NumEntries as usize,
            )
        };
        let routes = entries
            .iter()
            .filter_map(
                |route: &windows::Win32::NetworkManagement::IpHelper::MIB_IPFORWARD_ROW2| {
                    let prefix = Ipv4Cidr::new(
                        ipv4_addr(&route.DestinationPrefix.Prefix)?,
                        route.DestinationPrefix.PrefixLength,
                    )
                    .ok()?;
                    let path = ipv4_addr(&route.NextHop)
                        .filter(|gateway| !gateway.is_unspecified())
                        .map(|gateway| GatewayPath {
                            interface_index: route.InterfaceIndex,
                            gateway,
                        });
                    Some(Route {
                        prefix,
                        path,
                        explicit: route.Origin == NlroDHCP
                            || matches!(
                                route.Protocol,
                                MIB_IPPROTO_DHCP
                                    | MIB_IPPROTO_NETMGMT
                                    | MIB_IPPROTO_NT_STATIC
                                    | MIB_IPPROTO_NT_STATIC_NON_DOD
                            ),
                    })
                },
            )
            .collect();
        unsafe { FreeMibTable(table.cast()) };
        routes
    }

    pub(super) fn best_path(destination: Ipv4Addr, source: Ipv4Addr) -> Option<GatewayPath> {
        let mut route = MIB_IPFORWARDROW::default();
        // Unlike GetBestRoute2, this API can resolve a route without constraining
        // the lookup to the physical interface. A source address only breaks ties.
        let status = unsafe {
            GetBestRoute(
                u32::from_ne_bytes(destination.octets()),
                Some(u32::from_ne_bytes(source.octets())),
                &mut route,
            )
        };
        let gateway = Ipv4Addr::from(route.dwForwardNextHop.to_ne_bytes());
        (status == NO_ERROR.0 && !gateway.is_unspecified()).then_some(GatewayPath {
            interface_index: route.dwForwardIfIndex,
            gateway,
        })
    }

    fn ipv4_addr(address: &SOCKADDR_INET) -> Option<Ipv4Addr> {
        // SOCKADDR_INET is a tagged address union. Read IPv4 only for AF_INET.
        unsafe {
            (address.si_family == AF_INET)
                .then(|| Ipv4Addr::from(address.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: GatewayPath = GatewayPath {
        interface_index: 7,
        gateway: Ipv4Addr::new(192, 168, 100, 1),
    };
    const VPN: GatewayPath = GatewayPath {
        interface_index: 10,
        gateway: Ipv4Addr::new(10, 0, 0, 1),
    };

    fn route(prefix: &str, path: GatewayPath) -> Route {
        Route {
            prefix: prefix.parse().unwrap(),
            path: Some(path),
            explicit: true,
        }
    }

    fn select(routes: &[Route], fallback: Option<&str>) -> (Option<Ipv4Cidr>, Option<Ipv4Cidr>) {
        select_upstreams(
            DEFAULT.interface_index,
            "192.168.100.0/24".parse().unwrap(),
            fallback.map(|prefix| prefix.parse().unwrap()),
            routes,
            |_| Some(DEFAULT),
        )
    }

    #[test]
    fn only_an_explicit_physical_route_discovers_an_upstream() {
        let mut routes = vec![route("0.0.0.0/0", DEFAULT)];
        let expected = Some("10.168.10.0/24".parse().unwrap());
        assert_eq!(select(&routes, None), (None, None));
        assert_eq!(select(&routes, Some("10.168.10.0/24")), (None, expected));
        routes.extend([
            route("10.168.10.0/24", DEFAULT),
            route("10.168.10.0/24", DEFAULT),
            route("172.16.0.0/16", VPN),
            route("10.0.0.0/8", DEFAULT),
            route("172.15.0.0/16", DEFAULT),
            route("192.168.100.0/24", DEFAULT),
        ]);
        assert_eq!(select(&routes, None), (expected, None));
        routes.push(route("172.16.1.0/24", DEFAULT));
        assert_eq!(select(&routes, Some("10.168.10.0/24")), (None, expected));
    }

    #[test]
    fn fallbacks_do_not_include_ranges_routed_through_other_interfaces() {
        let default = route("0.0.0.0/0", DEFAULT);
        let routes = [default, route("10.168.10.128/25", VPN)];
        assert_eq!(select(&routes, Some("10.168.10.0/24")), (None, None));
        assert_eq!(select(&routes[1..], Some("10.168.11.0/24")), (None, None));
        assert_eq!(
            select_upstreams(
                DEFAULT.interface_index,
                "192.168.100.0/24".parse().unwrap(),
                Some("10.168.12.0/24".parse().unwrap()),
                &routes,
                |destination| Some(if destination == EGRESS_DESTINATION {
                    DEFAULT
                } else {
                    VPN
                }),
            ),
            (None, None)
        );
    }
}

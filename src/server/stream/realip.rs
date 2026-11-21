use super::*;

pub(in crate::server) fn effective_stream_remote(
    server: &crate::config::StreamServer,
    addresses: &ProxyProtocolAddresses,
) -> ListenAddress {
    if addresses.proxy_protocol_addr.is_none() {
        return addresses.transport_remote.clone();
    }
    let trusted = match &addresses.transport_remote {
        ListenAddress::Inet(address) => {
            let peer = normalize_stream_real_ip(address.ip());
            server
                .real_ip_from
                .iter()
                .any(|source| source.contains(peer))
        }
        ListenAddress::Unix(_) => server
            .real_ip_from
            .iter()
            .any(crate::config::RealIpSource::is_unix),
    };
    if !trusted {
        return addresses.transport_remote.clone();
    }
    addresses.remote.clone()
}

fn normalize_stream_real_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(address)),
        address => address,
    }
}

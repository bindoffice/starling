use super::*;

pub(in crate::server) fn effective_real_ip(
    server: &ServerConfig,
    request: &Request,
    uri: &str,
    peer: Option<std::net::IpAddr>,
    peer_address: &str,
    proxy_protocol_addr: Option<std::net::IpAddr>,
) -> String {
    let fallback = peer
        .map(|peer| peer.to_string())
        .unwrap_or_else(|| peer_address.to_string());
    let context = resolve_location(&server.locations, uri)
        .map(|location| &location.context)
        .unwrap_or(&server.context);
    let trusted = match peer {
        Some(peer) => real_ip_trusted(context, peer),
        None => context
            .real_ip_from
            .iter()
            .any(crate::config::RealIpSource::is_unix),
    };
    if context.real_ip_from.is_empty() || !trusted {
        return fallback;
    }
    if context
        .real_ip_header
        .eq_ignore_ascii_case("proxy_protocol")
    {
        return proxy_protocol_addr
            .map(|address| address.to_string())
            .unwrap_or(fallback);
    }
    let Some(value) = request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(&context.real_ip_header))
        .map(|header| header.value.as_str())
    else {
        return fallback;
    };
    let candidates = value
        .split(',')
        .filter_map(parse_geo_address)
        .map(normalize_real_ip)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return fallback;
    }
    let selected = if context
        .real_ip_header
        .eq_ignore_ascii_case("x-forwarded-for")
    {
        if context.real_ip_recursive {
            candidates
                .iter()
                .rev()
                .find(|address| !real_ip_trusted(context, **address))
                .copied()
                .or_else(|| candidates.first().copied())
        } else {
            candidates.last().copied()
        }
    } else {
        candidates.first().copied()
    };
    selected
        .map(|address| address.to_string())
        .unwrap_or(fallback)
}

pub(in crate::server) fn real_ip_trusted(
    context: &EffectiveConfig,
    address: std::net::IpAddr,
) -> bool {
    let address = normalize_real_ip(address);
    context
        .real_ip_from
        .iter()
        .any(|source| source.contains(address))
}

pub(in crate::server) fn normalize_real_ip(address: std::net::IpAddr) -> std::net::IpAddr {
    match address {
        std::net::IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(std::net::IpAddr::V4)
            .unwrap_or(std::net::IpAddr::V6(address)),
        address => address,
    }
}

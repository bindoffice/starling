use super::*;
use crate::config::{ListenConfig, StreamConfig, StreamServer};

pub(in crate::server) fn stream_server_for_route<'a>(
    stream: &'a StreamConfig,
    route: &StreamRoute,
) -> Option<&'a StreamServer> {
    let mut first = None;
    for server in stream_servers_for_address(stream, &route.address) {
        for listen in server
            .listens
            .iter()
            .filter(|listen| listen_matches(listen, route))
        {
            first.get_or_insert(server);
            if listen.default_server {
                return Some(server);
            }
        }
    }
    first
}

pub(in crate::server) fn stream_servers_for_address<'a>(
    stream: &'a StreamConfig,
    address: &ListenAddress,
) -> impl Iterator<Item = &'a StreamServer> + 'a {
    let address = address.clone();
    stream.servers.iter().filter(move |server| {
        server
            .listens
            .iter()
            .any(|listen| listen.address == address)
    })
}

pub(in crate::server) fn select_stream_server<'a>(
    stream: &'a StreamConfig,
    route: &StreamRoute,
    host: Option<&str>,
) -> Option<&'a StreamServer> {
    let default = stream_server_for_route(stream, route)?;
    let Some(host) = host.filter(|host| !host.is_empty()) else {
        return Some(default);
    };
    let host = host.trim_end_matches('.');

    for server in stream_servers_for_address(stream, &route.address)
        .filter(|server| server_listens_route(server, route))
    {
        if server.names.iter().any(
            |name| matches!(name, ServerName::Exact(value) if value.eq_ignore_ascii_case(host)),
        ) {
            return Some(server);
        }
    }

    let mut wildcard_match: Option<(&StreamServer, usize)> = None;
    for server in stream_servers_for_address(stream, &route.address)
        .filter(|server| server_listens_route(server, route))
    {
        for name in &server.names {
            let specificity = match name {
                ServerName::LeadingWildcard(value) | ServerName::TrailingWildcard(value)
                    if name.matches(host) =>
                {
                    value.len()
                }
                _ => 0,
            };
            if specificity > 0 && wildcard_match.is_none_or(|(_, previous)| specificity > previous)
            {
                wildcard_match = Some((server, specificity));
            }
        }
    }
    if let Some((server, _)) = wildcard_match {
        return Some(server);
    }

    for server in stream_servers_for_address(stream, &route.address)
        .filter(|server| server_listens_route(server, route))
    {
        if server
            .names
            .iter()
            .any(|name| matches!(name, ServerName::Regex(regex) if regex.is_match(host)))
        {
            return Some(server);
        }
    }

    Some(default)
}

fn server_listens_route(server: &StreamServer, route: &StreamRoute) -> bool {
    server
        .listens
        .iter()
        .any(|listen| listen_matches(listen, route))
}

fn listen_matches(listen: &ListenConfig, route: &StreamRoute) -> bool {
    listen.address == route.address && listen.udp == route.udp
}

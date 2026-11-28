use super::*;

#[cfg(test)]
pub(in crate::server) fn listener_signature(config: &RuntimeConfig) -> ListenerSignature {
    let groups = build_listener_groups(config);
    listener_signature_for_groups(config, &groups)
}

fn listener_signature_for_groups(
    config: &RuntimeConfig,
    groups: &[ListenerGroup],
) -> ListenerSignature {
    let http = groups
        .iter()
        .map(|group| {
            (
                group.address.clone(),
                group.ssl,
                group.http2,
                group.http3,
                group.tcp,
                group.quic,
                group.proxy_protocol,
                group.socket_options.clone(),
            )
        })
        .collect();
    let stream = build_stream_routes(config)
        .into_iter()
        .map(|route| {
            (
                route.address,
                route.ssl,
                route.udp,
                route.proxy_protocol,
                route.socket_options,
            )
        })
        .collect();
    let mail = build_mail_routes(config)
        .into_iter()
        .map(|route| {
            (
                route.address,
                route.ssl,
                route.proxy_protocol,
                route.socket_options,
            )
        })
        .collect();
    (http, stream, mail)
}

pub(in crate::server) fn listeners_reload_compatible(
    old_config: &RuntimeConfig,
    new_config: &RuntimeConfig,
    old_groups: &[ListenerGroup],
    new_groups: &[ListenerGroup],
) -> bool {
    if !http_listeners_reload_compatible(old_groups, new_groups) {
        return false;
    }
    let (_, old_stream, old_mail) = listener_signature_for_groups(old_config, old_groups);
    let (_, new_stream, new_mail) = listener_signature_for_groups(new_config, new_groups);
    old_stream == new_stream && old_mail == new_mail
}

fn http_listeners_reload_compatible(
    old_groups: &[ListenerGroup],
    new_groups: &[ListenerGroup],
) -> bool {
    for old in old_groups {
        let Some(new) = new_groups
            .iter()
            .find(|candidate| candidate.address == old.address)
        else {
            if old.tcp || !old.quic {
                return false;
            }
            continue;
        };
        if old.tcp != new.tcp || (!old.quic && new.quic) {
            return false;
        }
        if old.tcp
            && (old.ssl != new.ssl
                || old.http2 != new.http2
                || old.proxy_protocol != new.proxy_protocol
                || old.socket_options != new.socket_options)
        {
            return false;
        }
    }
    new_groups
        .iter()
        .all(|new| old_groups.iter().any(|old| old.address == new.address))
}

pub(in crate::server) fn build_listener_groups(config: &RuntimeConfig) -> Vec<ListenerGroup> {
    let Some(http) = &config.http else {
        return Vec::new();
    };
    let mut groups: HashMap<ListenAddress, ListenerAccumulator> = HashMap::new();
    for server in &http.servers {
        for listen in &server.listens {
            let entry = groups.entry(listen.address.clone()).or_default();
            let http3_enabled = http3_enabled_for_listener(server, listen);
            if !entry.server_ids.contains(&server.id) {
                entry.server_ids.push(server.id);
            }
            if listen.default_server || entry.default_server_id.is_none() {
                entry.default_server_id = Some(server.id);
            }
            entry.ssl |= listen.ssl;
            entry.http2 |= listen.http2 || server.http2;
            entry.http3 |= server.http3;
            entry.tcp |= !listen.quic;
            entry.quic |= listen.quic && http3_enabled;
            entry.proxy_protocol |= listen.proxy_protocol;
            if listen.socket_options_set {
                entry.socket_options = listen.socket_options.clone();
            }
        }
    }
    let mut result: Vec<_> = groups
        .into_iter()
        .map(|(address, group)| ListenerGroup {
            address,
            default_server_id: group.default_server_id.unwrap_or(group.server_ids[0]),
            server_ids: group.server_ids,
            ssl: group.ssl || group.quic,
            http2: group.http2,
            http3: group.http3,
            tcp: group.tcp,
            quic: group.quic,
            proxy_protocol: group.proxy_protocol,
            socket_options: group.socket_options,
        })
        .filter(|group| group.tcp || group.quic)
        .collect();
    result.sort_by_key(|group| group.address.clone());
    result
}

pub(in crate::server) fn http3_enabled_for_listener(
    server: &ServerConfig,
    listen: &crate::config::ListenConfig,
) -> bool {
    !listen.quic || !server.http3_explicit || server.http3
}

pub(in crate::server) fn current_listener_group<'a>(
    groups: &'a [ListenerGroup],
    fallback: &'a ListenerGroup,
) -> &'a ListenerGroup {
    groups
        .iter()
        .find(|candidate| candidate.address == fallback.address)
        .unwrap_or(fallback)
}

#[derive(Default)]
pub(in crate::server) struct ListenerAccumulator {
    server_ids: Vec<usize>,
    default_server_id: Option<usize>,
    ssl: bool,
    http2: bool,
    http3: bool,
    tcp: bool,
    quic: bool,
    proxy_protocol: bool,
    socket_options: ListenSocketOptions,
}

#[derive(Debug, Clone)]
pub(in crate::server) struct ProxyProtocolAddresses {
    pub(in crate::server) remote: ListenAddress,
    pub(in crate::server) transport_remote: ListenAddress,
    pub(in crate::server) proxy_protocol_addr: Option<IpAddr>,
    pub(in crate::server) proxy_protocol_port: Option<u16>,
    pub(in crate::server) proxy_protocol_server_addr: Option<IpAddr>,
    pub(in crate::server) proxy_protocol_server_port: Option<u16>,
    pub(in crate::server) proxy_protocol_tlvs: Option<Arc<[u8]>>,
}

impl ProxyProtocolAddresses {
    pub(in crate::server) fn without_header(remote: &ListenAddress) -> Self {
        Self {
            remote: remote.clone(),
            transport_remote: remote.clone(),
            proxy_protocol_addr: None,
            proxy_protocol_port: None,
            proxy_protocol_server_addr: None,
            proxy_protocol_server_port: None,
            proxy_protocol_tlvs: None,
        }
    }
}

pub(in crate::server) const PROXY_PROTOCOL_V2_SIGNATURE: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";

pub(in crate::server) async fn read_proxy_protocol<R: AsyncRead + Unpin>(
    reader: &mut R,
    remote: &ListenAddress,
) -> io::Result<ProxyProtocolAddresses> {
    const MAX_HEADER: usize = 108;

    let mut prefix = [0_u8; PROXY_PROTOCOL_V2_SIGNATURE.len()];
    reader.read_exact(&mut prefix).await?;
    if prefix == PROXY_PROTOCOL_V2_SIGNATURE {
        return read_proxy_protocol_v2(reader, remote).await;
    }

    let mut line = prefix.to_vec();
    loop {
        if line.last() == Some(&b'\n') {
            break;
        }
        if line.len() >= MAX_HEADER {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PROXY protocol header is too large",
            ));
        }
        let mut byte = [0_u8; 1];
        reader.read_exact(&mut byte).await?;
        line.push(byte[0]);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    let line = std::str::from_utf8(&line).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "PROXY protocol header is not valid UTF-8",
        )
    })?;
    let mut parts = line.split(' ');
    if parts.next() != Some("PROXY") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing PROXY protocol header",
        ));
    }
    match parts.next() {
        Some("UNKNOWN") => {
            return Ok(ProxyProtocolAddresses::without_header(remote));
        }
        Some("TCP4") | Some("TCP6") => {}
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported PROXY protocol family",
            ));
        }
    }
    let (Some(source), Some(destination), Some(source_port), Some(destination_port)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete PROXY protocol header",
        ));
    };
    let source = source
        .parse::<std::net::IpAddr>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid PROXY source address"))?;
    let destination = destination.parse::<std::net::IpAddr>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid PROXY destination address",
        )
    })?;
    let source_port = source_port
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid PROXY source port"))?;
    let destination_port = destination_port.parse::<u16>().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "invalid PROXY destination port")
    })?;
    Ok(ProxyProtocolAddresses {
        remote: ListenAddress::Inet(SocketAddr::new(source, source_port)),
        transport_remote: remote.clone(),
        proxy_protocol_addr: Some(source),
        proxy_protocol_port: Some(source_port),
        proxy_protocol_server_addr: Some(destination),
        proxy_protocol_server_port: Some(destination_port),
        proxy_protocol_tlvs: None,
    })
}

pub(in crate::server) async fn read_proxy_protocol_v2<R: AsyncRead + Unpin>(
    reader: &mut R,
    remote: &ListenAddress,
) -> io::Result<ProxyProtocolAddresses> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).await?;
    if header[0] >> 4 != 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported PROXY protocol version",
        ));
    }

    let command = header[0] & 0x0f;
    let family = header[1] >> 4;
    let transport = header[1] & 0x0f;
    let length = u16::from_be_bytes([header[2], header[3]]) as usize;
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;

    // Nginx ignores LOCAL and unsupported transport/family combinations.
    if command != 1 || transport != 1 {
        return Ok(ProxyProtocolAddresses::without_header(remote));
    }

    let (source, destination, source_port, destination_port) = match family {
        1 if payload.len() >= 12 => {
            let source = IpAddr::from([payload[0], payload[1], payload[2], payload[3]]);
            let destination = IpAddr::from([payload[4], payload[5], payload[6], payload[7]]);
            (
                source,
                destination,
                u16::from_be_bytes([payload[8], payload[9]]),
                u16::from_be_bytes([payload[10], payload[11]]),
            )
        }
        2 if payload.len() >= 36 => {
            let source = IpAddr::from(<[u8; 16]>::try_from(&payload[0..16]).unwrap());
            let destination = IpAddr::from(<[u8; 16]>::try_from(&payload[16..32]).unwrap());
            (
                source,
                destination,
                u16::from_be_bytes([payload[32], payload[33]]),
                u16::from_be_bytes([payload[34], payload[35]]),
            )
        }
        1 | 2 => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incomplete PROXY protocol address data",
            ));
        }
        _ => return Ok(ProxyProtocolAddresses::without_header(remote)),
    };
    let address_length = if family == 1 { 12 } else { 36 };
    let proxy_protocol_tlvs =
        (payload.len() > address_length).then(|| Arc::from(payload[address_length..].to_vec()));

    Ok(ProxyProtocolAddresses {
        remote: ListenAddress::Inet(SocketAddr::new(source, source_port)),
        transport_remote: remote.clone(),
        proxy_protocol_addr: Some(source),
        proxy_protocol_port: Some(source_port),
        proxy_protocol_server_addr: Some(destination),
        proxy_protocol_server_port: Some(destination_port),
        proxy_protocol_tlvs,
    })
}

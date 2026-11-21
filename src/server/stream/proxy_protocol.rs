use super::*;

const V2_SIGNATURE: &[u8; 12] = b"\r\n\r\n\0\r\nQUIT\n";
const V2_COMMAND_PROXY: u8 = 0x21;
const V2_COMMAND_LOCAL: u8 = 0x20;
const V2_AF_INET: u8 = 1;
const V2_AF_INET6: u8 = 2;
const V2_TRANSPORT_STREAM: u8 = 1;
const V2_TRANSPORT_DATAGRAM: u8 = 2;
const V2_TLV_CRC32C: u8 = 0x03;

#[derive(Clone, Copy)]
pub(in crate::server) enum StreamProxyProtocolTransport {
    Stream,
    Datagram,
}

pub(in crate::server) async fn send_stream_proxy_protocol<W>(
    outbound: &mut W,
    server: &crate::config::StreamServer,
    source: &ListenAddress,
    destination: &ListenAddress,
    transport: StreamProxyProtocolTransport,
    log: &mut StreamLogContext,
) -> bool
where
    W: AsyncWrite + Unpin,
{
    let result = write_stream_proxy_protocol(
        outbound,
        server.proxy_protocol,
        source,
        destination,
        transport,
        log.tls.as_ref(),
        server.proxy_connect_timeout,
    )
    .await;
    handle_proxy_protocol_result(result, log)
}

pub(in crate::server) async fn send_stream_proxy_protocol_datagram(
    outbound: &UdpSocket,
    server: &crate::config::StreamServer,
    source: &ListenAddress,
    destination: &ListenAddress,
    log: &mut StreamLogContext,
) -> bool {
    if server.proxy_protocol == crate::config::StreamProxyProtocol::Off {
        return true;
    }
    let header = build_stream_proxy_protocol_header_with_tls(
        server.proxy_protocol,
        source,
        destination,
        StreamProxyProtocolTransport::Datagram,
        None,
    );
    let result =
        match tokio::time::timeout(server.proxy_connect_timeout, outbound.send(&header)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(err)) => Err(err),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "PROXY protocol datagram write timed out",
            )),
        };
    handle_proxy_protocol_result(result, log)
}

fn handle_proxy_protocol_result(result: io::Result<()>, log: &mut StreamLogContext) -> bool {
    if let Err(err) = result {
        log.status = 500;
        log.logger.log(
            LogLevel::Error,
            format!("stream upstream PROXY protocol write failed: {err}"),
        );
        return false;
    }
    true
}

async fn write_stream_proxy_protocol<W>(
    outbound: &mut W,
    version: crate::config::StreamProxyProtocol,
    source: &ListenAddress,
    destination: &ListenAddress,
    transport: StreamProxyProtocolTransport,
    tls: Option<&TlsConnectionInfo>,
    timeout: Duration,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let header =
        build_stream_proxy_protocol_header_with_tls(version, source, destination, transport, tls);
    if header.is_empty() {
        return Ok(());
    }
    match tokio::time::timeout(timeout, async {
        outbound.write_all(&header).await?;
        outbound.flush().await
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "PROXY protocol header write timed out",
        )),
    }
}

#[cfg(test)]
pub(in crate::server) fn build_stream_proxy_protocol_header(
    version: crate::config::StreamProxyProtocol,
    source: &ListenAddress,
    destination: &ListenAddress,
    transport: StreamProxyProtocolTransport,
) -> Vec<u8> {
    build_stream_proxy_protocol_header_with_tls(version, source, destination, transport, None)
}

pub(in crate::server) fn build_stream_proxy_protocol_header_with_tls(
    version: crate::config::StreamProxyProtocol,
    source: &ListenAddress,
    destination: &ListenAddress,
    transport: StreamProxyProtocolTransport,
    tls: Option<&TlsConnectionInfo>,
) -> Vec<u8> {
    match version {
        crate::config::StreamProxyProtocol::Off => Vec::new(),
        crate::config::StreamProxyProtocol::V1 => build_proxy_protocol_v1(source, destination),
        crate::config::StreamProxyProtocol::V2 => {
            build_proxy_protocol_v2(source, destination, transport, tls)
        }
    }
}

fn build_proxy_protocol_v1(source: &ListenAddress, destination: &ListenAddress) -> Vec<u8> {
    let (ListenAddress::Inet(source), ListenAddress::Inet(destination)) = (source, destination)
    else {
        return b"PROXY UNKNOWN\r\n".to_vec();
    };
    let protocol = match (source.ip(), destination.ip()) {
        (IpAddr::V4(_), IpAddr::V4(_)) => "TCP4",
        (IpAddr::V6(_), IpAddr::V6(_)) => "TCP6",
        _ => return b"PROXY UNKNOWN\r\n".to_vec(),
    };
    format!(
        "PROXY {protocol} {} {} {} {}\r\n",
        source.ip(),
        destination.ip(),
        source.port(),
        destination.port()
    )
    .into_bytes()
}

fn build_proxy_protocol_v2(
    source: &ListenAddress,
    destination: &ListenAddress,
    transport: StreamProxyProtocolTransport,
    tls: Option<&TlsConnectionInfo>,
) -> Vec<u8> {
    let transport = match transport {
        StreamProxyProtocolTransport::Stream => V2_TRANSPORT_STREAM,
        StreamProxyProtocolTransport::Datagram => V2_TRANSPORT_DATAGRAM,
    };
    let (command, family, address_data) = match (source, destination) {
        (ListenAddress::Inet(source), ListenAddress::Inet(destination)) => {
            let family = match (source.ip(), destination.ip()) {
                (IpAddr::V4(_), IpAddr::V4(_)) => V2_AF_INET,
                _ => V2_AF_INET6,
            };
            let addresses = if family == V2_AF_INET {
                ipv4_address_data(*source, *destination).to_vec()
            } else {
                ipv6_address_data(*source, *destination).to_vec()
            };
            (V2_COMMAND_PROXY, family, addresses)
        }
        _ => (V2_COMMAND_LOCAL, 0, Vec::new()),
    };

    let mut tlvs = Vec::new();
    append_stream_proxy_protocol_v2_tls(&mut tlvs, tls);
    append_proxy_protocol_v2_crc(&mut tlvs);

    let payload_len = address_data.len() + tlvs.len();
    let mut header = Vec::with_capacity(16 + payload_len);
    header.extend_from_slice(V2_SIGNATURE);
    header.push(command);
    header.push(
        (family << 4)
            | if command == V2_COMMAND_LOCAL {
                0
            } else {
                transport
            },
    );
    header.extend_from_slice(&(payload_len as u16).to_be_bytes());
    header.extend_from_slice(&address_data);
    header.extend_from_slice(&tlvs);

    let crc = crc32c(&header);
    let crc_offset = header.len() - 4;
    header[crc_offset..].copy_from_slice(&crc.to_be_bytes());
    header
}

fn append_proxy_protocol_v2_crc(tlvs: &mut Vec<u8>) {
    tlvs.extend_from_slice(&[V2_TLV_CRC32C, 0, 4, 0, 0, 0, 0]);
}

fn ipv4_address_data(source: SocketAddr, destination: SocketAddr) -> [u8; 12] {
    let mut data = [0_u8; 12];
    let IpAddr::V4(source_ip) = source.ip() else {
        unreachable!("IPv4 address data requires an IPv4 source");
    };
    let IpAddr::V4(destination_ip) = destination.ip() else {
        unreachable!("IPv4 address data requires an IPv4 destination");
    };
    data[..4].copy_from_slice(&source_ip.octets());
    data[4..8].copy_from_slice(&destination_ip.octets());
    data[8..10].copy_from_slice(&source.port().to_be_bytes());
    data[10..].copy_from_slice(&destination.port().to_be_bytes());
    data
}

fn ipv6_address_data(source: SocketAddr, destination: SocketAddr) -> [u8; 36] {
    let mut data = [0_u8; 36];
    data[..16].copy_from_slice(&ipv6_mapped(source.ip()));
    data[16..32].copy_from_slice(&ipv6_mapped(destination.ip()));
    data[32..34].copy_from_slice(&source.port().to_be_bytes());
    data[34..].copy_from_slice(&destination.port().to_be_bytes());
    data
}

fn ipv6_mapped(address: IpAddr) -> [u8; 16] {
    match address {
        IpAddr::V6(address) => address.octets(),
        IpAddr::V4(address) => {
            let mut mapped = [0_u8; 16];
            mapped[10] = 0xff;
            mapped[11] = 0xff;
            mapped[12..].copy_from_slice(&address.octets());
            mapped
        }
    }
}

fn crc32c(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x82f6_3b78 & mask);
        }
    }
    !crc
}

use super::*;
use tokio::net::TcpSocket;

pub(in crate::server) async fn connect_stream_tcp(
    host: &str,
    port: u16,
    local_bind: Option<&str>,
    socket_options: StreamSocketOptions,
) -> io::Result<TcpStream> {
    let targets: Vec<_> = tokio::net::lookup_host((host, port)).await?.collect();
    if targets.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("no address resolved for {host}:{port}"),
        ));
    }
    let local_addresses = match local_bind {
        Some(value) => resolve_stream_bind_addresses(value).await?,
        None => Vec::new(),
    };
    let mut last_error = None;
    for target in targets {
        let local = if local_bind.is_some() {
            let Some(local) = local_addresses
                .iter()
                .copied()
                .find(|local| local.is_ipv4() == target.is_ipv4())
            else {
                continue;
            };
            Some(local)
        } else {
            None
        };
        let socket = if target.is_ipv4() {
            TcpSocket::new_v4()?
        } else {
            TcpSocket::new_v6()?
        };
        socket_options.apply_to_tcp_socket(&socket)?;
        if let Some(local) = local {
            socket.bind(local)?;
        }
        match socket.connect(target).await {
            Ok(stream) => return Ok(stream),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("no compatible local address for {host}:{port}"),
        )
    }))
}

pub(in crate::server) async fn bind_stream_udp_socket(
    local_bind: Option<&str>,
    target_is_ipv4: bool,
) -> io::Result<UdpSocket> {
    let mut addresses = match local_bind {
        Some(value) => resolve_stream_bind_addresses(value).await?,
        None if target_is_ipv4 => vec![SocketAddr::from(([0, 0, 0, 0], 0))],
        None => vec![SocketAddr::from(([0_u16; 8], 0))],
    };
    addresses.sort_by_key(|address| address.is_ipv4() != target_is_ipv4);
    let mut last_error = None;
    for address in addresses {
        match UdpSocket::bind(address).await {
            Ok(socket) => return Ok(socket),
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "no address available for stream UDP upstream",
        )
    }))
}

async fn resolve_stream_bind_addresses(value: &str) -> io::Result<Vec<SocketAddr>> {
    if value == "*" {
        return Ok(vec![
            SocketAddr::from(([0, 0, 0, 0], 0)),
            SocketAddr::from(([0_u16; 8], 0)),
        ]);
    }
    if let Some(port) = value.strip_prefix("*:") {
        let port = port.parse::<u16>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid proxy_bind address \"{value}\""),
            )
        })?;
        return Ok(vec![
            SocketAddr::from(([0, 0, 0, 0], port)),
            SocketAddr::from(([0_u16; 8], port)),
        ]);
    }
    if let Ok(address) = value.parse::<SocketAddr>() {
        return Ok(vec![address]);
    }
    if let Ok(address) = value.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(address, 0)]);
    }
    let lookup = if value.starts_with('[') && value.ends_with(']') || !value.contains(':') {
        format!("{value}:0")
    } else {
        value.to_string()
    };
    let addresses: Vec<_> = tokio::net::lookup_host(lookup).await?.collect();
    if addresses.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("no address resolved for proxy_bind \"{value}\""),
        ));
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_stream_proxy_bind_addresses() {
        assert_eq!(
            resolve_stream_bind_addresses("127.0.0.1").await.unwrap(),
            vec!["127.0.0.1:0".parse().unwrap()]
        );
        assert_eq!(
            resolve_stream_bind_addresses("127.0.0.1:12345")
                .await
                .unwrap(),
            vec!["127.0.0.1:12345".parse().unwrap()]
        );
        assert_eq!(
            resolve_stream_bind_addresses("::1").await.unwrap(),
            vec!["[::1]:0".parse().unwrap()]
        );
        assert_eq!(
            resolve_stream_bind_addresses("*:23456").await.unwrap(),
            vec![
                "0.0.0.0:23456".parse().unwrap(),
                "[::]:23456".parse().unwrap()
            ]
        );
    }
}

use super::*;
use tokio::net::{TcpSocket, TcpStream};

#[derive(Clone, Copy, Default)]
pub(in crate::server) struct UpstreamSocketOptions {
    keepalive: bool,
    receive_buffer_size: usize,
    send_buffer_size: usize,
    tcp_nodelay: bool,
}

impl UpstreamSocketOptions {
    pub(super) fn for_proxy(context: &EffectiveConfig) -> Self {
        Self {
            keepalive: context.proxy_socket_keepalive,
            receive_buffer_size: context.proxy_socket_rcvbuf,
            send_buffer_size: context.proxy_socket_sndbuf,
            tcp_nodelay: context.tcp_nodelay,
        }
    }

    pub(super) fn for_fastcgi(context: &EffectiveConfig) -> Self {
        Self {
            keepalive: context.fastcgi_socket_keepalive,
            receive_buffer_size: context.fastcgi_socket_rcvbuf,
            send_buffer_size: context.fastcgi_socket_sndbuf,
            tcp_nodelay: context.tcp_nodelay,
        }
    }

    pub(super) fn for_uwsgi(context: &EffectiveConfig) -> Self {
        Self {
            keepalive: context.uwsgi_socket_keepalive,
            receive_buffer_size: context.uwsgi_socket_rcvbuf,
            send_buffer_size: context.uwsgi_socket_sndbuf,
            tcp_nodelay: context.tcp_nodelay,
        }
    }

    pub(super) fn for_scgi(context: &EffectiveConfig) -> Self {
        Self {
            keepalive: context.scgi_socket_keepalive,
            receive_buffer_size: context.scgi_socket_rcvbuf,
            send_buffer_size: context.scgi_socket_sndbuf,
            tcp_nodelay: context.tcp_nodelay,
        }
    }

    pub(super) fn for_memcached(context: &EffectiveConfig) -> Self {
        Self {
            keepalive: context.memcached_socket_keepalive,
            receive_buffer_size: 0,
            send_buffer_size: 0,
            tcp_nodelay: context.tcp_nodelay,
        }
    }

    pub(super) fn for_grpc(context: &EffectiveConfig) -> Self {
        Self {
            keepalive: context.grpc.socket_keepalive,
            receive_buffer_size: context.grpc.socket_rcvbuf,
            send_buffer_size: context.grpc.socket_sndbuf,
            tcp_nodelay: context.tcp_nodelay,
        }
    }

    pub(in crate::server) fn for_tunnel(context: &EffectiveConfig) -> Self {
        Self {
            keepalive: context.tunnel.socket_keepalive,
            receive_buffer_size: context.tunnel.socket_rcvbuf,
            send_buffer_size: context.tunnel.socket_sndbuf,
            tcp_nodelay: context.tcp_nodelay,
        }
    }
}

pub(in crate::server) fn evaluate_upstream_bind(
    template: Option<&str>,
    variables: &Variables,
) -> Option<String> {
    template
        .map(|value| eval_variables(value, variables))
        .filter(|value| !value.is_empty())
}

pub(super) fn evaluate_proxy_bind(
    context: &EffectiveConfig,
    variables: &Variables,
) -> Option<String> {
    evaluate_upstream_bind(context.proxy_bind.as_deref(), variables)
}

pub(super) fn evaluate_fastcgi_bind(
    context: &EffectiveConfig,
    variables: &Variables,
) -> Option<String> {
    evaluate_upstream_bind(context.fastcgi_bind.as_deref(), variables)
}

pub(super) fn evaluate_uwsgi_bind(
    context: &EffectiveConfig,
    variables: &Variables,
) -> Option<String> {
    evaluate_upstream_bind(context.uwsgi_bind.as_deref(), variables)
}

pub(super) fn evaluate_scgi_bind(
    context: &EffectiveConfig,
    variables: &Variables,
) -> Option<String> {
    evaluate_upstream_bind(context.scgi_bind.as_deref(), variables)
}

pub(super) fn evaluate_memcached_bind(
    context: &EffectiveConfig,
    variables: &Variables,
) -> Option<String> {
    evaluate_upstream_bind(context.memcached_bind.as_deref(), variables)
}

pub(super) fn evaluate_grpc_bind(
    context: &EffectiveConfig,
    variables: &Variables,
) -> Option<String> {
    evaluate_upstream_bind(context.grpc.bind.as_deref(), variables)
}

pub(super) async fn connect_upstream_tcp(
    host: &str,
    port: u16,
    local_bind: Option<&str>,
    socket_options: UpstreamSocketOptions,
    bind_directive: &str,
) -> io::Result<TcpStream> {
    let lookup_host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    let targets: Vec<_> = tokio::net::lookup_host((lookup_host, port))
        .await?
        .collect();
    if targets.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("no address resolved for {host}:{port}"),
        ));
    }
    let local_addresses = match local_bind {
        Some(value) => resolve_upstream_bind_addresses(value, bind_directive).await?,
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
        if let Some(local) = local {
            socket.bind(local)?;
        }
        match socket.connect(target).await {
            Ok(stream) => {
                stream.set_nodelay(socket_options.tcp_nodelay)?;
                apply_upstream_socket_options(&stream, socket_options)?;
                return Ok(stream);
            }
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

async fn resolve_upstream_bind_addresses(
    value: &str,
    bind_directive: &str,
) -> io::Result<Vec<SocketAddr>> {
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
                format!("invalid {bind_directive} address \"{value}\""),
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
            format!("no address resolved for {bind_directive} \"{value}\""),
        ));
    }
    Ok(addresses)
}

#[cfg(unix)]
pub(super) fn apply_upstream_socket_options<S: std::os::fd::AsFd>(
    socket: &S,
    options: UpstreamSocketOptions,
) -> io::Result<()> {
    let socket = socket2::SockRef::from(socket);
    if options.keepalive {
        socket.set_keepalive(true)?;
    }
    if options.receive_buffer_size > 0 {
        socket.set_recv_buffer_size(options.receive_buffer_size)?;
    }
    if options.send_buffer_size > 0 {
        socket.set_send_buffer_size(options.send_buffer_size)?;
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn apply_upstream_socket_options<S: std::os::windows::io::AsSocket>(
    socket: &S,
    options: UpstreamSocketOptions,
) -> io::Result<()> {
    let socket = socket2::SockRef::from(socket);
    if options.keepalive {
        socket.set_keepalive(true)?;
    }
    if options.receive_buffer_size > 0 {
        socket.set_recv_buffer_size(options.receive_buffer_size)?;
    }
    if options.send_buffer_size > 0 {
        socket.set_send_buffer_size(options.send_buffer_size)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use socket2::SockRef;
    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn connects_from_proxy_bind_and_applies_socket_options() {
        let listener = match TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping HTTP upstream socket test: sockets not permitted");
                return;
            }
            Err(err) => panic!("bind upstream socket listener: {err}"),
        };
        let address = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            listener.accept().await.unwrap();
        });
        let stream = connect_upstream_tcp(
            "127.0.0.1",
            address.port(),
            Some("127.0.0.2"),
            UpstreamSocketOptions {
                keepalive: true,
                receive_buffer_size: 32 * 1024,
                send_buffer_size: 64 * 1024,
                tcp_nodelay: true,
            },
            "proxy_bind",
        )
        .await
        .unwrap();

        assert_eq!(
            stream.local_addr().unwrap().ip(),
            "127.0.0.2".parse::<IpAddr>().unwrap()
        );
        let socket = SockRef::from(&stream);
        assert!(socket.keepalive().unwrap());
        assert!(socket.recv_buffer_size().unwrap() >= 32 * 1024);
        assert!(socket.send_buffer_size().unwrap() >= 64 * 1024);
        assert!(stream.nodelay().unwrap());
        accept.await.unwrap();
    }
}

use std::io;
use std::net::{SocketAddr, TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};

use socket2::{Domain, Protocol, Socket, TcpKeepalive, Type};
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::net::{TcpListener, UdpSocket};

use super::*;

mod platform;

pub(in crate::server) fn bind_tcp_listener(
    address: SocketAddr,
    options: &ListenSocketOptions,
) -> io::Result<TcpListener> {
    let socket = create_socket(address, Type::STREAM, stream_protocol(options.multipath))?;
    apply_socket_options(&socket, address, options)?;
    socket.bind(&address.into())?;
    socket.listen(i32::try_from(options.backlog).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "listen backlog exceeds platform limit",
        )
    })?)?;
    platform::apply_tcp_listener_options(&socket, options)?;
    socket.set_nonblocking(true)?;

    let listener: StdTcpListener = socket.into();
    listener.set_nonblocking(true)?;
    TcpListener::from_std(listener)
}

pub(in crate::server) fn bind_udp_socket(
    address: SocketAddr,
    options: &ListenSocketOptions,
) -> io::Result<UdpSocket> {
    let socket = create_socket(address, Type::DGRAM, Protocol::UDP)?;
    apply_socket_options(&socket, address, options)?;
    socket.bind(&address.into())?;
    socket.set_nonblocking(true)?;

    let socket: StdUdpSocket = socket.into();
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket)
}

fn create_socket(address: SocketAddr, socket_type: Type, protocol: Protocol) -> io::Result<Socket> {
    let domain = if address.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    Socket::new(domain, socket_type, Some(protocol))
}

fn stream_protocol(multipath: bool) -> Protocol {
    #[cfg(target_os = "linux")]
    if multipath {
        return Protocol::MPTCP;
    }
    let _ = multipath;
    Protocol::TCP
}

fn apply_socket_options(
    socket: &Socket,
    address: SocketAddr,
    options: &ListenSocketOptions,
) -> io::Result<()> {
    apply_common_socket_options(socket, options)?;
    if address.is_ipv6() {
        socket.set_only_v6(options.ipv6only)?;
    }
    Ok(())
}

fn apply_common_socket_options(socket: &Socket, options: &ListenSocketOptions) -> io::Result<()> {
    socket.set_reuse_address(true)?;
    if options.reuseport {
        set_reuseport(socket)?;
    }
    if let Some(size) = options.receive_buffer_size {
        socket.set_recv_buffer_size(socket_buffer_size(size)?)?;
    }
    if let Some(size) = options.send_buffer_size {
        socket.set_send_buffer_size(socket_buffer_size(size)?)?;
    }
    apply_keepalive(socket, &options.keepalive)?;
    Ok(())
}

#[cfg(unix)]
pub(in crate::server) fn bind_unix_listener(
    path: &std::path::Path,
    options: &ListenSocketOptions,
) -> io::Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;

    if options.multipath {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "multipath is not supported for UNIX domain listen sockets",
        ));
    }
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    apply_common_socket_options(&socket, options)?;
    socket.bind(&socket2::SockAddr::unix(path)?)?;
    socket.listen(i32::try_from(options.backlog).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "listen backlog exceeds platform limit",
        )
    })?)?;
    socket.set_nonblocking(true)?;

    let listener: std::os::unix::net::UnixListener = socket.into();
    listener.set_nonblocking(true)?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666));
    UnixListener::from_std(listener)
}

fn apply_keepalive(socket: &Socket, keepalive: &ListenKeepalive) -> io::Result<()> {
    match keepalive {
        ListenKeepalive::Unset => Ok(()),
        ListenKeepalive::On => socket.set_keepalive(true),
        ListenKeepalive::Off => socket.set_keepalive(false),
        ListenKeepalive::Tuned {
            idle,
            interval,
            count,
        } => {
            let mut parameters = TcpKeepalive::new();
            if let Some(idle) = idle.filter(|duration| !duration.is_zero()) {
                parameters = parameters.with_time(idle);
            }
            if let Some(interval) = interval.filter(|duration| !duration.is_zero()) {
                parameters = parameters.with_interval(interval);
            }
            if let Some(count) = count.filter(|count| *count > 0) {
                parameters = parameters.with_retries(count);
            }
            socket.set_tcp_keepalive(&parameters)
        }
    }
}

fn socket_buffer_size(value: usize) -> io::Result<usize> {
    if value > i32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket buffer size exceeds platform limit",
        ));
    }
    Ok(value)
}

#[cfg(unix)]
fn set_reuseport(socket: &Socket) -> io::Result<()> {
    socket.set_reuse_port(true)
}

#[cfg(not(unix))]
fn set_reuseport(_socket: &Socket) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "reuseport is not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use socket2::SockRef;

    use super::*;

    #[tokio::test]
    async fn binds_tcp_listener_with_socket_options() {
        let address = SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = bind_tcp_listener(
            address,
            &ListenSocketOptions {
                receive_buffer_size: Some(32 * 1024),
                send_buffer_size: Some(64 * 1024),
                reuseport: true,
                keepalive: ListenKeepalive::Tuned {
                    idle: Some(Duration::from_secs(30)),
                    interval: Some(Duration::from_secs(5)),
                    count: Some(3),
                },
                ..ListenSocketOptions::default()
            },
        )
        .unwrap_or_else(|err| panic!("bind TCP listener: {err}"));

        let socket = SockRef::from(&listener);
        assert!(socket.reuse_port().unwrap());
        assert!(socket.recv_buffer_size().unwrap() >= 32 * 1024);
        assert!(socket.send_buffer_size().unwrap() >= 64 * 1024);
        assert!(socket.keepalive().unwrap());
        assert!(socket.tcp_keepalive_time().unwrap() >= Duration::from_secs(30));
        assert_eq!(socket.tcp_keepalive_retries().unwrap(), 3);
        assert_ne!(listener.local_addr().unwrap().port(), 0);
    }

    #[tokio::test]
    async fn binds_udp_socket_with_socket_options() {
        let address = SocketAddr::from(([127, 0, 0, 1], 0));
        let socket = bind_udp_socket(
            address,
            &ListenSocketOptions {
                receive_buffer_size: Some(16 * 1024),
                send_buffer_size: Some(32 * 1024),
                reuseport: true,
                ..ListenSocketOptions::default()
            },
        )
        .unwrap_or_else(|err| panic!("bind UDP socket: {err}"));

        let socket_ref = SockRef::from(&socket);
        assert!(socket_ref.reuse_port().unwrap());
        assert!(socket_ref.recv_buffer_size().unwrap() >= 16 * 1024);
        assert!(socket_ref.send_buffer_size().unwrap() >= 32 * 1024);
        assert_ne!(socket.local_addr().unwrap().port(), 0);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn binds_multipath_listener_when_supported() {
        let address = SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = match bind_tcp_listener(
            address,
            &ListenSocketOptions {
                multipath: true,
                ..ListenSocketOptions::default()
            },
        ) {
            Ok(listener) => listener,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::Unsupported
                        | io::ErrorKind::InvalidInput
                        | io::ErrorKind::AddrNotAvailable
                ) =>
            {
                eprintln!("skipping MPTCP listener test: {err}");
                return;
            }
            Err(err) => panic!("bind MPTCP listener: {err}"),
        };

        let socket = SockRef::from(&listener);
        assert_eq!(
            socket.protocol().unwrap().map(i32::from),
            Some(i32::from(Protocol::MPTCP))
        );
    }
}

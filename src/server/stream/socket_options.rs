use std::io;

use socket2::SockRef;

#[derive(Debug, Clone, Copy, Default)]
pub(in crate::server) struct StreamSocketOptions {
    nodelay: bool,
    keepalive: bool,
    receive_buffer_size: usize,
    send_buffer_size: usize,
}

impl StreamSocketOptions {
    pub(in crate::server) fn from_server(server: &crate::config::StreamServer) -> Self {
        Self {
            nodelay: server.tcp_nodelay,
            keepalive: server.proxy_socket_keepalive,
            receive_buffer_size: server.proxy_socket_rcvbuf,
            send_buffer_size: server.proxy_socket_sndbuf,
        }
    }

    pub(in crate::server) fn apply_to_tcp_socket(
        self,
        socket: &tokio::net::TcpSocket,
    ) -> io::Result<()> {
        socket.set_nodelay(self.nodelay)?;
        if self.keepalive {
            socket.set_keepalive(true)?;
        }
        if self.receive_buffer_size > 0 {
            socket.set_recv_buffer_size(to_socket_buffer_size(self.receive_buffer_size)?)?;
        }
        if self.send_buffer_size > 0 {
            socket.set_send_buffer_size(to_socket_buffer_size(self.send_buffer_size)?)?;
        }
        Ok(())
    }
}

fn to_socket_buffer_size(value: usize) -> io::Result<u32> {
    u32::try_from(value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "stream socket buffer size exceeds platform limit",
        )
    })
}

#[cfg(unix)]
pub(in crate::server) fn apply_stream_socket_options<S: std::os::fd::AsFd>(
    socket: &S,
    options: StreamSocketOptions,
) -> io::Result<()> {
    let socket = SockRef::from(socket);
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
pub(in crate::server) fn apply_stream_socket_options<S: std::os::windows::io::AsSocket>(
    socket: &S,
    options: StreamSocketOptions,
) -> io::Result<()> {
    let socket = SockRef::from(socket);
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
    use std::net::{TcpListener, TcpStream};

    use socket2::SockRef;

    use super::*;

    #[test]
    fn applies_keepalive_and_socket_buffers() {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping stream socket option test: sockets not permitted");
                return;
            }
            Err(err) => panic!("bind TCP listener: {err}"),
        };
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        apply_stream_socket_options(
            &stream,
            StreamSocketOptions {
                nodelay: true,
                keepalive: true,
                receive_buffer_size: 32 * 1024,
                send_buffer_size: 64 * 1024,
            },
        )
        .unwrap();

        let socket = SockRef::from(&stream);
        assert!(socket.keepalive().unwrap());
        assert!(socket.recv_buffer_size().unwrap() >= 32 * 1024);
        assert!(socket.send_buffer_size().unwrap() >= 64 * 1024);
    }
}

use std::io;
use std::net::TcpStream as StdTcpStream;
use std::sync::Arc;

use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

enum Socket {
    Tcp(StdTcpStream),
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
}

#[derive(Clone)]
pub(in crate::server) struct ClientSocketControl {
    socket: Arc<Socket>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::server) enum ClientCloseState {
    Open,
    ReadHalfClosed,
    Reset,
}

impl ClientSocketControl {
    pub(in crate::server) fn attach_tcp(
        stream: TcpStream,
    ) -> io::Result<(TcpStream, Option<Self>)> {
        let socket = stream.into_std()?;
        let control = socket
            .try_clone()
            .ok()
            .map(|socket| Self::from_socket(Socket::Tcp(socket)));
        Ok((TcpStream::from_std(socket)?, control))
    }

    #[cfg(unix)]
    pub(in crate::server) fn attach_unix(
        stream: UnixStream,
    ) -> io::Result<(UnixStream, Option<Self>)> {
        let socket = stream.into_std()?;
        let control = socket
            .try_clone()
            .ok()
            .map(|socket| Self::from_socket(Socket::Unix(socket)));
        Ok((UnixStream::from_std(socket)?, control))
    }

    fn from_socket(socket: Socket) -> Self {
        Self {
            socket: Arc::new(socket),
        }
    }

    pub(in crate::server) fn apply_nodelay(&self, enabled: bool) -> io::Result<()> {
        // Nginx only calls ngx_tcp_nodelay() for "on"; "off" never clears an
        // option already enabled earlier on the same connection.
        if enabled && let Socket::Tcp(socket) = self.socket.as_ref() {
            socket.set_nodelay(true)?;
        }
        Ok(())
    }

    /// Inspect peer closure without consuming request bytes.
    ///
    /// A TCP FIN closes only the peer's write half. It can cancel an in-flight
    /// request body, but the response direction remains writable. A reset or
    /// socket error makes both directions unusable.
    pub(in crate::server) fn close_state(&self) -> ClientCloseState {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, RawFd};

            let fd: RawFd = match self.socket.as_ref() {
                Socket::Tcp(socket) => socket.as_raw_fd(),
                #[cfg(unix)]
                Socket::Unix(socket) => socket.as_raw_fd(),
            };

            let mut byte = 0_u8;
            let result = unsafe {
                libc::recv(
                    fd,
                    (&mut byte as *mut u8).cast(),
                    1,
                    libc::MSG_PEEK | libc::MSG_DONTWAIT,
                )
            };
            if result > 0 {
                return ClientCloseState::Open;
            }
            if result == 0 {
                return ClientCloseState::ReadHalfClosed;
            }
            let Some(error) = io::Error::last_os_error().raw_os_error() else {
                return ClientCloseState::Reset;
            };
            if error == libc::EAGAIN || error == libc::EWOULDBLOCK || error == libc::EINTR {
                ClientCloseState::Open
            } else {
                ClientCloseState::Reset
            }
        }
        #[cfg(not(unix))]
        {
            ClientCloseState::Open
        }
    }

    pub(in crate::server) fn close_detected(&self) -> bool {
        self.close_state() != ClientCloseState::Open
    }

    pub(in crate::server) fn reset_detected(&self) -> bool {
        self.close_state() == ClientCloseState::Reset
    }

    #[cfg(test)]
    pub(in crate::server) fn enabled(&self) -> io::Result<bool> {
        match self.socket.as_ref() {
            Socket::Tcp(socket) => socket.nodelay(),
            #[cfg(unix)]
            Socket::Unix(_) => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn detects_a_peer_half_close_and_reset() {
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return,
            Err(error) => panic!("bind socket control test: {error}"),
        };
        let address = listener.local_addr().unwrap();

        let half_close_client = TcpStream::connect(address).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (_server, control) = ClientSocketControl::attach_tcp(server).unwrap();
        half_close_client
            .into_std()
            .unwrap()
            .shutdown(std::net::Shutdown::Write)
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            control.as_ref().unwrap().close_state(),
            ClientCloseState::ReadHalfClosed
        );

        let reset_client = TcpStream::connect(address).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (_server, control) = ClientSocketControl::attach_tcp(server).unwrap();
        let reset_client = reset_client.into_std().unwrap();
        socket2::SockRef::from(&reset_client)
            .set_linger(Some(Duration::ZERO))
            .unwrap();
        drop(reset_client);

        tokio::time::timeout(Duration::from_secs(1), async {
            while !control.as_ref().unwrap().reset_detected() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("peer reset should be visible through MSG_PEEK");
    }
}

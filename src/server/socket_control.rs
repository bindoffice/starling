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

    /// Check for a peer close without consuming request bytes.
    ///
    /// Nginx's `ngx_http_upstream_check_broken_connection()` treats both a
    /// pending reset and `recv() == 0` as a prematurely closed client.
    pub(in crate::server) fn close_detected(&self) -> bool {
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
                return false;
            }
            if result == 0 {
                return true;
            }
            let Some(error) = io::Error::last_os_error().raw_os_error() else {
                return false;
            };
            error != libc::EAGAIN && error != libc::EWOULDBLOCK && error != libc::EINTR
        }
        #[cfg(not(unix))]
        {
            false
        }
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
        assert!(control.as_ref().unwrap().close_detected());

        let reset_client = TcpStream::connect(address).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (_server, control) = ClientSocketControl::attach_tcp(server).unwrap();
        let reset_client = reset_client.into_std().unwrap();
        socket2::SockRef::from(&reset_client)
            .set_linger(Some(Duration::ZERO))
            .unwrap();
        drop(reset_client);

        tokio::time::timeout(Duration::from_secs(1), async {
            while !control.as_ref().unwrap().close_detected() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("peer reset should be visible through MSG_PEEK");
    }
}

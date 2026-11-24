use std::io;
use std::net::TcpStream as StdTcpStream;
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
#[cfg(unix)]
use tokio::io::unix::AsyncFd;
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
    #[cfg(unix)]
    close_fd: Option<Arc<AsyncFd<OwnedFd>>>,
}

impl std::fmt::Debug for ClientSocketControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ClientSocketControl")
    }
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
        #[cfg(unix)]
        let close_fd = duplicate_async_fd(socket.as_raw_fd());
        Self {
            socket: Arc::new(socket),
            #[cfg(unix)]
            close_fd,
        }
    }

    pub(in crate::server) async fn wait_for_close_event(&self) {
        #[cfg(unix)]
        if let Some(close_fd) = &self.close_fd {
            loop {
                let mut readiness = match close_fd.readable().await {
                    Ok(readiness) => readiness,
                    Err(_) => std::future::pending().await,
                };
                if self.close_detected() {
                    return;
                }
                readiness.clear_ready();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        std::future::pending().await
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

#[cfg(unix)]
impl AsRawFd for Socket {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            Socket::Tcp(socket) => socket.as_raw_fd(),
            Socket::Unix(socket) => socket.as_raw_fd(),
        }
    }
}

#[cfg(unix)]
fn duplicate_async_fd(fd: RawFd) -> Option<Arc<AsyncFd<OwnedFd>>> {
    let duplicate = unsafe { libc::dup(fd) };
    if duplicate < 0 {
        return None;
    }
    let duplicate = unsafe { OwnedFd::from_raw_fd(duplicate) };
    unsafe { AsyncFd::register(duplicate) }.ok().map(Arc::new)
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

    #[tokio::test]
    async fn close_event_wakes_without_polling_interval() {
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return,
            Err(error) => panic!("bind socket control test: {error}"),
        };
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (_server, control) = ClientSocketControl::attach_tcp(server).unwrap();
        let control = control.unwrap();
        drop(client);
        tokio::time::timeout(Duration::from_millis(100), control.wait_for_close_event())
            .await
            .expect("close event should wake immediately");
    }
}

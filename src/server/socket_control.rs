use std::io;
use std::net::TcpStream as StdTcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

struct ClientSocketControlInner {
    socket: Socket,
    nodelay_applied: AtomicBool,
}

#[derive(Clone)]
pub(in crate::server) struct ClientSocketControl {
    inner: Arc<ClientSocketControlInner>,
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
            inner: Arc::new(ClientSocketControlInner {
                socket,
                nodelay_applied: AtomicBool::new(false),
            }),
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

    fn apply_nodelay(&self, enabled: bool) -> io::Result<()> {
        // Nginx only calls ngx_tcp_nodelay() for "on"; "off" never clears an
        // option already enabled earlier on the same connection.
        if enabled && let Socket::Tcp(socket) = &self.inner.socket {
            socket.set_nodelay(true)?;
        }
        Ok(())
    }

    pub(in crate::server) fn apply_nodelay_once(&self, enabled: bool) -> io::Result<()> {
        if !enabled || self.inner.nodelay_applied.load(Ordering::Acquire) {
            return Ok(());
        }
        if self
            .inner
            .nodelay_applied
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }
        if let Err(error) = self.apply_nodelay(true) {
            self.inner.nodelay_applied.store(false, Ordering::Release);
            return Err(error);
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
            let fd: RawFd = match &self.inner.socket {
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

    pub(in crate::server) fn supports_sendfile(&self) -> bool {
        #[cfg(unix)]
        {
            matches!(&self.inner.socket, Socket::Tcp(_)) && self.close_fd.is_some()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    #[cfg(unix)]
    pub(in crate::server) async fn sendfile(
        &self,
        file: &tokio::fs::File,
        mut offset: u64,
        mut length: u64,
        timeout: Duration,
    ) -> io::Result<()> {
        let Socket::Tcp(_) = &self.inner.socket else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "sendfile requires a TCP client socket",
            ));
        };
        let Some(writable) = &self.close_fd else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "sendfile readiness socket is unavailable",
            ));
        };
        let output_fd = self.inner.socket.as_raw_fd();
        let input_fd = file.as_raw_fd();
        while length > 0 {
            let chunk = length.min(1024 * 1024);
            let mut file_offset = i64::try_from(offset).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "file offset exceeds i64")
            })?;
            let result = unsafe {
                libc::sendfile(
                    output_fd,
                    input_fd,
                    &mut file_offset,
                    chunk.min(usize::MAX as u64) as usize,
                )
            };
            if result > 0 {
                let sent = result as u64;
                offset = offset.saturating_add(sent);
                length -= sent;
                continue;
            }
            if result == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "static file ended before Content-Length",
                ));
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => {}
                Some(error) if error == libc::EAGAIN || error == libc::EWOULDBLOCK => {
                    let mut readiness = if timeout == Duration::ZERO {
                        writable.writable().await
                    } else {
                        tokio::time::timeout(timeout, writable.writable())
                            .await
                            .map_err(|_| {
                                io::Error::new(io::ErrorKind::TimedOut, "sendfile timed out")
                            })?
                    }?;
                    readiness.clear_ready();
                }
                _ => return Err(error),
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::server) fn enabled(&self) -> io::Result<bool> {
        match &self.inner.socket {
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

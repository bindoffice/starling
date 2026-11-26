use std::io;
use std::mem::MaybeUninit;
use std::net::TcpStream as StdTcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use std::os::fd::{AsRawFd, RawFd};
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
    #[cfg(unix)]
    socket: AsyncFd<Socket>,
    #[cfg(not(unix))]
    socket: Socket,
    nodelay_applied: AtomicBool,
}

#[derive(Clone)]
pub(in crate::server) struct ClientSocketControl {
    inner: Arc<ClientSocketControlInner>,
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
        let Ok(control_socket) = socket.try_clone() else {
            return Ok((TcpStream::from_std(socket)?, None));
        };
        let stream = TcpStream::from_std(control_socket)?;
        Ok((stream, Self::from_socket(Socket::Tcp(socket))))
    }

    #[cfg(unix)]
    pub(in crate::server) fn attach_unix(
        stream: UnixStream,
    ) -> io::Result<(UnixStream, Option<Self>)> {
        let socket = stream.into_std()?;
        let Ok(control_socket) = socket.try_clone() else {
            return Ok((UnixStream::from_std(socket)?, None));
        };
        let stream = UnixStream::from_std(control_socket)?;
        Ok((stream, Self::from_socket(Socket::Unix(socket))))
    }

    fn from_socket(socket: Socket) -> Option<Self> {
        #[cfg(unix)]
        // SAFETY: Socket owns a valid file descriptor for the lifetime of the
        // AsyncFd registration.
        let socket = unsafe { AsyncFd::register(socket) }.ok()?;
        Some(Self {
            inner: Arc::new(ClientSocketControlInner {
                socket,
                nodelay_applied: AtomicBool::new(false),
            }),
        })
    }

    pub(in crate::server) async fn wait_for_close_event(&self) {
        #[cfg(unix)]
        loop {
            let mut readiness = match self.inner.socket.readable().await {
                Ok(readiness) => readiness,
                Err(_) => std::future::pending().await,
            };
            if self.close_detected() {
                return;
            }
            readiness.clear_ready();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        #[cfg(not(unix))]
        std::future::pending().await
    }

    fn apply_nodelay(&self, enabled: bool) -> io::Result<()> {
        // Nginx only calls ngx_tcp_nodelay() for "on"; "off" never clears an
        // option already enabled earlier on the same connection.
        if enabled && let Socket::Tcp(socket) = self.socket() {
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
            let mut byte = [MaybeUninit::<u8>::uninit()];
            let result = match self.socket() {
                Socket::Tcp(socket) => socket2::SockRef::from(socket).peek(&mut byte),
                Socket::Unix(socket) => socket2::SockRef::from(socket).peek(&mut byte),
            };
            match result {
                Ok(0) => ClientCloseState::ReadHalfClosed,
                Ok(_) => ClientCloseState::Open,
                Err(error) => match error.raw_os_error() {
                    Some(code)
                        if code == libc::EAGAIN
                            || code == libc::EWOULDBLOCK
                            || code == libc::EINTR =>
                    {
                        ClientCloseState::Open
                    }
                    _ => ClientCloseState::Reset,
                },
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

    pub(in crate::server) fn set_reset_on_close(&self) -> io::Result<()> {
        match self.socket() {
            Socket::Tcp(socket) => socket2::SockRef::from(socket).set_linger(Some(Duration::ZERO)),
            #[cfg(unix)]
            Socket::Unix(socket) => socket2::SockRef::from(socket).set_linger(Some(Duration::ZERO)),
        }
    }

    pub(in crate::server) fn supports_sendfile(&self) -> bool {
        #[cfg(unix)]
        {
            matches!(self.socket(), Socket::Tcp(_))
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
        let Socket::Tcp(output) = self.socket() else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "sendfile requires a TCP client socket",
            ));
        };
        while length > 0 {
            let chunk = length.min(1024 * 1024);
            let mut file_offset = offset;
            match rustix::fs::sendfile(
                output,
                file,
                Some(&mut file_offset),
                chunk.min(usize::MAX as u64) as usize,
            ) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "static file ended before Content-Length",
                    ));
                }
                Ok(sent) => {
                    offset = offset.saturating_add(sent as u64);
                    length -= sent as u64;
                }
                Err(rustix::io::Errno::INTR) => {}
                Err(error)
                    if error == rustix::io::Errno::AGAIN
                        || error == rustix::io::Errno::WOULDBLOCK =>
                {
                    let mut readiness = if timeout == Duration::ZERO {
                        self.inner.socket.writable().await
                    } else {
                        tokio::time::timeout(timeout, self.inner.socket.writable())
                            .await
                            .map_err(|_| {
                                io::Error::new(io::ErrorKind::TimedOut, "sendfile timed out")
                            })?
                    }?;
                    readiness.clear_ready();
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::server) fn enabled(&self) -> io::Result<bool> {
        match self.socket() {
            Socket::Tcp(socket) => socket.nodelay(),
            #[cfg(unix)]
            Socket::Unix(_) => Ok(false),
        }
    }

    fn socket(&self) -> &Socket {
        #[cfg(unix)]
        {
            self.inner.socket.get_ref()
        }
        #[cfg(not(unix))]
        {
            &self.inner.socket
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

use super::*;
use tokio::io::AsyncBufReadExt;

#[derive(Debug, Clone, Copy)]
pub(in crate::server) struct ConnectionCloseConfig {
    pub lingering_close: LingeringClose,
    pub lingering_time: Duration,
    pub lingering_timeout: Duration,
    pub reset_timedout_connection: bool,
}

impl Default for ConnectionCloseConfig {
    fn default() -> Self {
        Self {
            lingering_close: LingeringClose::On,
            lingering_time: Duration::from_secs(30),
            lingering_timeout: Duration::from_secs(5),
            reset_timedout_connection: false,
        }
    }
}

impl From<&EffectiveConfig> for ConnectionCloseConfig {
    fn from(context: &EffectiveConfig) -> Self {
        Self {
            lingering_close: context.lingering_close,
            lingering_time: context.lingering_time,
            lingering_timeout: context.lingering_timeout,
            reset_timedout_connection: context.reset_timedout_connection,
        }
    }
}

impl From<&Arc<EffectiveConfig>> for ConnectionCloseConfig {
    fn from(context: &Arc<EffectiveConfig>) -> Self {
        Self::from(context.as_ref())
    }
}

pub(in crate::server) async fn lingering_close<R>(reader: &mut R, config: ConnectionCloseConfig)
where
    R: AsyncBufRead + Unpin,
{
    if !should_linger(reader, config).await {
        return;
    }

    let started = Instant::now();
    loop {
        let remaining = config.lingering_time.saturating_sub(started.elapsed());
        let wait = config.lingering_timeout.min(remaining);
        if wait == Duration::ZERO {
            break;
        }
        match tokio::time::timeout(wait, reader.fill_buf()).await {
            Ok(Ok([])) => break,
            Ok(Ok(buffer)) => {
                let length = buffer.len();
                reader.consume(length);
            }
            Ok(Err(_)) | Err(_) => break,
        }
    }
}

async fn should_linger<R>(reader: &mut R, config: ConnectionCloseConfig) -> bool
where
    R: AsyncBufRead + Unpin,
{
    match config.lingering_close {
        LingeringClose::Off => false,
        LingeringClose::Always => true,
        LingeringClose::On => matches!(
            tokio::time::timeout(Duration::from_millis(1), reader.fill_buf()).await,
            Ok(Ok(buffer)) if !buffer.is_empty()
        ),
    }
}

pub(in crate::server) trait TimeoutResetConnection {
    fn set_reset_on_close(&self) -> io::Result<()>;
}

pub(in crate::server) struct TimeoutReset {
    connection: Box<dyn TimeoutResetConnection + Send + Sync>,
}

impl TimeoutReset {
    pub(in crate::server) fn from_control(control: ClientSocketControl) -> Self {
        Self::new(control)
    }

    pub(in crate::server) fn set_reset_on_close(&self) -> io::Result<()> {
        self.connection.set_reset_on_close()
    }

    fn new<T>(connection: T) -> Self
    where
        T: TimeoutResetConnection + Send + Sync + 'static,
    {
        Self {
            connection: Box::new(connection),
        }
    }
}

impl TimeoutResetConnection for TcpStream {
    fn set_reset_on_close(&self) -> io::Result<()> {
        socket2::SockRef::from(self).set_linger(Some(Duration::ZERO))
    }
}

impl TimeoutResetConnection for socket2::Socket {
    fn set_reset_on_close(&self) -> io::Result<()> {
        self.set_linger(Some(Duration::ZERO))
    }
}

impl TimeoutResetConnection for ClientSocketControl {
    fn set_reset_on_close(&self) -> io::Result<()> {
        ClientSocketControl::set_reset_on_close(self)
    }
}

#[cfg(unix)]
impl TimeoutResetConnection for UnixStream {
    fn set_reset_on_close(&self) -> io::Result<()> {
        socket2::SockRef::from(self).set_linger(Some(Duration::ZERO))
    }
}

impl TimeoutResetConnection for tokio::io::DuplexStream {
    fn set_reset_on_close(&self) -> io::Result<()> {
        Ok(())
    }
}

impl<S> TimeoutResetConnection for tokio_rustls::server::TlsStream<S>
where
    S: TimeoutResetConnection,
{
    fn set_reset_on_close(&self) -> io::Result<()> {
        self.get_ref().0.set_reset_on_close()
    }
}

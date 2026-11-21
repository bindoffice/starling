use std::future::Future;

use super::*;

#[derive(Debug, Clone, Copy)]
pub(in crate::server) struct StreamRelayOutcome {
    pub(in crate::server) first_byte: Option<Duration>,
    pub(in crate::server) response: Duration,
    pub(in crate::server) success: bool,
    pub(in crate::server) status: u16,
    pub(in crate::server) bytes_sent: u64,
    pub(in crate::server) bytes_received: u64,
}

pub(in crate::server) fn format_stream_duration(duration: Option<Duration>) -> String {
    match duration {
        Some(duration) => format!("{}.{:03}", duration.as_secs(), duration.subsec_millis()),
        None => "-".into(),
    }
}

pub(in crate::server) struct TimedDuplex<'a, S> {
    inner: &'a mut S,
    started: Instant,
    first_read: Option<Duration>,
    bytes_read: u64,
    rate: u64,
    scratch: Vec<u8>,
    delay: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<'a, S> TimedDuplex<'a, S> {
    fn new(inner: &'a mut S, started: Instant, rate: u64, buffer_size: usize) -> Self {
        Self {
            inner,
            started,
            first_read: None,
            bytes_read: 0,
            rate,
            scratch: vec![0_u8; buffer_size],
            delay: None,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for TimedDuplex<'_, S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(delay) = this.delay.as_mut() {
            match delay.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(()) => this.delay = None,
            }
        }

        let limit = if this.rate == 0 {
            buffer.remaining()
        } else {
            buffer
                .remaining()
                .min(this.rate.min(usize::MAX as u64) as usize)
        }
        .min(this.scratch.len());
        if limit == 0 {
            return Poll::Ready(Ok(()));
        }

        let TimedDuplex { inner, scratch, .. } = this;
        let mut local = ReadBuf::new(&mut scratch[..limit]);
        match Pin::new(&mut **inner).poll_read(cx, &mut local) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
            Poll::Ready(Ok(())) => {}
        }
        let read = local.filled().len();
        if read == 0 {
            return Poll::Ready(Ok(()));
        }
        buffer.put_slice(local.filled());
        if this.first_read.is_none() {
            this.first_read = Some(this.started.elapsed());
        }
        this.bytes_read += read as u64;
        if let Some(delay) = stream_rate_delay(read, this.rate) {
            this.delay = Some(Box::pin(tokio::time::sleep(delay)));
        }
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for TimedDuplex<'_, S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.get_mut().inner).poll_write(cx, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.get_mut().inner).poll_shutdown(cx)
    }
}

pub(in crate::server) async fn relay_stream<A, B>(
    inbound: &mut A,
    outbound: &mut B,
    timeout: Duration,
    settings: StreamProxySettings,
    remote: &ListenAddress,
    logger: &Logger,
) -> StreamRelayOutcome
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let started = Instant::now();
    let mut inbound =
        TimedDuplex::new(inbound, started, settings.upload_rate, settings.buffer_size);
    let mut outbound = TimedDuplex::new(
        outbound,
        started,
        settings.download_rate,
        settings.buffer_size,
    );
    let result = tokio::time::timeout(
        timeout,
        copy_stream_bidirectional(&mut inbound, &mut outbound, settings.half_close),
    )
    .await;
    let response = started.elapsed();
    let bytes_received = inbound.bytes_read;
    let bytes_sent = outbound.bytes_read;
    match result {
        Ok(Ok(_)) => StreamRelayOutcome {
            first_byte: outbound.first_read,
            response,
            success: true,
            status: 200,
            bytes_sent,
            bytes_received,
        },
        Ok(Err(err)) => {
            logger.log(
                LogLevel::Info,
                format!("stream proxy for {remote} failed: {err}"),
            );
            StreamRelayOutcome {
                first_byte: outbound.first_read,
                response,
                success: false,
                status: 502,
                bytes_sent,
                bytes_received,
            }
        }
        Err(_) => {
            logger.log(
                LogLevel::Info,
                format!("stream proxy for {remote} timed out"),
            );
            StreamRelayOutcome {
                first_byte: outbound.first_read,
                response,
                success: false,
                status: 504,
                bytes_sent,
                bytes_received,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct CapturingReader {
        requested: Arc<AtomicUsize>,
    }

    impl AsyncRead for CapturingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            self.requested
                .fetch_max(buffer.remaining(), Ordering::Relaxed);
            let size = buffer.remaining().min(8);
            buffer.put_slice(&[b'x'; 8][..size]);
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn timed_duplex_uses_configured_buffer_size() {
        let requested = Arc::new(AtomicUsize::new(0));
        let mut reader = CapturingReader {
            requested: requested.clone(),
        };
        let mut timed = TimedDuplex::new(&mut reader, Instant::now(), 0, 4 * 1024);
        let mut output = vec![0_u8; 8 * 1024];

        let read = timed.read(&mut output).await.unwrap();

        assert_eq!(read, 8);
        assert_eq!(requested.load(Ordering::Relaxed), 4 * 1024);
    }
}

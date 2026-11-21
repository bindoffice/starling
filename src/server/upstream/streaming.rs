use super::body_window::{BodyWindow, spawn_windowed_body};
use super::*;
use std::sync::Arc;
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;

pub(super) const STREAM_CHANNEL_CAPACITY: usize = 16;
pub(super) const STREAM_READ_SIZE: usize = 16 * 1024;

pub(in crate::server) fn can_stream_response(
    context: &EffectiveConfig,
    status: u16,
    headers: &[Header],
) -> bool {
    let accel = (!ignores_accel_buffering(&context.proxy_ignore_headers))
        .then(|| parse_accel_buffering(headers))
        .flatten();
    let unbuffered = match accel {
        Some(AccelBuffering::No) => true,
        Some(AccelBuffering::Yes) => false,
        None => !context.proxy_buffering.enabled,
    };
    if !unbuffered {
        return false;
    }
    response_can_stream(context, status, headers)
}

pub(in crate::server) fn can_prestream_buffered_response(
    context: &EffectiveConfig,
    status: u16,
    headers: &[Header],
) -> bool {
    context.proxy_buffering.enabled && response_can_stream(context, status, headers)
}

pub(in crate::server) fn buffered_stream_threshold(context: &EffectiveConfig) -> u64 {
    context.proxy_buffering.resolved_busy_buffers_size() as u64
}

fn response_can_stream(context: &EffectiveConfig, status: u16, headers: &[Header]) -> bool {
    if !matches!(context.proxy_http_version.as_str(), "1.1" | "2")
        || context.proxy_force_ranges
        || response_filters_require_buffering(context)
    {
        return false;
    }
    if intercepts_upstream_status(
        status,
        should_intercept_error_pages(true, context),
        &context.error_pages,
    ) {
        return false;
    }
    let ignored = |name: &str| upstream_header_ignored(&context.proxy_ignore_headers, name);
    !headers.iter().any(|header| {
        (!ignored("X-Accel-Redirect")
            && header.name.eq_ignore_ascii_case("X-Accel-Redirect")
            && !header.value.is_empty())
            || (!ignored("X-Accel-Charset")
                && header.name.eq_ignore_ascii_case("X-Accel-Charset")
                && !header.value.is_empty())
    })
}

pub(in crate::server) fn response_filters_require_buffering(context: &EffectiveConfig) -> bool {
    context.gzip
        || context.gunzip
        || context.charset.charset.is_some()
        || !context.sub_filters.is_empty()
        || context.addition_before.is_some()
        || context.addition_after.is_some()
        || context.ssi.enabled
}

pub(in crate::server) fn spawn_http1_body_stream(
    connection: UpstreamConnection,
    head: &UpstreamResponseHead,
    read_timeout: Duration,
    limit_rate: u64,
    reuse: Option<tokio::sync::oneshot::Sender<UpstreamConnection>>,
) -> ResponseBody {
    if head.body_kind == UpstreamBodyKind::None {
        return ResponseBody::Empty;
    }
    let length = match head.body_kind {
        UpstreamBodyKind::ContentLength(length) => Some(length),
        UpstreamBodyKind::Chunked | UpstreamBodyKind::UntilEof => None,
        UpstreamBodyKind::None => Some(0),
    };
    let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let sink = StreamSender::Channel(sender.clone());
    let kind = head.body_kind;
    let UpstreamConnection {
        mut reader,
        key,
        created,
        idle_since,
        requests,
    } = connection;
    tokio::spawn(run_channel_producer(sender, async move {
        let mut rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
        let result = pump_body(&mut reader, kind, read_timeout, rate.as_mut(), &sink).await;
        if result.is_ok() {
            return_stream_connection(reuse, reader, key, created, idle_since, requests);
        }
        result
    }));
    ResponseBody::Stream(ResponseStream::new(
        receiver,
        length,
        kind == UpstreamBodyKind::Chunked,
    ))
}

pub(super) fn return_stream_connection(
    reuse: Option<tokio::sync::oneshot::Sender<UpstreamConnection>>,
    reader: tokio::io::BufReader<BoxedUpstreamIo>,
    key: UpstreamPoolKey,
    created: Instant,
    idle_since: Instant,
    requests: u64,
) {
    if let Some(reuse) = reuse
        && reader.buffer().is_empty()
    {
        let _ = reuse.send(UpstreamConnection {
            reader,
            key,
            created,
            idle_since,
            requests,
        });
    }
}

async fn pump_body<R>(
    reader: &mut R,
    kind: UpstreamBodyKind,
    timeout: Duration,
    mut rate: Option<&mut ReadRate>,
    sender: &StreamSender,
) -> io::Result<()>
where
    R: AsyncBufReadExt + AsyncReadExt + Unpin,
{
    match kind {
        UpstreamBodyKind::None => Ok(()),
        UpstreamBodyKind::ContentLength(length) => {
            pump_content_length(reader, length, timeout, rate.as_deref_mut(), sender).await
        }
        UpstreamBodyKind::Chunked => {
            pump_chunked(reader, timeout, rate.as_deref_mut(), sender).await
        }
        UpstreamBodyKind::UntilEof => pump_until_eof(reader, timeout, rate, sender).await,
    }
}

async fn pump_content_length<R>(
    reader: &mut R,
    length: u64,
    timeout: Duration,
    mut rate: Option<&mut ReadRate>,
    sender: &StreamSender,
) -> io::Result<()>
where
    R: AsyncReadExt + Unpin,
{
    let mut remaining = length;
    let mut buffer = vec![0_u8; STREAM_READ_SIZE];
    while remaining > 0 {
        let chunk_len = remaining.min(buffer.len() as u64) as usize;
        let read = with_read_timeout(
            timeout,
            reader.read(&mut buffer[..chunk_len]),
            "upstream response body timed out",
        )
        .await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upstream response ended before Content-Length",
            ));
        }
        send_data(sender, &buffer[..read], rate.as_deref_mut()).await?;
        remaining -= read as u64;
    }
    Ok(())
}

async fn pump_until_eof<R>(
    reader: &mut R,
    timeout: Duration,
    mut rate: Option<&mut ReadRate>,
    sender: &StreamSender,
) -> io::Result<()>
where
    R: AsyncReadExt + Unpin,
{
    let mut buffer = vec![0_u8; STREAM_READ_SIZE];
    loop {
        let read = with_read_timeout(
            timeout,
            reader.read(&mut buffer),
            "upstream response body timed out",
        )
        .await?;
        if read == 0 {
            return Ok(());
        }
        send_data(sender, &buffer[..read], rate.as_deref_mut()).await?;
    }
}

async fn pump_chunked<R>(
    reader: &mut R,
    timeout: Duration,
    mut rate: Option<&mut ReadRate>,
    sender: &StreamSender,
) -> io::Result<()>
where
    R: AsyncBufReadExt + AsyncReadExt + Unpin,
{
    loop {
        let mut line = String::new();
        let read = with_read_timeout(
            timeout,
            reader.read_line(&mut line),
            "upstream response body timed out",
        )
        .await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected end of chunked body",
            ));
        }
        let size_text = line
            .trim_end_matches(['\r', '\n'])
            .split(';')
            .next()
            .unwrap_or_default();
        let size = u64::from_str_radix(size_text.trim(), 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size"))?;
        if size == 0 {
            let trailers = with_read_timeout(
                timeout,
                crate::http::read_trailers(reader),
                "upstream response body timed out",
            )
            .await?;
            send_trailers(sender, trailers).await?;
            return Ok(());
        }

        let mut remaining = size;
        let mut buffer = vec![0_u8; STREAM_READ_SIZE];
        while remaining > 0 {
            let chunk_len = remaining.min(buffer.len() as u64) as usize;
            let read = with_read_timeout(
                timeout,
                reader.read(&mut buffer[..chunk_len]),
                "upstream response body timed out",
            )
            .await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "upstream chunk ended before its declared size",
                ));
            }
            send_data(sender, &buffer[..read], rate.as_deref_mut()).await?;
            remaining -= read as u64;
        }
        let mut crlf = [0_u8; 2];
        with_read_timeout(
            timeout,
            reader.read_exact(&mut crlf),
            "upstream response body timed out",
        )
        .await?;
        if crlf != *b"\r\n" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid chunk terminator",
            ));
        }
    }
}

pub(super) async fn send_data(
    sender: &StreamSender,
    data: &[u8],
    rate: Option<&mut ReadRate>,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if let Some(rate) = rate {
        rate.account(data.len() as u64).await;
    }
    sender.data(Bytes::copy_from_slice(data)).await
}

pub(super) async fn send_trailers(sender: &StreamSender, trailers: Vec<Header>) -> io::Result<()> {
    if trailers.is_empty() {
        return Ok(());
    }
    sender.trailers(trailers).await
}

pub(super) fn spawn_buffered_stream<F>(
    options: UpstreamBodyBufferOptions,
    length: Option<u64>,
    may_have_trailers: bool,
    producer: impl FnOnce(StreamSender) -> F,
) -> ResponseBody
where
    F: Future<Output = io::Result<()>> + Send + 'static,
{
    let window = Arc::new(BodyWindow::new(options));
    let sender = StreamSender::Window(window.clone());
    spawn_windowed_body(window, length, may_have_trailers, producer(sender))
}

pub(super) async fn run_channel_producer<F>(
    sender: mpsc::Sender<io::Result<ResponseStreamItem>>,
    producer: F,
) where
    F: Future<Output = io::Result<()>>,
{
    tokio::pin!(producer);
    tokio::select! {
        biased;
        _ = sender.closed() => {}
        result = &mut producer => {
            if let Err(error) = result {
                let _ = sender.send(Err(error)).await;
            }
        }
    }
}

#[derive(Clone)]
pub(super) enum StreamSender {
    Channel(mpsc::Sender<io::Result<ResponseStreamItem>>),
    Window(Arc<BodyWindow>),
}

impl StreamSender {
    async fn data(&self, data: Bytes) -> io::Result<()> {
        match self {
            Self::Channel(sender) => sender
                .send(Ok(ResponseStreamItem::Data(data)))
                .await
                .map_err(closed_stream),
            Self::Window(window) => window.push_data(data).await,
        }
    }

    async fn trailers(&self, trailers: Vec<Header>) -> io::Result<()> {
        match self {
            Self::Channel(sender) => sender
                .send(Ok(ResponseStreamItem::Trailers(trailers)))
                .await
                .map_err(closed_stream),
            Self::Window(window) => window.push_trailers(trailers).await,
        }
    }
}

fn closed_stream(_: mpsc::error::SendError<io::Result<ResponseStreamItem>>) -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "downstream body stream closed")
}

pub(super) async fn with_read_timeout<T>(
    timeout: Duration,
    future: impl std::future::Future<Output = io::Result<T>>,
    timeout_message: &'static str,
) -> io::Result<T> {
    if timeout == Duration::ZERO {
        return future.await;
    }
    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, timeout_message)),
    }
}

#[cfg(test)]
mod tests;

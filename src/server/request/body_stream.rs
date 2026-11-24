use super::*;
use crate::http::{RequestBodySender, RequestBodyStream, RequestHead};
use crate::server::http2::H2ConnectionFault;
use h2::RecvStream;
use tokio::io::{AsyncBufReadExt, ReadHalf};
use tokio::task::JoinHandle;

const BODY_CHUNK_SIZE: usize = 16 * 1024;

#[derive(Clone, Copy)]
enum BodyFraming {
    ContentLength(u64),
    Chunked,
}

pub(in crate::server) struct Http1BodyTask<S>
where
    S: AsyncRead + Send + 'static,
{
    stream: RequestBodyStream,
    task: JoinHandle<(BufReader<ReadHalf<S>>, io::Result<()>)>,
}

impl<S> Http1BodyTask<S>
where
    S: AsyncRead + Send + 'static,
{
    pub(in crate::server) fn spawn(
        reader: BufReader<ReadHalf<S>>,
        head: &RequestHead,
        timeout: Duration,
        limit: u64,
    ) -> Option<Self> {
        let framing = if head.chunked {
            BodyFraming::Chunked
        } else {
            BodyFraming::ContentLength(head.content_length.filter(|length| *length != 0)?)
        };
        let (sender, stream) = RequestBodyStream::channel();
        let task = tokio::spawn(async move {
            let mut reader = reader;
            let result = pump_body(&mut reader, framing, timeout, limit, &sender).await;
            if let Err(error) = &result {
                sender
                    .fail(crate::http::clone_request_body_error(error))
                    .await;
            } else {
                sender.finish();
            }
            (reader, result)
        });
        Some(Self { stream, task })
    }

    pub(in crate::server) fn stream(&self) -> RequestBodyStream {
        self.stream.clone()
    }

    pub(in crate::server) async fn close(&self) {
        self.stream.close().await;
    }

    pub(in crate::server) async fn finish(
        self,
    ) -> io::Result<(BufReader<ReadHalf<S>>, io::Result<()>)> {
        self.task
            .await
            .map_err(|error| io::Error::other(error.to_string()))
    }

    pub(in crate::server) fn abort(self) {
        self.task.abort();
    }
}

pub(in crate::server) struct H2BodyTask {
    stream: RequestBodyStream,
    task: JoinHandle<io::Result<()>>,
}

impl H2BodyTask {
    #[cfg(test)]
    pub(in crate::server) fn spawn(body: RecvStream, timeout: Duration, limit: u64) -> Self {
        Self::spawn_with_connection_fault_and_length(body, timeout, limit, None, None)
    }

    pub(in crate::server) fn spawn_with_connection_fault_and_length(
        body: RecvStream,
        timeout: Duration,
        limit: u64,
        connection_fault: Option<H2ConnectionFault>,
        content_length: Option<u64>,
    ) -> Self {
        let (sender, stream, window) = RequestBodyStream::channel_with_h2_window();
        let task = tokio::spawn(async move {
            let result = pump_h2_body(
                body,
                timeout,
                limit,
                content_length,
                &sender,
                connection_fault.as_ref(),
                window,
            )
            .await;
            if let Err(error) = &result {
                sender
                    .fail(crate::http::clone_request_body_error(error))
                    .await;
            }
            result
        });
        Self { stream, task }
    }

    pub(in crate::server) fn stream(&self) -> RequestBodyStream {
        self.stream.clone()
    }

    pub(in crate::server) async fn close(&self) {
        self.stream.close().await;
    }

    pub(in crate::server) fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub(in crate::server) async fn finish(self) -> io::Result<()> {
        self.task
            .await
            .map_err(|error| io::Error::other(error.to_string()))?
    }

    pub(in crate::server) fn detach(self) {
        // Dropping a Tokio JoinHandle detaches the body pump. The receiver has
        // already been closed, so the pump discards remaining DATA while still
        // observing protocol-level events such as request trailers.
        drop(self);
    }
}

async fn pump_h2_body(
    mut body: RecvStream,
    timeout: Duration,
    limit: u64,
    content_length: Option<u64>,
    sender: &RequestBodySender,
    connection_fault: Option<&H2ConnectionFault>,
    mut window: tokio::sync::watch::Receiver<Option<u32>>,
) -> io::Result<()> {
    let mut total = 0_u64;
    let mut active = true;
    loop {
        let data = match wait_for_h2_data(&mut body, timeout, &mut window).await? {
            Some(data) => data,
            None => break,
        };
        let Some(data) = data else {
            break;
        };
        let mut next_data = Some(data.map_err(h2_body_io_error)?);

        // Nginx presents DATA frames received in one read event as one request
        // body chunk. Drain frames that are already buffered without waiting
        // for another network event, while preserving flow-control accounting.
        let mut chunks = Vec::new();
        while let Some(data) = next_data.take() {
            release_h2_body_capacity(&mut body, data.len())?;
            let new_total = total
                .checked_add(data.len() as u64)
                .ok_or_else(crate::http::request_body_too_large_error)?;
            if limit != 0 && new_total > limit {
                send_h2_chunks(sender, chunks, &mut active).await;
                return Err(crate::http::request_body_too_large_error());
            }
            if content_length.is_some_and(|expected| new_total > expected) {
                send_h2_chunks(sender, chunks, &mut active).await;
                return Err(content_length_mismatch_error());
            }
            total = new_total;
            chunks.push(data);
            next_data = match poll_h2_data_now(&mut body).await {
                Some(Some(data)) => Some(data.map_err(h2_body_io_error)?),
                _ => None,
            };
        }
        let end_stream = body.is_end_stream();
        let content_length_mismatch =
            end_stream && content_length.is_some_and(|expected| total != expected);
        if content_length_mismatch {
            return Err(content_length_mismatch_error());
        }
        if end_stream {
            sender.finish();
        }
        send_h2_chunks(sender, chunks, &mut active).await;
    }

    let trailers = if timeout == Duration::ZERO {
        body.trailers().await
    } else {
        match tokio::time::timeout(timeout, body.trailers()).await {
            Ok(trailers) => trailers,
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "request body timed out",
                ));
            }
        }
    };
    if trailers.map_err(h2_body_io_error)?.is_some() {
        if let Some(fault) = connection_fault {
            fault.protocol_error();
        }
        return Err(crate::http::malformed_request_body_error(
            "request trailers are not supported",
        ));
    }
    if content_length.is_some_and(|expected| total != expected) {
        return Err(content_length_mismatch_error());
    }
    sender.finish();
    Ok(())
}

async fn send_h2_chunks(sender: &RequestBodySender, chunks: Vec<Bytes>, active: &mut bool) {
    let data = if chunks.len() == 1 {
        chunks.into_iter().next().expect("one request body chunk")
    } else {
        let length = chunks.iter().map(Bytes::len).sum();
        if length == 0 {
            return;
        }
        let mut combined = Vec::with_capacity(length);
        for chunk in chunks {
            combined.extend_from_slice(&chunk);
        }
        Bytes::from(combined)
    };
    if !data.is_empty() {
        send_or_discard(sender, &data, active).await;
    }
}

async fn wait_for_h2_data(
    body: &mut RecvStream,
    timeout: Duration,
    window: &mut tokio::sync::watch::Receiver<Option<u32>>,
) -> io::Result<Option<Option<Result<Bytes, h2::Error>>>> {
    let next = async {
        let mut window_open = true;
        loop {
            apply_h2_window_target(body, window)?;
            tokio::select! {
                changed = window.changed(), if window_open => {
                    if changed.is_err() {
                        window_open = false;
                    }
                }
                data = body.data() => {
                    return Ok::<Option<Result<Bytes, h2::Error>>, io::Error>(data);
                }
            }
        }
    };
    if timeout == Duration::ZERO {
        return Ok(Some(next.await?));
    }
    match tokio::time::timeout(timeout, next).await {
        Ok(data) => Ok(Some(data?)),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "request body timed out",
        )),
    }
}

fn apply_h2_window_target(
    body: &mut RecvStream,
    window: &tokio::sync::watch::Receiver<Option<u32>>,
) -> io::Result<()> {
    let target = *window.borrow();
    if let Some(target) = target {
        body.set_target_window_size(target)
            .map_err(io::Error::other)?;
    }
    Ok(())
}

async fn poll_h2_data_now(body: &mut RecvStream) -> Option<Option<Result<Bytes, h2::Error>>> {
    std::future::poll_fn(|context| {
        std::task::Poll::Ready(match body.poll_data(context) {
            std::task::Poll::Ready(data) => Some(data),
            std::task::Poll::Pending => None,
        })
    })
    .await
}

fn release_h2_body_capacity(body: &mut RecvStream, length: usize) -> io::Result<()> {
    body.flow_control()
        .release_capacity(length)
        .map_err(io::Error::other)
}

fn content_length_mismatch_error() -> io::Error {
    crate::http::malformed_request_body_error("request body length does not match Content-Length")
}

async fn pump_body<R>(
    reader: &mut R,
    framing: BodyFraming,
    timeout: Duration,
    limit: u64,
    sender: &RequestBodySender,
) -> io::Result<()>
where
    R: AsyncBufRead + Unpin,
{
    let mut reader = TimedReader::new(reader, timeout);
    match framing {
        BodyFraming::ContentLength(length) => {
            if limit != 0 && length > limit {
                return Err(crate::http::request_body_too_large_error());
            }
            pump_content_length(&mut reader, length, sender).await
        }
        BodyFraming::Chunked => pump_chunked(&mut reader, limit, sender).await,
    }
}

async fn pump_content_length<R>(
    reader: &mut R,
    length: u64,
    sender: &RequestBodySender,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut remaining = length;
    let mut active = true;
    let mut buffer = vec![0_u8; BODY_CHUNK_SIZE];
    while remaining > 0 {
        let wanted = remaining.min(buffer.len() as u64) as usize;
        let read = reader.read(&mut buffer[..wanted]).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request body ended before Content-Length",
            ));
        }
        send_or_discard(sender, &buffer[..read], &mut active).await;
        remaining -= read as u64;
    }
    Ok(())
}

async fn pump_chunked<R>(reader: &mut R, limit: u64, sender: &RequestBodySender) -> io::Result<()>
where
    R: AsyncBufRead + Unpin,
{
    let mut total = 0_u64;
    let mut active = true;
    let mut buffer = vec![0_u8; BODY_CHUNK_SIZE];
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected end of chunked request body",
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
            read_trailers(reader).await?;
            return Ok(());
        }
        total = total
            .checked_add(size)
            .ok_or_else(crate::http::request_body_too_large_error)?;
        if limit != 0 && total > limit {
            return Err(crate::http::request_body_too_large_error());
        }
        let mut remaining = size;
        while remaining > 0 {
            let wanted = remaining.min(buffer.len() as u64) as usize;
            let read = reader.read(&mut buffer[..wanted]).await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "chunk ended before its declared size",
                ));
            }
            send_or_discard(sender, &buffer[..read], &mut active).await;
            remaining -= read as u64;
        }
        let mut crlf = [0_u8; 2];
        reader.read_exact(&mut crlf).await?;
        if crlf != *b"\r\n" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid chunk terminator",
            ));
        }
    }
}

async fn read_trailers<R>(reader: &mut R) -> io::Result<()>
where
    R: AsyncBufRead + Unpin,
{
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "chunked request body ended before trailers completed",
            ));
        }
        if line == "\r\n" || line == "\n" {
            return Ok(());
        }
        let line = line.trim_end_matches(['\r', '\n']);
        let Some((name, _)) = line.split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid chunked trailer",
            ));
        };
        if name.is_empty() || !name.bytes().all(is_token_byte) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid chunked trailer",
            ));
        }
    }
}

async fn send_or_discard(sender: &RequestBodySender, data: &[u8], active: &mut bool) {
    if *active && !sender.send(Bytes::copy_from_slice(data)).await {
        *active = false;
    }
}

fn h2_body_io_error(error: h2::Error) -> io::Error {
    if error.is_reset() || error.is_go_away() {
        return io::Error::new(io::ErrorKind::ConnectionAborted, error);
    }
    if error.is_io() {
        return error
            .into_io()
            .unwrap_or_else(|| io::Error::new(io::ErrorKind::ConnectionAborted, "HTTP/2 I/O"));
    }
    io::Error::other(error)
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

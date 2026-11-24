use super::*;
use crate::http::{RequestBodySender, RequestBodyStream, RequestHead};
use tokio::io::{AsyncBufReadExt, ReadHalf};
use tokio::task::JoinHandle;

mod h2;

pub(in crate::server) use h2::H2BodyTask;

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
        mut reader: BufReader<ReadHalf<S>>,
        head: &RequestHead,
        timeout: Duration,
        limit: u64,
    ) -> Option<Self> {
        let mut framing = if head.chunked {
            BodyFraming::Chunked
        } else {
            BodyFraming::ContentLength(head.content_length.filter(|length| *length != 0)?)
        };
        let initial = if let BodyFraming::ContentLength(length) = framing {
            let available = reader.buffer().len().min(length as usize);
            if available == 0 {
                None
            } else {
                let data = Bytes::copy_from_slice(&reader.buffer()[..available]);
                reader.consume(available);
                framing = BodyFraming::ContentLength(length - available as u64);
                Some(data)
            }
        } else {
            None
        };
        let (sender, stream) = RequestBodyStream::channel();
        let initial = match initial {
            Some(data) if sender.try_send(data.clone()) => None,
            initial => initial,
        };
        let task = tokio::spawn(async move {
            let mut reader = reader;
            let result = pump_body(&mut reader, framing, timeout, limit, &sender, initial).await;
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

async fn pump_body<R>(
    reader: &mut R,
    framing: BodyFraming,
    timeout: Duration,
    limit: u64,
    sender: &RequestBodySender,
    initial: Option<Bytes>,
) -> io::Result<()>
where
    R: AsyncBufRead + Unpin,
{
    let mut reader = TimedReader::new(reader, timeout);
    if let Some(initial) = initial {
        let mut active = true;
        send_or_discard(sender, &initial, &mut active).await;
    }
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
    let mut combined = Vec::with_capacity(BODY_CHUNK_SIZE);
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
            if let Err(error) = read_trailers(reader).await {
                flush_chunk_prefix(sender, &mut combined, &mut active).await;
                return Err(error);
            }
            flush_chunk_prefix(sender, &mut combined, &mut active).await;
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
                flush_chunk_prefix(sender, &mut combined, &mut active).await;
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "chunk ended before its declared size",
                ));
            }
            combined.extend_from_slice(&buffer[..read]);
            if combined.len() >= BODY_CHUNK_SIZE {
                send_or_discard(sender, &combined, &mut active).await;
                combined.clear();
            }
            remaining -= read as u64;
        }
        let mut crlf = [0_u8; 2];
        if let Err(error) = reader.read_exact(&mut crlf).await {
            flush_chunk_prefix(sender, &mut combined, &mut active).await;
            return Err(error);
        }
        if crlf != *b"\r\n" {
            flush_chunk_prefix(sender, &mut combined, &mut active).await;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid chunk terminator",
            ));
        }
        if !has_complete_buffered_chunk(reader).await && !combined.is_empty() {
            send_or_discard(sender, &combined, &mut active).await;
            combined.clear();
        }
    }
}

async fn flush_chunk_prefix(sender: &RequestBodySender, combined: &mut Vec<u8>, active: &mut bool) {
    if !combined.is_empty() {
        send_or_discard(sender, combined, active).await;
        combined.clear();
    }
}

async fn has_complete_buffered_chunk<R>(reader: &mut R) -> bool
where
    R: AsyncBufRead + Unpin,
{
    std::future::poll_fn(|context| {
        let mut reader = std::pin::Pin::new(&mut *reader);
        match reader.as_mut().poll_fill_buf(context) {
            std::task::Poll::Ready(Ok(buffer)) => {
                std::task::Poll::Ready(buffered_chunk_complete(buffer))
            }
            _ => std::task::Poll::Ready(false),
        }
    })
    .await
}

fn buffered_chunk_complete(buffer: &[u8]) -> bool {
    let Some(line_end) = buffer.iter().position(|byte| *byte == b'\n') else {
        return false;
    };
    let line = buffer[..line_end]
        .strip_suffix(b"\r")
        .unwrap_or(&buffer[..line_end]);
    let size_text = line.split(|byte| *byte == b';').next().unwrap_or_default();
    let Ok(size_text) = std::str::from_utf8(size_text) else {
        return false;
    };
    let Ok(size) = usize::from_str_radix(size_text.trim(), 16) else {
        return false;
    };
    let data_start = line_end + 1;
    let Some(data_end) = data_start.checked_add(size) else {
        return false;
    };
    data_end
        .checked_add(2)
        .is_some_and(|end| end <= buffer.len())
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

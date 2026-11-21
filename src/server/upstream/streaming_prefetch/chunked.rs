use super::super::streaming::{
    STREAM_READ_SIZE, StreamSender, return_stream_connection, send_data, send_trailers,
    spawn_buffered_stream, with_read_timeout,
};
use super::super::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt};

#[derive(Clone, Copy)]
enum ChunkedReadState {
    NeedSize,
    Data { remaining: u64 },
    NeedCrlf,
}

pub(in crate::server) struct PrefetchedChunkedStream {
    prefix: Vec<u8>,
    state: ChunkedReadState,
    rate: Option<ReadRate>,
}

pub(in crate::server) enum ChunkedPrefetch {
    Complete {
        body: Vec<u8>,
        trailers: Vec<Header>,
    },
    Stream(PrefetchedChunkedStream),
}

pub(in crate::server) async fn prefetch_chunked<R>(
    reader: &mut R,
    threshold: u64,
    timeout: Duration,
    mut rate: Option<ReadRate>,
) -> io::Result<ChunkedPrefetch>
where
    R: AsyncBufReadExt + AsyncReadExt + Unpin,
{
    let mut body = Vec::new();
    let mut state = ChunkedReadState::NeedSize;
    loop {
        if body.len() as u64 > threshold {
            return Ok(ChunkedPrefetch::Stream(PrefetchedChunkedStream {
                prefix: body,
                state,
                rate,
            }));
        }
        match state {
            ChunkedReadState::NeedSize => {
                let size = read_chunk_size(reader, timeout).await?;
                if size == 0 {
                    let trailers = with_read_timeout(
                        timeout,
                        crate::http::read_trailers(reader),
                        "upstream response body timed out",
                    )
                    .await?;
                    return Ok(ChunkedPrefetch::Complete { body, trailers });
                }
                state = ChunkedReadState::Data { remaining: size };
            }
            ChunkedReadState::Data { remaining } => {
                let wanted = threshold
                    .saturating_sub(body.len() as u64)
                    .saturating_add(1)
                    .min(remaining);
                read_append(
                    reader,
                    wanted,
                    &mut body,
                    timeout,
                    rate.as_mut(),
                    "upstream chunk ended before its declared size",
                )
                .await?;
                state = if wanted == remaining {
                    ChunkedReadState::NeedCrlf
                } else {
                    ChunkedReadState::Data {
                        remaining: remaining - wanted,
                    }
                };
            }
            ChunkedReadState::NeedCrlf => {
                read_chunk_crlf(reader, timeout).await?;
                state = ChunkedReadState::NeedSize;
            }
        }
    }
}

pub(in crate::server) fn spawn_prefetched_chunked_stream(
    connection: UpstreamConnection,
    prefetched: PrefetchedChunkedStream,
    timeout: Duration,
    buffer_options: UpstreamBodyBufferOptions,
    reuse: Option<tokio::sync::oneshot::Sender<UpstreamConnection>>,
) -> ResponseBody {
    let UpstreamConnection {
        reader,
        key,
        created,
        idle_since,
        requests,
    } = connection;
    spawn_buffered_stream(buffer_options, None, true, move |sender| async move {
        let mut reader = reader;
        let result = pump_prefetched_chunked(&mut reader, prefetched, timeout, &sender).await;
        if result.is_ok() {
            return_stream_connection(reuse, reader, key, created, idle_since, requests);
        }
        result
    })
}

async fn pump_prefetched_chunked<R>(
    reader: &mut R,
    prefetched: PrefetchedChunkedStream,
    timeout: Duration,
    sender: &StreamSender,
) -> io::Result<()>
where
    R: AsyncBufReadExt + AsyncReadExt + Unpin,
{
    let PrefetchedChunkedStream {
        prefix,
        mut state,
        mut rate,
    } = prefetched;
    send_data(sender, &prefix, None).await?;
    loop {
        match state {
            ChunkedReadState::NeedSize => {
                let size = read_chunk_size(reader, timeout).await?;
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
                state = ChunkedReadState::Data { remaining: size };
            }
            ChunkedReadState::Data { remaining } => {
                send_content_length_data(
                    reader,
                    remaining,
                    timeout,
                    rate.as_mut(),
                    sender,
                    "upstream chunk ended before its declared size",
                )
                .await?;
                state = ChunkedReadState::NeedCrlf;
            }
            ChunkedReadState::NeedCrlf => {
                read_chunk_crlf(reader, timeout).await?;
                state = ChunkedReadState::NeedSize;
            }
        }
    }
}

async fn read_chunk_size<R>(reader: &mut R, timeout: Duration) -> io::Result<u64>
where
    R: AsyncBufReadExt + Unpin,
{
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
    u64::from_str_radix(size_text.trim(), 16)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size"))
}

async fn read_chunk_crlf<R>(reader: &mut R, timeout: Duration) -> io::Result<()>
where
    R: AsyncReadExt + Unpin,
{
    let mut crlf = [0_u8; 2];
    with_read_timeout(
        timeout,
        reader.read_exact(&mut crlf),
        "upstream response body timed out",
    )
    .await?;
    if crlf == *b"\r\n" {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid chunk terminator",
        ))
    }
}

async fn read_append<R>(
    reader: &mut R,
    length: u64,
    body: &mut Vec<u8>,
    timeout: Duration,
    mut rate: Option<&mut ReadRate>,
    eof_message: &'static str,
) -> io::Result<()>
where
    R: AsyncReadExt + Unpin,
{
    let mut remaining = length;
    while remaining > 0 {
        let wanted = remaining.min(STREAM_READ_SIZE as u64) as usize;
        let start = body.len();
        body.resize(start + wanted, 0);
        let read = match with_read_timeout(
            timeout,
            reader.read(&mut body[start..]),
            "upstream response body timed out",
        )
        .await
        {
            Ok(read) => read,
            Err(error) => {
                body.truncate(start);
                return Err(error);
            }
        };
        if read == 0 {
            body.truncate(start);
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, eof_message));
        }
        body.truncate(start + read);
        if let Some(rate) = rate.as_deref_mut() {
            rate.account(read as u64).await;
        }
        remaining -= read as u64;
    }
    Ok(())
}

async fn send_content_length_data<R>(
    reader: &mut R,
    length: u64,
    timeout: Duration,
    mut rate: Option<&mut ReadRate>,
    sender: &StreamSender,
    eof_message: &'static str,
) -> io::Result<()>
where
    R: AsyncReadExt + Unpin,
{
    let mut remaining = length;
    let mut buffer = vec![0_u8; STREAM_READ_SIZE];
    while remaining > 0 {
        let wanted = remaining.min(buffer.len() as u64) as usize;
        let read = with_read_timeout(
            timeout,
            reader.read(&mut buffer[..wanted]),
            "upstream response body timed out",
        )
        .await?;
        if read == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, eof_message));
        }
        send_data(sender, &buffer[..read], rate.as_deref_mut()).await?;
        remaining -= read as u64;
    }
    Ok(())
}

use super::*;
use std::sync::Arc;

mod body_reader;
mod buffer;
mod buffering;
mod head;
pub(crate) use body_reader::read_trailers;
pub use buffer::*;
pub use buffering::*;
pub use head::*;

#[derive(Debug)]
pub struct UpstreamResponse {
    pub status: u16,
    pub version: String,
    pub headers: Vec<Header>,
    pub trailers: Vec<Header>,
    pub early_hints: Vec<Vec<Header>>,
    pub body: ResponseBody,
    pub body_temp_file: Option<Arc<TempFileGuard>>,
}

pub async fn read_upstream_response<R: AsyncBufReadExt + AsyncReadExt + Unpin>(
    reader: &mut R,
) -> io::Result<UpstreamResponse> {
    read_upstream_response_timed(reader)
        .await
        .map(|(response, _)| response)
}

pub async fn read_upstream_response_timed<R: AsyncBufReadExt + AsyncReadExt + Unpin>(
    reader: &mut R,
) -> io::Result<(UpstreamResponse, Duration)> {
    read_upstream_response_timed_with_rate(reader, 0).await
}

pub async fn read_upstream_response_timed_with_rate<R: AsyncBufReadExt + AsyncReadExt + Unpin>(
    reader: &mut R,
    limit_rate: u64,
) -> io::Result<(UpstreamResponse, Duration)> {
    read_upstream_response_timed_with_rate_and_header_limit(reader, limit_rate, MAX_HEADER_BYTES)
        .await
}

pub async fn read_upstream_response_timed_with_rate_and_header_limit<
    R: AsyncBufReadExt + AsyncReadExt + Unpin,
>(
    reader: &mut R,
    limit_rate: u64,
    max_header_bytes: usize,
) -> io::Result<(UpstreamResponse, Duration)> {
    read_upstream_response_timed_with_options(
        reader,
        limit_rate,
        max_header_bytes,
        UpstreamBodyBufferOptions::memory(MAX_BODY_BYTES),
    )
    .await
}

pub async fn read_upstream_response_timed_with_options<
    R: AsyncBufReadExt + AsyncReadExt + Unpin,
>(
    reader: &mut R,
    limit_rate: u64,
    max_header_bytes: usize,
    buffer_options: UpstreamBodyBufferOptions,
) -> io::Result<(UpstreamResponse, Duration)> {
    read_upstream_response_timed_with_policy(
        reader,
        limit_rate,
        max_header_bytes,
        UpstreamBodyBufferPolicy::fixed(buffer_options),
    )
    .await
}

pub async fn read_upstream_response_timed_with_policy<R: AsyncBufReadExt + AsyncReadExt + Unpin>(
    reader: &mut R,
    limit_rate: u64,
    max_header_bytes: usize,
    buffer_policy: UpstreamBodyBufferPolicy,
) -> io::Result<(UpstreamResponse, Duration)> {
    let head = read_upstream_response_head(reader, max_header_bytes).await?;
    let header_time = head.header_time;
    let response = read_upstream_response_body(reader, head, limit_rate, buffer_policy).await?;
    Ok((response, header_time))
}

pub async fn read_upstream_response_body<R: AsyncBufReadExt + AsyncReadExt + Unpin>(
    reader: &mut R,
    head: UpstreamResponseHead,
    limit_rate: u64,
    buffer_policy: UpstreamBodyBufferPolicy,
) -> io::Result<UpstreamResponse> {
    let mut read_rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
    let mut body_buffer = BodyBuffer::new(buffer_policy.options_for_headers(&head.headers));
    let trailers = match head.body_kind {
        UpstreamBodyKind::None => Vec::new(),
        UpstreamBodyKind::Chunked => {
            body_reader::read_chunked_body(reader, &mut body_buffer, read_rate.as_mut()).await?
        }
        UpstreamBodyKind::ContentLength(length) => {
            body_reader::read_content_length_body(
                reader,
                length,
                &mut body_buffer,
                read_rate.as_mut(),
            )
            .await?;
            Vec::new()
        }
        UpstreamBodyKind::UntilEof => {
            body_reader::read_to_end_body(reader, &mut body_buffer, read_rate.as_mut()).await?;
            Vec::new()
        }
    };
    let buffered = if head.body_kind == UpstreamBodyKind::None {
        BufferedUpstreamBody {
            body: ResponseBody::Empty,
            temp_file: None,
        }
    } else {
        body_buffer.finish().await?
    };
    Ok(UpstreamResponse {
        status: head.status,
        version: head.version,
        headers: head.headers,
        trailers,
        early_hints: head.early_hints,
        body: buffered.body,
        body_temp_file: buffered.temp_file,
    })
}

pub async fn read_request<R: AsyncBufReadExt + AsyncReadExt + Unpin>(
    reader: &mut R,
    remote_addr: Arc<str>,
    local_addr: Arc<str>,
    header_timeout: Duration,
    max_body: u64,
) -> io::Result<Option<Request>> {
    let head = match tokio::time::timeout(header_timeout, read_request_head(reader)).await {
        Ok(result) => result?,
        Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "header timeout")),
    };
    let Some(head) = head else {
        return Ok(None);
    };
    let body = tokio::time::timeout(
        Duration::from_secs(60),
        read_request_body(reader, &head, max_body),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "body timeout"))??;
    Ok(Some(Request {
        method: head.method,
        target: head.target,
        version: head.version,
        headers: head.headers.into(),
        body,
        body_stream: None,
        remote_addr,
        local_addr,
        keep_alive: head.keep_alive,
    }))
}

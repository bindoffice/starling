use super::*;
use crate::config::UpstreamBufferingConfig;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;

pub(in crate::server) struct GatewayStreamingPolicy<'a> {
    pub(in crate::server) buffering: &'a UpstreamBufferingConfig,
    pub(in crate::server) ignore_headers: &'a [String],
    pub(in crate::server) force_ranges: bool,
    pub(in crate::server) intercept_errors: bool,
}

pub(in crate::server) enum GatewayResponseRead<R> {
    Buffered {
        response: Response,
        reader: R,
        protocol_complete: bool,
    },
    Streamed(Response),
}

#[derive(Debug, Clone, Copy)]
pub(in crate::server) enum GatewayBodyHandling {
    DirectStream,
    BufferedStream { threshold: usize },
    CompleteBuffer,
}

pub(in crate::server) async fn read_cgi_response<R>(
    mut reader: R,
    limit_rate: u64,
    max_header_bytes: usize,
    buffer_policy: UpstreamBodyBufferPolicy,
    streaming: Option<&GatewayStreamingPolicy<'_>>,
    context: &EffectiveConfig,
    read_timeout: Duration,
) -> io::Result<GatewayResponseRead<R>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut start = read_cgi_head(&mut reader, max_header_bytes, read_timeout).await?;
    truncate_prefix_to_content_length(&mut start.body_prefix, start.head.content_length());
    let handling = streaming
        .map(|policy| {
            gateway_body_handling(context, policy, start.head.status(), start.head.headers())
        })
        .unwrap_or(GatewayBodyHandling::CompleteBuffer);
    match handling {
        GatewayBodyHandling::DirectStream => {
            let body = spawn_cgi_body_stream(
                reader,
                start.body_prefix,
                start.head.content_length(),
                read_timeout,
                limit_rate,
            );
            Ok(GatewayResponseRead::Streamed(
                start.head.into_response(body, None)?,
            ))
        }
        GatewayBodyHandling::BufferedStream { threshold } => {
            read_buffered_cgi_response(
                reader,
                start,
                threshold,
                limit_rate,
                read_timeout,
                buffer_policy,
            )
            .await
        }
        GatewayBodyHandling::CompleteBuffer => {
            let response =
                finish_cgi_response(&mut reader, start, limit_rate, buffer_policy, read_timeout)
                    .await?;
            Ok(GatewayResponseRead::Buffered {
                response,
                reader,
                protocol_complete: false,
            })
        }
    }
}

pub(in crate::server) fn gateway_body_handling(
    context: &EffectiveConfig,
    policy: &GatewayStreamingPolicy<'_>,
    status: u16,
    headers: &[Header],
) -> GatewayBodyHandling {
    let accel = (!ignores_accel_buffering(policy.ignore_headers))
        .then(|| parse_accel_buffering(headers))
        .flatten();
    let buffered = match accel {
        Some(AccelBuffering::No) => false,
        Some(AccelBuffering::Yes) => true,
        None => policy.buffering.enabled,
    };
    let ignored = |name: &str| upstream_header_ignored(policy.ignore_headers, name);
    if policy.force_ranges
        || response_filters_require_buffering(context)
        || intercepts_upstream_status(status, policy.intercept_errors, &context.error_pages)
        || headers.iter().any(|header| {
            (!ignored("X-Accel-Redirect")
                && header.name.eq_ignore_ascii_case("X-Accel-Redirect")
                && !header.value.is_empty())
                || (!ignored("X-Accel-Charset")
                    && header.name.eq_ignore_ascii_case("X-Accel-Charset")
                    && !header.value.is_empty())
        })
    {
        return GatewayBodyHandling::CompleteBuffer;
    }
    if buffered {
        GatewayBodyHandling::BufferedStream {
            threshold: policy.buffering.resolved_busy_buffers_size(),
        }
    } else {
        GatewayBodyHandling::DirectStream
    }
}

async fn read_buffered_cgi_response<R>(
    mut reader: R,
    start: CgiHeadStart,
    threshold: usize,
    limit_rate: u64,
    read_timeout: Duration,
    buffer_policy: UpstreamBodyBufferPolicy,
) -> io::Result<GatewayResponseRead<R>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let length = start.head.content_length();
    let buffered_options = buffer_policy.options_for_headers(start.head.headers());
    let mut prefix = start.body_prefix;
    let mut rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
    if let Some(rate) = rate.as_mut() {
        rate.account(prefix.len() as u64).await;
    }
    if length.is_some_and(|length| prefix.len() as u64 == length) {
        let response = start
            .head
            .into_response(ResponseBody::Bytes(prefix), None)?;
        return Ok(GatewayResponseRead::Buffered {
            response,
            reader,
            protocol_complete: false,
        });
    }
    if length.is_some_and(|length| length > threshold as u64) || prefix.len() > threshold {
        let body = spawn_buffered_cgi_body_stream(
            reader,
            prefix,
            length,
            read_timeout,
            limit_rate,
            buffered_options,
        );
        return Ok(GatewayResponseRead::Streamed(
            start.head.into_response(body, None)?,
        ));
    }

    let mut read_buffer = vec![0_u8; STREAM_READ_SIZE];
    loop {
        if prefix.len() > threshold {
            let body = spawn_buffered_cgi_body_stream(
                reader,
                prefix,
                length,
                read_timeout,
                limit_rate,
                buffered_options,
            );
            return Ok(GatewayResponseRead::Streamed(
                start.head.into_response(body, None)?,
            ));
        }
        let mut wanted = threshold
            .saturating_sub(prefix.len())
            .saturating_add(1)
            .min(read_buffer.len());
        if let Some(length) = length {
            wanted = wanted.min((length - prefix.len() as u64) as usize);
        }
        let read = with_read_timeout(
            read_timeout,
            reader.read(&mut read_buffer[..wanted]),
            "gateway response body timed out",
        )
        .await?;
        if read == 0 {
            if length.is_some_and(|length| prefix.len() as u64 != length) {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "CGI response ended before Content-Length",
                ));
            }
            let response = start
                .head
                .into_response(ResponseBody::Bytes(prefix), None)?;
            return Ok(GatewayResponseRead::Buffered {
                response,
                reader,
                protocol_complete: false,
            });
        }
        prefix.extend_from_slice(&read_buffer[..read]);
        if let Some(rate) = rate.as_mut() {
            rate.account(read as u64).await;
        }
        if length.is_some_and(|length| prefix.len() as u64 == length) {
            let response = start
                .head
                .into_response(ResponseBody::Bytes(prefix), None)?;
            return Ok(GatewayResponseRead::Buffered {
                response,
                reader,
                protocol_complete: false,
            });
        }
    }
}

fn spawn_cgi_body_stream<R>(
    mut reader: R,
    prefix: Vec<u8>,
    length: Option<u64>,
    read_timeout: Duration,
    limit_rate: u64,
) -> ResponseBody
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let sink = StreamSender::Channel(sender.clone());
    tokio::spawn(run_channel_producer(sender, async move {
        let mut rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
        if let Some(rate) = rate.as_mut() {
            rate.account(prefix.len() as u64).await;
        }
        pump_cgi_body(
            &mut reader,
            &prefix,
            length,
            read_timeout,
            rate.as_mut(),
            &sink,
        )
        .await
    }));
    ResponseBody::Stream(ResponseStream::new(receiver, length, false))
}

fn spawn_buffered_cgi_body_stream<R>(
    mut reader: R,
    prefix: Vec<u8>,
    length: Option<u64>,
    read_timeout: Duration,
    limit_rate: u64,
    buffer_options: UpstreamBodyBufferOptions,
) -> ResponseBody
where
    R: AsyncRead + Unpin + Send + 'static,
{
    spawn_buffered_stream(buffer_options, length, false, move |sender| async move {
        let mut rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
        pump_cgi_body(
            &mut reader,
            &prefix,
            length,
            read_timeout,
            rate.as_mut(),
            &sender,
        )
        .await
    })
}

async fn pump_cgi_body<R>(
    reader: &mut R,
    prefix: &[u8],
    length: Option<u64>,
    read_timeout: Duration,
    mut rate: Option<&mut ReadRate>,
    sender: &StreamSender,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let prefix_length = prefix.len() as u64;
    if length.is_some_and(|length| prefix_length > length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "CGI response body exceeds Content-Length",
        ));
    }
    send_data(sender, prefix, rate.as_deref_mut()).await?;

    match length {
        Some(length) => {
            let mut remaining = length - prefix_length;
            let mut buffer = vec![0_u8; STREAM_READ_SIZE];
            while remaining > 0 {
                let size = remaining.min(buffer.len() as u64) as usize;
                let read = with_read_timeout(
                    read_timeout,
                    reader.read(&mut buffer[..size]),
                    "gateway response body timed out",
                )
                .await?;
                if read == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "CGI response ended before Content-Length",
                    ));
                }
                send_data(sender, &buffer[..read], rate.as_deref_mut()).await?;
                remaining -= read as u64;
            }
        }
        None => {
            let mut buffer = vec![0_u8; STREAM_READ_SIZE];
            loop {
                let read = with_read_timeout(
                    read_timeout,
                    reader.read(&mut buffer),
                    "gateway response body timed out",
                )
                .await?;
                if read == 0 {
                    break;
                }
                send_data(sender, &buffer[..read], rate.as_deref_mut()).await?;
            }
        }
    }
    Ok(())
}

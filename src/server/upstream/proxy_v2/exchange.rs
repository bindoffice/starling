use super::headers::{H2ResponseHead, read_h2_response_head};
use super::response::{H2ResponseReadOptions, read_h2_response};
use super::*;
use crate::http::RequestBodyStream;
use crate::http::is_forbidden_trailer_name;

#[derive(Clone, Copy)]
pub(in crate::server) enum H2StreamingPolicy<'a> {
    Proxy(&'a EffectiveConfig),
    Grpc,
}

impl H2StreamingPolicy<'_> {
    pub(super) fn allows(self, status: u16, headers: &[Header]) -> bool {
        match self {
            Self::Proxy(context) => can_stream_response(context, status, headers),
            Self::Grpc => true,
        }
    }

    pub(super) fn allows_prestream(self, status: u16, headers: &[Header]) -> bool {
        match self {
            Self::Proxy(context) => can_prestream_buffered_response(context, status, headers),
            Self::Grpc => false,
        }
    }
}

pub(in crate::server) struct H2ExchangeOptions<'a> {
    pub(in crate::server) send_timeout: Duration,
    pub(in crate::server) read_timeout: Duration,
    pub(in crate::server) buffer_policy: UpstreamBodyBufferPolicy,
    pub(in crate::server) limit_rate: u64,
    pub(in crate::server) streaming: Option<H2StreamingPolicy<'a>>,
}

pub(in crate::server) async fn timed_h2_exchange(
    connection: &mut ProxyV2Connection,
    request: http::Request<()>,
    body: &[u8],
    body_stream: Option<&RequestBodyStream>,
    body_limit: u64,
    options: H2ExchangeOptions<'_>,
) -> Result<(UpstreamResponse, Duration), ProxyFailure> {
    let H2ExchangeOptions {
        send_timeout,
        read_timeout,
        buffer_policy,
        limit_rate,
        streaming,
    } = options;
    let mut sender =
        match tokio::time::timeout(send_timeout, connection.sender.clone().ready()).await {
            Ok(Ok(sender)) => sender,
            Ok(Err(_)) => return Err(ProxyFailure::Failure),
            Err(_) => return Err(ProxyFailure::Timeout),
        };
    let end_stream = body.is_empty() && body_stream.is_none();
    let (response_future, mut stream) = sender
        .send_request(request, end_stream)
        .map_err(|_| ProxyFailure::InvalidHeader)?;
    connection.requests = connection.requests.saturating_add(1);
    if !end_stream {
        send_h2_body(&mut stream, body, body_stream, body_limit, send_timeout).await?;
    }

    let started = Instant::now();
    let response =
        match tokio::time::timeout(read_timeout, read_h2_response_head(response_future)).await {
            Ok(Ok(response)) => response,
            Ok(Err(failure)) => return Err(failure),
            Err(_) => return Err(ProxyFailure::Timeout),
        };
    let header_time = started.elapsed();
    let H2ResponseHead {
        parts,
        body,
        early_hints,
    } = response;
    let driver = connection.driver_guard();
    read_h2_response(
        body,
        H2ResponseReadOptions {
            parts,
            early_hints,
            buffer_policy,
            limit_rate,
            header_time,
            read_timeout,
            streaming,
            driver,
        },
    )
    .await
}

async fn send_h2_body(
    stream: &mut h2::SendStream<Bytes>,
    body: &[u8],
    body_stream: Option<&RequestBodyStream>,
    body_limit: u64,
    timeout: Duration,
) -> Result<(), ProxyFailure> {
    if let Some(body_stream) = body_stream {
        let mut total = 0_u64;
        while let Some(chunk) = body_stream
            .next_limited(&mut total, body_limit)
            .await
            .map_err(proxy_failure_from_io)?
        {
            send_h2_chunk(stream, &chunk, false, timeout).await?;
        }
        send_h2_request_trailers(stream, body_stream).await?;
        return Ok(());
    }
    send_h2_chunk(stream, body, true, timeout).await
}

async fn send_h2_request_trailers(
    stream: &mut h2::SendStream<Bytes>,
    body_stream: &RequestBodyStream,
) -> Result<(), ProxyFailure> {
    let trailers = body_stream.take_trailers().await;
    let mut fields = http::HeaderMap::new();
    for trailer in trailers {
        if is_forbidden_trailer_name(&trailer.name) {
            continue;
        }
        let Some(name) = crate::server::http2::h2_header_name(&trailer.name) else {
            continue;
        };
        let Ok(value) = crate::server::http2::h2_header_value(&trailer.value) else {
            continue;
        };
        fields.append(name, value);
    }
    if fields.is_empty() {
        stream
            .send_data(Bytes::new(), true)
            .map_err(|_| ProxyFailure::Failure)
    } else {
        stream
            .send_trailers(fields)
            .map_err(|_| ProxyFailure::Failure)
    }
}

async fn send_h2_chunk(
    stream: &mut h2::SendStream<Bytes>,
    body: &[u8],
    last: bool,
    timeout: Duration,
) -> Result<(), ProxyFailure> {
    let mut offset = 0;
    while offset < body.len() {
        stream.reserve_capacity(body.len() - offset);
        let capacity = match tokio::time::timeout(
            timeout,
            std::future::poll_fn(|context| stream.poll_capacity(context)),
        )
        .await
        {
            Ok(Some(Ok(capacity))) if capacity > 0 => capacity,
            Ok(Some(Ok(_))) | Ok(None) | Ok(Some(Err(_))) => {
                return Err(ProxyFailure::Failure);
            }
            Err(_) => return Err(ProxyFailure::Timeout),
        };
        let end = offset.saturating_add(capacity).min(body.len());
        let final_chunk = last && end == body.len();
        stream
            .send_data(Bytes::copy_from_slice(&body[offset..end]), final_chunk)
            .map_err(|_| ProxyFailure::Failure)?;
        offset = end;
    }
    Ok(())
}

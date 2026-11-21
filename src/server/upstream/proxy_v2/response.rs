use super::connection::ProxyV2DriverGuard;
use super::headers::convert_headers;
use super::streaming::{spawn_h2_body_stream, spawn_prefetched_h2_body_stream};
use super::streaming_prefetch::{
    H2UntilEndPrefetch, next_h2_data, prefetch_h2_content_length, prefetch_h2_until_end,
    read_h2_trailers, release_capacity,
};
use super::*;
use crate::http::upstream_body_kind;

pub(super) struct H2ResponseReadOptions<'a> {
    pub(super) parts: http::response::Parts,
    pub(super) early_hints: Vec<Vec<Header>>,
    pub(super) buffer_policy: UpstreamBodyBufferPolicy,
    pub(super) limit_rate: u64,
    pub(super) header_time: Duration,
    pub(super) read_timeout: Duration,
    pub(super) streaming: Option<H2StreamingPolicy<'a>>,
    pub(super) driver: ProxyV2DriverGuard,
}

pub(super) async fn read_h2_response(
    mut stream: h2::RecvStream,
    options: H2ResponseReadOptions<'_>,
) -> Result<(UpstreamResponse, Duration), ProxyFailure> {
    let H2ResponseReadOptions {
        parts,
        early_hints,
        buffer_policy,
        limit_rate,
        header_time,
        read_timeout,
        streaming,
        driver,
    } = options;
    let status = parts.status.as_u16();
    let headers = convert_headers(&parts.headers)?;
    let buffer_options = buffer_policy.options_for_headers(&headers);
    if streaming.is_some_and(|policy| policy.allows(status, &headers)) {
        let body_kind = upstream_body_kind(status, &headers);
        let length = match body_kind {
            UpstreamBodyKind::None => Some(0),
            UpstreamBodyKind::ContentLength(length) => Some(length),
            UpstreamBodyKind::Chunked | UpstreamBodyKind::UntilEof => None,
        };
        let body = if body_kind == UpstreamBodyKind::None {
            ResponseBody::Empty
        } else {
            spawn_h2_body_stream(stream, length, read_timeout, limit_rate, driver)
        };
        return Ok((
            UpstreamResponse {
                status,
                version: "HTTP/2.0".into(),
                headers,
                trailers: Vec::new(),
                early_hints,
                body,
                body_temp_file: None,
            },
            header_time,
        ));
    }
    if streaming.is_some_and(|policy| policy.allows_prestream(status, &headers)) {
        let threshold = buffered_stream_threshold(match streaming.expect("prestream policy") {
            H2StreamingPolicy::Proxy(context) => context,
            H2StreamingPolicy::Grpc => unreachable!("gRPC does not prestream"),
        });
        match upstream_body_kind(status, &headers) {
            UpstreamBodyKind::ContentLength(length) if length > threshold => {
                let rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
                let prefetched =
                    prefetch_h2_content_length(&mut stream, length, threshold, read_timeout, rate)
                        .await
                        .map_err(proxy_failure_from_io)?;
                let body = spawn_prefetched_h2_body_stream(
                    stream,
                    prefetched,
                    Some(length),
                    read_timeout,
                    driver,
                    buffer_options.clone(),
                );
                return Ok((
                    UpstreamResponse {
                        status,
                        version: "HTTP/2.0".into(),
                        headers,
                        trailers: Vec::new(),
                        early_hints,
                        body,
                        body_temp_file: None,
                    },
                    header_time,
                ));
            }
            UpstreamBodyKind::Chunked | UpstreamBodyKind::UntilEof => {
                let rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
                match prefetch_h2_until_end(&mut stream, threshold, read_timeout, rate)
                    .await
                    .map_err(proxy_failure_from_io)?
                {
                    H2UntilEndPrefetch::Complete { body, trailers } => {
                        return Ok((
                            UpstreamResponse {
                                status,
                                version: "HTTP/2.0".into(),
                                headers,
                                trailers,
                                early_hints,
                                body: ResponseBody::Bytes(body),
                                body_temp_file: None,
                            },
                            header_time,
                        ));
                    }
                    H2UntilEndPrefetch::Stream(prefetched) => {
                        let body = spawn_prefetched_h2_body_stream(
                            stream,
                            prefetched,
                            None,
                            read_timeout,
                            driver,
                            buffer_options.clone(),
                        );
                        return Ok((
                            UpstreamResponse {
                                status,
                                version: "HTTP/2.0".into(),
                                headers,
                                trailers: Vec::new(),
                                early_hints,
                                body,
                                body_temp_file: None,
                            },
                            header_time,
                        ));
                    }
                }
            }
            UpstreamBodyKind::ContentLength(_) | UpstreamBodyKind::None => {}
        }
    }
    let mut body = BodyBuffer::new(buffer_options);
    let mut rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
    while let Some(chunk) = next_h2_data(&mut stream, read_timeout)
        .await
        .map_err(|_| ProxyFailure::InvalidHeader)?
    {
        let chunk_len = chunk.len();
        if let Some(rate) = rate.as_mut() {
            rate.account(chunk_len as u64).await;
        }
        body.append(&chunk)
            .await
            .map_err(|_| ProxyFailure::InvalidHeader)?;
        release_capacity(&mut stream, chunk_len).map_err(|_| ProxyFailure::InvalidHeader)?;
    }
    let trailers = read_h2_trailers(&mut stream, read_timeout)
        .await
        .map_err(|_| ProxyFailure::InvalidHeader)?;
    let body = body
        .finish()
        .await
        .map_err(|_| ProxyFailure::InvalidHeader)?;
    Ok((
        UpstreamResponse {
            status,
            version: "HTTP/2.0".into(),
            headers,
            trailers,
            early_hints,
            body: body.body,
            body_temp_file: body.temp_file,
        },
        header_time,
    ))
}

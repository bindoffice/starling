use super::*;

mod reuse;
mod write;

pub(in crate::server) use reuse::{
    can_reuse_proxy_connection, header_has_token, spawn_stream_connection_reuse,
};
use reuse::{can_reuse_stream_head, deferred_stream_connection};
#[cfg(test)]
pub(in crate::server) use write::write_proxy_request;
use write::write_proxy_request_timed;

pub(in crate::server) async fn open_proxy_upstream(
    proxy: &ProxyPassConfig,
    host: &str,
    port: u16,
    context: &EffectiveConfig,
    variables: &Variables,
    key: UpstreamPoolKey,
) -> Result<UpstreamConnection, ProxyFailure> {
    let io = open_proxy_upstream_io(proxy, host, port, context, variables, &[]).await?;
    Ok(UpstreamConnection::new(io, key))
}

pub(in crate::server) async fn open_proxy_upstream_io(
    proxy: &ProxyPassConfig,
    host: &str,
    port: u16,
    context: &EffectiveConfig,
    variables: &Variables,
    alpn_protocols: &[Vec<u8>],
) -> Result<BoxedUpstreamIo, ProxyFailure> {
    let socket_options = UpstreamSocketOptions::for_proxy(context);
    let local_bind = evaluate_proxy_bind(context, variables);
    if let Some(path) = host.strip_prefix("unix:") {
        #[cfg(unix)]
        {
            let connect = tokio::net::UnixStream::connect(path);
            let stream = match tokio::time::timeout(context.proxy_connect_timeout, connect).await {
                Ok(Ok(stream)) => stream,
                Ok(Err(_)) => return Err(ProxyFailure::Failure),
                Err(_) => return Err(ProxyFailure::Timeout),
            };
            if apply_upstream_socket_options(&stream, socket_options).is_err() {
                return Err(ProxyFailure::Failure);
            }
            let io: BoxedUpstreamIo = if proxy.scheme == "https" {
                let stream = match tokio::time::timeout(
                    context.proxy_connect_timeout,
                    upgrade_proxy_tls_with_alpn(stream, host, context, variables, alpn_protocols),
                )
                .await
                {
                    Ok(Ok(stream)) => stream,
                    Ok(Err(_)) => return Err(ProxyFailure::Failure),
                    Err(_) => return Err(ProxyFailure::Timeout),
                };
                Box::new(stream)
            } else {
                Box::new(stream)
            };
            return Ok(io);
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            return Err(ProxyFailure::Failure);
        }
    }
    let connect = connect_upstream_tcp(
        host,
        port,
        local_bind.as_deref(),
        socket_options,
        "proxy_bind",
    );
    let stream = match tokio::time::timeout(context.proxy_connect_timeout, connect).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(_)) => return Err(ProxyFailure::Failure),
        Err(_) => return Err(ProxyFailure::Timeout),
    };
    let io: BoxedUpstreamIo = if proxy.scheme == "https" {
        let stream = match tokio::time::timeout(
            context.proxy_connect_timeout,
            upgrade_proxy_tls_with_alpn(stream, host, context, variables, alpn_protocols),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) => return Err(ProxyFailure::Failure),
            Err(_) => return Err(ProxyFailure::Timeout),
        };
        Box::new(stream)
    } else {
        Box::new(stream)
    };
    Ok(io)
}

pub(in crate::server) async fn timed_connection_proxy_exchange(
    mut connection: UpstreamConnection,
    outgoing: &ProxyRequestMessage,
    version: &str,
    context: &EffectiveConfig,
    limit_rate: u64,
    streaming_allowed: bool,
    stream_reuse_allowed: bool,
) -> Result<ConnectionProxyOutcome, ProxyFailure> {
    connection.requests = connection.requests.saturating_add(1);
    let write_result = write_proxy_request_timed(
        &mut connection.reader,
        outgoing,
        version,
        Some(context.proxy_send_timeout),
    )
    .await;
    let mut write_error = None;
    let write_incomplete = match write_result {
        Ok(()) => false,
        Err(error) if crate::http::request_body_error_status(&error).is_some() => {
            return Err(proxy_failure_from_io(error));
        }
        Err(error) if outgoing.body_stream.is_none() => {
            return Err(proxy_failure_from_io(error));
        }
        Err(error) => {
            write_error = Some(error);
            true
        }
    };
    let head_result = {
        let mut reader = TimedReader::new(&mut connection.reader, context.proxy_read_timeout);
        read_upstream_response_head(&mut reader, context.proxy_buffering.buffer_size).await
    };
    let mut head = match head_result {
        Ok(head) => head,
        Err(error) => return Err(proxy_failure_from_io(write_error.unwrap_or(error))),
    };
    if outgoing.method.eq_ignore_ascii_case("HEAD") {
        head.body_kind = UpstreamBodyKind::None;
    }
    let header_time = head.header_time;
    if head.status == 101 {
        return Ok(ConnectionProxyOutcome {
            response: UpstreamResponse {
                status: head.status,
                version: head.version,
                headers: head.headers,
                trailers: Vec::new(),
                early_hints: head.early_hints,
                body: ResponseBody::Empty,
                body_temp_file: None,
            },
            reusable: None,
            deferred: None,
            upgrade: Some(connection),
            header_time,
        });
    }
    if has_accel_redirect(&head.headers, context) {
        // nginx follows X-Accel-Redirect from the response headers and does
        // not wait for the upstream body. The connection cannot be reused
        // because the unread body would otherwise corrupt the next response.
        return Ok(ConnectionProxyOutcome {
            response: UpstreamResponse {
                status: head.status,
                version: head.version,
                headers: head.headers,
                trailers: Vec::new(),
                early_hints: head.early_hints,
                body: ResponseBody::Empty,
                body_temp_file: None,
            },
            reusable: None,
            deferred: None,
            upgrade: None,
            header_time,
        });
    }
    let stream_reuse = stream_reuse_allowed
        && !write_incomplete
        && can_reuse_stream_head(outgoing, version, &head);
    let body_streaming_allowed = streaming_allowed && !outgoing.method.eq_ignore_ascii_case("HEAD");
    if body_streaming_allowed
        && head.body_kind != UpstreamBodyKind::None
        && can_stream_response(context, head.status, &head.headers)
    {
        let (reuse_tx, reuse_rx) = deferred_stream_connection(stream_reuse);
        let body = spawn_http1_body_stream(
            connection,
            &head,
            context.proxy_read_timeout,
            limit_rate,
            reuse_tx,
        );
        return Ok(ConnectionProxyOutcome {
            response: UpstreamResponse {
                status: head.status,
                version: head.version,
                headers: head.headers,
                trailers: Vec::new(),
                early_hints: head.early_hints,
                body,
                body_temp_file: None,
            },
            reusable: None,
            deferred: reuse_rx,
            upgrade: None,
            header_time,
        });
    }
    if body_streaming_allowed
        && can_prestream_buffered_response(context, head.status, &head.headers)
    {
        let threshold = buffered_stream_threshold(context);
        match head.body_kind {
            UpstreamBodyKind::ContentLength(length) if length > threshold => {
                let rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
                let (prefix, rate) = streaming_prefetch::prefetch_content_length(
                    &mut connection.reader,
                    length,
                    threshold,
                    context.proxy_read_timeout,
                    rate,
                )
                .await
                .map_err(proxy_failure_from_io)?;
                let (reuse_tx, reuse_rx) = deferred_stream_connection(stream_reuse);
                let body = streaming_prefetch::spawn_prefetched_content_length_stream(
                    connection,
                    prefix,
                    length,
                    context.proxy_read_timeout,
                    rate,
                    UpstreamBodyBufferOptions::upstream(&context.proxy_buffering),
                    reuse_tx,
                );
                return Ok(ConnectionProxyOutcome {
                    response: UpstreamResponse {
                        status: head.status,
                        version: head.version,
                        headers: head.headers,
                        trailers: Vec::new(),
                        early_hints: head.early_hints,
                        body,
                        body_temp_file: None,
                    },
                    reusable: None,
                    deferred: reuse_rx,
                    upgrade: None,
                    header_time,
                });
            }
            UpstreamBodyKind::Chunked => {
                let rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
                match streaming_prefetch::prefetch_chunked(
                    &mut connection.reader,
                    threshold,
                    context.proxy_read_timeout,
                    rate,
                )
                .await
                .map_err(proxy_failure_from_io)?
                {
                    streaming_prefetch::ChunkedPrefetch::Complete { body, trailers } => {
                        let reusable = connection.reader.buffer().is_empty();
                        return Ok(ConnectionProxyOutcome {
                            response: UpstreamResponse {
                                status: head.status,
                                version: head.version,
                                headers: head.headers,
                                trailers,
                                early_hints: head.early_hints,
                                body: ResponseBody::Bytes(body),
                                body_temp_file: None,
                            },
                            reusable: reusable.then_some(connection),
                            deferred: None,
                            upgrade: None,
                            header_time,
                        });
                    }
                    streaming_prefetch::ChunkedPrefetch::Stream(prefetched) => {
                        let (reuse_tx, reuse_rx) = deferred_stream_connection(stream_reuse);
                        let body = streaming_prefetch::spawn_prefetched_chunked_stream(
                            connection,
                            prefetched,
                            context.proxy_read_timeout,
                            UpstreamBodyBufferOptions::upstream(&context.proxy_buffering),
                            reuse_tx,
                        );
                        return Ok(ConnectionProxyOutcome {
                            response: UpstreamResponse {
                                status: head.status,
                                version: head.version,
                                headers: head.headers,
                                trailers: Vec::new(),
                                early_hints: head.early_hints,
                                body,
                                body_temp_file: None,
                            },
                            reusable: None,
                            deferred: reuse_rx,
                            upgrade: None,
                            header_time,
                        });
                    }
                }
            }
            UpstreamBodyKind::UntilEof => {
                let rate = (limit_rate != 0).then(|| ReadRate::new(limit_rate));
                match streaming_prefetch::prefetch_until_eof(
                    &mut connection.reader,
                    threshold,
                    context.proxy_read_timeout,
                    rate,
                )
                .await
                .map_err(proxy_failure_from_io)?
                {
                    streaming_prefetch::UntilEofPrefetch::Complete(body) => {
                        return Ok(ConnectionProxyOutcome {
                            response: UpstreamResponse {
                                status: head.status,
                                version: head.version,
                                headers: head.headers,
                                trailers: Vec::new(),
                                early_hints: head.early_hints,
                                body: ResponseBody::Bytes(body),
                                body_temp_file: None,
                            },
                            reusable: None,
                            deferred: None,
                            upgrade: None,
                            header_time,
                        });
                    }
                    streaming_prefetch::UntilEofPrefetch::Stream { prefix, rate } => {
                        let body = streaming_prefetch::spawn_prefetched_until_eof_stream(
                            connection,
                            prefix,
                            context.proxy_read_timeout,
                            rate,
                            UpstreamBodyBufferOptions::upstream(&context.proxy_buffering),
                        );
                        return Ok(ConnectionProxyOutcome {
                            response: UpstreamResponse {
                                status: head.status,
                                version: head.version,
                                headers: head.headers,
                                trailers: Vec::new(),
                                early_hints: head.early_hints,
                                body,
                                body_temp_file: None,
                            },
                            reusable: None,
                            deferred: None,
                            upgrade: None,
                            header_time,
                        });
                    }
                }
            }
            UpstreamBodyKind::ContentLength(_) | UpstreamBodyKind::None => {}
        }
    }
    let response = {
        let mut reader = TimedReader::new(&mut connection.reader, context.proxy_read_timeout);
        read_upstream_response_body(
            &mut reader,
            head,
            limit_rate,
            UpstreamBodyBufferPolicy::upstream(
                &context.proxy_buffering,
                !ignores_accel_buffering(&context.proxy_ignore_headers),
            ),
        )
        .await
        .map_err(proxy_failure_from_io)?
    };
    let reusable = connection.reader.buffer().is_empty();
    Ok(ConnectionProxyOutcome {
        response,
        reusable: reusable.then_some(connection),
        deferred: None,
        upgrade: None,
        header_time,
    })
}

pub(in crate::server) struct ConnectionProxyOutcome {
    pub(in crate::server) response: UpstreamResponse,
    pub(in crate::server) reusable: Option<UpstreamConnection>,
    pub(in crate::server) deferred: Option<tokio::sync::oneshot::Receiver<UpstreamConnection>>,
    pub(in crate::server) upgrade: Option<UpstreamConnection>,
    pub(in crate::server) header_time: Duration,
}

fn has_accel_redirect(headers: &[Header], context: &EffectiveConfig) -> bool {
    !upstream_header_ignored(&context.proxy_ignore_headers, "X-Accel-Redirect")
        && headers.iter().any(|header| {
            header.name.eq_ignore_ascii_case("X-Accel-Redirect") && !header.value.is_empty()
        })
}

#[cfg(test)]
pub(in crate::server) async fn exchange_upstream_connection(
    connection: &mut UpstreamConnection,
    outgoing: &ProxyRequestMessage,
    version: &str,
) -> io::Result<(UpstreamResponse, Duration)> {
    exchange_upstream_connection_with_rate(connection, outgoing, version, 0).await
}

#[cfg(test)]
pub(in crate::server) async fn exchange_upstream_connection_with_rate(
    connection: &mut UpstreamConnection,
    outgoing: &ProxyRequestMessage,
    version: &str,
    limit_rate: u64,
) -> io::Result<(UpstreamResponse, Duration)> {
    write_proxy_request(&mut connection.reader, outgoing, version).await?;
    read_upstream_response_timed_with_rate(&mut connection.reader, limit_rate).await
}

pub(in crate::server) async fn upgrade_tls_client<S>(
    stream: S,
    raw_name: &str,
    options: ClientTlsOptions<'_>,
    cache: &ClientTlsConfigCache,
) -> io::Result<tokio_rustls::client::TlsStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let config = cache.get_or_build(options)?;
    let connector = TlsConnector::from(config);
    let server_name = if let Ok(address) = raw_name.parse::<std::net::IpAddr>() {
        tokio_rustls::rustls::pki_types::ServerName::IpAddress(address.into())
    } else {
        tokio_rustls::rustls::pki_types::ServerName::try_from(raw_name.to_string()).map_err(
            |err| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid proxy TLS server name: {err}"),
                )
            },
        )?
    };
    connector
        .connect(server_name, stream)
        .await
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(test)]
pub(in crate::server) async fn timed_proxy_exchange<S>(
    stream: S,
    outgoing: &ProxyRequestMessage,
    version: &str,
    context: &EffectiveConfig,
) -> Result<(u16, Vec<Header>, Vec<u8>), ProxyFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match tokio::time::timeout(
        context.proxy_read_timeout,
        proxy_exchange(stream, outgoing, version),
    )
    .await
    {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(err)) => Err(proxy_failure_from_io(err)),
        Err(_) => Err(ProxyFailure::Timeout),
    }
}

#[cfg(test)]
pub(in crate::server) async fn proxy_exchange<S>(
    mut stream: S,
    outgoing: &ProxyRequestMessage,
    version: &str,
) -> io::Result<(u16, Vec<Header>, Vec<u8>)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_proxy_request(&mut stream, outgoing, version).await?;
    let mut reader = BufReader::new(stream);
    let (response, _) = crate::http::read_upstream_response_timed(&mut reader).await?;
    let body = response
        .body
        .into_memory(MAX_BODY_BYTES as u64)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "response body too large"))?;
    Ok((response.status, response.headers, body))
}

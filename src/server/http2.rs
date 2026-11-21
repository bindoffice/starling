use super::*;

mod body;
mod connection;
mod fault;
mod informational;
mod plaintext;

use connection::settings_for_connection;
pub(in crate::server) use fault::H2ConnectionFault;
pub(in crate::server) use informational::send_h2_early_hints;
pub(in crate::server) use plaintext::handle_plaintext_http_connection;

pub(super) async fn handle_h2<S>(
    stream: S,
    addresses: ConnectionAddresses,
    group: ListenerGroup,
    state: SharedState,
    nodelay: Option<ClientSocketControl>,
    mut shutdown: Option<watch::Receiver<ShutdownMode>>,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let settings = {
        let config = state.config.read().await.clone();
        settings_for_connection(&config, &group, &addresses)
    };
    let limits = settings.limits;
    let mut builder = h2_server::Builder::new();
    builder
        .max_concurrent_streams(limits.max_concurrent_streams)
        .initial_window_size(limits.body_preread_size)
        .max_header_list_size(receive_header_list_limit(limits.max_header_list_size));
    let handshake = async {
        if settings.header_timeout == Duration::ZERO {
            Some(builder.handshake(stream).await)
        } else {
            tokio::time::timeout(settings.header_timeout, builder.handshake(stream))
                .await
                .ok()
        }
    };
    let handshake = if shutdown.is_some() {
        tokio::select! {
            handshake = handshake => handshake,
            _ = wait_for_optional_shutdown(&mut shutdown) => return,
        }
    } else {
        handshake.await
    };
    let Some(handshake) = handshake else {
        state
            .logger
            .log(LogLevel::Info, "HTTP/2 handshake timed out");
        return;
    };
    let connection = match handshake {
        Ok(connection) => connection,
        Err(err) => {
            state
                .logger
                .log(LogLevel::Info, format!("HTTP/2 handshake failed: {err}"));
            return;
        }
    };

    connection::serve_connection(
        connection, settings, addresses, group, state, nodelay, shutdown,
    )
    .await;
}

pub(super) async fn handle_h2_request(
    request: HttpRequest<RecvStream>,
    mut respond: SendResponse<Bytes>,
    group: &ListenerGroup,
    connection: ConnectionAddresses,
    state: &SharedState,
    options: H2RequestOptions,
    connection_fault: Option<H2ConnectionFault>,
) -> io::Result<()> {
    let H2RequestOptions {
        max_header_field_size,
        max_header_list_size,
        nodelay,
    } = options;
    let (parts, body) = request.into_parts();
    let method = parts.method.as_str().to_string();
    if h2_header_field_too_large(&parts, max_header_field_size)
        || h2_header_list_too_large(&parts, max_header_list_size)
    {
        if let Some(fault) = &connection_fault {
            fault.enhance_your_calm();
            return Ok(());
        }
        return send_h2_response(
            respond,
            error_response(431),
            &method,
            ServerTokens::On,
            Http2Limits::default().chunk_size,
        )
        .await;
    }
    let content_length = match h2_content_length(&parts.headers) {
        Ok(length) => length,
        Err(_) => {
            return send_h2_response(
                respond,
                error_response(400),
                &method,
                ServerTokens::On,
                Http2Limits::default().chunk_size,
            )
            .await;
        }
    };
    let body_end = body.is_end_stream();
    if body_end && content_length.is_some_and(|length| length != 0) {
        return send_h2_response(
            respond,
            error_response(400),
            &method,
            ServerTokens::On,
            Http2Limits::default().chunk_size,
        )
        .await;
    }
    if parts
        .headers
        .get("expect")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
    {
        let informational = HttpResponse::builder()
            .status(StatusCode::CONTINUE)
            .body(())
            .map_err(io::Error::other)?;
        respond
            .send_informational(informational)
            .map_err(io::Error::other)?;
    }

    let snapshot = state.config_snapshot().await;
    let config = &snapshot.config;
    let header_count = parts.headers.len() as u64;
    let mut request = request_from_h2_parts(
        parts,
        Vec::new(),
        connection.remote.variable_address(),
        connection.local.variable_address(),
    );
    let server_context = request_server_context(config, group, &request);
    let server_tokens = server_context
        .map(|server| server.context.server_tokens)
        .unwrap_or(ServerTokens::On);
    let chunk_size = server_context
        .map(|server| server.context.http2_limits.chunk_size)
        .unwrap_or_else(|| Http2Limits::default().chunk_size);
    if header_count
        > server_context
            .map(|server| server.context.max_headers)
            .unwrap_or(1000)
    {
        return send_h2_response(
            respond,
            error_response(400),
            &method,
            server_tokens,
            chunk_size,
        )
        .await;
    }
    let body_timeout = server_context
        .map(|server| server.context.client_body_timeout)
        .unwrap_or_else(|| Duration::from_secs(60));
    let body_task = (!body_end).then(|| {
        H2BodyTask::spawn_with_connection_fault_and_length(
            body,
            body_timeout,
            0,
            connection_fault,
            content_length,
        )
    });
    request.body_stream = body_task.as_ref().map(H2BodyTask::stream);
    let client_abort = ClientAbortState::default();
    let mut client_closed = false;
    let action = {
        // Keep dispatch in an inner scope: it owns the request body stream, so
        // it must be dropped before waiting for the body task's cleanup.
        let dispatch = dispatch_http_request(
            snapshot,
            group,
            &request,
            connection,
            ConnectionRequestState::new(Instant::now(), 0),
            Some(client_abort.clone()),
            state,
        );
        tokio::pin!(dispatch);
        loop {
            if client_closed {
                break dispatch.await;
            }
            tokio::select! {
                action = &mut dispatch => break action,
                _ = std::future::poll_fn(|context| respond.poll_reset(context)) => {
                    if client_abort.preserve() {
                        client_closed = true;
                    } else {
                        break None;
                    }
                }
            }
        }
    };
    let Some(action) = action else {
        if let Some(task) = body_task {
            finish_or_detach_body_task(task).await;
        }
        respond.send_reset(h2::Reason::CANCEL);
        return Ok(());
    };
    if client_closed {
        if let Some(task) = body_task {
            finish_or_detach_body_task(task).await;
        }
        match action {
            DispatchAction::Response(mut outcome) => {
                let _ = drain_after_client_abort(&mut outcome.response);
            }
            DispatchAction::Tunnel(_) => {}
        }
        return Ok(());
    }
    let outcome = match action {
        DispatchAction::Response(outcome) => outcome,
        DispatchAction::Tunnel(_) => {
            if let Some(task) = body_task {
                finish_or_detach_body_task(task).await;
            }
            return send_h2_response(
                respond,
                error_response(405),
                &method,
                server_tokens,
                chunk_size,
            )
            .await;
        }
    };
    if let Some(task) = body_task {
        finish_or_detach_body_task(task).await;
    }
    if let Some(nodelay) = &nodelay {
        nodelay.apply_nodelay(outcome.tcp_nodelay)?;
    }
    send_h2_early_hints(&mut respond, outcome.response.early_hints())?;
    timeout_io(
        outcome.send_timeout,
        send_h2_response(
            respond,
            outcome.response,
            &request.method,
            outcome.server_tokens,
            outcome.http2_chunk_size,
        ),
    )
    .await
}

pub(super) struct H2RequestOptions {
    pub(super) max_header_field_size: usize,
    pub(super) max_header_list_size: usize,
    pub(super) nodelay: Option<ClientSocketControl>,
}

async fn finish_or_detach_body_task(task: H2BodyTask) {
    task.close().await;
    if task.is_finished() {
        let _ = task.finish().await;
    } else {
        task.detach();
    }
}

pub(super) fn h2_content_length(headers: &http::HeaderMap) -> io::Result<Option<u64>> {
    let mut content_length = None;
    for value in headers.get_all(http::header::CONTENT_LENGTH).iter() {
        let parsed = value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length"))?;
        if content_length.is_some_and(|previous| previous != parsed) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "conflicting Content-Length headers",
            ));
        }
        content_length = Some(parsed);
    }
    Ok(content_length)
}

fn h2_header_field_too_large(parts: &http::request::Parts, limit: usize) -> bool {
    if limit == 0 {
        return false;
    }
    let uri = &parts.uri;
    let uri_field_too_large = uri.scheme_str().is_some_and(|value| value.len() > limit)
        || uri
            .authority()
            .is_some_and(|value| value.as_str().len() > limit)
        || uri
            .path_and_query()
            .is_some_and(|value| value.as_str().len() > limit);
    // The h2 crate exposes only the aggregate decoded header-list size. Check each
    // decoded field here to preserve Nginx's per-field large_client_header_buffers
    // limit before the request enters routing or application code.
    uri_field_too_large
        || parts.method.as_str().len() > limit
        || parts
            .headers
            .iter()
            .any(|(name, value)| name.as_str().len() > limit || value.as_bytes().len() > limit)
}

fn h2_header_list_too_large(parts: &http::request::Parts, limit: usize) -> bool {
    if limit == 0 {
        return false;
    }
    let uri = &parts.uri;
    let mut size = ":method".len().saturating_add(parts.method.as_str().len());
    if let Some(scheme) = uri.scheme_str() {
        size = size
            .saturating_add(":scheme".len())
            .saturating_add(scheme.len());
    }
    if let Some(authority) = uri.authority() {
        size = size
            .saturating_add(":authority".len())
            .saturating_add(authority.as_str().len());
    }
    if let Some(path) = uri.path_and_query() {
        size = size
            .saturating_add(":path".len())
            .saturating_add(path.as_str().len());
    }
    size = parts.headers.iter().fold(size, |total, (name, value)| {
        total
            .saturating_add(name.as_str().len())
            .saturating_add(value.as_bytes().len())
    });
    size > limit
}

fn receive_header_list_limit(limit: u32) -> u32 {
    // The h2 crate enforces the advertised limit itself and answers with
    // 431/RST_STREAM. Keep a bounded decode ceiling here, then enforce Nginx's
    // aggregate limit after decoding so protocol violations become
    // GOAWAY(ENHANCE_YOUR_CALM). The 4x ceiling matches the crate's own abuse
    // guard and prevents unbounded header allocation.
    limit.saturating_mul(4)
}

#[cfg(test)]
pub(super) fn http2_limits_for_connection(
    config: &RuntimeConfig,
    group: &ListenerGroup,
    connection: &ConnectionAddresses,
) -> Http2Limits {
    settings_for_connection(config, group, connection).limits
}

pub(super) fn request_from_h2_parts(
    parts: http::request::Parts,
    body: Vec<u8>,
    remote_addr: String,
    local_addr: String,
) -> Request {
    let mut headers: Vec<Header> = parts
        .headers
        .iter()
        .map(|(name, value)| Header {
            name: name.as_str().to_string(),
            value: String::from_utf8_lossy(value.as_bytes()).into_owned(),
        })
        .collect();
    if !headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("host"))
        && let Some(authority) = parts.uri.authority()
    {
        headers.push(Header {
            name: "Host".into(),
            value: authority.as_str().to_string(),
        });
    }
    let target = parts
        .uri
        .path_and_query()
        .map(|path| path.as_str().to_string())
        .unwrap_or_else(|| parts.uri.to_string());
    Request {
        method: parts.method.as_str().to_string(),
        target,
        version: "HTTP/2.0".into(),
        headers,
        body,
        body_stream: None,
        remote_addr,
        local_addr,
        keep_alive: true,
    }
}

pub(super) async fn send_h2_response(
    mut respond: SendResponse<Bytes>,
    mut response: Response,
    request_method: &str,
    server_tokens: ServerTokens,
    chunk_size: usize,
) -> io::Result<()> {
    prepare_response_headers(&mut response, server_tokens);
    let mut builder = HttpResponse::builder()
        .status(StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    for header in &response.headers {
        if is_h2_connection_header(&header.name) {
            continue;
        }
        let Ok(name) = http::header::HeaderName::from_bytes(header.name.as_bytes()) else {
            continue;
        };
        let value = sanitize_header_value(&header.value);
        let Ok(value) = http::header::HeaderValue::from_str(&value) else {
            continue;
        };
        builder = builder.header(name, value);
    }
    let response_head = builder.body(()).map_err(io::Error::other)?;
    let is_head = request_method.eq_ignore_ascii_case("HEAD");
    let has_body = !is_head && !response.body.is_empty();
    let has_trailers = !is_head
        && !response.trailers.is_empty()
        && response.status >= 200
        && !matches!(response.status, 204 | 304);
    let has_output = has_body || has_trailers;
    let mut send = respond
        .send_response(response_head, !has_output)
        .map_err(io::Error::other)?;
    if !has_output {
        return Ok(());
    }

    let body = std::mem::replace(&mut response.body, ResponseBody::Empty);
    let mut trailers = match body::send_h2_body(&mut send, body, has_trailers, chunk_size).await {
        Ok(trailers) => trailers,
        Err(error) => {
            send.send_reset(h2_reset_reason(&error));
            return Err(error);
        }
    };
    if has_trailers {
        trailers.extend(response.trailers.iter().cloned());
    }
    if !trailers.is_empty() {
        body::send_h2_trailers(&mut send, &trailers)?;
    }
    Ok(())
}

fn h2_reset_reason(error: &io::Error) -> h2::Reason {
    match error.kind() {
        io::ErrorKind::BrokenPipe
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset => h2::Reason::CANCEL,
        _ => h2::Reason::INTERNAL_ERROR,
    }
}

pub(super) fn is_h2_connection_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-connection"
            | "transfer-encoding"
            | "upgrade"
            | "te"
            | "trailer"
    )
}

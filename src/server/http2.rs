use super::*;

mod body;
mod connection;
mod fault;
mod informational;
mod plaintext;
mod request;
mod send;

use connection::settings_for_connection;
pub(in crate::server) use fault::H2ConnectionFault;
pub(in crate::server) use informational::send_h2_early_hints;
pub(in crate::server) use plaintext::handle_plaintext_http_connection;
use request::{
    h2_content_length, h2_header_field_too_large, h2_header_list_too_large,
    receive_header_list_limit, request_from_h2_parts,
};
pub(super) use send::{is_h2_connection_header, send_h2_response};

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
        let config = state.current_config();
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

    let snapshot = state.config_snapshot();
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
            DispatchRequestContext {
                request: &request,
                connection,
                connection_request: ConnectionRequestState::new(0, Instant::now(), 0),
                invalid_host: false,
                client_abort: Some(client_abort.clone()),
            },
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

#[cfg(test)]
pub(super) fn http2_limits_for_connection(
    config: &RuntimeConfig,
    group: &ListenerGroup,
    connection: &ConnectionAddresses,
) -> Http2Limits {
    settings_for_connection(config, group, connection).limits
}

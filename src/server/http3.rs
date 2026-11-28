use super::*;
use h3::error::Code;
use h3_quinn::{BidiStream, RecvStream, SendStream};
use std::io;
use std::net::{IpAddr, SocketAddr};

mod body;
mod config;
mod request;
mod send;

use body::H3BodyTask;
pub(in crate::server) use config::{
    build_http3_server_config, install_http3_server_configs, server_uses_http3,
};
use request::{request_from_h3_parts, request_may_have_body};
use send::send_h3_response;

type H3RequestStream = h3::server::RequestStream<BidiStream<Bytes>, Bytes>;
type H3SendStream = h3::server::RequestStream<SendStream<Bytes>, Bytes>;
type H3RecvStream = h3::server::RequestStream<RecvStream, Bytes>;

const H3_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const H3_BODY_TIMEOUT_GRACE: Duration = Duration::from_millis(10);

pub(in crate::server) async fn http3_accept_loop(
    endpoint: quinn::Endpoint,
    group: ListenerGroup,
    state: SharedState,
    mut shutdown: watch::Receiver<ShutdownMode>,
) {
    let mut connections = JoinSet::new();
    let shutdown_mode = loop {
        tokio::select! {
            changed = shutdown.changed() => {
                let mode = if changed.is_err() {
                    ShutdownMode::Fast
                } else {
                    *shutdown.borrow_and_update()
                };
                if mode.requested() {
                    break mode;
                }
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(err)) = joined {
                    state.logger.log(
                        LogLevel::Error,
                        format!("HTTP/3 connection task failed: {err}"),
                    );
                }
            }
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else {
                    break ShutdownMode::Fast;
                };
                let remote = incoming.remote_address();
                let state = state.clone();
                let group = group.clone();
                let shutdown = shutdown.clone();
                connections.spawn(async move {
                    let connection = match incoming.await {
                        Ok(connection) => connection,
                        Err(err) => {
                            state.logger.log(
                                LogLevel::Info,
                                format!("HTTP/3 handshake from {remote} failed: {err}"),
                            );
                            return;
                        }
                    };
                    if let Err(err) =
                        handle_http3_connection(connection, group, state.clone(), shutdown).await
                    {
                        state.logger.log(
                            LogLevel::Info,
                            format!("HTTP/3 connection from {remote} failed: {err}"),
                        );
                    }
                });
                tokio::task::yield_now().await;
            }
        }
    };

    match shutdown_mode {
        ShutdownMode::Graceful => {
            let wait = async {
                while let Some(result) = connections.join_next().await {
                    if let Err(err) = result {
                        state.logger.log(
                            LogLevel::Error,
                            format!("HTTP/3 connection task failed: {err}"),
                        );
                    }
                }
            };
            let _ = tokio::time::timeout(H3_SHUTDOWN_GRACE, wait).await;
            endpoint.close(quinn::VarInt::from_u32(0), b"shutdown");
        }
        ShutdownMode::Fast => {
            endpoint.close(quinn::VarInt::from_u32(0), b"shutdown");
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        }
        ShutdownMode::Running => {}
    }
}

async fn handle_http3_connection(
    connection: quinn::Connection,
    group: ListenerGroup,
    state: SharedState,
    mut shutdown: watch::Receiver<ShutdownMode>,
) -> io::Result<()> {
    let remote = connection.remote_address();
    let local = SocketAddr::new(
        connection
            .local_ip()
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
        group.address.port(),
    );
    let mut addresses =
        ConnectionAddresses::direct(ListenAddress::Inet(remote), ListenAddress::Inet(local));
    let tls = h3_tls_connection_info(&connection);
    let max_field_section_size = {
        let snapshot = state.config_snapshot();
        let active_group = current_listener_group(&snapshot.listener_groups, &group);
        request_server_context_in_group(&snapshot.config, active_group, &tls.server_name)
            .map(|(_, server)| {
                server
                    .context
                    .large_client_header_buffers
                    .number
                    .saturating_mul(server.context.large_client_header_buffers.size)
                    as u64
            })
            .unwrap_or(64 * 1024)
    };
    addresses.tls = Some(tls);

    let mut builder = h3::server::builder();
    builder.max_field_section_size(max_field_section_size);
    let mut h3_connection = builder
        .build(h3_quinn::Connection::new(connection.clone()))
        .await
        .map_err(|error| io::Error::other(error.to_string()))?;
    let connection_stats = Arc::new(Mutex::new(state.stats.connection_started()));
    let connection_id = connection_stats
        .lock()
        .map(|guard| guard.id())
        .unwrap_or_default();
    let started = Instant::now();
    let mut requests = JoinSet::new();
    let mut served_requests = 0_u64;
    let in_flight = Arc::new(AtomicU64::new(0));

    loop {
        let mode = if shutdown.has_changed().unwrap_or(false) {
            Some(*shutdown.borrow_and_update())
        } else {
            None
        };
        if let Some(ShutdownMode::Fast) = mode {
            connection.close(quinn::VarInt::from_u32(0), b"shutdown");
            break;
        }
        if let Some(ShutdownMode::Graceful) = mode {
            let _ = h3_connection.shutdown(served_requests as usize).await;
            break;
        }

        tokio::select! {
            joined = requests.join_next(), if !requests.is_empty() => {
                if let Some(Err(err)) = joined {
                    state.logger.log(
                        LogLevel::Error,
                        format!("HTTP/3 request task failed: {err}"),
                    );
                }
            }
            accepted = h3_connection.accept() => {
                let resolver = match accepted {
                    Ok(Some(resolver)) => resolver,
                    Ok(None) => break,
                    Err(err) => {
                        state.logger.log(
                            LogLevel::Info,
                            format!("HTTP/3 connection error: {err}"),
                        );
                        break;
                    }
                };
                state.stats.requests.fetch_add(1, Ordering::Relaxed);
                served_requests = served_requests.saturating_add(1);
                if in_flight.fetch_add(1, Ordering::Relaxed) == 0
                    && let Ok(mut guard) = connection_stats.lock()
                {
                    guard.set_writing();
                }
                let state = state.clone();
                let group = group.clone();
                let addresses = addresses.clone();
                let connection_stats = connection_stats.clone();
                let in_flight = in_flight.clone();
                requests.spawn(async move {
                    let result = async {
                        let (request, stream) = resolver
                            .resolve_request()
                            .await
                            .map_err(|error| io::Error::other(error.to_string()))?;
                        handle_http3_request(
                            request,
                            stream,
                            &group,
                            addresses,
                            &state,
                            ConnectionRequestState::new(connection_id, started, served_requests),
                        )
                        .await
                    }
                    .await;
                    if let Err(err) = result {
                        state.logger.log(
                            LogLevel::Info,
                            format!("HTTP/3 request failed: {err}"),
                        );
                    }
                    if in_flight.fetch_sub(1, Ordering::Relaxed) == 1
                        && let Ok(mut guard) = connection_stats.lock()
                    {
                        guard.set_waiting();
                    }
                });
            }
            changed = shutdown.changed() => {
                let mode = if changed.is_err() {
                    ShutdownMode::Fast
                } else {
                    *shutdown.borrow_and_update()
                };
                match mode {
                    ShutdownMode::Graceful => {
                        let _ = h3_connection.shutdown(served_requests as usize).await;
                        break;
                    }
                    ShutdownMode::Fast => {
                        connection.close(quinn::VarInt::from_u32(0), b"shutdown");
                        break;
                    }
                    ShutdownMode::Running => {}
                }
            }
        }
    }

    while let Some(result) = requests.join_next().await {
        if let Err(err) = result {
            state.logger.log(
                LogLevel::Error,
                format!("HTTP/3 request task failed: {err}"),
            );
        }
    }
    Ok(())
}

async fn handle_http3_request(
    request: HttpRequest<()>,
    stream: H3RequestStream,
    group: &ListenerGroup,
    connection: ConnectionAddresses,
    state: &SharedState,
    connection_request: ConnectionRequestState,
) -> io::Result<()> {
    let (parts, ()) = request.into_parts();
    let method = parts.method.as_str().to_string();
    let (mut respond, body) = stream.split();

    let snapshot = state.config_snapshot();
    let config = &snapshot.config;
    let active_group = current_listener_group(&snapshot.listener_groups, group);
    let request_host = parts
        .uri
        .authority()
        .map(|authority| authority.host().to_ascii_lowercase())
        .unwrap_or_default();
    let request_server = request_server_context_in_group(config, active_group, &request_host);
    let request_server_id = request_server.map(|(server_id, _)| server_id);
    let server_context = request_server.map(|(_, server)| server);
    let server_tokens = server_context
        .map(|server| server.context.server_tokens)
        .unwrap_or(ServerTokens::On);
    let header_field_limit = server_context
        .map(|server| server.context.large_client_header_buffers.size)
        .unwrap_or(8 * 1024);
    let header_list_limit = server_context
        .map(|server| {
            server
                .context
                .large_client_header_buffers
                .number
                .saturating_mul(server.context.large_client_header_buffers.size)
        })
        .unwrap_or(64 * 1024);
    let body_timeout = server_context
        .map(|server| server.context.client_body_timeout)
        .unwrap_or_else(|| Duration::from_secs(60));
    let body_timeout = if body_timeout.is_zero() {
        body_timeout
    } else {
        body_timeout.saturating_add(H3_BODY_TIMEOUT_GRACE)
    };

    if h2_header_field_too_large(&parts, header_field_limit)
        || h2_header_list_too_large(&parts, header_list_limit)
    {
        send_h3_response(&mut respond, error_response(431), &method, ServerTokens::On).await?;
        return Ok(());
    }
    let invalid_request_headers = invalid_h2_request_headers(&parts);
    let content_length = match h2_content_length(&parts.headers) {
        Ok(length) => length,
        Err(_) => {
            send_h3_response(&mut respond, error_response(400), &method, ServerTokens::On).await?;
            return Ok(());
        }
    };
    if parts.headers.len() as u64
        > server_context
            .map(|server| server.context.max_headers)
            .unwrap_or(1000)
    {
        send_h3_response(&mut respond, error_response(400), &method, server_tokens).await?;
        return Ok(());
    }

    let _http_request_budget = state.http2_budget.acquire_request().await;
    let may_have_body = request_may_have_body(&method, content_length);
    let _http_body_budget = if may_have_body {
        Some(state.http2_budget.acquire_body().await)
    } else {
        None
    };
    let mut request = request_from_h3_parts(
        parts,
        connection.remote_addr_text.clone(),
        connection.local_addr_text.clone(),
    );
    let request_host = request.host_without_port().into_owned();
    let body_task = may_have_body.then(|| H3BodyTask::spawn(body, body_timeout, 0, content_length));
    if let Some(task) = &body_task {
        request.body_stream = Some(task.stream());
    }

    let action = dispatch_http_request(
        snapshot,
        group,
        DispatchRequestContext {
            request: &mut request,
            host: request_host,
            server_id: request_server_id,
            connection,
            connection_request,
            invalid_host: invalid_request_headers,
            client_abort: None,
            request_length: None,
        },
        state,
    )
    .await;

    let Some(action) = action else {
        if let Some(task) = body_task {
            let _ = finish_or_detach_body_task(task).await;
        }
        respond.stop_stream(Code::H3_REQUEST_CANCELLED);
        return Ok(());
    };
    let outcome = match action {
        DispatchAction::Response(outcome) => outcome,
        DispatchAction::Tunnel(_) => {
            if let Some(task) = body_task {
                let _ = finish_or_detach_body_task(task).await;
            }
            send_h3_response(&mut respond, error_response(405), &method, server_tokens).await?;
            return Ok(());
        }
    };
    let body_task_outcome = if let Some(task) = body_task {
        finish_or_detach_body_task(task).await
    } else {
        BodyTaskOutcome::Finished(Ok(()))
    };
    if let BodyTaskOutcome::Finished(Err(error)) = &body_task_outcome {
        let status = crate::http::request_body_error_status(error).unwrap_or(500);
        send_h3_response(&mut respond, error_response(status), &method, server_tokens).await?;
        return Ok(());
    }
    let send_timeout = outcome.send_timeout;
    let deferred_access_log = outcome.deferred_access_log;
    let result = timeout_io(
        send_timeout,
        send_h3_response(
            &mut respond,
            outcome.response,
            &request.method,
            outcome.server_tokens,
        ),
    )
    .await;
    if let Some(log) = deferred_access_log {
        log.write();
    }
    result
}

enum BodyTaskOutcome {
    Finished(io::Result<()>),
    Detached,
}

async fn finish_or_detach_body_task(task: H3BodyTask) -> BodyTaskOutcome {
    task.close().await;
    for _ in 0..4 {
        if task.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    if task.is_finished() {
        BodyTaskOutcome::Finished(task.finish().await)
    } else {
        task.detach();
        BodyTaskOutcome::Detached
    }
}

fn h3_tls_connection_info(connection: &quinn::Connection) -> TlsConnectionInfo {
    let handshake = connection
        .handshake_data()
        .and_then(|data| data.downcast::<quinn::crypto::rustls::HandshakeData>().ok());
    let server_name = handshake
        .as_ref()
        .and_then(|data| data.server_name.as_deref())
        .unwrap_or_default()
        .to_string();
    let alpn_protocol = handshake
        .as_ref()
        .and_then(|data| data.protocol.as_deref())
        .map(|protocol| String::from_utf8_lossy(protocol).into_owned())
        .unwrap_or_else(|| "h3".to_string());
    TlsConnectionInfo {
        protocol: "TLSv1.3".to_string(),
        cipher: String::new(),
        server_name,
        alpn_protocol,
        client_certificate: None,
        client_certificate_verified: false,
        client_certificate_error: None,
        session_reused: false,
    }
}

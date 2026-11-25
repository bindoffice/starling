use super::body_stream::Http1BodyTask;
use super::*;

mod request_body;
mod response_write;
mod upgrade;
use error_response::{log_and_reject_request, write_request_rejection};
use request_body::close_http1_body;
use response_write::{DispatchWrite, write_dispatch_response};

mod error_response;

pub(in crate::server) async fn handle_io_with_shutdown<S>(
    stream: S,
    connection: ConnectionAddresses,
    group: ListenerGroup,
    state: SharedState,
    reset_target: Option<TimeoutReset>,
    control: Option<ClientSocketControl>,
    mut shutdown: Option<watch::Receiver<ShutdownMode>>,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let remote = connection.remote.clone();
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = Some(BufReader::new(read_half));
    let mut body_task: Option<Http1BodyTask<S>> = None;
    let mut connection_stats = state.stats.connection_started();
    let mut served_requests = 0_u64;
    let mut timed_out = false;
    let mut keepalive_policy: Option<KeepalivePolicy> = None;
    let mut close_config = ConnectionCloseConfig::default();
    let connection_started = Instant::now();
    let mut head_parse_buffers = RequestHeadParseBuffers::default();
    let mut response_head_scratch = Vec::with_capacity(512);
    let client_abort = ClientAbortState::with_socket(control.clone());
    let shutdown_enabled = shutdown.is_some();
    'connection: loop {
        if shutdown_requested(&shutdown) {
            break;
        }
        let pipelined = reader
            .as_ref()
            .is_some_and(|reader| !reader.buffer().is_empty());
        if let Some(policy) = keepalive_policy.take() {
            connection_stats.set_idle();
            match wait_for_keepalive_phase(
                reader.as_mut().expect("HTTP/1 reader available"),
                policy.initial_idle_timeout(),
                &mut shutdown,
            )
            .await
            {
                KeepalivePhase::Data => {}
                KeepalivePhase::Closed | KeepalivePhase::Shutdown => break,
                KeepalivePhase::TimedOut => {
                    if !policy.becomes_reusable() {
                        timed_out = true;
                        break;
                    }
                    connection_stats.set_waiting();
                    match wait_for_keepalive_phase(
                        reader.as_mut().expect("HTTP/1 reader available"),
                        policy.remaining_idle_timeout(),
                        &mut shutdown,
                    )
                    .await
                    {
                        KeepalivePhase::Data => {}
                        KeepalivePhase::Closed | KeepalivePhase::Shutdown => break,
                        KeepalivePhase::TimedOut => {
                            timed_out = true;
                            break;
                        }
                        KeepalivePhase::Failed(err) => {
                            state.logger.log(
                                LogLevel::Info,
                                format!("client keepalive read from {remote} failed: {err}"),
                            );
                            break;
                        }
                    }
                }
                KeepalivePhase::Failed(err) => {
                    state.logger.log(
                        LogLevel::Info,
                        format!("client keepalive read from {remote} failed: {err}"),
                    );
                    break;
                }
            }
        }
        let snapshot = state.config_snapshot();
        let config = &snapshot.config;
        let active_group = current_listener_group(&snapshot.listener_groups, &group);
        let default_context = connection_server_context(config, active_group);
        close_config = default_context
            .map(|server| ConnectionCloseConfig::from(&server.context))
            .unwrap_or_default();
        let header_timeout = default_context
            .map(|server| server.context.client_header_timeout)
            .unwrap_or_else(|| Duration::from_secs(60));
        let header_limits = default_context
            .map(|server| {
                RequestHeadLimits::from_buffers(
                    server.context.client_header_buffer_size,
                    server.context.large_client_header_buffers.number,
                    server.context.large_client_header_buffers.size,
                )
            })
            .unwrap_or_default();
        connection_stats.set_reading();
        let head = {
            let mut timed_reader = TimedReader::new(
                reader.as_mut().expect("HTTP/1 reader available"),
                header_timeout,
            );
            let head = read_request_head_with_limits_and_buffers(
                &mut timed_reader,
                header_limits,
                &mut head_parse_buffers,
            );
            tokio::select! {
                head = head => head,
                _ = wait_for_optional_shutdown(&mut shutdown), if shutdown_enabled => {
                    break 'connection;
                }
            }
        };
        let mut head = match head {
            Ok(Some(head)) => head,
            Ok(None) => break,
            Err(err) => {
                timed_out = err.kind() == io::ErrorKind::TimedOut;
                let status = if err.kind() == io::ErrorKind::TimedOut {
                    408
                } else if err.kind() == io::ErrorKind::Unsupported {
                    501
                } else if err.to_string().contains("request line too long") {
                    414
                } else if err.to_string().contains("unsupported HTTP version") {
                    505
                } else {
                    400
                };
                let send_timeout = default_context
                    .map(|server| server.context.send_timeout)
                    .unwrap_or_else(|| Duration::from_secs(60));
                log_and_reject_request(
                    &state.logger,
                    &mut write_half,
                    status,
                    send_timeout,
                    "GET",
                    "HTTP/1.1",
                    ServerTokens::On,
                    format!("client request header error from {remote}: {err}"),
                )
                .await;
                break;
            }
        };
        state.stats.requests.fetch_add(1, Ordering::Relaxed);
        served_requests = served_requests.saturating_add(1);
        connection_stats.set_writing();
        let request_host = head.host_without_port().into_owned();
        let request_server = request_server_context_in_group(config, active_group, &request_host);
        let request_server_id = request_server.map(|(server_id, _)| server_id);
        let request_context = request_server.map(|(_, server)| server);
        let ignore_invalid_headers = request_context
            .map(|server| server.context.ignore_invalid_headers)
            .unwrap_or(true);
        let underscores_in_headers = request_context
            .map(|server| server.context.underscores_in_headers)
            .unwrap_or(false);
        if let Err(err) = head.apply_header_policy(ignore_invalid_headers, underscores_in_headers) {
            let send_timeout = request_context
                .map(|server| server.context.send_timeout)
                .unwrap_or_else(|| Duration::from_secs(60));
            log_and_reject_request(
                &state.logger,
                &mut write_half,
                400,
                send_timeout,
                &head.method,
                &head.version,
                request_context
                    .map(|server| server.context.server_tokens)
                    .unwrap_or(ServerTokens::On),
                format!("client request header error from {remote}: {err}"),
            )
            .await;
            break;
        }
        if let Some(server) = request_context {
            close_config = ConnectionCloseConfig::from(&server.context);
        }
        let body_timeout = request_context
            .map(|server| server.context.client_body_timeout)
            .unwrap_or_else(|| Duration::from_secs(60));
        let send_timeout = request_context
            .map(|server| server.context.send_timeout)
            .unwrap_or_else(|| Duration::from_secs(60));
        if head.headers.len() as u64
            > request_context
                .map(|server| server.context.max_headers)
                .unwrap_or(1000)
        {
            log_and_reject_request(
                &state.logger,
                &mut write_half,
                400,
                send_timeout,
                &head.method,
                &head.version,
                request_context
                    .map(|server| server.context.server_tokens)
                    .unwrap_or(ServerTokens::On),
                format!("client request from {remote} sent too many header lines"),
            )
            .await;
            break;
        }
        if head.expect_continue && head.version == "HTTP/1.1" {
            let mut writer = TimedWriter::new(&mut write_half, send_timeout);
            if let Err(err) = writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await {
                timed_out = err.kind() == io::ErrorKind::TimedOut;
                break;
            }
            if let Err(err) = writer.flush().await {
                timed_out = err.kind() == io::ErrorKind::TimedOut;
                break;
            }
        }
        let invalid_host = head.invalid_host;
        body_task = request_body::spawn_http1_body(&mut reader, &head, invalid_host, body_timeout);
        let body_stream = body_task.as_ref().map(Http1BodyTask::stream);
        let request_length = head.request_length;
        let mut request = Request {
            method: head.method,
            target: head.target,
            version: head.version,
            headers: head.headers.into(),
            body: Vec::new(),
            body_stream,
            remote_addr: connection.remote_addr_text.clone(),
            local_addr: connection.local_addr_text.clone(),
            keep_alive: head.keep_alive,
        };

        client_abort.reset_for_request();
        let (action, client_closed_during_dispatch) = {
            let dispatch = dispatch_http_request(
                snapshot,
                &group,
                DispatchRequestContext {
                    request: &mut request,
                    host: request_host,
                    server_id: request_server_id,
                    connection: connection.clone(),
                    connection_request: ConnectionRequestState::new(
                        connection_stats.id(),
                        connection_started,
                        served_requests,
                    )
                    .with_pipelined(pipelined),
                    invalid_host,
                    client_abort: Some(client_abort.clone()),
                    request_length: Some(request_length),
                },
                &state,
            );
            tokio::pin!(dispatch);
            match wait_for_dispatch(dispatch.as_mut(), control.as_ref(), &client_abort).await {
                DispatchWait::Completed {
                    output,
                    client_closed,
                } => (output, client_closed),
                DispatchWait::Cancelled => {
                    state.logger.log(
                        LogLevel::Info,
                        format!(
                            "client {remote} closed while waiting for upstream; \
                             cancelling the upstream request"
                        ),
                    );
                    close_http1_body(&mut body_task).await;
                    break 'connection;
                }
            }
        };
        let Some(action) = action else {
            close_http1_body(&mut body_task).await;
            break;
        };
        let mut outcome;
        let request_logger;
        match action {
            DispatchAction::Response(selected) => {
                request_logger = selected.error_logger.clone();
                outcome = selected;
            }
            DispatchAction::Tunnel(tunnel) => {
                let tunnel_logger = tunnel
                    .error_logger
                    .clone()
                    .unwrap_or_else(|| state.logger.clone());
                request_logger = tunnel_logger.clone();
                close_http1_body(&mut body_task).await;
                if let Some(control) = &control
                    && let Err(err) = control.apply_nodelay_once(tunnel.tcp_nodelay)
                {
                    request_logger.log(
                        LogLevel::Alert,
                        format!("setsockopt(TCP_NODELAY) failed: {err}"),
                    );
                    break;
                }
                let Some(reader) = reader.take() else {
                    break;
                };
                let prefix = reader.buffer().to_vec();
                let mut tunnel_reader = PrefixedStream::new(reader.into_inner(), prefix);
                let _ = serve_tunnel(
                    &mut tunnel_reader,
                    &mut write_half,
                    tunnel,
                    &tunnel_logger,
                    &remote,
                    control.as_ref(),
                )
                .await;
                break;
            }
        }
        if client_closed_during_dispatch {
            close_http1_body(&mut body_task).await;
            let mut response = outcome.response;
            let _ = drain_after_client_abort(&mut response);
            if let Some(log) = outcome.deferred_access_log.take() {
                log.write();
            }
            break 'connection;
        }
        if let Some(error) = body_task
            .as_ref()
            .and_then(Http1BodyTask::unconsumed_initial_error)
        {
            state.logger.log(
                LogLevel::Info,
                format!("client request body error from {remote}: {error}"),
            );
            close_http1_body(&mut body_task).await;
            write_request_rejection(
                &mut write_half,
                400,
                send_timeout,
                &request.method,
                &request.version,
                outcome.server_tokens,
            )
            .await;
            break;
        }
        if let Some(proxy_upgrade) = outcome.upgrade.take() {
            upgrade::handle_proxy_upgrade(
                &mut body_task,
                &mut reader,
                &mut write_half,
                &mut outcome,
                *proxy_upgrade,
                &request,
                control.as_ref(),
                &request_logger,
                &remote,
            )
            .await;
            break;
        }
        if let Some(task) = body_task.as_ref() {
            task.close().await;
        }
        if let Some(control) = &control
            && let Err(err) = control.apply_nodelay_once(outcome.tcp_nodelay)
        {
            request_logger.log(
                LogLevel::Alert,
                format!("setsockopt(TCP_NODELAY) failed: {err}"),
            );
            break;
        }
        close_config = outcome.close;
        let keep_alive = outcome.keep_alive && !shutdown_requested(&shutdown);
        if !keep_alive {
            if outcome.keep_alive {
                outcome.response_prepared = false;
            }
            outcome.keep_alive = false;
            outcome.keepalive_header = None;
        }
        match write_dispatch_response(
            &mut write_half,
            &mut outcome,
            &request,
            control.as_ref(),
            &mut response_head_scratch,
        )
        .await
        {
            DispatchWrite::Written => {}
            DispatchWrite::ClientClosed => {
                request_logger.log(
                    LogLevel::Info,
                    format!("client {remote} closed the connection during the response"),
                );
                let _ = drain_after_client_abort(&mut outcome.response);
                if let Some(log) = outcome.deferred_access_log.take() {
                    log.write();
                }
                break;
            }
            DispatchWrite::Failed(err) => {
                timed_out = err.kind() == io::ErrorKind::TimedOut;
                let _ = drain_after_client_abort(&mut outcome.response);
                if let Some(log) = outcome.deferred_access_log.take() {
                    log.write();
                }
                if err.kind() != io::ErrorKind::BrokenPipe {
                    request_logger.log(LogLevel::Error, format!("write() failed: {err}"));
                }
                break;
            }
        }
        if let Some(log) = outcome.deferred_access_log.take() {
            log.write();
        }
        if let Some(task) = body_task.take() {
            if !keep_alive {
                task.abort();
                break;
            }
            match task.finish().await {
                Ok((restored, result)) => {
                    reader = Some(restored);
                    if let Err(err) = result {
                        timed_out = err.kind() == io::ErrorKind::TimedOut;
                        request_logger.log(
                            LogLevel::Info,
                            format!(
                                "client request body for \"{} {}\" from {remote} failed: {err}",
                                request.method, request.target
                            ),
                        );
                        break;
                    }
                }
                Err(err) => {
                    request_logger.log(
                        LogLevel::Error,
                        format!(
                            "client request body task for \"{} {}\" failed: {err}",
                            request.method, request.target
                        ),
                    );
                    break;
                }
            }
        }
        if !keep_alive {
            break;
        }
        keepalive_policy = Some(KeepalivePolicy::new(
            outcome.keepalive_timeout,
            outcome.keepalive_min_timeout,
        ));
    }
    close_http1_body(&mut body_task).await;
    if timed_out && close_config.reset_timedout_connection {
        if let Some(reset_target) = reset_target
            && let Err(err) = reset_target.set_reset_on_close()
        {
            state.logger.log(
                LogLevel::Alert,
                format!("setsockopt(SO_LINGER) failed: {err}"),
            );
        }
        return;
    }
    let _ = write_half.shutdown().await;
    if let Some(reader) = reader.as_mut() {
        lingering_close(reader, close_config).await;
    }
}

use super::*;

pub(in crate::server) trait StreamIo:
    AsyncRead + AsyncWrite + Send + Unpin
{
}

impl<T> StreamIo for T where T: AsyncRead + AsyncWrite + Send + Unpin {}

pub(in crate::server) type BoxedStream = Box<dyn StreamIo>;

struct StreamSessionDispatch<'a> {
    addresses: &'a ProxyProtocolAddresses,
    server: &'a crate::config::StreamServer,
    config: &'a RuntimeConfig,
    state: &'a SharedState,
    has_preread_data: bool,
}

#[cfg(test)]
pub(in crate::server) async fn handle_stream_connection<S>(
    inbound: S,
    addresses: &ProxyProtocolAddresses,
    route: StreamRoute,
    state: SharedState,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let local = route.address.clone();
    let mut passes = 0;
    handle_stream_connection_boxed(
        Box::new(inbound),
        addresses,
        local,
        route,
        state,
        &mut passes,
    )
    .await;
}

pub(in crate::server) async fn handle_stream_connection_boxed(
    mut inbound: BoxedStream,
    addresses: &ProxyProtocolAddresses,
    local: ListenAddress,
    route: StreamRoute,
    state: SharedState,
    passes: &mut usize,
) {
    let config = state.current_config();
    let Some(stream) = config.stream.as_ref() else {
        return;
    };
    let Some(default_server) = stream_server_for_route(stream, &route) else {
        return;
    };
    let remote = effective_stream_remote(default_server, addresses);
    let logger = stream_logger(&state, default_server).await;
    let mut log = StreamLogContext {
        logger,
        started: Instant::now(),
        remote,
        realip_remote: addresses.transport_remote.clone(),
        local,
        proxy_protocol_addr: addresses.proxy_protocol_addr,
        proxy_protocol_port: addresses.proxy_protocol_port,
        proxy_protocol_server_addr: addresses.proxy_protocol_server_addr,
        proxy_protocol_server_port: addresses.proxy_protocol_server_port,
        proxy_protocol_tlvs: addresses.proxy_protocol_tlvs.clone(),
        protocol: if route.udp { "UDP" } else { "TCP" },
        server_name: default_server.server_name.clone(),
        limit_conn_status: String::new(),
        status: 0,
        bytes_sent: 0,
        bytes_received: 0,
        upstream_addr: String::new(),
        upstream_bytes_sent: 0,
        upstream_bytes_received: 0,
        upstream_connect_time: None,
        upstream_first_byte_time: None,
        upstream_session_time: None,
        tls: None,
        ssl_preread: None,
        variables: HashMap::new(),
        maps: stream.maps.clone(),
        geos: stream.geos.clone(),
        split_clients: stream.split_clients.clone(),
    };
    let (ssl_preread, prefix) = if default_server.ssl_preread {
        match read_stream_ssl_preread_with_timeout(
            &mut inbound,
            default_server.preread_buffer_size,
            default_server.preread_timeout,
        )
        .await
        {
            Ok(result) => result,
            Err(err) if err.kind() == io::ErrorKind::TimedOut => {
                log.status = 504;
                log.logger.log(
                    LogLevel::Info,
                    format!("stream preread for {} timed out", log.remote),
                );
                log_stream_access(&config, default_server, &log);
                return;
            }
            Err(err) => {
                log.status = 400;
                log.logger.log(
                    LogLevel::Info,
                    format!("stream preread for {} failed: {err}", log.remote),
                );
                log_stream_access(&config, default_server, &log);
                return;
            }
        }
    } else {
        (None, Vec::new())
    };
    log.ssl_preread = ssl_preread;
    let server = {
        let host = log
            .ssl_preread
            .as_ref()
            .map(|preread| preread.server_name.as_str());
        select_stream_server(stream, &route, host)
            .cloned()
            .unwrap_or_else(|| default_server.clone())
    };
    log.server_name = server.server_name.clone();
    log.remote = effective_stream_remote(&server, addresses);
    let has_preread_data = !prefix.is_empty();
    let inbound = PrefixedStream::new(inbound, prefix);
    if route.ssl {
        let tls_config = state
            .stream_tls_configs
            .read()
            .await
            .get(&route.address)
            .cloned();
        let Some(tls_config) = tls_config else {
            log.logger.log(
                LogLevel::Error,
                format!(
                    "missing TLS configuration for stream listener {}",
                    route.address
                ),
            );
            log.status = 500;
            log_stream_access(&config, &server, &log);
            return;
        };
        match tokio::time::timeout(
            server.ssl_handshake_timeout,
            accept_stream_tls(inbound, tls_config),
        )
        .await
        {
            Ok(Ok(inbound)) => {
                let mut tls = tls_connection_info(inbound.get_ref().1);
                let selected = select_stream_server(stream, &route, Some(&tls.server_name))
                    .cloned()
                    .unwrap_or(server);
                tls.client_certificate_verified = tls.client_certificate.is_some()
                    && matches!(
                        selected.ssl_verify_client,
                        ClientCertificateVerify::On | ClientCertificateVerify::Optional
                    );
                log.server_name = selected.server_name.clone();
                log.remote = effective_stream_remote(&selected, addresses);
                log.logger = stream_logger(&state, &selected).await;
                log.tls = Some(tls);
                let inbound: BoxedStream = Box::new(inbound);
                let outcome = Box::pin(process_stream_session(
                    inbound,
                    StreamSessionDispatch {
                        addresses,
                        server: &selected,
                        config: &config,
                        state: &state,
                        has_preread_data: false,
                    },
                    &mut log,
                    passes,
                ))
                .await;
                if outcome == StreamProcessOutcome::Completed {
                    log_stream_access(&config, &selected, &log);
                }
            }
            Ok(Err(err)) => {
                log.status = 400;
                log.logger.log(
                    LogLevel::Info,
                    format!("stream TLS handshake for {} failed: {err}", log.remote),
                );
                log_stream_access(&config, &server, &log);
            }
            Err(_) => {
                log.status = 504;
                log.logger.log(
                    LogLevel::Info,
                    format!("stream TLS handshake for {} timed out", log.remote),
                );
                log_stream_access(&config, &server, &log);
            }
        }
    } else {
        let inbound: BoxedStream = Box::new(inbound);
        let outcome = Box::pin(process_stream_session(
            inbound,
            StreamSessionDispatch {
                addresses,
                server: &server,
                config: &config,
                state: &state,
                has_preread_data,
            },
            &mut log,
            passes,
        ))
        .await;
        if outcome == StreamProcessOutcome::Completed {
            log_stream_access(&config, &server, &log);
        }
    }
}

async fn process_stream_session(
    mut inbound: BoxedStream,
    dispatch: StreamSessionDispatch<'_>,
    log: &mut StreamLogContext,
    passes: &mut usize,
) -> StreamProcessOutcome {
    let StreamSessionDispatch {
        addresses,
        server,
        config,
        state,
        has_preread_data,
    } = dispatch;
    log.logger = stream_logger(state, server).await;
    apply_stream_sets(&server.sets, log);
    let Ok(_limit_guard) = acquire_stream_connection_limits(server, state, log) else {
        return StreamProcessOutcome::Completed;
    };
    let access_remote = log.remote.clone();
    if reject_stream_access(server, &access_remote, log) {
        return StreamProcessOutcome::Completed;
    }
    match server.handler {
        StreamHandler::Return => {
            let Some(template) = &server.return_value else {
                log.status = 500;
                return StreamProcessOutcome::Completed;
            };
            write_stream_return(&mut inbound, template, log).await;
            StreamProcessOutcome::Completed
        }
        StreamHandler::Proxy => {
            proxy_stream(inbound, server, config, &state.client_tls_cache, log).await;
            StreamProcessOutcome::Completed
        }
        StreamHandler::Pass => {
            if pass_stream(
                inbound,
                addresses,
                server,
                state,
                log,
                has_preread_data,
                passes,
            )
            .await
            {
                StreamProcessOutcome::Passed
            } else {
                StreamProcessOutcome::Completed
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamProcessOutcome {
    Completed,
    Passed,
}

pub(in crate::server) async fn proxy_stream<S>(
    mut inbound: S,
    server: &crate::config::StreamServer,
    config: &RuntimeConfig,
    client_tls_cache: &ClientTlsConfigCache,
    log: &mut StreamLogContext,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let remote = log.remote.clone();
    let remote = &remote;
    let Some(proxy) = &server.proxy_pass else {
        log.status = 502;
        return;
    };
    let target = eval_stream_log_variables(&proxy.target, log);
    if target.is_empty() {
        log.status = 502;
        log.logger.log(
            LogLevel::Error,
            "empty stream upstream after variable evaluation",
        );
        return;
    }
    let upstream_name = proxy.upstream.as_deref().or_else(|| {
        config
            .stream
            .as_ref()
            .and_then(|stream| stream.upstreams.get(&target))
            .map(|_| target.as_str())
    });
    let peer = remote.socket_addr().map(|address| address.ip());
    let hash_key = remote.variable_address();
    let targets = resolve_stream_targets(config, upstream_name, &target, peer, &hash_key);
    if targets.is_empty() {
        log.status = 502;
        log.logger.log(
            LogLevel::Error,
            format!("invalid stream upstream \"{target}\""),
        );
        return;
    }

    let retry_started = Instant::now();
    let mut attempts = 0;
    let mut last_status = 502;
    for target in targets {
        let Ok(mut guard) = target.try_acquire() else {
            continue;
        };
        attempts += 1;
        append_stream_upstream_addr(log, &upstream_target_display(&target));
        let result = proxy_stream_attempt(
            &mut inbound,
            &target,
            StreamProxyContext {
                remote,
                server,
                tls_default_name: &target.host,
                attempt_started: Instant::now(),
                client_tls_cache,
                log,
            },
        )
        .await;

        match result {
            StreamAttemptResult::Relayed { connect, relay } => {
                update_stream_log_after_relay(log, connect, relay);
                record_stream_timing(&mut guard, connect, relay);
                last_status = relay.status;
                if relay.success {
                    return;
                }
                if relay.bytes_received == 0
                    && relay.bytes_sent == 0
                    && can_retry_stream_upstream(server, retry_started, attempts)
                {
                    continue;
                }
                return;
            }
            StreamAttemptResult::Failed {
                status,
                connect,
                retryable,
            } => {
                log.status = status;
                log.upstream_connect_time = Some(connect);
                last_status = status;
                if let Some(guard) = guard.as_mut() {
                    guard.mark_failed();
                }
                if !retryable || !can_retry_stream_upstream(server, retry_started, attempts) {
                    return;
                }
            }
        }
    }
    log.status = last_status;
}

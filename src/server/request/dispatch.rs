use super::*;

pub(in crate::server) enum DispatchAction {
    Response(DispatchOutcome),
    Tunnel(TunnelDispatch),
}

pub(in crate::server) struct DispatchOutcome {
    pub(in crate::server) response: Response,
    pub(in crate::server) error_logger: Arc<Logger>,
    pub(in crate::server) keepalive_timeout: Duration,
    pub(in crate::server) keepalive_min_timeout: Duration,
    pub(in crate::server) keep_alive: bool,
    pub(in crate::server) keepalive_header: Option<u64>,
    pub(in crate::server) send_timeout: Duration,
    pub(in crate::server) tcp_nodelay: bool,
    pub(in crate::server) limit_rate: ResponseRate,
    pub(in crate::server) server_tokens: ServerTokens,
    pub(in crate::server) chunked_transfer_encoding: bool,
    pub(in crate::server) http2_chunk_size: usize,
    pub(in crate::server) close: ConnectionCloseConfig,
    pub(in crate::server) _body_file_guard: Option<Arc<RequestBodyFile>>,
    pub(in crate::server) _connection_guard: Option<ConnectionLimitGuard>,
    pub(in crate::server) deferred_access_log: Option<Box<DeferredAccessLog>>,
    pub(in crate::server) upgrade: Option<Box<ProxyUpgrade>>,
}

pub(in crate::server) struct DeferredAccessLog {
    formats: HashMap<String, crate::config::AccessLogFormat>,
    context: EffectiveConfig,
    variables: Variables,
}

impl DeferredAccessLog {
    pub(in crate::server) fn write(mut self) {
        if !self.variables.upstream_response_time.is_empty() {
            self.variables.upstream_response_time =
                format!("{:.3}", self.variables.started.elapsed().as_secs_f64());
        }
        log_access_with_formats(&self.formats, &self.context, &self.variables);
    }

    pub(in crate::server) fn write_upgraded(self, bytes_sent: u64, body_bytes_sent: u64) {
        let mut updated = self;
        updated.variables.bytes_sent = bytes_sent;
        updated.variables.body_bytes_sent = body_bytes_sent;
        updated.write();
    }
}

pub(in crate::server) struct DispatchRequestContext<'a> {
    pub(in crate::server) request: &'a mut Request,
    pub(in crate::server) connection: ConnectionAddresses,
    pub(in crate::server) connection_request: ConnectionRequestState,
    pub(in crate::server) invalid_host: bool,
    pub(in crate::server) client_abort: Option<ClientAbortState>,
    pub(in crate::server) request_length: Option<u64>,
}

pub(in crate::server) async fn dispatch_http_request(
    snapshot: RuntimeSnapshot,
    group: &ListenerGroup,
    context: DispatchRequestContext<'_>,
    state: &SharedState,
) -> Option<DispatchAction> {
    let DispatchRequestContext {
        request,
        connection,
        connection_request,
        invalid_host,
        client_abort,
        request_length,
    } = context;
    let RuntimeSnapshot {
        config,
        listener_groups,
        upstream_keepalive_generation,
    } = snapshot;
    let Some(http) = config.http.as_ref() else {
        return Some(DispatchAction::Response(DispatchOutcome {
            response: error_response(404),
            error_logger: state.logger.clone(),
            keepalive_timeout: Duration::ZERO,
            keepalive_min_timeout: Duration::ZERO,
            keep_alive: false,
            keepalive_header: None,
            send_timeout: Duration::ZERO,
            tcp_nodelay: true,
            limit_rate: ResponseRate::default(),
            server_tokens: ServerTokens::On,
            chunked_transfer_encoding: true,
            http2_chunk_size: Http2Limits::default().chunk_size,
            close: ConnectionCloseConfig::default(),
            _body_file_guard: None,
            _connection_guard: None,
            deferred_access_log: None,
            upgrade: None,
        }));
    };
    let active_group = current_listener_group(&listener_groups, group);
    let server = select_server(http, active_group, &request.host_without_port());
    if invalid_host {
        request.keep_alive = false;
    }
    if server.context.ignore_invalid_headers && !server.context.underscores_in_headers {
        request
            .headers
            .retain(|header| !header.name.as_bytes().contains(&b'_'));
    }
    let host = match request.host_without_port() {
        host if !host.is_empty() => host.to_string(),
        _ => server
            .names
            .first()
            .map(server_name_value)
            .unwrap_or_default(),
    };
    let mut variables = Variables::new(
        request,
        &host,
        server,
        &connection.local,
        &connection.remote,
    );
    if let Some(header_length) = request_length {
        variables.request_length = header_length
            .saturating_add(request.buffered_body_len())
            .to_string();
    }
    variables.connection = connection_request.connection_id.to_string();
    variables.connection_requests = connection_request.served_requests.to_string();
    variables.pipe = if connection_request.pipelined {
        "p".into()
    } else {
        ".".into()
    };
    variables.connection_time =
        format!("{:.3}", connection_request.started.elapsed().as_secs_f64());
    if let Some((regex, captures)) = server.names.iter().find_map(|name| {
        let regex = name.regex()?;
        let captures = regex.captures(&host)?;
        Some((regex, captures))
    }) {
        set_regex_captures(&mut variables, regex, &captures);
    }
    variables.error_logs = Some(RequestErrorLogs::new(
        state.logger.clone(),
        state.http_error_loggers.read().await.clone(),
    ));
    variables.client_abort = client_abort;
    if let Some(tls) = &connection.tls {
        variables.ssl_protocol = tls.protocol.clone();
        variables.ssl_cipher = tls.cipher.clone();
        variables.ssl_server_name = tls.server_name.clone();
        variables.ssl_alpn_protocol = tls.alpn_protocol.clone();
        variables.ssl_client_verify = if tls.client_certificate.is_some() {
            "SUCCESS".into()
        } else {
            "NONE".into()
        };
        if let Some(certificate) = &tls.client_certificate {
            variables.ssl_client_cert = certificate.certificate.clone();
            variables.ssl_client_escaped_cert = certificate.escaped_certificate.clone();
            variables.ssl_client_fingerprint = certificate.fingerprint.clone();
            variables.ssl_client_serial = certificate.serial.clone();
            variables.ssl_client_s_dn = certificate.subject_dn.clone();
            variables.ssl_client_i_dn = certificate.issuer_dn.clone();
            variables.ssl_client_s_dn_legacy = certificate.subject_dn_legacy.clone();
            variables.ssl_client_i_dn_legacy = certificate.issuer_dn_legacy.clone();
            variables.ssl_client_v_start = certificate.valid_start.clone();
            variables.ssl_client_v_end = certificate.valid_end.clone();
            variables.ssl_client_v_remain = certificate.valid_remain.clone();
        }
    }
    variables.realip_remote_addr = connection.remote.variable_address();
    variables.proxy_protocol_port = connection
        .proxy_protocol_port
        .map(|port| port.to_string())
        .unwrap_or_default();
    variables.proxy_protocol_server_addr = connection
        .proxy_protocol_server_addr
        .map(|address| address.to_string())
        .unwrap_or_default();
    variables.proxy_protocol_server_port = connection
        .proxy_protocol_server_port
        .map(|port| port.to_string())
        .unwrap_or_default();
    variables.proxy_protocol_tlvs = connection.proxy_protocol_tlvs.clone();
    variables.proxy_protocol_addr = connection
        .proxy_protocol_addr
        .map(|address| address.to_string())
        .unwrap_or_default();
    let mut env = RequestEnv {
        uri: String::from("/"),
        args: None,
        unparsed_uri: None,
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    if request.method == "CONNECT" {
        // CONNECT uses an authority-form target, not a path. The request URI
        // stays available as the original target for logging and variables.
        env.uri = "/".into();
        env.args = None;
        env.unparsed_uri = Some(request.target.clone());
    } else {
        let (path, query) = request.path_and_query();
        match normalize_uri_with_merge(path, server.context.merge_slashes) {
            Some(uri) => {
                env.uri = uri;
                env.args = query.map(str::to_string);
                env.unparsed_uri = Some(match query {
                    Some(query) => format!("{path}?{query}"),
                    None => path.to_string(),
                });
            }
            None => {
                variables.uri.clear();
                variables.args.clear();
                variables.status = 400;
                let error_logger = http_logger_for_context(state, &server.context).await;
                let response = error_response(400);
                log_access(http, &server.context, &variables);
                return Some(DispatchAction::Response(DispatchOutcome {
                    response,
                    error_logger,
                    keepalive_timeout: server.context.keepalive_timeout,
                    keepalive_min_timeout: server.context.keepalive_min_timeout,
                    keep_alive: false,
                    keepalive_header: None,
                    send_timeout: server.context.send_timeout,
                    tcp_nodelay: server.context.tcp_nodelay,
                    limit_rate: response_rate(&server.context, &variables),
                    server_tokens: server.context.server_tokens,
                    chunked_transfer_encoding: server.context.chunked_transfer_encoding,
                    http2_chunk_size: server.context.http2_limits.chunk_size,
                    close: ConnectionCloseConfig::from(&server.context),
                    _body_file_guard: None,
                    _connection_guard: None,
                    deferred_access_log: None,
                    upgrade: None,
                }));
            }
        }
    }
    variables.uri = env.uri.clone();
    variables.args = env.args.clone().unwrap_or_default();
    variables.remote_addr = effective_real_ip(
        server,
        request,
        &env.uri,
        connection.remote.socket_addr().map(|address| address.ip()),
        &variables.realip_remote_addr,
        connection.proxy_protocol_addr,
    );
    variables.scheme = if group.ssl { "https" } else { "http" }.into();
    variables.https = if group.ssl { "on" } else { "" }.into();
    variables.http2 = if request.version == "HTTP/2.0" {
        if group.ssl { "h2" } else { "h2c" }.into()
    } else {
        String::new()
    };
    variables.limits = state.limits.clone();
    variables.stats = state.stats.clone();
    variables.upstream_keepalive = state.upstream_keepalive.clone();
    variables.upstream_keepalive_generation = upstream_keepalive_generation;
    variables.proxy_cache = state.proxy_cache.clone();
    variables.client_tls_cache = state.client_tls_cache.clone();

    let post_action_env = env.clone();
    let mut connection_guard = None;
    let mut processed = if invalid_host {
        let outcome = configured_error_page(
            http,
            server,
            request,
            error_response(400),
            &server.context,
            &mut variables,
            0,
        )
        .await;
        Processed {
            response: outcome.response,
            context: outcome.context,
            tunnel: false,
            upgrade: None,
        }
    } else {
        process_request_with_connection_guard(
            http,
            server,
            request,
            env,
            &mut variables,
            0,
            &mut connection_guard,
        )
        .await
    };
    let processed_logger = variables.error_logger_for(&processed.context);
    variables.set_variable_warning_context(&processed.context, processed_logger);
    let max_body = processed.context.client_max_body_size;
    let mut response = if max_body != 0 && request.buffered_body_len() > max_body {
        error_response(413)
    } else {
        processed.response
    };
    if processed.tunnel && response.status == 200 {
        variables.status = 200;
        variables.body_bytes_sent = 0;
        let tunnel = match build_tunnel_dispatch(http, &processed.context, &mut variables) {
            Ok(mut tunnel) => {
                tunnel.error_logger =
                    Some(http_logger_for_context(state, &processed.context).await);
                tunnel.connection_guard = connection_guard.take();
                tunnel
            }
            Err(response) => {
                let error_logger = http_logger_for_context(state, &processed.context).await;
                return Some(DispatchAction::Response(DispatchOutcome {
                    response: *response,
                    error_logger,
                    keepalive_timeout: processed.context.keepalive_timeout,
                    keepalive_min_timeout: processed.context.keepalive_min_timeout,
                    keep_alive: false,
                    keepalive_header: None,
                    send_timeout: processed.context.send_timeout,
                    tcp_nodelay: processed.context.tcp_nodelay,
                    limit_rate: ResponseRate::default(),
                    server_tokens: processed.context.server_tokens,
                    chunked_transfer_encoding: processed.context.chunked_transfer_encoding,
                    http2_chunk_size: processed.context.http2_limits.chunk_size,
                    close: ConnectionCloseConfig::from(&processed.context),
                    _body_file_guard: None,
                    _connection_guard: connection_guard.take(),
                    deferred_access_log: None,
                    upgrade: None,
                }));
            }
        };
        if processed.context.log_not_found {
            log_access(http, &processed.context, &variables);
        }
        return Some(DispatchAction::Tunnel(tunnel));
    }
    if let Some(upgrade) = processed.upgrade {
        variables.status = response.status;
        prepare_userid_variables(&processed.context, &mut variables);
        response.remove_header("Content-Length");
        response.remove_header("Transfer-Encoding");
        response.set_header("Connection", "upgrade");
        sync_sent_response_variables(
            &response,
            request,
            &mut variables,
            false,
            None,
            processed.context.chunked_transfer_encoding,
        );
        variables.bytes_sent = 0;
        variables.body_bytes_sent = 0;
        let should_log_access = processed.context.log_not_found || response.status != 404;
        let deferred_access_log = should_log_access.then(|| {
            Box::new(DeferredAccessLog {
                formats: http.log_formats.clone(),
                context: processed.context.clone(),
                variables: variables.clone(),
            })
        });
        let error_logger = http_logger_for_context(state, &processed.context).await;
        return Some(DispatchAction::Response(DispatchOutcome {
            response,
            error_logger,
            keepalive_timeout: processed.context.keepalive_timeout,
            keepalive_min_timeout: processed.context.keepalive_min_timeout,
            keep_alive: false,
            keepalive_header: None,
            send_timeout: processed.context.send_timeout,
            tcp_nodelay: processed.context.tcp_nodelay,
            limit_rate: ResponseRate::default(),
            server_tokens: processed.context.server_tokens,
            chunked_transfer_encoding: processed.context.chunked_transfer_encoding,
            http2_chunk_size: processed.context.http2_limits.chunk_size,
            close: ConnectionCloseConfig::from(&processed.context),
            _body_file_guard: variables.request_body_file_guard.clone(),
            _connection_guard: connection_guard,
            deferred_access_log,
            upgrade: Some(upgrade),
        }));
    }
    if matches!(response.status, 444 | 499) {
        variables.status = response.status;
        start_post_action(
            http,
            server,
            request,
            &post_action_env,
            &variables,
            &processed.context,
            0,
        );
        return None;
    }
    variables.status = response.status;
    prepare_userid_variables(&processed.context, &mut variables);
    maybe_gunzip(&mut response, &processed.context, request).await;
    apply_addition(
        http,
        server,
        request,
        &mut response,
        &processed.context,
        &mut variables,
    )
    .await;
    apply_sub_filters(&mut response, &processed.context, &variables).await;
    apply_charset(&mut response, &processed.context, &variables).await;
    apply_ssi(
        http,
        server,
        request,
        &mut response,
        &processed.context,
        &mut variables,
        0,
    )
    .await;
    maybe_gzip(&mut response, &processed.context, request).await;
    let conditional = evaluate_conditional_request(&response, request, &processed.context);
    if conditional == ConditionalResult::PreconditionFailed {
        let outcome = configured_error_page(
            http,
            server,
            request,
            error_response(412),
            &processed.context,
            &mut variables,
            0,
        )
        .await;
        let mut replaced = outcome.response;
        replaced.inherit_early_hints_from(&response);
        response = replaced;
        processed.context = outcome.context;
        let processed_logger = variables.error_logger_for(&processed.context);
        variables.set_variable_warning_context(&processed.context, processed_logger);
    }
    apply_expires(&mut response, &processed.context, &variables);
    apply_response_headers(&mut response, &processed.context, request, &mut variables);
    apply_userid(&mut response, &processed.context, &mut variables);
    apply_msie_compat(&processed.context, request, &mut response);
    apply_redirect_location(
        &mut response,
        &processed.context,
        server,
        request,
        &connection.local,
        group.ssl,
    );
    if conditional == ConditionalResult::NotModified {
        finalize_not_modified(&mut response);
    }
    apply_response_range(&mut response, request, processed.context.max_ranges);
    apply_response_trailers(&mut response, &processed.context, request, &mut variables);
    let keep_alive = should_keep_alive(
        request,
        &processed.context,
        &response,
        false,
        connection_request,
    );
    let keepalive_header = keepalive_header_timeout(&processed.context, keep_alive);
    let response_options = ResponseWriteOptions {
        request_method: &request.method,
        request_version: &request.version,
        keep_alive,
        keep_alive_header: keepalive_header,
        server_tokens: processed.context.server_tokens,
        chunked_transfer_encoding: processed.context.chunked_transfer_encoding,
        rate: ResponseRate::default(),
    };
    let chunked = prepare_response_for_write(&mut response, &response_options);
    sync_sent_response_variables(
        &response,
        request,
        &mut variables,
        keep_alive,
        keepalive_header,
        processed.context.chunked_transfer_encoding,
    );
    let limit_rate =
        response_rate_with_upstream(response_rate(&processed.context, &variables), &response);
    variables.status = response.status;
    variables.bytes_sent = response_head_length(&response).saturating_add(response_wire_body_len(
        &response,
        &request.method,
        chunked,
    ));
    variables.body_bytes_sent = response.body_len();

    let should_log_access = processed.context.log_not_found || response.status != 404;
    let deferred_access_log = (should_log_access && response.body.is_stream()).then(|| {
        Box::new(DeferredAccessLog {
            formats: http.log_formats.clone(),
            context: processed.context.clone(),
            variables: variables.clone(),
        })
    });
    if should_log_access && deferred_access_log.is_none() {
        log_access(http, &processed.context, &variables);
    }
    start_post_action(
        http,
        server,
        request,
        &post_action_env,
        &variables,
        &processed.context,
        0,
    );

    let error_logger = http_logger_for_context(state, &processed.context).await;
    Some(DispatchAction::Response(DispatchOutcome {
        response,
        error_logger,
        keepalive_timeout: processed.context.keepalive_timeout,
        keepalive_min_timeout: processed.context.keepalive_min_timeout,
        keep_alive,
        keepalive_header,
        send_timeout: processed.context.send_timeout,
        tcp_nodelay: processed.context.tcp_nodelay,
        limit_rate,
        server_tokens: processed.context.server_tokens,
        chunked_transfer_encoding: processed.context.chunked_transfer_encoding,
        http2_chunk_size: processed.context.http2_limits.chunk_size,
        close: ConnectionCloseConfig::from(&processed.context),
        _body_file_guard: variables.request_body_file_guard.clone(),
        _connection_guard: connection_guard,
        deferred_access_log,
        upgrade: None,
    }))
}

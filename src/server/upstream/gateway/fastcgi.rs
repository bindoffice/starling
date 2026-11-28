use super::*;

mod codec;
mod protocol;
mod streaming;
pub(in crate::server) use codec::*;
pub(in crate::server) use protocol::*;
use streaming::*;

pub(in crate::server) async fn fastcgi_request(
    fastcgi: &FastCgiPassConfig,
    http: &HttpConfig,
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Response {
    if let Some(client_abort) = variables.client_abort.as_ref() {
        client_abort.mark_upstream_active();
    }
    let streaming_request_body = request_body_stream_enabled(request, context);
    let keepalive = fastcgi_keepalive_context(fastcgi, http, context);
    let cache = prepare_gateway_cache(&context.fastcgi_cache, request, variables);
    let background_update = cache.as_ref().and_then(|plan| {
        plan.wants_background_update(variables)
            .then(|| GatewayBackgroundUpdate::FastCgi {
                fastcgi: fastcgi.clone(),
                http: http.clone(),
                request: request.clone(),
                env: env.clone(),
                context: context.clone(),
                variables: variables.clone(),
            })
    });
    let GatewayCacheLookup {
        response: immediate,
        mut stale,
        revalidation,
        store_allowed: cache_store_allowed,
        lock_guard: mut cache_lock_guard,
    } = match &cache {
        Some(plan) => lookup_gateway_cache(plan, request, variables, background_update).await,
        None => GatewayCacheLookup::default(),
    };
    if let Some(response) = immediate {
        return response;
    }
    let upstream_request =
        prepare_gateway_upstream_request(request, cache.is_some(), revalidation.as_ref());
    let request = upstream_request.as_ref().unwrap_or(request);
    let targets = resolve_fastcgi_targets(fastcgi, http, variables, request_peer(request));
    let mut attempted = Vec::new();
    let mut last_failure = ProxyFailure::Failure;
    let mut next = NextUpstreamState::new(&context.fastcgi_next_upstream);
    let mut targets = targets.into_iter();
    let mut upstream_statuses: Vec<String> = Vec::new();
    let mut upstream_response_times: Vec<String> = Vec::new();
    while let Some(target) = targets.next() {
        let mut guard = match target.try_acquire() {
            Ok(guard) => guard,
            Err(()) => continue,
        };
        if !next.start_attempt() {
            break;
        }
        attempted.push(upstream_target_display(&target));
        let attempt_started = Instant::now();
        match fastcgi_attempt(
            request,
            env,
            context,
            variables,
            (&target.host, target.port),
            keepalive.as_ref(),
        )
        .await
        {
            Ok((mut response, reusable)) => {
                let response_time = attempt_started.elapsed();
                upstream_statuses.push(response.status.to_string());
                upstream_response_times.push(format!("{:.3}", response_time.as_secs_f64()));
                if !streaming_request_body
                    && !targets.as_slice().is_empty()
                    && next.can_retry_status(request, response.status)
                {
                    if let Some(guard) = guard.as_mut() {
                        guard.mark_failed();
                    }
                    continue;
                }
                if let Some(guard) = guard.as_mut() {
                    guard.record_timing(UpstreamAttemptTiming::response_only(response_time));
                }
                if response.status == 304
                    && let Some(plan) = &cache
                    && let Some(cached) = stale.as_ref()
                    && let Some(valid) = revalidated_gateway_validity(
                        &response,
                        cached,
                        &context.fastcgi_cache,
                        &context.fastcgi_ignore_headers,
                    )
                {
                    if cache_store_allowed {
                        let _ = plan.zone.refresh(&plan.key, cached.created, &valid).await;
                    }
                    let cached = stale.take().expect("expired gateway cache response");
                    variables.upstream_addr = attempted.join(", ");
                    variables.upstream_status = upstream_statuses.join(", ");
                    variables.upstream_response_time = upstream_response_times.join(", ");
                    return proxy_response_from_revalidated(cached, variables);
                }
                if let Some(plan) = &cache
                    && plan.use_stale_status(response.status)
                    && let Some(stale) = stale.take()
                {
                    variables.upstream_addr = attempted.join(", ");
                    variables.upstream_status = upstream_statuses.join(", ");
                    variables.upstream_response_time = upstream_response_times.join(", ");
                    return proxy_response_from_stale(stale, variables);
                }
                let cache_validity = if let Some(plan) = &cache
                    && cache_store_allowed
                    && let Some(valid) = gateway_cache_validity(
                        &response,
                        &context.fastcgi_cache,
                        &context.fastcgi_ignore_headers,
                    )
                    && plan.note_use().await
                {
                    Some(valid)
                } else {
                    None
                };
                if let (Some(keepalive), Some(connection)) = (keepalive.as_ref(), reusable) {
                    variables.upstream_keepalive.put_for(
                        variables.upstream_keepalive_worker,
                        &keepalive.name,
                        &keepalive.config,
                        connection,
                    );
                }
                variables.upstream_addr = attempted.join(", ");
                variables.upstream_status = upstream_statuses.join(", ");
                variables.upstream_response_time = upstream_response_times.join(", ");
                if let (Some(plan), Some(valid)) = (&cache, cache_validity) {
                    if intercepts_upstream_status(
                        response.status,
                        context.fastcgi_intercept_errors,
                        &context.error_pages,
                    ) {
                        plan.zone.store_error(&plan.key, response.status, &valid);
                    } else {
                        mark_cache_fill_ranges(&mut response);
                        response = plan
                            .zone
                            .store_or_tee(&plan.key, response, &valid, cache_lock_guard.take())
                            .await;
                    }
                }
                return response;
            }
            Err(failure) => {
                upstream_statuses.push("-".into());
                upstream_response_times
                    .push(format!("{:.3}", attempt_started.elapsed().as_secs_f64()));
                if let Some(guard) = guard.as_mut() {
                    guard.mark_failed();
                }
                last_failure = failure;
                if streaming_request_body
                    || targets.as_slice().is_empty()
                    || !next.can_retry_failure(request, &last_failure)
                {
                    break;
                }
            }
        }
    }
    variables.upstream_addr = attempted.join(", ");
    variables.upstream_status = upstream_statuses.join(", ");
    variables.upstream_response_time = upstream_response_times.join(", ");
    if let Some(plan) = &cache
        && let Some(cached) = stale.as_ref()
        && plan.use_stale_error(cached, &last_failure)
    {
        let stale = stale.take().expect("stale gateway cache response");
        return proxy_response_from_stale(stale, variables);
    }
    if let Some(plan) = &cache
        && cache_store_allowed
        && !variables.proxy_cache_background
    {
        let status = proxy_failure_status(&last_failure);
        if matches!(status, 502 | 504)
            && let Some(valid) = configured_cache_validity(&context.fastcgi_cache.valid, status)
            && plan.note_use().await
        {
            plan.zone.store_error(&plan.key, status, &valid);
        }
    }
    match last_failure {
        ProxyFailure::Timeout => error_response(504),
        ProxyFailure::Failure | ProxyFailure::InvalidHeader => error_response(502),
        ProxyFailure::RequestBody(status) => error_response(status),
    }
}

fn fastcgi_keepalive_context(
    fastcgi: &FastCgiPassConfig,
    http: &HttpConfig,
    context: &EffectiveConfig,
) -> Option<UpstreamKeepaliveContext> {
    if !context.fastcgi_keep_conn {
        return None;
    }
    gateway_keepalive_context(&fastcgi.raw, http)
}

pub(in crate::server) async fn fastcgi_attempt(
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    variables: &Variables,
    target: (&str, u16),
    keepalive: Option<&UpstreamKeepaliveContext>,
) -> Result<(Response, Option<UpstreamConnection>), ProxyFailure> {
    let (host, port) = target;
    let key = fastcgi_upstream_pool_key(host, port, context, variables);
    let mut connection = match keepalive {
        Some(keepalive) => variables.upstream_keepalive.take_for(
            variables.upstream_keepalive_worker,
            &keepalive.name,
            &key,
            &keepalive.config,
        ),
        None => None,
    };
    if connection.is_none() {
        connection = Some(open_fastcgi_connection(host, port, context, variables, key).await?);
    }
    let connection = connection.expect("FastCGI connection");
    match fastcgi_exchange_connection(connection, request, env, context, variables, keepalive).await
    {
        Ok((response, reusable)) => Ok((response, reusable.filter(|_| keepalive.is_some()))),
        Err(failure) => Err(failure),
    }
}

async fn open_fastcgi_connection(
    host: &str,
    port: u16,
    context: &EffectiveConfig,
    variables: &Variables,
    key: UpstreamPoolKey,
) -> Result<UpstreamConnection, ProxyFailure> {
    let socket_options = UpstreamSocketOptions::for_fastcgi(context);
    let local_bind = evaluate_fastcgi_bind(context, variables);
    let io = open_gateway_io(
        host,
        port,
        context.fastcgi_connect_timeout,
        socket_options,
        local_bind.as_deref(),
        "fastcgi_bind",
    )
    .await?;
    Ok(UpstreamConnection::new(io, key))
}

fn fastcgi_upstream_pool_key(
    host: &str,
    port: u16,
    context: &EffectiveConfig,
    variables: &Variables,
) -> UpstreamPoolKey {
    UpstreamPoolKey {
        generation: variables.upstream_keepalive_generation,
        scheme: "fastcgi".into(),
        host: host.to_string(),
        port,
        tls_name: None,
        tls_verify: false,
        tls_trusted_certificate: None,
        tls_verify_depth: None,
        tls_crl: None,
        tls_client_certificate: None,
        tls_client_certificate_key: None,
        tls_protocols: None,
        tls_ciphers: None,
        proxy_bind: evaluate_fastcgi_bind(context, variables),
        socket_keepalive: context.fastcgi_socket_keepalive,
        socket_rcvbuf: context.fastcgi_socket_rcvbuf,
        socket_sndbuf: context.fastcgi_socket_sndbuf,
        http_version: "fastcgi".into(),
    }
}

async fn fastcgi_exchange_connection(
    mut connection: UpstreamConnection,
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    variables: &Variables,
    keepalive: Option<&UpstreamKeepaliveContext>,
) -> Result<(Response, Option<UpstreamConnection>), ProxyFailure> {
    connection.requests = connection.requests.saturating_add(1);
    let body_stream = context
        .fastcgi_pass_request_body
        .then(|| request_body_stream_for_upstream(request, context))
        .flatten()
        .cloned();
    write_fastcgi_request_with_stream(
        &mut connection.reader,
        request,
        env,
        context,
        variables,
        FastCgiRequestBody {
            stream: body_stream.as_ref(),
            limit: context.client_max_body_size,
            send_timeout: context.fastcgi_send_timeout,
        },
    )
    .await
    .map_err(proxy_failure_from_io)?;
    let limit_rate = complex_size(context.fastcgi_limit_rate.as_deref(), variables);
    let policy = GatewayStreamingPolicy {
        buffering: &context.fastcgi_buffering,
        ignore_headers: &context.fastcgi_ignore_headers,
        force_ranges: context.fastcgi_force_ranges,
        intercept_errors: context.fastcgi_intercept_errors,
    };
    let outcome = read_fastcgi_response_streaming(
        connection.reader,
        FastCgiResponseReadOptions {
            catch_stderr: &context.fastcgi_catch_stderr,
            limit_rate,
            max_header_bytes: cgi_header_limit(&context.fastcgi_buffering),
            buffer_policy: UpstreamBodyBufferPolicy::upstream(
                &context.fastcgi_buffering,
                !ignores_accel_buffering(&context.fastcgi_ignore_headers),
            ),
            streaming: Some(&policy),
            context,
            read_timeout: context.fastcgi_read_timeout,
        },
    )
    .await
    .map_err(proxy_failure_from_io)?;
    let (mut response, reusable) = match outcome {
        GatewayResponseRead::Buffered {
            response,
            reader,
            protocol_complete,
        } => {
            connection.reader = reader;
            let reusable = if protocol_complete {
                connection.reader.buffer().is_empty().then_some(connection)
            } else {
                if let Some(keepalive) = keepalive {
                    spawn_fastcgi_keepalive_drain(
                        connection,
                        context.fastcgi_read_timeout,
                        variables.upstream_keepalive_worker,
                        variables.upstream_keepalive.clone(),
                        keepalive.name.clone(),
                        keepalive.config.clone(),
                    );
                }
                None
            };
            (response, reusable)
        }
        GatewayResponseRead::Streamed(response) => (response, None),
    };
    // Nginx does not consume a FastCGI response body for HEAD requests, so
    // the connection cannot be returned to the keepalive pool.
    let reusable = if request.method.eq_ignore_ascii_case("HEAD") {
        None
    } else {
        reusable
    };
    filter_gateway_response_headers(
        &mut response,
        FASTCGI_DEFAULT_HIDE_HEADERS,
        &context.fastcgi_hide_headers,
        &context.fastcgi_pass_headers,
        &context.fastcgi_ignore_headers,
    );
    Ok((response, reusable))
}

fn spawn_fastcgi_keepalive_drain(
    mut connection: UpstreamConnection,
    read_timeout: Duration,
    worker: usize,
    pool: Arc<UpstreamKeepalivePool>,
    upstream: Arc<str>,
    config: UpstreamKeepaliveConfig,
) {
    tokio::spawn(async move {
        loop {
            match read_fastcgi_record_with_timeout(&mut connection.reader, read_timeout).await {
                Ok(record) if record.record_type == FCGI_END_REQUEST => {
                    if connection.reader.buffer().is_empty() {
                        pool.put_for(worker, &upstream, &config, connection);
                    }
                    return;
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });
}

#[cfg(test)]
pub(in crate::server) async fn fastcgi_exchange<S>(
    mut stream: S,
    request: &Request,
    env: &RequestEnv,
    _location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<Response, ProxyFailure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let body_stream = context
        .fastcgi_pass_request_body
        .then(|| request_body_stream_for_upstream(request, context))
        .flatten()
        .cloned();
    write_fastcgi_request_with_stream(
        &mut stream,
        request,
        env,
        context,
        variables,
        FastCgiRequestBody {
            stream: body_stream.as_ref(),
            limit: context.client_max_body_size,
            send_timeout: context.fastcgi_send_timeout,
        },
    )
    .await
    .map_err(proxy_failure_from_io)?;
    let policy = GatewayStreamingPolicy {
        buffering: &context.fastcgi_buffering,
        ignore_headers: &context.fastcgi_ignore_headers,
        force_ranges: context.fastcgi_force_ranges,
        intercept_errors: context.fastcgi_intercept_errors,
    };
    let outcome = read_fastcgi_response_streaming(
        stream,
        FastCgiResponseReadOptions {
            catch_stderr: &context.fastcgi_catch_stderr,
            limit_rate: 0,
            max_header_bytes: cgi_header_limit(&context.fastcgi_buffering),
            buffer_policy: UpstreamBodyBufferPolicy::upstream(
                &context.fastcgi_buffering,
                !ignores_accel_buffering(&context.fastcgi_ignore_headers),
            ),
            streaming: Some(&policy),
            context,
            read_timeout: context.fastcgi_read_timeout,
        },
    )
    .await
    .map_err(proxy_failure_from_io)?;
    let mut response = match outcome {
        GatewayResponseRead::Buffered { response, .. } => response,
        GatewayResponseRead::Streamed(response) => response,
    };
    filter_gateway_response_headers(
        &mut response,
        FASTCGI_DEFAULT_HIDE_HEADERS,
        &context.fastcgi_hide_headers,
        &context.fastcgi_pass_headers,
        &context.fastcgi_ignore_headers,
    );
    Ok(response)
}

use super::*;

mod request;

use request::{uwsgi_upstream_pool_key, write_uwsgi_request};

pub(in crate::server) async fn uwsgi_request(
    uwsgi: &UwsgiPassConfig,
    http: &HttpConfig,
    request: &Request,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Response {
    if let Some(client_abort) = variables.client_abort.as_ref() {
        client_abort.mark_upstream_active();
    }
    let streaming_request_body = request_body_stream_enabled(request, context);
    let cache = prepare_gateway_cache(&context.uwsgi_cache, request, variables);
    let background_update = cache.as_ref().and_then(|plan| {
        plan.wants_background_update(variables)
            .then(|| GatewayBackgroundUpdate::Uwsgi {
                uwsgi: uwsgi.clone(),
                http: http.clone(),
                request: request.clone(),
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
    let keepalive = gateway_keepalive_context(strip_uwsgi_scheme(&uwsgi.raw).0, http);
    let (targets, tls) = resolve_uwsgi_targets(uwsgi, http, variables, request_peer(request));
    let mut attempted = Vec::new();
    let mut last_failure = ProxyFailure::Failure;
    let mut next = NextUpstreamState::new(&context.uwsgi_next_upstream);
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
        match uwsgi_attempt_with_keepalive(
            request,
            context,
            variables,
            (&target.host, target.port),
            tls,
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
                        &context.uwsgi_cache,
                        &context.uwsgi_ignore_headers,
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
                        &context.uwsgi_cache,
                        &context.uwsgi_ignore_headers,
                    )
                    && plan.note_use().await
                {
                    Some(valid)
                } else {
                    None
                };
                if let (Some(keepalive), Some(connection)) = (keepalive.as_ref(), reusable) {
                    variables
                        .upstream_keepalive
                        .put(&keepalive.name, &keepalive.config, connection)
                        .await;
                }
                variables.upstream_addr = attempted.join(", ");
                variables.upstream_status = upstream_statuses.join(", ");
                variables.upstream_response_time = upstream_response_times.join(", ");
                if let (Some(plan), Some(valid)) = (&cache, cache_validity) {
                    if intercepts_upstream_status(
                        response.status,
                        context.uwsgi_intercept_errors,
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
            && let Some(valid) = configured_cache_validity(&context.uwsgi_cache.valid, status)
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

#[cfg(test)]
pub(in crate::server) async fn uwsgi_attempt(
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
    target: (&str, u16),
    tls: bool,
) -> Result<Response, ProxyFailure> {
    uwsgi_attempt_with_keepalive(request, context, variables, target, tls, None)
        .await
        .map(|(response, _)| response)
}

async fn uwsgi_attempt_with_keepalive(
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
    target: (&str, u16),
    tls: bool,
    keepalive: Option<&UpstreamKeepaliveContext>,
) -> Result<(Response, Option<UpstreamConnection>), ProxyFailure> {
    let (host, port) = target;
    let key = uwsgi_upstream_pool_key(host, port, tls, context, variables);
    let mut connection = match keepalive {
        Some(keepalive) => {
            variables
                .upstream_keepalive
                .take(&keepalive.name, &key, &keepalive.config)
                .await
        }
        None => None,
    };
    if connection.is_none() {
        let socket_options = UpstreamSocketOptions::for_uwsgi(context);
        let local_bind = evaluate_uwsgi_bind(context, variables);
        let stream = open_gateway_io(
            host,
            port,
            context.uwsgi_connect_timeout,
            socket_options,
            local_bind.as_deref(),
            "uwsgi_bind",
        )
        .await?;
        let stream: BoxedUpstreamIo = if tls {
            match tokio::time::timeout(
                context.uwsgi_connect_timeout,
                upgrade_uwsgi_tls(stream, host, context, variables),
            )
            .await
            {
                Ok(Ok(stream)) => Box::new(stream),
                Ok(Err(failure)) => return Err(failure),
                Err(_) => return Err(ProxyFailure::Timeout),
            }
        } else {
            stream
        };
        connection = Some(UpstreamConnection::new(stream, key));
    }
    uwsgi_exchange_connection(
        connection.expect("uWSGI connection"),
        request,
        context,
        variables,
    )
    .await
}

#[cfg(test)]
pub(in crate::server) async fn uwsgi_exchange<S>(
    stream: S,
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<Response, ProxyFailure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    uwsgi_exchange_with_cache(stream, request, context, variables).await
}

async fn uwsgi_exchange_connection(
    mut connection: UpstreamConnection,
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<(Response, Option<UpstreamConnection>), ProxyFailure> {
    connection.requests = connection.requests.saturating_add(1);
    write_uwsgi_request(&mut connection.reader, request, context, variables).await?;
    let outcome = read_cgi_response(
        connection.reader,
        complex_size(context.uwsgi_limit_rate.as_deref(), variables),
        cgi_header_limit(&context.uwsgi_buffering),
        UpstreamBodyBufferPolicy::upstream(
            &context.uwsgi_buffering,
            !ignores_accel_buffering(&context.uwsgi_ignore_headers),
        ),
        Some(&GatewayStreamingPolicy {
            buffering: &context.uwsgi_buffering,
            ignore_headers: &context.uwsgi_ignore_headers,
            force_ranges: context.uwsgi_force_ranges,
            intercept_errors: context.uwsgi_intercept_errors,
        }),
        context,
        context.uwsgi_read_timeout,
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
            let reusable =
                protocol_complete && request.method != "HEAD" && connection.idle_is_open();
            (response, reusable.then_some(connection))
        }
        GatewayResponseRead::Streamed(response) => (response, None),
    };
    filter_gateway_response_headers(
        &mut response,
        UWSGI_DEFAULT_HIDE_HEADERS,
        &context.uwsgi_hide_headers,
        &context.uwsgi_pass_headers,
        &context.uwsgi_ignore_headers,
    );
    Ok((response, reusable))
}

#[cfg(test)]
async fn uwsgi_exchange_with_cache<S>(
    mut stream: S,
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<Response, ProxyFailure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    write_uwsgi_request(&mut stream, request, context, variables).await?;
    let limit_rate = complex_size(context.uwsgi_limit_rate.as_deref(), variables);
    let policy = GatewayStreamingPolicy {
        buffering: &context.uwsgi_buffering,
        ignore_headers: &context.uwsgi_ignore_headers,
        force_ranges: context.uwsgi_force_ranges,
        intercept_errors: context.uwsgi_intercept_errors,
    };
    let outcome = read_cgi_response(
        stream,
        limit_rate,
        cgi_header_limit(&context.uwsgi_buffering),
        UpstreamBodyBufferPolicy::upstream(
            &context.uwsgi_buffering,
            !ignores_accel_buffering(&context.uwsgi_ignore_headers),
        ),
        Some(&policy),
        context,
        context.uwsgi_read_timeout,
    )
    .await
    .map_err(proxy_failure_from_io)?;
    let mut response = match outcome {
        GatewayResponseRead::Buffered { response, .. } => response,
        GatewayResponseRead::Streamed(response) => response,
    };
    filter_gateway_response_headers(
        &mut response,
        UWSGI_DEFAULT_HIDE_HEADERS,
        &context.uwsgi_hide_headers,
        &context.uwsgi_pass_headers,
        &context.uwsgi_ignore_headers,
    );
    Ok(response)
}

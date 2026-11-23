use super::*;

pub(in crate::server) async fn scgi_request(
    scgi: &ScgiPassConfig,
    http: &HttpConfig,
    request: &Request,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Response {
    if let Some(client_abort) = variables.client_abort.as_ref() {
        client_abort.mark_upstream_active();
    }
    let streaming_request_body = request_body_stream_enabled(request, context);
    let cache = prepare_gateway_cache(&context.scgi_cache, request, variables);
    let background_update = cache.as_ref().and_then(|plan| {
        plan.wants_background_update(variables)
            .then(|| GatewayBackgroundUpdate::Scgi {
                scgi: scgi.clone(),
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
    let revalidated_request = revalidation.map(|revalidation| {
        let mut outgoing = request.clone();
        revalidation.apply_to_request(&mut outgoing);
        outgoing
    });
    let request = revalidated_request.as_ref().unwrap_or(request);
    let targets = resolve_gateway_targets(&scgi.raw, http, variables, request_peer(request));
    let mut attempted = Vec::new();
    let mut last_failure = ProxyFailure::Failure;
    let mut next = NextUpstreamState::new(&context.scgi_next_upstream);
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
        match scgi_attempt_with_cache(request, context, variables, (&target.host, target.port))
            .await
        {
            Ok(mut response) => {
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
                        &context.scgi_cache,
                        &context.scgi_ignore_headers,
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
                        &context.scgi_cache,
                        &context.scgi_ignore_headers,
                    )
                    && plan.note_use().await
                {
                    Some(valid)
                } else {
                    None
                };
                variables.upstream_addr = attempted.join(", ");
                variables.upstream_status = upstream_statuses.join(", ");
                variables.upstream_response_time = upstream_response_times.join(", ");
                if let (Some(plan), Some(valid)) = (&cache, cache_validity) {
                    if intercepts_upstream_status(
                        response.status,
                        context.scgi_intercept_errors,
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
            && let Some(valid) = configured_cache_validity(&context.scgi_cache.valid, status)
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
pub(in crate::server) async fn scgi_attempt(
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
    target: (&str, u16),
) -> Result<Response, ProxyFailure> {
    scgi_attempt_with_cache(request, context, variables, target).await
}

async fn scgi_attempt_with_cache(
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
    target: (&str, u16),
) -> Result<Response, ProxyFailure> {
    let (host, port) = target;
    let socket_options = UpstreamSocketOptions::for_scgi(context);
    let local_bind = evaluate_scgi_bind(context, variables);
    let stream = open_gateway_io(
        host,
        port,
        context.scgi_connect_timeout,
        socket_options,
        local_bind.as_deref(),
        "scgi_bind",
    )
    .await?;
    scgi_exchange_with_cache(stream, request, context, variables).await
}

#[cfg(test)]
pub(in crate::server) async fn scgi_exchange<S>(
    stream: S,
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<Response, ProxyFailure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    scgi_exchange_with_cache(stream, request, context, variables).await
}

async fn scgi_exchange_with_cache<S>(
    mut stream: S,
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<Response, ProxyFailure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let streaming_request_body = request_body_stream_enabled(request, context);
    let packet = build_scgi_header(request, context, variables, streaming_request_body)
        .map_err(|_| ProxyFailure::Failure)?;
    {
        write_all_with_timeout(&mut stream, &packet, context.scgi_send_timeout)
            .await
            .map_err(proxy_failure_from_io)?;
        if let Some(body_stream) = context
            .scgi_pass_request_body
            .then(|| request_body_stream_for_upstream(request, context))
            .flatten()
            .cloned()
        {
            let mut total = 0_u64;
            while let Some(chunk) = body_stream
                .next_limited(&mut total, context.client_max_body_size)
                .await
                .map_err(proxy_failure_from_io)?
            {
                write_all_with_timeout(&mut stream, &chunk, context.scgi_send_timeout)
                    .await
                    .map_err(proxy_failure_from_io)?;
            }
        } else if context.scgi_pass_request_body {
            write_all_with_timeout(&mut stream, &request.body, context.scgi_send_timeout)
                .await
                .map_err(proxy_failure_from_io)?;
        }
        flush_with_timeout(&mut stream, context.scgi_send_timeout)
            .await
            .map_err(proxy_failure_from_io)?;
    }
    let limit_rate = complex_size(context.scgi_limit_rate.as_deref(), variables);
    let policy = GatewayStreamingPolicy {
        buffering: &context.scgi_buffering,
        ignore_headers: &context.scgi_ignore_headers,
        force_ranges: context.scgi_force_ranges,
        intercept_errors: context.scgi_intercept_errors,
    };
    let outcome = read_cgi_response(
        stream,
        limit_rate,
        context.scgi_buffering.buffer_size,
        UpstreamBodyBufferPolicy::upstream(
            &context.scgi_buffering,
            !ignores_accel_buffering(&context.scgi_ignore_headers),
        ),
        Some(&policy),
        context,
        context.scgi_read_timeout,
    )
    .await
    .map_err(proxy_failure_from_io)?;
    let mut response = match outcome {
        GatewayResponseRead::Buffered { response, .. } => response,
        GatewayResponseRead::Streamed(response) => response,
    };
    filter_gateway_response_headers(
        &mut response,
        SCGI_DEFAULT_HIDE_HEADERS,
        &context.scgi_hide_headers,
        &context.scgi_pass_headers,
        &context.scgi_ignore_headers,
    );
    Ok(response)
}

pub(in crate::server) fn build_scgi_header(
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
    streaming_request_body: bool,
) -> io::Result<Vec<u8>> {
    let mut parameters = Vec::new();
    for parameter in &context.scgi_params {
        let value = eval_variables(&parameter.value, variables);
        if parameter.if_not_empty && value.is_empty() {
            continue;
        }
        parameters.push((parameter.name.clone(), value));
    }
    if context.scgi_pass_request_headers {
        append_cgi_request_headers(&mut parameters, &request.headers);
    }
    if !parameters
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("CONTENT_LENGTH"))
    {
        let content_length = request
            .header("content-length")
            .map(str::to_string)
            .or_else(|| request.body_file().map(|file| file.len().to_string()))
            .unwrap_or_else(|| {
                if streaming_request_body {
                    String::new()
                } else {
                    request.body.len().to_string()
                }
            });
        parameters.push(("CONTENT_LENGTH".into(), content_length));
    }
    let mut parameters = encode_null_separated_params(&parameters);
    parameters.push(b',');
    let mut packet = Vec::with_capacity(parameters.len() + 24);
    packet.extend_from_slice(parameters.len().to_string().as_bytes());
    packet.push(b':');
    packet.extend_from_slice(&parameters);
    Ok(packet)
}

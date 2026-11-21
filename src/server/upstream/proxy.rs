use super::*;

mod cache;
mod response;
mod result;
mod transport;
use cache::*;
use response::finalize_proxy_response;
use result::ProxyAttemptTarget;
pub(in crate::server) use result::{ProxyAttemptResult, ReusableUpstreamConnection};
pub(in crate::server) use transport::*;

pub(in crate::server) async fn proxy_request(
    proxy: &ProxyPassConfig,
    http: &HttpConfig,
    request: &Request,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Response {
    if let Some(client_abort) = variables.client_abort.as_ref() {
        client_abort.mark_upstream_active();
    }
    let keepalive = proxy.upstream.as_ref().and_then(|name| {
        let upstream = http.upstreams.get(name)?;
        let config = upstream.keepalive.clone()?;
        (config.max_cached != 0).then(|| UpstreamKeepaliveContext {
            name: name.clone(),
            config,
        })
    });
    let cache = prepare_proxy_cache(proxy, request, context, variables);
    let mut cache_store_allowed = cache
        .as_ref()
        .is_some_and(|plan| plan.allows_store(request));
    let mut cache_lock_guard = None;
    let mut stale = None;
    let mut revalidation = None;
    if let Some(plan) = &cache
        && !plan.bypass
    {
        let mut lookup = match plan.zone.lookup(&plan.key).await {
            Ok(lookup) => lookup,
            Err(_) => CacheLookup::Miss,
        };
        let mut expired = false;
        lookup = match std::mem::replace(&mut lookup, CacheLookup::Miss) {
            CacheLookup::Expired(cached) => {
                stale = Some(cached);
                expired = true;
                CacheLookup::Miss
            }
            lookup => lookup,
        };
        if !variables.proxy_cache_background
            && expired
            && stale
                .as_ref()
                .is_some_and(|cached| allows_stale_updating(&plan.use_stale, cached))
            && plan.zone.is_updating(&plan.key)
        {
            return proxy_response_from_updating(
                stale.take().expect("expired cache response"),
                variables,
            );
        }
        if !variables.proxy_cache_background
            && expired
            && plan.background_update
            && stale
                .as_ref()
                .is_some_and(|cached| allows_stale_updating(&plan.use_stale, cached))
        {
            return match plan.zone.try_acquire_update_lock(&plan.key).await {
                Some(guard) => {
                    start_background_update(
                        BackgroundUpdateContext {
                            proxy,
                            http,
                            request,
                            env,
                            location,
                            config: context,
                            variables,
                        },
                        guard,
                    );
                    proxy_response_from_stale(
                        stale.take().expect("expired cache response"),
                        variables,
                    )
                }
                None => proxy_response_from_updating(
                    stale.take().expect("expired cache response"),
                    variables,
                ),
            };
        }
        if !variables.proxy_cache_background
            && !matches!(lookup, CacheLookup::Hit(_) | CacheLookup::Error(_))
            && plan.lock.enabled
        {
            match plan.zone.acquire_lock(&plan.key, &plan.lock).await {
                CacheLockOutcome::Acquired(guard) => {
                    cache_lock_guard = Some(guard);
                    lookup = match plan.zone.lookup(&plan.key).await {
                        Ok(lookup) => lookup,
                        Err(_) => CacheLookup::Miss,
                    };
                    lookup = match std::mem::replace(&mut lookup, CacheLookup::Miss) {
                        CacheLookup::Expired(cached) => {
                            stale = Some(cached);
                            expired = true;
                            CacheLookup::Miss
                        }
                        lookup => lookup,
                    };
                }
                CacheLockOutcome::TimedOut => {
                    cache_store_allowed = false;
                }
            }
        }
        if let Some(response) = cached_lookup_response(lookup, variables) {
            return response;
        }
        if expired {
            variables.upstream_cache_status = "EXPIRED".into();
            if plan.revalidate {
                revalidation = stale.as_ref().map(CacheRevalidation::from_cached);
            }
        } else {
            variables.upstream_cache_status = "MISS".into();
        }
    }
    let upstream_request = proxy_cache_upstream_request(request, cache.as_ref());
    let cache_head_conversion = upstream_request.is_some();
    let request = upstream_request.as_ref().unwrap_or(request);
    let streaming_request_body = request_body_stream_enabled(request, context);
    // A HEAD-to-GET cache fill has no downstream body reader, and a background
    // refresh has no downstream reader at all. Buffer in both cases so the
    // cache tee is not dropped with the response stream.
    let streaming_allowed = !variables.proxy_cache_background && !cache_head_conversion;
    let targets = resolve_proxy_targets(proxy, http, variables, request_peer(request));
    let mut attempted = Vec::new();
    let mut last_failure = ProxyFailure::Failure;
    let mut next = NextUpstreamState::new(&context.proxy_next_upstream);
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
        match proxy_attempt(
            proxy,
            request,
            env,
            location,
            context,
            variables,
            ProxyAttemptTarget {
                host: &target.host,
                port: target.port,
                keepalive: keepalive.as_ref(),
                revalidation: revalidation.as_ref(),
                streaming_allowed,
            },
        )
        .await
        {
            Ok(result) => {
                let response_time = attempt_started.elapsed();
                upstream_statuses.push(result.response.status.to_string());
                upstream_response_times.push(format!("{:.3}", response_time.as_secs_f64()));
                if !targets.as_slice().is_empty()
                    && !streaming_request_body
                    && next.can_retry_status(request, result.response.status)
                {
                    if let Some(guard) = guard.as_mut() {
                        guard.mark_failed();
                    }
                    continue;
                }
                if let Some(guard) = guard.as_mut() {
                    guard.record_timing(result.timing);
                }
                if let Some(reusable) = result.reusable {
                    match reusable {
                        ReusableUpstreamConnection::Http1 {
                            upstream,
                            config,
                            connection,
                        } => {
                            variables
                                .upstream_keepalive
                                .put(&upstream, &config, connection)
                                .await;
                        }
                        ReusableUpstreamConnection::Http2 {
                            upstream,
                            config,
                            connection,
                        } => {
                            variables
                                .upstream_keepalive
                                .put_http2(&upstream, &config, connection)
                                .await;
                        }
                    }
                }
                if result.response.status == 304
                    && let Some(plan) = &cache
                    && plan.revalidate
                    && let Some(cached) = stale.as_ref()
                {
                    let valid = revalidated_validity(&result.response, cached, context);
                    if cache_store_allowed && let Some(valid) = valid {
                        let _ = plan.zone.refresh(&plan.key, cached.created, &valid).await;
                    }
                    let cached = stale.take().expect("expired cache response");
                    variables.upstream_addr = attempted.join(", ");
                    variables.upstream_status = upstream_statuses.join(", ");
                    variables.upstream_response_time = upstream_response_times.join(", ");
                    return proxy_response_from_revalidated(cached, variables);
                }
                let stale_status = cache
                    .as_ref()
                    .is_some_and(|plan| plan.use_stale.status(result.response.status));
                if stale_status && let Some(stale) = stale.take() {
                    variables.upstream_addr = attempted.join(", ");
                    variables.upstream_status = upstream_statuses.join(", ");
                    variables.upstream_response_time = upstream_response_times.join(", ");
                    return proxy_response_from_stale(stale, variables);
                }
                let mut response = result.response;
                let cache_validity = if let Some(plan) = &cache
                    && cache_store_allowed
                    && let Some(valid) = proxy_cache_validity(&response, context)
                    && plan.zone.note_use(&plan.key, plan.min_uses)
                {
                    Some(valid)
                } else {
                    None
                };
                if let (Some(plan), Some(valid)) = (&cache, cache_validity) {
                    response = store_proxy_cache_response(
                        plan,
                        response,
                        valid,
                        cache_lock_guard.take(),
                        context,
                    )
                    .await;
                }
                variables.upstream_addr = attempted.join(", ");
                variables.upstream_status = upstream_statuses.join(", ");
                variables.upstream_response_time = upstream_response_times.join(", ");
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
                if targets.as_slice().is_empty()
                    || streaming_request_body
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
        && allows_stale_error(&plan.use_stale, cached, &last_failure)
    {
        let stale = stale.take().expect("stale cache response");
        return proxy_response_from_stale(stale, variables);
    }
    store_proxy_cache_failure(
        cache.as_ref(),
        cache_store_allowed,
        variables.proxy_cache_background,
        &last_failure,
        context,
    );
    match last_failure {
        ProxyFailure::Timeout => error_response(504),
        ProxyFailure::Failure | ProxyFailure::InvalidHeader => error_response(502),
        ProxyFailure::RequestBody(status) => error_response(status),
    }
}

pub(in crate::server) async fn proxy_attempt(
    proxy: &ProxyPassConfig,
    request: &Request,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &Variables,
    target: ProxyAttemptTarget<'_>,
) -> Result<ProxyAttemptResult, ProxyFailure> {
    let ProxyAttemptTarget {
        host,
        port,
        keepalive,
        revalidation,
        streaming_allowed,
    } = target;
    let mut upstream_variables = variables.clone();
    upstream_variables.proxy_host =
        if port == 0 || port == if proxy.scheme == "https" { 443 } else { 80 } {
            host.to_string()
        } else {
            format!("{host}:{port}")
        };
    upstream_variables.proxy_port = if port != 0 {
        port.to_string()
    } else {
        String::new()
    };
    upstream_variables.upstream_addr = if port == 0 {
        host.to_string()
    } else {
        format!("{host}:{port}")
    };
    let mut outgoing =
        build_proxy_request(proxy, request, env, location, context, &upstream_variables);
    if let Some(revalidation) = revalidation {
        revalidation.apply(&mut outgoing);
    }
    let limit_rate = complex_size(context.proxy_limit_rate.as_deref(), &upstream_variables);
    let attempt_started = Instant::now();
    let version = context.proxy_http_version.as_str();
    let (upstream_response, reusable, deferred_connection, connect, exchange_header_time) =
        if version == "2" {
            let pool_key = keepalive.map(|_| {
                proxy_upstream_pool_key(proxy, host, port, version, context, &upstream_variables)
            });
            let mut connection = match (keepalive, pool_key.as_ref()) {
                (Some(keepalive), Some(key)) => {
                    variables
                        .upstream_keepalive
                        .take_http2(&keepalive.name, key, &keepalive.config)
                        .await
                }
                _ => None,
            };
            if connection.is_none() {
                let key = pool_key.clone().unwrap_or_else(|| {
                    proxy_upstream_pool_key(
                        proxy,
                        host,
                        port,
                        version,
                        context,
                        &upstream_variables,
                    )
                });
                let io = open_proxy_upstream_io(
                    proxy,
                    host,
                    port,
                    context,
                    &upstream_variables,
                    &[b"h2".to_vec()],
                )
                .await?;
                connection =
                    Some(ProxyV2Connection::connect(io, key, context.proxy_send_timeout).await?);
            }
            let mut connection = connection.expect("HTTP/2 connection");
            let connect = attempt_started.elapsed();
            let (response, header_time) = timed_proxy_v2_exchange(
                &mut connection,
                &outgoing,
                proxy,
                context,
                limit_rate,
                streaming_allowed,
            )
            .await?;
            let reusable = match (keepalive, pool_key) {
                (Some(keepalive), Some(_key)) if connection.idle_is_open() => {
                    Some(ReusableUpstreamConnection::Http2 {
                        upstream: keepalive.name.clone(),
                        config: keepalive.config.clone(),
                        connection,
                    })
                }
                _ => None,
            };
            (response, reusable, None, connect, header_time)
        } else {
            let pool_key = keepalive.map(|_| {
                proxy_upstream_pool_key(proxy, host, port, version, context, &upstream_variables)
            });
            let mut connection = match (keepalive, pool_key.as_ref()) {
                (Some(keepalive), Some(key)) => {
                    variables
                        .upstream_keepalive
                        .take(&keepalive.name, key, &keepalive.config)
                        .await
                }
                _ => None,
            };
            if connection.is_none() {
                let key = pool_key.clone().unwrap_or_else(|| {
                    proxy_upstream_pool_key(
                        proxy,
                        host,
                        port,
                        version,
                        context,
                        &upstream_variables,
                    )
                });
                connection = Some(
                    open_proxy_upstream(proxy, host, port, context, &upstream_variables, key)
                        .await?,
                );
            }
            let connect = attempt_started.elapsed();
            let (response, reusable_connection, deferred_connection, header_time) =
                timed_connection_proxy_exchange(
                    connection.expect("connection"),
                    &outgoing,
                    version,
                    context,
                    limit_rate,
                    streaming_allowed,
                    keepalive.is_some(),
                )
                .await?;
            let reusable = match (keepalive, pool_key, reusable_connection) {
                (Some(keepalive), Some(_key), Some(connection))
                    if can_reuse_proxy_connection(&response, &outgoing, version) =>
                {
                    Some(ReusableUpstreamConnection::Http1 {
                        upstream: keepalive.name.clone(),
                        config: keepalive.config.clone(),
                        connection,
                    })
                }
                _ => None,
            };
            (
                response,
                reusable,
                deferred_connection,
                connect,
                header_time,
            )
        };
    if let (Some(keepalive), Some(receiver)) = (keepalive, deferred_connection) {
        spawn_stream_connection_reuse(
            receiver,
            variables.upstream_keepalive.clone(),
            keepalive.name.clone(),
            keepalive.config.clone(),
        );
    }
    let header = connect.saturating_add(exchange_header_time);
    let timing = UpstreamAttemptTiming {
        connect,
        first_byte: Some(header),
        header,
        response: attempt_started.elapsed(),
    };
    let response = finalize_proxy_response(
        upstream_response,
        proxy,
        env,
        location,
        context,
        &upstream_variables,
    )
    .await;
    Ok(ProxyAttemptResult {
        response,
        reusable,
        timing,
    })
}

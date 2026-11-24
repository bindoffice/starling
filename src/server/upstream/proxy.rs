use super::*;

mod attempt;
mod cache;
mod response;
mod result;
mod transport;
use attempt::proxy_attempt;
use cache::*;
use response::finalize_proxy_response;
use result::ProxyAttemptTarget;
pub(in crate::server) use result::{ProxyAttemptResult, ProxyUpgrade, ReusableUpstreamConnection};
pub(in crate::server) use transport::*;

pub(in crate::server) struct ProxyRequestResult {
    pub(in crate::server) response: Response,
    pub(in crate::server) upgrade: Option<ProxyUpgrade>,
}

impl From<Response> for ProxyRequestResult {
    fn from(response: Response) -> Self {
        Self {
            response,
            upgrade: None,
        }
    }
}

pub(in crate::server) async fn proxy_request(
    proxy: &ProxyPassConfig,
    http: &HttpConfig,
    request: &Request,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> ProxyRequestResult {
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
            )
            .into();
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
            }
            .into();
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
            return response.into();
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
    let strip_cache_headers = request.header("range").is_some()
        && cache
            .as_ref()
            .is_some_and(|plan| cache_store_allowed && !plan.bypass && plan.would_store_now());
    let streaming_allowed =
        !variables.proxy_cache_background && !cache_head_conversion && !strip_cache_headers;
    let targets = resolve_proxy_targets(proxy, http, variables, request_peer(request)).await;
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
                strip_cache_headers,
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
                    return proxy_response_from_revalidated(cached, variables).into();
                }
                let stale_status = cache
                    .as_ref()
                    .is_some_and(|plan| plan.use_stale.status(result.response.status));
                if stale_status && let Some(stale) = stale.take() {
                    variables.upstream_addr = attempted.join(", ");
                    variables.upstream_status = upstream_statuses.join(", ");
                    variables.upstream_response_time = upstream_response_times.join(", ");
                    return proxy_response_from_stale(stale, variables).into();
                }
                let upgrade = result.upgrade;
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
                return ProxyRequestResult { response, upgrade };
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
        return proxy_response_from_stale(stale, variables).into();
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
    .into()
}

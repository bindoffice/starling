use super::store::unix_time;
use super::*;
use crate::config::{
    ProxyCacheConfig, ProxyCacheLockConfig, ProxyCacheUseStaleConfig, ProxyCacheValid,
};
use crate::http::UpstreamControl;

pub(in crate::server) struct ProxyCachePlan {
    pub(in crate::server) zone: Arc<ProxyCacheZone>,
    pub(in crate::server) key: String,
    pub(in crate::server) bypass: bool,
    pub(in crate::server) no_store: bool,
    pub(in crate::server) min_uses: usize,
    pub(in crate::server) max_range_offset: Option<u64>,
    pub(in crate::server) convert_head: bool,
    pub(in crate::server) lock: ProxyCacheLockConfig,
    pub(in crate::server) revalidate: bool,
    pub(in crate::server) background_update: bool,
    pub(in crate::server) use_stale: ProxyCacheUseStaleConfig,
}

pub(in crate::server) fn prepare_proxy_cache(
    proxy: &ProxyPassConfig,
    request: &Request,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Option<ProxyCachePlan> {
    let ProxyCacheConfig::Zone(name) = &context.proxy_cache else {
        return None;
    };
    let zone = variables.proxy_cache.zone(name)?;
    let mut key_variables = variables.clone();
    key_variables.proxy_host = cache_proxy_host(proxy, &key_variables);
    let key = eval_variables(&context.proxy_cache_key, &key_variables);
    if key.is_empty() || !context.proxy_cache_methods.allows(&request.method) {
        variables.upstream_cache_status = "BYPASS".into();
        return None;
    }
    let bypass = conditions_match(&context.proxy_cache_bypass, &key_variables);
    let no_store = conditions_match(&context.proxy_no_cache, &key_variables);
    if bypass {
        variables.upstream_cache_status = "BYPASS".into();
    }
    Some(ProxyCachePlan {
        zone,
        key,
        bypass,
        no_store,
        min_uses: context.proxy_cache_min_uses,
        max_range_offset: context.proxy_cache_max_range_offset,
        convert_head: context.proxy_cache_convert_head,
        lock: context.proxy_cache_lock.clone(),
        revalidate: context.proxy_cache_revalidate,
        background_update: context.proxy_cache_background_update,
        use_stale: context.proxy_cache_use_stale,
    })
}

impl ProxyCachePlan {
    pub(in crate::server) fn allows_store(&self, request: &Request) -> bool {
        !self.no_store && range::allows_cache_store(request, self.max_range_offset)
    }
}

pub(in crate::server) fn proxy_cache_upstream_request(
    request: &Request,
    cache: Option<&ProxyCachePlan>,
) -> Option<Request> {
    if !request.method.eq_ignore_ascii_case("HEAD") || !cache.is_some_and(|plan| plan.convert_head)
    {
        return None;
    }
    let mut converted = request.clone();
    converted.method = "GET".into();
    converted.body.clear();
    converted.body_stream = None;
    converted.headers.retain(|header| {
        !header.name.eq_ignore_ascii_case("content-length")
            && !header.name.eq_ignore_ascii_case("transfer-encoding")
            && !header.name.eq_ignore_ascii_case("expect")
    });
    Some(converted)
}

pub(in crate::server) fn proxy_response_from_cached(
    cached: CachedResponse,
    variables: &mut Variables,
) -> Response {
    proxy_response_from_cached_status(cached, variables, "HIT")
}

pub(in crate::server) fn proxy_response_from_stale(
    cached: CachedResponse,
    variables: &mut Variables,
) -> Response {
    proxy_response_from_cached_status(cached, variables, "STALE")
}

pub(in crate::server) fn proxy_response_from_updating(
    cached: CachedResponse,
    variables: &mut Variables,
) -> Response {
    proxy_response_from_cached_status(cached, variables, "UPDATING")
}

pub(in crate::server) fn proxy_response_from_revalidated(
    cached: CachedResponse,
    variables: &mut Variables,
) -> Response {
    proxy_response_from_cached_status(cached, variables, "REVALIDATED")
}

pub(in crate::server) fn proxy_response_from_cached_error(
    status: u16,
    variables: &mut Variables,
) -> Response {
    variables.upstream_cache_status = "HIT".into();
    variables.upstream_status = status.to_string();
    error_response(status)
}

fn proxy_response_from_cached_status(
    cached: CachedResponse,
    variables: &mut Variables,
    cache_status: &str,
) -> Response {
    let mut response = Response {
        status: cached.status,
        headers: cached.headers,
        trailers: Vec::new(),
        body: cached.body,
        body_temp_file: None,
        upstream: None,
        builtin_error: false,
        msie_padding: false,
    };
    response.set_content_length();
    variables.upstream_cache_status = cache_status.into();
    variables.upstream_status = response.status.to_string();
    response
}

pub(in crate::server) fn proxy_cache_use_stale_on_failure(
    config: &ProxyCacheUseStaleConfig,
    failure: &ProxyFailure,
) -> bool {
    match failure {
        ProxyFailure::Failure => config.error(),
        ProxyFailure::Timeout => config.timeout(),
        ProxyFailure::InvalidHeader => config.invalid_header(),
        ProxyFailure::RequestBody(_) => false,
    }
}

pub(in crate::server) fn proxy_cache_validity(
    response: &Response,
    context: &EffectiveConfig,
) -> Option<CacheValidity> {
    upstream_cache_validity(
        response,
        &context.proxy_ignore_headers,
        &context.proxy_cache_valid,
    )
}

pub(in crate::server) fn upstream_cache_validity(
    response: &Response,
    ignore_headers: &[String],
    valid_rules: &[ProxyCacheValid],
) -> Option<CacheValidity> {
    upstream_cache_validity_parts(
        response.status,
        &response.headers,
        !response.trailers.is_empty(),
        response.upstream_control(),
        ignore_headers,
        valid_rules,
    )
}

pub(in crate::server) fn cached_upstream_cache_validity(
    status: u16,
    headers: &[Header],
    ignore_headers: &[String],
    valid_rules: &[ProxyCacheValid],
) -> Option<CacheValidity> {
    upstream_cache_validity_parts(status, headers, false, None, ignore_headers, valid_rules)
}

fn upstream_cache_validity_parts(
    status: u16,
    headers: &[Header],
    has_trailers: bool,
    control: Option<&UpstreamControl>,
    ignore_headers: &[String],
    valid_rules: &[ProxyCacheValid],
) -> Option<CacheValidity> {
    let has_control = control.is_some_and(|control| {
        control.redirect.is_some()
            || control.limit_rate.is_some()
            || control.buffering == Some(AccelBuffering::No)
            || control.charset.is_some()
            || !control.early_hints.is_empty()
    });
    if status == 206 || has_trailers || has_control {
        return None;
    }

    let ignore = |name: &str| {
        ignore_headers
            .iter()
            .any(|ignored| ignored.eq_ignore_ascii_case(name))
    };
    if !ignore("Set-Cookie")
        && headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case("set-cookie"))
    {
        return None;
    }
    if !ignore("Vary")
        && headers.iter().any(|header| {
            header.name.eq_ignore_ascii_case("vary")
                && header.value.split(',').any(|value| value.trim() == "*")
        })
    {
        return None;
    }

    let raw_headers = control
        .map(|control| control.headers.as_slice())
        .unwrap_or(headers);
    let cache_headers = control::response_cache_headers(
        headers,
        raw_headers,
        control::CacheHeaderPolicy {
            ignore_cache_control: ignore("Cache-Control"),
            ignore_expires: ignore("Expires"),
            ignore_x_accel_expires: ignore("X-Accel-Expires"),
        },
        unix_time(),
    );
    if !cache_headers.cacheable {
        return None;
    }
    let valid = cache_headers
        .valid
        .or_else(|| configured_validity(valid_rules, status))
        .filter(|valid| !valid.is_zero())?;
    Some(CacheValidity::new(valid, cache_headers.stale))
}

pub(in crate::server) fn configured_validity(
    rules: &[ProxyCacheValid],
    status: u16,
) -> Option<Duration> {
    rules
        .iter()
        .filter(|rule| rule.statuses.contains(&status))
        .map(|rule| rule.valid)
        .max()
}

pub(in crate::server) fn configured_cache_validity(
    rules: &[ProxyCacheValid],
    status: u16,
) -> Option<CacheValidity> {
    configured_validity(rules, status)
        .filter(|valid| !valid.is_zero())
        .map(|valid| CacheValidity::new(valid, CacheStaleWindows::default()))
}

pub(in crate::server) fn proxy_failure_status(failure: &ProxyFailure) -> u16 {
    match failure {
        ProxyFailure::Timeout => 504,
        ProxyFailure::Failure | ProxyFailure::InvalidHeader => 502,
        ProxyFailure::RequestBody(status) => *status,
    }
}

fn conditions_match(conditions: &[String], variables: &Variables) -> bool {
    conditions.iter().any(|condition| {
        let value = eval_variables(condition, variables);
        !value.is_empty() && value != "0"
    })
}

fn cache_proxy_host(proxy: &ProxyPassConfig, variables: &Variables) -> String {
    if let Some(upstream) = &proxy.upstream {
        return upstream.clone();
    }
    let host = eval_variables(&proxy.host, variables);
    let default_port = if proxy.scheme == "https" { 443 } else { 80 };
    if proxy.port == 0 || proxy.port == default_port {
        host
    } else {
        format!("{host}:{}", proxy.port)
    }
}

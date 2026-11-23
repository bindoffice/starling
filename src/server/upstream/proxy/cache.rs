use super::*;

pub(super) fn cached_lookup_response(
    lookup: CacheLookup,
    variables: &mut Variables,
) -> Option<Response> {
    match lookup {
        CacheLookup::Hit(cached) => Some(proxy_response_from_cached(cached, variables)),
        CacheLookup::Error(status) => Some(proxy_response_from_cached_error(status, variables)),
        CacheLookup::Miss | CacheLookup::Expired(_) => None,
    }
}

pub(super) async fn store_proxy_cache_response(
    plan: &ProxyCachePlan,
    mut response: Response,
    validity: CacheValidity,
    guard: Option<CacheLockGuard>,
    context: &EffectiveConfig,
) -> Response {
    mark_cache_fill_ranges(&mut response);
    if intercepts_upstream_status(
        response.status,
        context.proxy_intercept_errors,
        &context.error_pages,
    ) {
        plan.zone.store_error(&plan.key, response.status, &validity);
        response
    } else {
        plan.zone
            .store_or_tee(&plan.key, response, &validity, guard)
            .await
    }
}

pub(super) fn store_proxy_cache_failure(
    plan: Option<&ProxyCachePlan>,
    store_allowed: bool,
    background: bool,
    failure: &ProxyFailure,
    context: &EffectiveConfig,
) {
    let Some(plan) = plan.filter(|_| store_allowed && !background) else {
        return;
    };
    let status = proxy_failure_status(failure);
    if !matches!(status, 502 | 504) {
        return;
    }
    if let Some(validity) = configured_cache_validity(&context.proxy_cache_valid, status)
        && plan.zone.note_use(&plan.key, plan.min_uses)
    {
        plan.zone.store_error(&plan.key, status, &validity);
    }
}

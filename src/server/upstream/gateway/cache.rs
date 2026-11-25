use super::*;
use crate::config::{
    GatewayCacheConfig, ProxyCacheConfig, ProxyCacheLockConfig, ProxyCacheUseStaleConfig,
};

pub(in crate::server) struct GatewayCachePlan {
    pub(in crate::server) zone: Arc<ProxyCacheZone>,
    pub(in crate::server) key: String,
    pub(in crate::server) bypass: bool,
    no_store: bool,
    min_uses: usize,
    max_range_offset: Option<u64>,
    lock: ProxyCacheLockConfig,
    revalidate: bool,
    background_update: bool,
    use_stale: ProxyCacheUseStaleConfig,
}

#[derive(Default)]
pub(in crate::server) struct GatewayCacheLookup {
    pub(in crate::server) response: Option<Response>,
    pub(in crate::server) stale: Option<CachedResponse>,
    pub(in crate::server) revalidation: Option<CacheRevalidation>,
    pub(in crate::server) store_allowed: bool,
    pub(in crate::server) lock_guard: Option<CacheLockGuard>,
}

pub(in crate::server) enum GatewayBackgroundUpdate {
    FastCgi {
        fastcgi: FastCgiPassConfig,
        http: HttpConfig,
        request: Request,
        env: RequestEnv,
        context: EffectiveConfig,
        variables: Variables,
    },
    Uwsgi {
        uwsgi: UwsgiPassConfig,
        http: HttpConfig,
        request: Request,
        context: EffectiveConfig,
        variables: Variables,
    },
    Scgi {
        scgi: ScgiPassConfig,
        http: HttpConfig,
        request: Request,
        context: EffectiveConfig,
        variables: Variables,
    },
}

impl GatewayBackgroundUpdate {
    async fn execute(self) {
        match self {
            Self::FastCgi {
                fastcgi,
                http,
                request,
                env,
                context,
                mut variables,
            } => {
                variables.proxy_cache_background = true;
                let _ = fastcgi_request(&fastcgi, &http, &request, &env, &context, &mut variables)
                    .await;
            }
            Self::Uwsgi {
                uwsgi,
                http,
                request,
                context,
                mut variables,
            } => {
                variables.proxy_cache_background = true;
                let _ = uwsgi_request(&uwsgi, &http, &request, &context, &mut variables).await;
            }
            Self::Scgi {
                scgi,
                http,
                request,
                context,
                mut variables,
            } => {
                variables.proxy_cache_background = true;
                let _ = scgi_request(&scgi, &http, &request, &context, &mut variables).await;
            }
        }
    }
}

pub(in crate::server) fn prepare_gateway_cache(
    config: &GatewayCacheConfig,
    request: &Request,
    variables: &mut Variables,
) -> Option<GatewayCachePlan> {
    let ProxyCacheConfig::Zone(name) = &config.cache else {
        return None;
    };
    let zone = variables.proxy_cache.zone(name)?;
    let key = eval_variables(&config.key, variables);
    if key.is_empty() || !config.methods.allows(&request.method) {
        variables.upstream_cache_status = "BYPASS".into();
        return None;
    }
    let bypass = conditions_match(&config.bypass, variables);
    let no_store = conditions_match(&config.no_cache, variables);
    if bypass {
        variables.upstream_cache_status = "BYPASS".into();
    }
    Some(GatewayCachePlan {
        zone,
        key,
        bypass,
        no_store,
        min_uses: config.min_uses,
        max_range_offset: config.max_range_offset,
        lock: config.lock.clone(),
        revalidate: config.revalidate,
        background_update: config.background_update,
        use_stale: config.use_stale,
    })
}

const GATEWAY_CACHE_REQUEST_HEADERS: &[&str] = &[
    "if-modified-since",
    "if-unmodified-since",
    "if-none-match",
    "if-match",
    "range",
    "if-range",
];

pub(in crate::server) fn prepare_gateway_upstream_request(
    request: &Request,
    cacheable: bool,
    revalidation: Option<&CacheRevalidation>,
) -> Option<Request> {
    if !cacheable && revalidation.is_none() {
        return None;
    }
    let mut outgoing = request.clone();
    if cacheable {
        Arc::make_mut(&mut outgoing.headers).retain(|header| {
            !GATEWAY_CACHE_REQUEST_HEADERS
                .iter()
                .any(|name| header.name.eq_ignore_ascii_case(name))
        });
    }
    if let Some(revalidation) = revalidation {
        revalidation.apply_to_request(&mut outgoing);
    }
    Some(outgoing)
}

impl GatewayCachePlan {
    pub(in crate::server) fn allows_store(&self, request: &Request) -> bool {
        !self.no_store && gateway_cache_allows_store(request, self.max_range_offset)
    }

    pub(in crate::server) fn wants_background_update(&self, variables: &Variables) -> bool {
        self.background_update && !variables.proxy_cache_background
    }

    pub(in crate::server) fn use_stale_error(
        &self,
        cached: &CachedResponse,
        failure: &ProxyFailure,
    ) -> bool {
        allows_stale_error(&self.use_stale, cached, failure)
    }

    pub(in crate::server) fn use_stale_status(&self, status: u16) -> bool {
        self.use_stale.status(status)
    }

    pub(in crate::server) async fn note_use(&self) -> bool {
        self.zone.note_use(&self.key, self.min_uses)
    }
}

pub(in crate::server) async fn lookup_gateway_cache(
    plan: &GatewayCachePlan,
    request: &Request,
    variables: &mut Variables,
    background_update: Option<GatewayBackgroundUpdate>,
) -> GatewayCacheLookup {
    let mut state = GatewayCacheLookup {
        store_allowed: plan.allows_store(request),
        ..GatewayCacheLookup::default()
    };
    if plan.bypass {
        return state;
    }
    let mut lookup = match plan.zone.lookup(&plan.key).await {
        Ok(lookup) => lookup,
        Err(_) => CacheLookup::Miss,
    };
    let mut expired = false;
    lookup = match std::mem::replace(&mut lookup, CacheLookup::Miss) {
        CacheLookup::Expired(cached) => {
            state.stale = Some(cached);
            expired = true;
            CacheLookup::Miss
        }
        lookup => lookup,
    };
    if !variables.proxy_cache_background
        && expired
        && state
            .stale
            .as_ref()
            .is_some_and(|cached| allows_stale_updating(&plan.use_stale, cached))
        && plan.zone.is_updating(&plan.key)
    {
        state.response = state
            .stale
            .take()
            .map(|cached| proxy_response_from_updating(cached, variables));
        return state;
    }
    if !variables.proxy_cache_background
        && expired
        && plan.background_update
        && let Some(background_update) = background_update
        && state
            .stale
            .as_ref()
            .is_some_and(|cached| allows_stale_updating(&plan.use_stale, cached))
    {
        return match plan.zone.try_acquire_update_lock(&plan.key).await {
            Some(guard) => {
                start_gateway_background_update(guard, background_update);
                state.response = state
                    .stale
                    .take()
                    .map(|cached| proxy_response_from_stale(cached, variables));
                state
            }
            None => {
                state.response = state
                    .stale
                    .take()
                    .map(|cached| proxy_response_from_updating(cached, variables));
                state
            }
        };
    }
    if let CacheLookup::Error(status) = &lookup {
        state.response = Some(proxy_response_from_cached_error(*status, variables));
        return state;
    }
    if !variables.proxy_cache_background
        && !matches!(lookup, CacheLookup::Hit(_) | CacheLookup::Error(_))
        && plan.lock.enabled
    {
        match plan.zone.acquire_lock(&plan.key, &plan.lock).await {
            CacheLockOutcome::Acquired(guard) => {
                state.lock_guard = Some(guard);
                lookup = match plan.zone.lookup(&plan.key).await {
                    Ok(lookup) => lookup,
                    Err(_) => CacheLookup::Miss,
                };
                lookup = match std::mem::replace(&mut lookup, CacheLookup::Miss) {
                    CacheLookup::Expired(cached) => {
                        state.stale = Some(cached);
                        expired = true;
                        CacheLookup::Miss
                    }
                    lookup => lookup,
                };
            }
            CacheLockOutcome::TimedOut => {
                state.store_allowed = false;
            }
        }
    }
    match lookup {
        CacheLookup::Hit(cached) => {
            state.response = Some(proxy_response_from_cached(cached, variables));
            return state;
        }
        CacheLookup::Error(status) => {
            state.response = Some(proxy_response_from_cached_error(status, variables));
            return state;
        }
        CacheLookup::Miss | CacheLookup::Expired(_) => {}
    }
    if expired {
        variables.upstream_cache_status = "EXPIRED".into();
        if plan.revalidate {
            state.revalidation = state.stale.as_ref().map(|cached| {
                let revalidation = CacheRevalidation::from_cached(cached);
                variables.upstream_cache_last_modified =
                    revalidation.last_modified().unwrap_or_default().into();
                variables.upstream_cache_etag = revalidation.etag().unwrap_or_default().into();
                revalidation
            });
        }
    } else {
        variables.upstream_cache_status = "MISS".into();
    }
    state
}

pub(in crate::server) fn start_gateway_background_update(
    guard: CacheLockGuard,
    update: GatewayBackgroundUpdate,
) {
    tokio::spawn(async move {
        let _guard = guard;
        update.execute().await;
    });
}

pub(in crate::server) fn gateway_cache_validity(
    response: &Response,
    config: &GatewayCacheConfig,
    ignore_headers: &[String],
) -> Option<CacheValidity> {
    upstream_cache_validity(response, ignore_headers, &config.valid)
}

pub(in crate::server) fn revalidated_gateway_validity(
    response: &Response,
    cached: &CachedResponse,
    config: &GatewayCacheConfig,
    ignore_headers: &[String],
) -> Option<CacheValidity> {
    revalidated_upstream_validity(response, cached, ignore_headers, &config.valid)
}

fn conditions_match(conditions: &[String], variables: &Variables) -> bool {
    conditions.iter().any(|condition| {
        let value = eval_variables(condition, variables);
        !value.is_empty() && value != "0"
    })
}

fn gateway_cache_allows_store(request: &Request, max_range_offset: Option<u64>) -> bool {
    allows_cache_store(request, max_range_offset)
}

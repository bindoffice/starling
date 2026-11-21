use super::*;
use crate::config::ProxyCacheConfig;

#[derive(Clone, Copy)]
pub(super) struct ContentKind {
    fastcgi: bool,
    uwsgi: bool,
    scgi: bool,
    grpc: bool,
    memcached: bool,
    proxied: bool,
}

impl ContentKind {
    fn from_context(context: &EffectiveConfig) -> Self {
        Self {
            fastcgi: context.fastcgi_pass.is_some(),
            uwsgi: context.uwsgi_pass.is_some(),
            scgi: context.scgi_pass.is_some(),
            grpc: context.grpc.pass.is_some(),
            memcached: context.memcached_pass.is_some(),
            proxied: context.proxy_pass.is_some(),
        }
    }

    fn gateway(self) -> bool {
        self.fastcgi || self.uwsgi || self.scgi
    }

    pub(super) fn upstream(self) -> bool {
        self.gateway() || self.grpc || self.memcached || self.proxied
    }

    pub(super) fn intercept_errors(self, context: &EffectiveConfig) -> bool {
        if self.gateway() {
            (self.fastcgi && context.fastcgi_intercept_errors)
                || (self.uwsgi && context.uwsgi_intercept_errors)
                || (self.scgi && context.scgi_intercept_errors)
        } else if self.grpc {
            context.grpc.intercept_errors
        } else if self.memcached {
            true
        } else {
            should_intercept_error_pages(self.proxied, context)
        }
    }

    pub(super) fn force_ranges(self, context: &EffectiveConfig) -> bool {
        if self.fastcgi {
            context.fastcgi_force_ranges
        } else if self.uwsgi {
            context.uwsgi_force_ranges
        } else if self.scgi {
            context.scgi_force_ranges
        } else if self.grpc {
            false
        } else if self.memcached {
            true
        } else {
            context.proxy_force_ranges
        }
    }

    pub(super) fn ignore_client_abort(self, context: &EffectiveConfig) -> bool {
        if self.fastcgi {
            context.fastcgi_ignore_client_abort
        } else if self.uwsgi {
            context.uwsgi_ignore_client_abort
        } else if self.scgi {
            context.scgi_ignore_client_abort
        } else if self.grpc {
            context.grpc.ignore_client_abort
        } else if self.proxied {
            context.proxy_ignore_client_abort
        } else {
            false
        }
    }

    fn preserves_durable_output(self, request: &Request, context: &EffectiveConfig) -> bool {
        if self.proxied {
            return !matches!(context.proxy_store, StoreConfig::Off)
                || (context.proxy_cache != ProxyCacheConfig::Off
                    && context.proxy_cache_methods.allows(&request.method));
        }
        if self.fastcgi {
            return !matches!(context.fastcgi_store, StoreConfig::Off)
                || (context.fastcgi_cache.cache != ProxyCacheConfig::Off
                    && context.fastcgi_cache.methods.allows(&request.method));
        }
        if self.uwsgi {
            return !matches!(context.uwsgi_store, StoreConfig::Off)
                || (context.uwsgi_cache.cache != ProxyCacheConfig::Off
                    && context.uwsgi_cache.methods.allows(&request.method));
        }
        if self.scgi {
            return !matches!(context.scgi_store, StoreConfig::Off)
                || (context.scgi_cache.cache != ProxyCacheConfig::Off
                    && context.scgi_cache.methods.allows(&request.method));
        }
        false
    }
}

pub(super) async fn serve_content(
    http: &HttpConfig,
    request: &Request,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> (Response, ContentKind) {
    let kind = ContentKind::from_context(context);
    let preserve_on_client_abort =
        kind.ignore_client_abort(context) || kind.preserves_durable_output(request, context);
    if preserve_on_client_abort && let Some(client_abort) = variables.client_abort.as_ref() {
        client_abort.mark_preserve();
    }
    variables.response_from_upstream = kind.upstream();
    let mut response = if let Some(fastcgi_pass) = &context.fastcgi_pass {
        fastcgi_request(fastcgi_pass, http, request, env, context, variables).await
    } else if let Some(uwsgi_pass) = &context.uwsgi_pass {
        uwsgi_request(uwsgi_pass, http, request, context, variables).await
    } else if let Some(scgi_pass) = &context.scgi_pass {
        scgi_request(scgi_pass, http, request, context, variables).await
    } else if let Some(grpc_pass) = &context.grpc.pass {
        grpc_request(grpc_pass, http, request, env, context, variables).await
    } else if let Some(memcached_pass) = &context.memcached_pass {
        memcached_request(memcached_pass, http, request, context, variables).await
    } else if let Some(proxy) = &context.proxy_pass {
        proxy_request(proxy, http, request, env, location, context, variables).await
    } else if let Some(response) = serve_dav(request, env, context, location).await {
        response
    } else {
        serve_static(request, env, context, location, variables)
            .await
            .unwrap_or_else(|| error_response(404))
    };
    if preserve_on_client_abort {
        response.mark_preserve_on_client_abort();
    }
    if kind.gateway() {
        serve_gateway_store(&mut response, env, location, context, variables).await;
        return (response, kind);
    }
    (response, kind)
}

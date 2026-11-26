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
    fn none() -> Self {
        Self {
            fastcgi: false,
            uwsgi: false,
            scgi: false,
            grpc: false,
            memcached: false,
            proxied: false,
        }
    }

    fn from_handler(handler: ContentHandlerKind) -> Self {
        Self {
            fastcgi: handler == ContentHandlerKind::FastCgi,
            uwsgi: handler == ContentHandlerKind::Uwsgi,
            scgi: handler == ContentHandlerKind::Scgi,
            grpc: handler == ContentHandlerKind::Grpc,
            memcached: handler == ContentHandlerKind::Memcached,
            proxied: handler == ContentHandlerKind::Proxy,
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
) -> (Response, ContentKind, Option<Box<ProxyUpgrade>>) {
    let handler = variables
        .content_handler
        .or_else(|| content_handler_for_context(context));
    let kind = handler
        .map(ContentKind::from_handler)
        .unwrap_or_else(ContentKind::none);
    let preserve_on_client_abort =
        kind.ignore_client_abort(context) || kind.preserves_durable_output(request, context);
    if preserve_on_client_abort && let Some(client_abort) = variables.client_abort.as_ref() {
        client_abort.mark_preserve();
    }
    let mut upgrade = None;
    let mut response = match handler {
        Some(ContentHandlerKind::FastCgi) => {
            if let Some(pass) = &context.fastcgi_pass {
                Box::pin(fastcgi_request(
                    pass, http, request, env, context, variables,
                ))
                .await
            } else {
                error_response(500)
            }
        }
        Some(ContentHandlerKind::Uwsgi) => {
            if let Some(pass) = &context.uwsgi_pass {
                Box::pin(uwsgi_request(pass, http, request, context, variables)).await
            } else {
                error_response(500)
            }
        }
        Some(ContentHandlerKind::Scgi) => {
            if let Some(pass) = &context.scgi_pass {
                Box::pin(scgi_request(pass, http, request, context, variables)).await
            } else {
                error_response(500)
            }
        }
        Some(ContentHandlerKind::Grpc) => {
            if let Some(pass) = &context.grpc.pass {
                Box::pin(grpc_request(pass, http, request, env, context, variables)).await
            } else {
                error_response(500)
            }
        }
        Some(ContentHandlerKind::Memcached) => {
            if let Some(pass) = &context.memcached_pass {
                Box::pin(memcached_request(pass, http, request, context, variables)).await
            } else {
                error_response(500)
            }
        }
        Some(ContentHandlerKind::Proxy) => {
            if let Some(proxy) = &context.proxy_pass {
                let result = Box::pin(proxy_request(
                    proxy, http, request, env, location, context, variables,
                ))
                .await;
                upgrade = result.upgrade.map(Box::new);
                result.response
            } else {
                error_response(500)
            }
        }
        Some(ContentHandlerKind::Tunnel) => error_response(500),
        Some(ContentHandlerKind::Wasm) => serve_wasm(request, env, context, variables).await,
        None => {
            if let Some(response) = serve_dav(request, env, context, location, variables).await {
                response
            } else {
                serve_static(request, env, context, location, variables)
                    .await
                    .unwrap_or_else(|| error_response(404))
            }
        }
    };
    set_streamed_request_body_variable(request, variables).await;
    let response_from_upstream = kind.upstream()
        && !matches!(
            variables.upstream_cache_status.as_str(),
            "HIT" | "STALE" | "UPDATING" | "REVALIDATED"
        );
    if response_from_upstream || variables.response_from_upstream != response_from_upstream {
        variables.response_from_upstream = response_from_upstream;
    }
    if preserve_on_client_abort {
        response.mark_preserve_on_client_abort();
    }
    if kind.gateway() {
        Box::pin(serve_gateway_store(
            &mut response,
            env,
            location,
            context,
            variables,
        ))
        .await;
        return (response, kind, None);
    }
    (response, kind, upgrade)
}

async fn serve_wasm(
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Response {
    let Some(plugin) = &context.wasm.plugin else {
        return error_response(500);
    };
    let logger = variables.error_logger_for(context);
    let body = match wasm_request_body(request, context.wasm.request_body_max).await {
        Ok(body) => body,
        Err(status) => return error_response(status),
    };
    let permit = match tokio::time::timeout(
        context.wasm.timeout,
        context.wasm.instance_limiter.clone().acquire_owned(),
    )
    .await
    {
        Ok(Ok(permit)) => permit,
        Ok(Err(_)) => return error_response(500),
        Err(_) => {
            log_wasm_error(
                logger.as_ref(),
                plugin,
                "timed out waiting for an instance slot",
            );
            return error_response(500);
        }
    };
    let wasm_request = crate::wasm::WasmRequest {
        method: request.method.clone(),
        target: request.target.clone(),
        uri: env.uri.clone(),
        args: env.args.clone().unwrap_or_default(),
        version: request.version.clone(),
        scheme: variables.scheme.to_string(),
        host: variables.host.clone(),
        remote_addr: request.remote_addr.to_string(),
        local_addr: request.local_addr.to_string(),
        headers: request
            .headers
            .iter()
            .map(|header| (header.name.to_string(), header.value.clone()))
            .collect(),
        body,
    };
    let result = crate::wasm::call(
        plugin.module.clone(),
        wasm_request,
        crate::wasm::WasmLimits {
            timeout: context.wasm.timeout,
            memory_max: context.wasm.memory_max,
            fuel: context.wasm.fuel,
            response_body_max: context.wasm.response_body_max,
        },
    )
    .await;
    drop(permit);
    match result {
        Ok((wasm_response, stats)) => {
            if logger
                .as_ref()
                .is_some_and(|logger| logger.enabled(crate::config::LogLevel::Debug))
            {
                let fuel = stats
                    .fuel_used
                    .map(|fuel| fuel.to_string())
                    .unwrap_or_else(|| "-".into());
                logger.as_ref().unwrap().log(
                    crate::config::LogLevel::Debug,
                    format!(
                        "wasm module {} sha1={} completed in {}ms fuel={} memory={}",
                        plugin.path.display(),
                        plugin.module.digest(),
                        stats.elapsed.as_millis(),
                        fuel,
                        stats.memory_bytes
                    ),
                );
            }
            wasm_response_to_http(wasm_response, context)
        }
        Err(error) => {
            log_wasm_error(logger.as_ref(), plugin, error.message());
            error_response(500)
        }
    }
}

async fn wasm_request_body(request: &Request, limit: u64) -> Result<Vec<u8>, u16> {
    let body = if let Some(stream) = request.body_stream.as_ref() {
        stream
            .read_to_end(limit)
            .await
            .map_err(|error| crate::http::request_body_error_status(&error).unwrap_or(500))?
    } else {
        if request.body.len() as u64 > limit {
            return Err(413);
        }
        request.body.clone()
    };
    if body.len() as u64 > limit {
        return Err(413);
    }
    Ok(body)
}

fn wasm_response_to_http(
    wasm_response: crate::wasm::WasmResponse,
    context: &EffectiveConfig,
) -> Response {
    let status = wasm_response.status;
    let has_body = !wasm_response.body.is_empty();
    let mut response = Response::new(status);
    for (name, value) in wasm_response.headers {
        response.add_header(&name, value);
    }
    if has_body && !matches!(status, 204 | 205 | 304) && response.header("Content-Type").is_none() {
        response.set_header_static("Content-Type", context.default_type.clone());
    }
    if matches!(status, 204 | 304) {
        response.body = ResponseBody::Empty;
        response.remove_header("Content-Length");
    } else {
        response.body = ResponseBody::Bytes(wasm_response.body);
        response.set_content_length();
    }
    response
}

fn log_wasm_error(
    logger: Option<&Arc<crate::logging::Logger>>,
    plugin: &crate::config::WasmPlugin,
    message: &str,
) {
    if let Some(logger) = logger {
        let message: String = message.chars().take(1024).collect();
        logger.log(
            crate::config::LogLevel::Error,
            format!(
                "wasm module {} sha1={} failed: {message}",
                plugin.path.display(),
                plugin.module.digest()
            ),
        );
    }
}

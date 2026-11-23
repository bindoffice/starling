use super::*;

mod connection;
mod protocol;
mod streaming;
pub(in crate::server) use connection::*;
pub(in crate::server) use protocol::*;
pub(in crate::server) use streaming::*;

pub(in crate::server) async fn memcached_request(
    memcached: &MemcachedPassConfig,
    http: &HttpConfig,
    request: &Request,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Response {
    if let Some(client_abort) = variables.client_abort.as_ref() {
        client_abort.mark_upstream_active();
    }
    if !matches!(request.method.as_str(), "GET" | "HEAD") {
        return error_response(405);
    }

    let raw_key = eval_variables("$memcached_key", variables);
    if raw_key.is_empty() {
        return error_response(500);
    }
    let key = escape_memcached_key(raw_key.as_bytes());
    let keepalive = memcached_keepalive_context(memcached, http);
    let targets = resolve_gateway_targets(&memcached.raw, http, variables, request_peer(request));
    let mut attempted = Vec::new();
    let mut last_failure = ProxyFailure::Failure;
    let mut next = NextUpstreamState::new(&context.memcached_next_upstream);
    let mut targets = targets.into_iter();
    let mut upstream_statuses = Vec::new();
    let mut upstream_response_times = Vec::new();

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
        match memcached_attempt(
            &key,
            request,
            context,
            variables,
            (&target.host, target.port),
            keepalive.as_ref(),
        )
        .await
        {
            Ok((response, reusable)) => {
                let response_time = attempt_started.elapsed();
                upstream_statuses.push(response.status.to_string());
                upstream_response_times.push(format!("{:.3}", response_time.as_secs_f64()));
                if !targets.as_slice().is_empty() && next.can_retry_status(request, response.status)
                {
                    if let Some(guard) = guard.as_mut() {
                        guard.mark_failed();
                    }
                    continue;
                }
                if let Some(guard) = guard.as_mut() {
                    guard.record_timing(UpstreamAttemptTiming::response_only(response_time));
                }
                if let (Some(keepalive), Some(connection)) = (keepalive.as_ref(), reusable) {
                    variables
                        .upstream_keepalive
                        .put(&keepalive.name, &keepalive.config, connection)
                        .await;
                }
                set_upstream_variables(
                    variables,
                    &attempted,
                    &upstream_statuses,
                    &upstream_response_times,
                );
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
                if targets.as_slice().is_empty() || !next.can_retry_failure(request, &last_failure)
                {
                    break;
                }
            }
        }
    }

    set_upstream_variables(
        variables,
        &attempted,
        &upstream_statuses,
        &upstream_response_times,
    );
    match last_failure {
        ProxyFailure::Timeout => error_response(504),
        ProxyFailure::Failure | ProxyFailure::InvalidHeader => error_response(502),
        ProxyFailure::RequestBody(status) => error_response(status),
    }
}

async fn memcached_attempt(
    key: &[u8],
    request: &Request,
    context: &EffectiveConfig,
    variables: &Variables,
    target: (&str, u16),
    keepalive: Option<&UpstreamKeepaliveContext>,
) -> Result<(Response, Option<UpstreamConnection>), ProxyFailure> {
    let (host, port) = target;
    let pool_key = memcached_upstream_pool_key(host, port, context, variables);
    let mut connection = match keepalive {
        Some(keepalive) => {
            variables
                .upstream_keepalive
                .take(&keepalive.name, &pool_key, &keepalive.config)
                .await
        }
        None => None,
    };
    if connection.is_none() {
        connection =
            Some(open_memcached_connection(host, port, context, variables, pool_key).await?);
    }
    let connection = connection.expect("Memcached connection");
    let keepalive_enabled = keepalive.is_some();
    let exchange = memcached_exchange_connection_streaming(
        connection,
        key,
        request,
        context,
        keepalive.cloned(),
        variables.upstream_keepalive.clone(),
    )
    .await?;
    match exchange {
        MemcachedExchange::Buffered {
            response,
            connection,
        } => {
            let connection = *connection;
            let reusable = keepalive_enabled && connection.reader.buffer().is_empty();
            Ok((
                memcached_response(response, context, variables),
                reusable.then_some(connection),
            ))
        }
        MemcachedExchange::Streamed { body, gzip } => Ok((
            memcached_value_response(body, gzip, context, variables),
            None,
        )),
    }
}

fn memcached_response(
    response: MemcachedResponse,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Response {
    match response {
        MemcachedResponse::NotFound => Response::empty(404),
        MemcachedResponse::Value { body, gzip } => {
            memcached_value_response(ResponseBody::Bytes(body), gzip, context, variables)
        }
    }
}

fn memcached_value_response(
    body: ResponseBody,
    gzip: bool,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Response {
    let content_type = content_type(Path::new(&variables.uri), context);
    let mut response = Response {
        status: 200,
        headers: vec![Header {
            name: "Content-Type".into(),
            value: content_type,
        }],
        trailers: Vec::new(),
        chunked_expected: false,
        body,
        body_temp_file: None,
        upstream: None,
        builtin_error: false,
        msie_padding: false,
        allow_ranges: false,
        single_range: false,
    };
    if gzip {
        response.set_header("Content-Encoding", "gzip");
    }
    response.set_content_length();
    response
}

fn set_upstream_variables(
    variables: &mut Variables,
    attempted: &[String],
    statuses: &[String],
    response_times: &[String],
) {
    variables.upstream_addr = attempted.join(", ");
    variables.upstream_status = statuses.join(", ");
    variables.upstream_response_time = response_times.join(", ");
}

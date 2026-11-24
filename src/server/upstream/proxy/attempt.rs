use super::*;

pub(super) async fn proxy_attempt(
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
        strip_cache_headers,
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
    if strip_cache_headers {
        outgoing.headers.retain(|(name, _)| {
            !name.eq_ignore_ascii_case("range") && !name.eq_ignore_ascii_case("if-range")
        });
    }
    let limit_rate = complex_size(context.proxy_limit_rate.as_deref(), &upstream_variables);
    let attempt_started = Instant::now();
    let version = context.proxy_http_version.as_str();
    let (
        upstream_response,
        reusable,
        deferred_connection,
        upgrade_connection,
        connect,
        exchange_header_time,
    ) = if version == "2" {
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
                proxy_upstream_pool_key(proxy, host, port, version, context, &upstream_variables)
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
        (response, reusable, None, None, connect, header_time)
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
                proxy_upstream_pool_key(proxy, host, port, version, context, &upstream_variables)
            });
            connection = Some(
                open_proxy_upstream(proxy, host, port, context, &upstream_variables, key).await?,
            );
        }
        let connect = attempt_started.elapsed();
        let exchange = timed_connection_proxy_exchange(
            connection.expect("connection"),
            &outgoing,
            version,
            context,
            limit_rate,
            streaming_allowed,
            keepalive.is_some(),
        )
        .await?;
        let response = exchange.response;
        let reusable_connection = exchange.reusable;
        let deferred_connection = exchange.deferred;
        let upgrade_connection = exchange.upgrade;
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
            upgrade_connection,
            connect,
            exchange.header_time,
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
    let upgrade = upgrade_connection
        .filter(|_| {
            response.status == 101
                && version == "1.1"
                && request.version == "HTTP/1.1"
                && request.header("upgrade").is_some()
        })
        .map(|connection| ProxyUpgrade {
            connection,
            read_timeout: context.proxy_read_timeout,
            write_timeout: context.proxy_send_timeout,
            buffer_size: context.proxy_buffering.buffer_size,
        });
    Ok(ProxyAttemptResult {
        response,
        reusable,
        upgrade,
        timing,
    })
}

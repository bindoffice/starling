use super::*;

pub(in crate::server) fn memcached_keepalive_context(
    memcached: &MemcachedPassConfig,
    http: &HttpConfig,
) -> Option<UpstreamKeepaliveContext> {
    let upstream = http.upstreams.get(&memcached.raw)?;
    let config = upstream.keepalive.clone()?;
    (config.max_cached != 0).then(|| UpstreamKeepaliveContext {
        name: memcached.raw.clone(),
        config,
    })
}

pub(super) async fn open_memcached_connection(
    host: &str,
    port: u16,
    context: &EffectiveConfig,
    variables: &Variables,
    key: UpstreamPoolKey,
) -> Result<UpstreamConnection, ProxyFailure> {
    let local_bind = evaluate_memcached_bind(context, variables);
    let io = open_gateway_io(
        host,
        port,
        context.memcached_connect_timeout,
        UpstreamSocketOptions::for_memcached(context),
        local_bind.as_deref(),
        "memcached_bind",
    )
    .await?;
    Ok(UpstreamConnection::new(io, key))
}

#[cfg(test)]
pub(in crate::server) async fn memcached_exchange_connection(
    connection: &mut UpstreamConnection,
    key: &[u8],
    context: &EffectiveConfig,
) -> Result<MemcachedResponse, ProxyFailure> {
    connection.requests = connection.requests.saturating_add(1);
    let request = build_memcached_request(key);
    let write = async {
        let mut writer =
            TimedWriter::new(connection.reader.get_mut(), context.memcached_send_timeout);
        writer.write_all(&request).await?;
        writer.flush().await
    };
    write.await.map_err(proxy_failure_from_io)?;

    let mut reader = TimedReader::new(&mut connection.reader, context.memcached_read_timeout);
    read_memcached_response(
        &mut reader,
        key,
        context.memcached_gzip_flag,
        context.memcached_buffer_size,
    )
    .await
    .map_err(proxy_failure_from_io)
}

pub(in crate::server) fn memcached_upstream_pool_key(
    host: &str,
    port: u16,
    context: &EffectiveConfig,
    variables: &Variables,
) -> UpstreamPoolKey {
    UpstreamPoolKey {
        generation: variables.upstream_keepalive_generation,
        scheme: "memcached".into(),
        host: host.to_string(),
        port,
        tls_name: None,
        tls_verify: false,
        tls_trusted_certificate: None,
        tls_verify_depth: None,
        tls_crl: None,
        tls_client_certificate: None,
        tls_client_certificate_key: None,
        tls_protocols: None,
        tls_ciphers: None,
        proxy_bind: evaluate_memcached_bind(context, variables),
        socket_keepalive: context.memcached_socket_keepalive,
        socket_rcvbuf: 0,
        socket_sndbuf: 0,
        http_version: "memcached".into(),
    }
}

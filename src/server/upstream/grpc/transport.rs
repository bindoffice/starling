use super::*;

pub(in crate::server) async fn open_grpc_upstream(
    host: &str,
    port: u16,
    tls: bool,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<BoxedUpstreamIo, ProxyFailure> {
    open_grpc_upstream_io(host, port, tls, context, variables).await
}

async fn open_grpc_upstream_io(
    host: &str,
    port: u16,
    tls: bool,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<BoxedUpstreamIo, ProxyFailure> {
    let io = open_gateway_io(
        host,
        port,
        context.grpc.connect_timeout,
        UpstreamSocketOptions::for_grpc(context),
        evaluate_grpc_bind(context, variables).as_deref(),
        "grpc_bind",
    )
    .await?;
    if !tls {
        return Ok(io);
    }
    let stream = match tokio::time::timeout(
        context.grpc.connect_timeout,
        upgrade_grpc_tls(io, host, context, variables),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(_)) => return Err(ProxyFailure::Failure),
        Err(_) => return Err(ProxyFailure::Timeout),
    };
    Ok(Box::new(stream))
}

async fn upgrade_grpc_tls<S>(
    stream: S,
    host: &str,
    context: &EffectiveConfig,
    variables: &Variables,
) -> io::Result<tokio_rustls::client::TlsStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let raw_name = grpc_tls_name(context, variables, host);
    upgrade_tls_client(
        stream,
        &raw_name,
        ClientTlsOptions {
            verify: context.grpc.ssl_verify,
            server_name: context.grpc.ssl_server_name,
            session_reuse: context.grpc.ssl_session_reuse,
            trusted_certificate: context.grpc.ssl_trusted_certificate.as_deref(),
            verify_depth: context.grpc.ssl_verify_depth,
            crl: context.grpc.ssl_crl.as_deref(),
            client_certificate: context.grpc.ssl_certificate.as_deref(),
            client_certificate_key: context.grpc.ssl_certificate_key.as_deref(),
            client_certificate_passwords: &context.grpc.ssl_passwords,
            protocols: context.grpc.ssl_protocols,
            ciphers: &context.grpc.ssl_ciphers,
            alpn_protocols: &[b"h2".to_vec()],
        },
        &variables.client_tls_cache,
    )
    .await
}

pub(in crate::server) fn grpc_upstream_pool_key(
    host: &str,
    port: u16,
    tls: bool,
    context: &EffectiveConfig,
    variables: &Variables,
) -> UpstreamPoolKey {
    UpstreamPoolKey {
        generation: variables.upstream_keepalive_generation,
        scheme: if tls { "https" } else { "http" }.into(),
        host: host.to_string(),
        port,
        tls_name: tls.then(|| grpc_tls_name(context, variables, host)),
        tls_verify: tls && context.grpc.ssl_verify,
        tls_trusted_certificate: tls
            .then(|| context.grpc.ssl_trusted_certificate.clone())
            .flatten(),
        tls_verify_depth: tls.then_some(context.grpc.ssl_verify_depth),
        tls_crl: tls.then(|| context.grpc.ssl_crl.clone()).flatten(),
        tls_client_certificate: tls.then(|| context.grpc.ssl_certificate.clone()).flatten(),
        tls_client_certificate_key: tls
            .then(|| context.grpc.ssl_certificate_key.clone())
            .flatten(),
        tls_protocols: tls.then_some(context.grpc.ssl_protocols),
        tls_ciphers: tls.then(|| context.grpc.ssl_ciphers.clone()),
        proxy_bind: evaluate_grpc_bind(context, variables),
        socket_keepalive: context.grpc.socket_keepalive,
        socket_rcvbuf: context.grpc.socket_rcvbuf,
        socket_sndbuf: context.grpc.socket_sndbuf,
        http_version: "2".into(),
    }
}

fn grpc_tls_name(context: &EffectiveConfig, variables: &Variables, host: &str) -> String {
    context
        .grpc
        .ssl_name
        .as_deref()
        .map(|name| eval_variables(name, variables))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| host.to_string())
}

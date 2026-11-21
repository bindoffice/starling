use super::*;

#[cfg(test)]
pub(in crate::server) async fn upgrade_proxy_tls<S>(
    stream: S,
    host: &str,
    context: &EffectiveConfig,
    variables: &Variables,
) -> io::Result<tokio_rustls::client::TlsStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    upgrade_proxy_tls_with_alpn(stream, host, context, variables, &[]).await
}

pub(in crate::server) async fn upgrade_proxy_tls_with_alpn<S>(
    stream: S,
    host: &str,
    context: &EffectiveConfig,
    variables: &Variables,
    alpn_protocols: &[Vec<u8>],
) -> io::Result<tokio_rustls::client::TlsStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let raw_name = proxy_tls_name(context, variables, host);
    upgrade_tls_client(
        stream,
        &raw_name,
        ClientTlsOptions {
            verify: context.proxy_ssl_verify,
            server_name: context.proxy_ssl_server_name,
            session_reuse: context.proxy_ssl_session_reuse,
            trusted_certificate: context.proxy_ssl_trusted_certificate.as_deref(),
            verify_depth: context.proxy_ssl_verify_depth,
            crl: context.proxy_ssl_crl.as_deref(),
            client_certificate: context.proxy_ssl_certificate.as_deref(),
            client_certificate_key: context.proxy_ssl_certificate_key.as_deref(),
            client_certificate_passwords: &context.proxy_ssl_passwords,
            protocols: context.proxy_ssl_protocols,
            ciphers: &context.proxy_ssl_ciphers,
            alpn_protocols,
        },
        &variables.client_tls_cache,
    )
    .await
}

pub(in crate::server) fn proxy_tls_name(
    context: &EffectiveConfig,
    variables: &Variables,
    host: &str,
) -> String {
    context
        .proxy_ssl_name
        .as_deref()
        .map(|name| eval_variables(name, variables))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| host.to_string())
}

pub(in crate::server) fn proxy_upstream_pool_key(
    proxy: &ProxyPassConfig,
    host: &str,
    port: u16,
    version: &str,
    context: &EffectiveConfig,
    variables: &Variables,
) -> UpstreamPoolKey {
    let tls = proxy.scheme == "https";
    UpstreamPoolKey {
        generation: variables.upstream_keepalive_generation,
        scheme: proxy.scheme.clone(),
        host: host.to_string(),
        port,
        tls_name: tls.then(|| proxy_tls_name(context, variables, host)),
        tls_verify: tls && context.proxy_ssl_verify,
        tls_trusted_certificate: if tls {
            context.proxy_ssl_trusted_certificate.clone()
        } else {
            None
        },
        tls_verify_depth: tls.then_some(context.proxy_ssl_verify_depth),
        tls_crl: tls.then(|| context.proxy_ssl_crl.clone()).flatten(),
        tls_client_certificate: tls.then(|| context.proxy_ssl_certificate.clone()).flatten(),
        tls_client_certificate_key: tls
            .then(|| context.proxy_ssl_certificate_key.clone())
            .flatten(),
        tls_protocols: tls.then_some(context.proxy_ssl_protocols),
        tls_ciphers: tls.then(|| context.proxy_ssl_ciphers.clone()),
        proxy_bind: evaluate_proxy_bind(context, variables),
        socket_keepalive: context.proxy_socket_keepalive,
        socket_rcvbuf: context.proxy_socket_rcvbuf,
        socket_sndbuf: context.proxy_socket_sndbuf,
        http_version: version.to_string(),
    }
}

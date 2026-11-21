use super::*;

pub(in crate::server) async fn upgrade_uwsgi_tls<S>(
    stream: S,
    host: &str,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<tokio_rustls::client::TlsStream<S>, ProxyFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let raw_name = context
        .uwsgi_ssl_name
        .as_deref()
        .map(|name| eval_variables(name, variables))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| host.to_string());
    upgrade_tls_client(
        stream,
        &raw_name,
        ClientTlsOptions {
            verify: context.uwsgi_ssl_verify,
            server_name: context.uwsgi_ssl_server_name,
            session_reuse: context.uwsgi_ssl_session_reuse,
            trusted_certificate: context.uwsgi_ssl_trusted_certificate.as_deref(),
            verify_depth: context.uwsgi_ssl_verify_depth,
            crl: context.uwsgi_ssl_crl.as_deref(),
            client_certificate: context.uwsgi_ssl_certificate.as_deref(),
            client_certificate_key: context.uwsgi_ssl_certificate_key.as_deref(),
            client_certificate_passwords: &context.uwsgi_ssl_passwords,
            protocols: context.uwsgi_ssl_protocols,
            ciphers: &context.uwsgi_ssl_ciphers,
            alpn_protocols: &[],
        },
        &variables.client_tls_cache,
    )
    .await
    .map_err(proxy_failure_from_io)
}

use super::*;

#[derive(Clone)]
pub(in crate::server) struct TunnelTlsConfig {
    name: Option<String>,
    server_name: bool,
    session_reuse: bool,
    verify: bool,
    trusted_certificate: Option<PathBuf>,
    verify_depth: u32,
    crl: Option<PathBuf>,
    client_certificate: Option<PathBuf>,
    client_certificate_key: Option<PathBuf>,
    passwords: Vec<Vec<u8>>,
    protocols: TlsProtocols,
    ciphers: TlsCipherConfig,
}

impl TunnelTlsConfig {
    pub(super) fn from_context(context: &EffectiveConfig, variables: &Variables) -> Self {
        Self {
            name: context
                .proxy_ssl_name
                .as_deref()
                .map(|name| eval_variables(name, variables))
                .filter(|name| !name.is_empty()),
            server_name: context.proxy_ssl_server_name,
            session_reuse: context.proxy_ssl_session_reuse,
            verify: context.proxy_ssl_verify,
            trusted_certificate: context.proxy_ssl_trusted_certificate.clone(),
            verify_depth: context.proxy_ssl_verify_depth,
            crl: context.proxy_ssl_crl.clone(),
            client_certificate: context.proxy_ssl_certificate.clone(),
            client_certificate_key: context.proxy_ssl_certificate_key.clone(),
            passwords: context.proxy_ssl_passwords.clone(),
            protocols: context.proxy_ssl_protocols,
            ciphers: context.proxy_ssl_ciphers.clone(),
        }
    }

    pub(super) async fn upgrade(
        &self,
        stream: BoxedUpstreamIo,
        host: &str,
        cache: &ClientTlsConfigCache,
    ) -> Result<BoxedUpstreamIo, ProxyFailure> {
        let raw_name = self.name.as_deref().unwrap_or(host);
        let stream = upgrade_tls_client(
            stream,
            raw_name,
            ClientTlsOptions {
                verify: self.verify,
                server_name: self.server_name,
                session_reuse: self.session_reuse,
                trusted_certificate: self.trusted_certificate.as_deref(),
                verify_depth: self.verify_depth,
                crl: self.crl.as_deref(),
                client_certificate: self.client_certificate.as_deref(),
                client_certificate_key: self.client_certificate_key.as_deref(),
                client_certificate_passwords: &self.passwords,
                protocols: self.protocols,
                ciphers: &self.ciphers,
                alpn_protocols: &[],
            },
            cache,
        )
        .await
        .map_err(proxy_failure_from_io)?;
        Ok(Box::new(stream))
    }
}

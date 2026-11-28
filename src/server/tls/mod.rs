use super::*;

#[derive(Debug, Clone)]
pub(super) struct TlsConnectionInfo {
    pub(super) protocol: String,
    pub(super) cipher: String,
    pub(super) server_name: String,
    pub(super) alpn_protocol: String,
    pub(super) client_certificate: Option<ClientCertificateInfo>,
    pub(super) client_certificate_verified: bool,
    pub(super) client_certificate_error: Option<String>,
    pub(super) session_reused: bool,
}

pub(super) fn client_certificate_verify_failure(error: &io::Error) -> &'static str {
    let error = error.to_string();
    if error.contains("certificate revoked") {
        "certificate revoked"
    } else if error.contains("certificate status unknown") {
        "certificate status unknown"
    } else {
        "certificate status request failed"
    }
}

#[derive(Debug, Clone)]
pub(super) struct ClientCertificateInfo {
    pub(super) certificate: String,
    pub(super) escaped_certificate: String,
    pub(super) fingerprint: String,
    pub(super) serial: String,
    pub(super) subject_dn: String,
    pub(super) issuer_dn: String,
    pub(super) subject_dn_legacy: String,
    pub(super) issuer_dn_legacy: String,
    pub(super) valid_start: String,
    pub(super) valid_end: String,
    pub(super) valid_remain: String,
}

pub(super) fn build_tls_configs(
    config: &RuntimeConfig,
    groups: &[ListenerGroup],
) -> io::Result<HashMap<ListenAddress, Arc<HttpTlsConfigs>>> {
    let mut configs = HashMap::new();
    for group in groups.iter().filter(|group| group.ssl) {
        let tls_config = build_tls_config(config, group).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("cannot configure TLS for {}: {err}", group.address),
            )
        })?;
        configs.insert(group.address.clone(), tls_config);
    }
    Ok(configs)
}

#[cfg(test)]
pub(super) struct CertificateResolver {
    pub(super) exact: HashMap<String, Arc<CertifiedKey>>,
    pub(super) wildcard: Vec<(String, Arc<CertifiedKey>)>,
    pub(super) default: Arc<CertifiedKey>,
    pub(super) stapling: Option<Arc<ocsp::StaplingState>>,
}

#[cfg(test)]
impl std::fmt::Debug for CertificateResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CertificateResolver")
            .field("exact_names", &self.exact.keys().collect::<Vec<_>>())
            .field(
                "wildcard_names",
                &self
                    .wildcard
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl ResolvesServerCert for CertificateResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let Some(server_name) = client_hello.server_name() else {
            return self.default_certificate();
        };
        let server_name = server_name.trim_end_matches('.').to_ascii_lowercase();
        if let Some(certificate) = self.exact.get(&server_name) {
            return Some(certificate.clone());
        }
        for (suffix, certificate) in &self.wildcard {
            let matches = if let Some(suffix) = suffix.strip_prefix('.') {
                server_name == suffix || server_name.ends_with(&format!(".{suffix}"))
            } else if let Some(prefix) = suffix.strip_suffix('.') {
                server_name == prefix || server_name.starts_with(&format!("{prefix}."))
            } else {
                false
            };
            if matches {
                return Some(certificate.clone());
            }
        }
        self.default_certificate()
    }
}

#[cfg(test)]
impl CertificateResolver {
    fn default_certificate(&self) -> Option<Arc<CertifiedKey>> {
        if let Some(stapling) = &self.stapling {
            return Some(stapling.certificate());
        }
        Some(self.default.clone())
    }
}

pub(super) struct ServerCertificateEntry {
    certificate: Arc<CertifiedKey>,
    stapling: Option<Arc<ocsp::StaplingState>>,
}

impl ServerCertificateEntry {
    pub(super) fn new(
        certificate: Arc<CertifiedKey>,
        stapling: Option<Arc<ocsp::StaplingState>>,
    ) -> Self {
        Self {
            certificate,
            stapling,
        }
    }

    fn resolved_certificate(&self) -> Arc<CertifiedKey> {
        self.stapling
            .as_ref()
            .map(|stapling| stapling.certificate())
            .unwrap_or_else(|| self.certificate.clone())
    }
}

pub(super) struct ServerCertificateResolver {
    entries: Vec<ServerCertificateEntry>,
}

impl ServerCertificateResolver {
    pub(super) fn new(entries: Vec<ServerCertificateEntry>) -> Self {
        Self { entries }
    }
}

impl std::fmt::Debug for ServerCertificateResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerCertificateResolver")
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl ResolvesServerCert for ServerCertificateResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let offered = client_hello.signature_schemes();
        self.entries
            .iter()
            .find(|entry| entry.certificate.key.choose_scheme(offered).is_some())
            .or_else(|| self.entries.first())
            .map(ServerCertificateEntry::resolved_certificate)
    }
}

pub(super) struct HttpTlsConfigs {
    pub(super) default: Arc<HttpTlsEntry>,
    pub(super) exact: HashMap<String, Arc<HttpTlsEntry>>,
    pub(super) wildcard: Vec<(String, Arc<HttpTlsEntry>)>,
}

pub(super) struct HttpTlsEntry {
    pub(super) config: Arc<tokio_rustls::rustls::ServerConfig>,
    pub(super) client_ocsp: Option<Arc<ocsp::ClientOcspVerifier>>,
}

impl HttpTlsConfigs {
    fn select(&self, client_hello: ClientHello<'_>) -> Arc<HttpTlsEntry> {
        self.select_name(client_hello.server_name())
    }

    pub(super) fn select_name(&self, server_name: Option<&str>) -> Arc<HttpTlsEntry> {
        let Some(server_name) = server_name else {
            return self.default.clone();
        };
        let server_name = server_name.trim_end_matches('.').to_ascii_lowercase();
        if let Some(config) = self.exact.get(&server_name) {
            return config.clone();
        }
        for (suffix, config) in &self.wildcard {
            let matches = if let Some(suffix) = suffix.strip_prefix('.') {
                server_name == suffix || server_name.ends_with(&format!(".{suffix}"))
            } else if let Some(prefix) = suffix.strip_suffix('.') {
                server_name == prefix || server_name.starts_with(&format!("{prefix}."))
            } else {
                false
            };
            if matches {
                return config.clone();
            }
        }
        self.default.clone()
    }
}

pub(super) fn build_tls_config(
    config: &RuntimeConfig,
    group: &ListenerGroup,
) -> io::Result<Arc<HttpTlsConfigs>> {
    install_crypto_provider();

    let Some(http) = &config.http else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "TLS listener has no http context",
        ));
    };
    if group.quic
        && !http.servers[group.default_server_id]
            .context
            .ssl_protocols
            .tls13
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HTTP/3 requires TLSv1.3",
        ));
    }
    let default = Arc::new(build_http_server_tls_config(
        &http.servers[group.default_server_id],
        server_uses_http2(&http.servers[group.default_server_id], &group.address),
    )?);
    let mut exact = HashMap::new();
    let mut wildcard = Vec::new();
    for server_id in &group.server_ids {
        let server = &http.servers[*server_id];
        let server_config = if *server_id == group.default_server_id {
            default.clone()
        } else {
            Arc::new(build_http_server_tls_config(
                server,
                server_uses_http2(server, &group.address),
            )?)
        };
        for name in server.names.iter() {
            match name {
                ServerName::Exact(name) if !name.is_empty() => {
                    exact
                        .entry(name.clone())
                        .or_insert_with(|| server_config.clone());
                }
                ServerName::LeadingWildcard { value: name, .. }
                | ServerName::TrailingWildcard { value: name, .. } => {
                    wildcard.push((name.clone(), server_config.clone()));
                }
                _ => {}
            }
        }
    }
    wildcard.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    Ok(Arc::new(HttpTlsConfigs {
        default,
        exact,
        wildcard,
    }))
}

pub(super) fn build_http_server_tls_config(
    server: &ServerConfig,
    http2: bool,
) -> io::Result<HttpTlsEntry> {
    let certificates = load_certificate_entries(
        &server.context.ssl_certificates,
        &server.context.ssl_certificate_keys,
        server.context.ssl_certificate.as_deref(),
        server.context.ssl_certificate_key.as_deref(),
        &server.context.ssl_passwords,
    )?;
    let mut entries = Vec::with_capacity(certificates.len());
    for certificate in certificates {
        let stapling = ocsp::build_stapling(
            ocsp::StaplingOptions {
                enabled: server.context.ssl_stapling,
                verify: server.context.ssl_stapling_verify,
                file: server.context.ssl_stapling_file.as_deref(),
                responder: server.context.ssl_stapling_responder.as_deref(),
                trusted_certificate: server.context.ssl_trusted_certificate.as_deref(),
                resolver: &server.context.resolver,
                resolver_timeout: server.context.resolver_timeout,
            },
            certificate.clone(),
        )?;
        entries.push(ServerCertificateEntry::new(certificate, stapling));
    }
    let resolver = Arc::new(ServerCertificateResolver::new(entries));
    let versions = rustls_protocol_versions(server.context.ssl_protocols);
    let provider = Arc::new(tls_crypto_provider(&server.context.ssl_ciphers));
    let builder = tokio_rustls::rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&versions)
        .map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid HTTP TLS cipher configuration: {err}"),
            )
        })?;
    let client_verifier = build_client_cert_verifier(
        server.context.ssl_verify_client,
        server.context.ssl_verify_depth,
        server.context.ssl_client_certificate.as_deref(),
        server.context.ssl_trusted_certificate.as_deref(),
        server.context.ssl_crl.as_deref(),
        &provider,
    )?;
    let client_ocsp = ocsp::ClientOcspVerifier::build(ocsp::ClientOcspOptions {
        mode: server.context.ssl_ocsp,
        verify_client: server.context.ssl_verify_client,
        responder: server.context.ssl_ocsp_responder.as_deref(),
        cache: server.context.ssl_ocsp_cache.as_deref(),
        client_certificate: server.context.ssl_client_certificate.as_deref(),
        trusted_certificate: server.context.ssl_trusted_certificate.as_deref(),
        resolver: &server.context.resolver,
        resolver_timeout: server.context.resolver_timeout,
    })?;
    let mut server_config = match client_verifier {
        Some(verifier) => builder
            .with_client_cert_verifier(verifier)
            .with_cert_resolver(resolver),
        None => builder.with_no_client_auth().with_cert_resolver(resolver),
    };
    server_config.ignore_client_order = server.context.ssl_prefer_server_ciphers;
    server_config.alpn_protocols = if http2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    Ok(HttpTlsEntry {
        config: Arc::new(server_config),
        client_ocsp,
    })
}

pub(super) fn server_uses_http2(server: &ServerConfig, address: &ListenAddress) -> bool {
    server.http2
        || server
            .listens
            .iter()
            .any(|listen| &listen.address == address && listen.http2)
}

pub(super) fn rustls_protocol_versions(
    protocols: TlsProtocols,
) -> Vec<&'static tokio_rustls::rustls::SupportedProtocolVersion> {
    let mut versions = Vec::with_capacity(2);
    if protocols.tls13 {
        versions.push(&tokio_rustls::rustls::version::TLS13);
    }
    if protocols.tls12 {
        versions.push(&tokio_rustls::rustls::version::TLS12);
    }
    versions
}

pub(super) fn tls_crypto_provider(
    ciphers: &TlsCipherConfig,
) -> tokio_rustls::rustls::crypto::CryptoProvider {
    use tokio_rustls::rustls::crypto::ring::cipher_suite::{
        TLS13_AES_128_GCM_SHA256, TLS13_AES_256_GCM_SHA384, TLS13_CHACHA20_POLY1305_SHA256,
    };

    let mut provider = tokio_rustls::rustls::crypto::ring::default_provider();
    provider.cipher_suites = vec![
        TLS13_AES_256_GCM_SHA384,
        TLS13_AES_128_GCM_SHA256,
        TLS13_CHACHA20_POLY1305_SHA256,
    ];
    provider
        .cipher_suites
        .extend(ciphers.suites.iter().copied().map(tls12_cipher_suite));
    provider
}

pub(super) fn tls12_cipher_suite(
    suite: TlsCipherSuite,
) -> tokio_rustls::rustls::SupportedCipherSuite {
    use tokio_rustls::rustls::crypto::ring::cipher_suite::{
        TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256, TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256, TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384, TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
    };

    match suite {
        TlsCipherSuite::EcdheEcdsaAes256GcmSha384 => TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        TlsCipherSuite::EcdheEcdsaAes128GcmSha256 => TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        TlsCipherSuite::EcdheEcdsaChaCha20Poly1305Sha256 => {
            TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256
        }
        TlsCipherSuite::EcdheRsaAes256GcmSha384 => TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        TlsCipherSuite::EcdheRsaAes128GcmSha256 => TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        TlsCipherSuite::EcdheRsaChaCha20Poly1305Sha256 => {
            TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256
        }
    }
}

mod certificates;
mod client;
mod client_cache;
mod client_verify;
mod info;
pub(super) mod ocsp;
mod stream;

pub(super) use certificates::*;
pub(super) use client::*;
pub(super) use client_cache::*;
pub(super) use client_verify::*;
pub(super) use info::*;
pub(super) use stream::*;

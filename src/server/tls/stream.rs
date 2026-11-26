use super::*;
use crate::config::Regex;
use crate::config::StreamServer;
use crate::server::stream::stream_servers_for_address;

pub(in crate::server) struct StreamTlsConfigs {
    pub(in crate::server) default: Arc<StreamTlsEntry>,
    pub(in crate::server) exact: HashMap<String, Arc<StreamTlsEntry>>,
    pub(in crate::server) wildcard: Vec<(String, Arc<StreamTlsEntry>)>,
    pub(in crate::server) regex: Vec<(Regex, Arc<StreamTlsEntry>)>,
}

pub(in crate::server) struct StreamTlsEntry {
    pub(in crate::server) config: Arc<tokio_rustls::rustls::ServerConfig>,
    pub(in crate::server) client_ocsp: Option<Arc<super::ocsp::ClientOcspVerifier>>,
}

impl StreamTlsConfigs {
    fn select(&self, client_hello: ClientHello<'_>) -> Arc<StreamTlsEntry> {
        self.select_name(client_hello.server_name())
    }

    fn select_name(&self, server_name: Option<&str>) -> Arc<StreamTlsEntry> {
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
        for (regex, config) in &self.regex {
            if regex.is_match(&server_name) {
                return config.clone();
            }
        }
        self.default.clone()
    }
}

#[derive(Debug)]
struct RejectHandshakeResolver;

impl ResolvesServerCert for RejectHandshakeResolver {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        None
    }
}

pub(in crate::server) async fn accept_stream_tls<S>(
    stream: S,
    configs: Arc<StreamTlsConfigs>,
) -> Result<tokio_rustls::server::TlsStream<S>, StreamTlsAcceptError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let start = LazyConfigAcceptor::new(tokio_rustls::rustls::server::Acceptor::default(), stream)
        .await
        .map_err(StreamTlsAcceptError::handshake)?;
    let entry = configs.select(start.client_hello());
    let stream = start
        .into_stream(entry.config.clone())
        .await
        .map_err(StreamTlsAcceptError::handshake)?;
    if let Some(verifier) = &entry.client_ocsp {
        let certificates = stream
            .get_ref()
            .1
            .peer_certificates()
            .map(|certificates| certificates.to_vec())
            .unwrap_or_default();
        if let Err(error) = verifier.validate(&certificates).await {
            return Err(StreamTlsAcceptError {
                error,
                connection: Some(Box::new(tls_connection_info(stream.get_ref().1))),
            });
        }
    }
    Ok(stream)
}

#[derive(Debug)]
pub(in crate::server) struct StreamTlsAcceptError {
    error: io::Error,
    connection: Option<Box<TlsConnectionInfo>>,
}

impl StreamTlsAcceptError {
    fn handshake(error: io::Error) -> Self {
        Self {
            error,
            connection: None,
        }
    }

    pub(in crate::server) fn client_ocsp_failure(
        &self,
    ) -> Option<(&io::Error, &TlsConnectionInfo)> {
        self.connection
            .as_deref()
            .map(|connection| (&self.error, connection))
    }
}

impl std::fmt::Display for StreamTlsAcceptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

pub(in crate::server) fn build_stream_tls_configs(
    config: &RuntimeConfig,
) -> io::Result<HashMap<ListenAddress, Arc<StreamTlsConfigs>>> {
    let mut configs = HashMap::new();
    let Some(stream) = &config.stream else {
        return Ok(configs);
    };
    let addresses: HashSet<_> = stream
        .servers
        .iter()
        .flat_map(|server| &server.listens)
        .filter(|listen| listen.ssl)
        .map(|listen| listen.address.clone())
        .collect();
    for address in addresses {
        let servers: Vec<_> = stream_servers_for_address(stream, &address)
            .filter(|server| {
                server
                    .listens
                    .iter()
                    .any(|listen| listen.ssl && listen.address == address)
            })
            .collect();
        let default_server = servers
            .iter()
            .copied()
            .find(|server| {
                server
                    .listens
                    .iter()
                    .any(|listen| listen.ssl && listen.address == address && listen.default_server)
            })
            .or_else(|| servers.first().copied())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("no stream server configured for TLS listener {address}"),
                )
            })?;
        let tls_configs = build_stream_tls_config(address.clone(), default_server, &servers)?;
        configs.insert(address, Arc::new(tls_configs));
    }
    Ok(configs)
}

fn build_stream_tls_config(
    address: ListenAddress,
    default_server: &StreamServer,
    servers: &[&StreamServer],
) -> io::Result<StreamTlsConfigs> {
    let default = Arc::new(
        build_stream_server_tls_config(default_server).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("cannot configure stream TLS for {address}: {err}"),
            )
        })?,
    );
    let mut exact = HashMap::new();
    let mut wildcard = Vec::new();
    let mut regexes = Vec::new();
    for server in servers {
        let server_config = if server.id == default_server.id {
            default.clone()
        } else {
            Arc::new(build_stream_server_tls_config(server).map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("cannot configure stream TLS for {address}: {err}"),
                )
            })?)
        };
        for name in &server.names {
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
                ServerName::Regex { regex, .. } => {
                    regexes.push((regex.clone(), server_config.clone()));
                }
                _ => {}
            }
        }
    }
    wildcard.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    Ok(StreamTlsConfigs {
        default,
        exact,
        wildcard,
        regex: regexes,
    })
}

fn build_stream_server_tls_config(server: &StreamServer) -> io::Result<StreamTlsEntry> {
    install_crypto_provider();
    let versions = rustls_protocol_versions(server.ssl_protocols);
    let provider = Arc::new(tls_crypto_provider(&server.ssl_ciphers));
    let builder = tokio_rustls::rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&versions)
        .map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid stream TLS cipher configuration: {err}"),
            )
        })?;
    if server.ssl_reject_handshake {
        let mut server_config = builder
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(RejectHandshakeResolver));
        server_config.ignore_client_order = server.ssl_prefer_server_ciphers;
        apply_stream_alpn(&mut server_config, server);
        return Ok(StreamTlsEntry {
            config: Arc::new(server_config),
            client_ocsp: None,
        });
    }
    let certificates = load_certificate_entries(
        &server.ssl_certificates,
        &server.ssl_certificate_keys,
        server.ssl_certificate.as_deref(),
        server.ssl_certificate_key.as_deref(),
        &server.ssl_passwords,
    )?;
    let mut entries = Vec::with_capacity(certificates.len());
    for certificate in certificates {
        let stapling = super::ocsp::build_stapling(
            super::ocsp::StaplingOptions {
                enabled: server.ssl_stapling,
                verify: server.ssl_stapling_verify,
                file: server.ssl_stapling_file.as_deref(),
                responder: server.ssl_stapling_responder.as_deref(),
                trusted_certificate: server.ssl_trusted_certificate.as_deref(),
                resolver: &server.resolver,
                resolver_timeout: server.resolver_timeout,
            },
            certificate.clone(),
        )?;
        entries.push(ServerCertificateEntry::new(certificate, stapling));
    }
    let resolver = Arc::new(ServerCertificateResolver::new(entries));
    let client_verifier = build_client_cert_verifier(
        server.ssl_verify_client,
        server.ssl_verify_depth,
        server.ssl_client_certificate.as_deref(),
        server.ssl_trusted_certificate.as_deref(),
        server.ssl_crl.as_deref(),
        &provider,
    )?;
    let client_ocsp = super::ocsp::ClientOcspVerifier::build(super::ocsp::ClientOcspOptions {
        mode: server.ssl_ocsp,
        verify_client: server.ssl_verify_client,
        responder: server.ssl_ocsp_responder.as_deref(),
        cache: server.ssl_ocsp_cache.as_deref(),
        client_certificate: server.ssl_client_certificate.as_deref(),
        trusted_certificate: server.ssl_trusted_certificate.as_deref(),
        resolver: &server.resolver,
        resolver_timeout: server.resolver_timeout,
    })?;
    let mut server_config = match client_verifier {
        Some(verifier) => builder
            .with_client_cert_verifier(verifier)
            .with_cert_resolver(resolver),
        None => builder.with_no_client_auth().with_cert_resolver(resolver),
    };
    server_config.ignore_client_order = server.ssl_prefer_server_ciphers;
    apply_stream_alpn(&mut server_config, server);
    Ok(StreamTlsEntry {
        config: Arc::new(server_config),
        client_ocsp,
    })
}

fn apply_stream_alpn(config: &mut tokio_rustls::rustls::ServerConfig, server: &StreamServer) {
    if !server.ssl_alpn.is_empty() {
        config.alpn_protocols = server
            .ssl_alpn
            .iter()
            .map(|protocol| protocol.as_bytes().to_vec())
            .collect();
    }
}

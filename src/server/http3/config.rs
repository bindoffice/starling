use super::*;

pub(in crate::server) fn server_uses_http3(server: &ServerConfig, address: &ListenAddress) -> bool {
    server.http3
        || server.listens.iter().any(|listen| {
            &listen.address == address && listen.quic && (!server.http3_explicit || server.http3)
        })
}

struct Http3CertificateResolver {
    configs: Arc<HttpTlsConfigs>,
}

impl std::fmt::Debug for Http3CertificateResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Http3CertificateResolver")
            .finish_non_exhaustive()
    }
}

impl ResolvesServerCert for Http3CertificateResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let entry = self.configs.select_name(client_hello.server_name());
        entry.config.cert_resolver.resolve(client_hello)
    }
}

pub(in crate::server) fn build_http3_server_config(
    config: &RuntimeConfig,
    group: &ListenerGroup,
    tls_configs: Arc<HttpTlsConfigs>,
) -> io::Result<quinn::ServerConfig> {
    let Some(http) = &config.http else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HTTP/3 listener has no http context",
        ));
    };
    let server = &http.servers[group.default_server_id];
    if !server_uses_http3(server, &group.address) && !group.quic {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("HTTP/3 is not enabled for {}", group.address),
        ));
    }
    if !server.context.ssl_protocols.tls13 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "HTTP/3 requires TLSv1.3",
        ));
    }

    let mut crypto = (*tls_configs.default.config).clone();
    crypto.alpn_protocols = vec![b"h3".to_vec()];
    // 0-RTT remains opt-in. QUIC only permits an early-data limit of 0 or u32::MAX.
    crypto.max_early_data_size = 0;
    crypto.cert_resolver = Arc::new(Http3CertificateResolver {
        configs: tls_configs,
    });
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(crypto))
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let mut quic_config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    let mut transport = quinn::TransportConfig::default();
    transport
        .max_concurrent_bidi_streams(server.context.http2_limits.max_concurrent_streams.into());
    if !server.context.keepalive_timeout.is_zero() {
        transport.max_idle_timeout(Some(
            server
                .context
                .keepalive_timeout
                .try_into()
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?,
        ));
    }
    quic_config.transport = Arc::new(transport);
    Ok(quic_config)
}

pub(in crate::server) fn install_http3_server_configs(
    state: &SharedState,
    config: &RuntimeConfig,
    groups: &[ListenerGroup],
    tls_configs: &HashMap<ListenAddress, Arc<HttpTlsConfigs>>,
) -> io::Result<()> {
    let mut desired = HashMap::new();
    for group in groups.iter().filter(|group| group.quic) {
        let tls_config = tls_configs.get(&group.address).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("no QUIC TLS configuration for listener {}", group.address),
            )
        })?;
        desired.insert(
            group.address.clone(),
            build_http3_server_config(config, group, tls_config)?,
        );
    }

    let mut endpoints = state
        .http3_endpoints
        .lock()
        .map_err(|_| io::Error::other("HTTP/3 endpoint registry lock was poisoned"))?;
    for address in desired.keys() {
        if !endpoints.contains_key(address) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("adding QUIC listener {address} requires a restart"),
            ));
        }
    }

    let removed = endpoints
        .keys()
        .filter(|address| !desired.contains_key(*address))
        .cloned()
        .collect::<Vec<_>>();
    for address in removed {
        if let Some(endpoint) = endpoints.remove(&address) {
            endpoint.close(quinn::VarInt::from_u32(0), b"HTTP/3 disabled");
        }
    }
    for (address, server_config) in desired {
        if let Some(endpoint) = endpoints.get(&address) {
            endpoint.set_server_config(Some(server_config));
        }
    }
    Ok(())
}

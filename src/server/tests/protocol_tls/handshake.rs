use super::*;

#[tokio::test]
async fn completes_tls_handshake_and_serves_https() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certificate = fixture.join("test.crt");
    let key = fixture.join("test.key");
    let ca_certificate = fixture.join("ca.crt");
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18443 ssl;
                        server_name localhost;
                        ssl_certificate {};
                        ssl_certificate_key {};
                        ssl_protocols TLSv1.1 TLSv1.2;
                        ssl_prefer_server_ciphers on;
                        ssl_ciphers ECDHE-RSA-AES128-GCM-SHA256:DHE-RSA-AES128-GCM-SHA256;
                        location / {{
                            return 200 "secure:$scheme:$https:$ssl_protocol:$ssl_cipher:$ssl_server_name:$ssl_client_verify\n";
                        }}
                    }}
                }}
            "#,
        certificate.display(),
        key.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    assert_eq!(config.warnings.len(), 2);
    assert!(
        config
            .warnings
            .iter()
            .any(|warning| warning.message.contains("TLSv1.1"))
    );
    assert!(
        config
            .warnings
            .iter()
            .any(|warning| warning.message.contains("DHE-RSA-AES128-GCM-SHA256"))
    );
    let group = build_listener_groups(&config)[0].clone();
    assert!(group.ssl);
    let server_config = build_tls_configs(&config, std::slice::from_ref(&group))
        .unwrap()
        .remove(&group.address)
        .unwrap();
    assert!(server_config.default.config.ignore_client_order);
    let logger = Arc::new(Logger::new(&config.core.error_logs, &config.prefix).unwrap());
    let http_error_loggers = build_http_error_loggers(&config).unwrap();
    let (reload, _) = watch::channel(0_u64);
    let state = SharedState {
        config: Arc::new(ArcSwap::from_pointee(RuntimeSnapshot {
            config: config.clone(),
            listener_groups: build_listener_groups(&config).into(),
            http_error_loggers,
            upstream_keepalive_generation: 0,
        })),
        reload: Arc::new(reload),
        tls_configs: Arc::new(RwLock::new(HashMap::new())),
        stream_tls_configs: Arc::new(RwLock::new(HashMap::new())),
        logger,
        stream_loggers: Arc::new(RwLock::new(
            build_stream_loggers(&config).expect("stream error loggers"),
        )),
        limits: Arc::new(Mutex::new(LimitState::default())),
        stats: Arc::new(ConnectionStats::default()),
        upstream_keepalive: Arc::new(UpstreamKeepalivePool::default()),
        proxy_cache: Arc::new(ProxyCacheRegistry::new(config.http.as_ref())),
        client_tls_cache: Arc::new(ClientTlsConfigCache::default()),
        http2_budget: Arc::new(Http2Budget::new()),
    };
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45679".parse().unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        let stream = accept_http_tls(server_io, server_config).await.unwrap();
        let mut connection = ConnectionAddresses::direct(ListenAddress::Inet(remote), local);
        connection.tls = Some(tls_connection_info(stream.get_ref().1));
        handle_io(stream, connection, group, state).await;
    });

    let mut roots = RootCertStore::empty();
    let mut certificate_reader = StdBufReader::new(File::open(ca_certificate).unwrap());
    for certificate in rustls_pemfile::certs(&mut certificate_reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let client_config =
        ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS12])
            .with_root_certificates(roots)
            .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_config));
    let domain = TlsServerName::try_from("localhost").unwrap();
    let mut client = connector.connect(domain, client_io).await.unwrap();
    assert_eq!(
        client.get_ref().1.protocol_version(),
        Some(tokio_rustls::rustls::ProtocolVersion::TLSv1_2)
    );
    assert_eq!(
        client
            .get_ref()
            .1
            .negotiated_cipher_suite()
            .map(|suite| suite.suite()),
        Some(tokio_rustls::rustls::CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256)
    );
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    if let Err(err) = client.read_to_end(&mut response).await {
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(
        response.ends_with("secure:https:on:TLSv1.2:ECDHE-RSA-AES128-GCM-SHA256:localhost:NONE\n")
    );
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

use super::*;

#[tokio::test]
async fn configures_stream_tls_termination() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certificate = fixture.join("test.crt");
    let key = fixture.join("test.key");
    let ca_certificate = fixture.join("ca.crt");
    let client_ca_certificate = fixture.join("client-ca.crt");
    let client_certificate = fixture.join("client.crt");
    let client_key = fixture.join("client.key");
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    server {{
                        listen 127.0.0.1:15443 ssl;
                        ssl_certificate {};
                        ssl_certificate_key {};
                        ssl_protocols TLSv1.3;
                        ssl_prefer_server_ciphers on;
                        ssl_verify_client on;
                        ssl_client_certificate {};
                        proxy_pass 127.0.0.1:15432;
                    }}
                }}
            "#,
        certificate.display(),
        key.display(),
        client_ca_certificate.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let stream = config.stream.as_ref().unwrap();
    assert!(stream.servers[0].listens[0].ssl);
    assert_eq!(
        stream.servers[0].ssl_certificate.as_deref(),
        Some(certificate.as_path())
    );
    assert_eq!(
        stream.servers[0].ssl_certificate_key.as_deref(),
        Some(key.as_path())
    );
    let routes = build_stream_routes(&config);
    assert!(routes[0].ssl);
    let tls_config = build_stream_tls_configs(&config)
        .unwrap()
        .remove(&routes[0].address)
        .unwrap();
    assert!(tls_config.default.ignore_client_order);

    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        let mut inbound = accept_stream_tls(server_io, tls_config).await.unwrap();
        assert!(
            inbound
                .get_ref()
                .1
                .peer_certificates()
                .is_some_and(|certificates| !certificates.is_empty())
        );
        let mut ping = [0_u8; 4];
        inbound.read_exact(&mut ping).await.unwrap();
        inbound.write_all(&ping).await.unwrap();
        inbound.shutdown().await.unwrap();
    });

    let mut roots = RootCertStore::empty();
    let mut certificate_reader = StdBufReader::new(File::open(ca_certificate).unwrap());
    for certificate in rustls_pemfile::certs(&mut certificate_reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let mut client_certificate_reader = StdBufReader::new(File::open(client_certificate).unwrap());
    let client_certificates = rustls_pemfile::certs(&mut client_certificate_reader)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut client_key_reader = StdBufReader::new(File::open(client_key).unwrap());
    let client_private_key = rustls_pemfile::private_key(&mut client_key_reader)
        .unwrap()
        .unwrap();
    let client_config =
        ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS13])
            .with_root_certificates(roots)
            .with_client_auth_cert(client_certificates, client_private_key)
            .unwrap();
    let connector = TlsConnector::from(Arc::new(client_config));
    let domain = TlsServerName::try_from("localhost").unwrap();
    let mut client = connector.connect(domain, client_io).await.unwrap();
    assert_eq!(
        client.get_ref().1.protocol_version(),
        Some(tokio_rustls::rustls::ProtocolVersion::TLSv1_3)
    );
    client.write_all(b"ping").await.unwrap();
    let mut pong = [0_u8; 4];
    client.read_exact(&mut pong).await.unwrap();
    assert_eq!(&pong, b"ping");
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn proxies_https_upstreams_with_tls_verification() {
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
                        listen 127.0.0.1:18126;
                        location / {{
                            proxy_pass https://backend.test/;
                            proxy_http_version 1.1;
                            proxy_ssl_name localhost;
                            proxy_ssl_verify on;
                            proxy_ssl_trusted_certificate {};
                            proxy_ssl_protocols TLSv1.2;
                            proxy_ssl_ciphers ECDHE-RSA-AES128-GCM-SHA256;
                        }}
                    }}
                }}
            "#,
        ca_certificate.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let server = &config.http.as_ref().unwrap().servers[0];
    let location = &server.locations[0];
    let context = &location.context;
    let request = Request {
        method: "GET".into(),
        target: "/secure".into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18126".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18126".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
        uri: "/secure".into(),
        args: None,
        unparsed_uri: Some("/secure".into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    let outgoing = build_proxy_request(
        context.proxy_pass.as_ref().unwrap(),
        &request,
        &env,
        Some(location),
        context,
        &variables,
    );

    let certified =
        load_certificate_paths(Some(certificate.as_path()), Some(key.as_path()), &[]).unwrap();
    let resolver = Arc::new(CertificateResolver {
        exact: HashMap::new(),
        wildcard: Vec::new(),
        default: certified,
        stapling: None,
    });
    let versions = rustls_protocol_versions(TlsProtocols {
        tls12: true,
        tls13: false,
    });
    let server_config =
        tokio_rustls::rustls::ServerConfig::builder_with_protocol_versions(&versions)
            .with_no_client_auth()
            .with_cert_resolver(resolver);
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;

        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let inbound = acceptor.accept(server_io).await.unwrap();
        assert_eq!(inbound.get_ref().1.server_name(), None);
        assert_eq!(
            inbound.get_ref().1.protocol_version(),
            Some(tokio_rustls::rustls::ProtocolVersion::TLSv1_2)
        );
        assert_eq!(
            inbound
                .get_ref()
                .1
                .negotiated_cipher_suite()
                .map(|suite| suite.suite()),
            Some(tokio_rustls::rustls::CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256)
        );
        let mut reader = tokio::io::BufReader::new(inbound);
        let mut head = Vec::new();
        loop {
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).await.unwrap();
            head.extend_from_slice(&line);
            if line == b"\r\n" {
                break;
            }
        }
        let head = String::from_utf8(head).unwrap();
        assert!(head.starts_with("GET /secure HTTP/1.1\r\n"));
        let mut inbound = reader.into_inner();
        inbound
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 8\r\n\r\nupstream",
            )
            .await
            .unwrap();
        inbound.shutdown().await.unwrap();
    });
    let inbound = upgrade_proxy_tls(client_io, "localhost", context, &variables)
        .await
        .unwrap();
    assert_eq!(
        inbound
            .get_ref()
            .1
            .negotiated_cipher_suite()
            .map(|suite| suite.suite()),
        Some(tokio_rustls::rustls::CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256)
    );
    let (status, headers, body) = timed_proxy_exchange(inbound, &outgoing, "1.1", context)
        .await
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(
        headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case("content-length"))
            .map(|header| header.value.as_str()),
        Some("8")
    );
    assert_eq!(body, b"upstream");
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn presents_proxy_client_certificates_and_honors_upstream_crls() {
    use tokio_rustls::rustls::server::WebPkiClientVerifier;

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let ca_certificate = fixture.join("crl-ca.crt");
    let server_certificate = fixture.join("crl-server.crt");
    let server_key = fixture.join("crl-server.key");
    let crl = fixture.join("client.crl");
    let provider = Arc::new(tls_crypto_provider(&TlsCipherConfig::default()));
    let verifier = WebPkiClientVerifier::builder_with_provider(
        Arc::new(load_root_certificates(&ca_certificate).unwrap()),
        provider.clone(),
    )
    .with_crls(load_revocation_lists(&crl).unwrap())
    .enforce_revocation_expiration()
    .build()
    .unwrap();
    let server_config = Arc::new(
        tokio_rustls::rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&rustls_protocol_versions(TlsProtocols::default()))
            .unwrap()
            .with_client_cert_verifier(verifier)
            .with_cert_resolver(Arc::new(CertificateResolver {
                exact: HashMap::new(),
                wildcard: Vec::new(),
                default: load_certificate_paths(
                    Some(server_certificate.as_path()),
                    Some(server_key.as_path()),
                    &[],
                )
                .unwrap(),
                stapling: None,
            })),
    );

    for (certificate, key, passwords, accepted) in [
        ("crl-client.crt", "crl-client.key", Vec::new(), true),
        (
            "crl-client.crt",
            "crl-client-encrypted.key",
            vec![b"wrong-password".to_vec(), b"stream-secret".to_vec()],
            true,
        ),
        (
            "crl-revoked-client.crt",
            "crl-revoked-client.key",
            Vec::new(),
            false,
        ),
    ] {
        let certificate = fixture.join(certificate);
        let key = fixture.join(key);
        let client_config = build_client_tls_config(ClientTlsOptions {
            verify: true,
            server_name: true,
            session_reuse: true,
            trusted_certificate: Some(ca_certificate.as_path()),
            verify_depth: 1,
            crl: None,
            client_certificate: Some(certificate.as_path()),
            client_certificate_key: Some(key.as_path()),
            client_certificate_passwords: &passwords,
            protocols: TlsProtocols::default(),
            ciphers: &TlsCipherConfig::default(),
            alpn_protocols: &[],
        })
        .unwrap();
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let server_config = server_config.clone();
        let server_task = tokio::spawn(async move {
            match TlsAcceptor::from(server_config).accept(server_io).await {
                Ok(stream) => stream
                    .get_ref()
                    .1
                    .peer_certificates()
                    .is_some_and(|certificates| !certificates.is_empty()),
                Err(err) => {
                    eprintln!("upstream TLS client authentication failed: {err}");
                    false
                }
            }
        });
        let connector = TlsConnector::from(client_config);
        let domain = TlsServerName::try_from("localhost").unwrap();
        let result = connector.connect(domain, client_io).await;
        if accepted {
            let client = result.unwrap();
            assert!(server_task.await.unwrap());
            drop(client);
        } else {
            let client = result.ok();
            assert!(!server_task.await.unwrap());
            drop(client);
        }
    }
}

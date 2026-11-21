use super::*;

#[test]
fn exposes_client_certificate_metadata_variables() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certificates = load_certificates(&fixture.join("client.crt")).unwrap();
    let certificate = client_certificate_info(&certificates[0]).unwrap();
    assert!(
        certificate
            .certificate
            .starts_with("-----BEGIN CERTIFICATE-----\n\t")
    );
    assert!(
        certificate
            .certificate
            .ends_with("-----END CERTIFICATE-----")
    );
    assert!(
        certificate
            .escaped_certificate
            .starts_with("-----BEGIN%20CERTIFICATE-----%0A")
    );
    assert_eq!(
        certificate.fingerprint,
        "fb02ddff63563d54a3c7b9d64092439a41bae38b"
    );
    assert_eq!(
        certificate.serial,
        "62:AC:35:94:3F:01:CB:DE:E5:35:E1:A8:06:6F:BF:A9:F5:33:87:24"
    );
    assert_eq!(certificate.subject_dn, "CN=starling-client");
    assert_eq!(certificate.issuer_dn, "CN=Starling Client Test CA");
    assert_eq!(certificate.subject_dn_legacy, "/CN=starling-client");
    assert_eq!(certificate.issuer_dn_legacy, "/CN=Starling Client Test CA");
    assert_eq!(certificate.valid_start, "Nov 19 11:28:03 2026 GMT");
    assert_eq!(certificate.valid_end, "Nov 16 11:28:03 2036 GMT");
    assert!(certificate.valid_remain.parse::<u64>().unwrap() > 0);
}

#[tokio::test]
async fn enforces_http_client_certificate_verification() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certificate = fixture.join("test.crt");
    let key = fixture.join("test.key");
    let ca_certificate = fixture.join("ca.crt");
    let client_ca_certificate = fixture.join("client-ca.crt");
    let client_certificate = fixture.join("client.crt");
    let client_key = fixture.join("client.key");

    let mut roots = RootCertStore::empty();
    let mut certificate_reader = StdBufReader::new(File::open(&ca_certificate).unwrap());
    for certificate in rustls_pemfile::certs(&mut certificate_reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let client_config = |with_certificate: bool| {
        let builder =
            ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS12])
                .with_root_certificates(roots.clone());
        if with_certificate {
            let mut certificate_reader =
                StdBufReader::new(File::open(&client_certificate).unwrap());
            let certificates = rustls_pemfile::certs(&mut certificate_reader)
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let mut key_reader = StdBufReader::new(File::open(&client_key).unwrap());
            let private_key = rustls_pemfile::private_key(&mut key_reader)
                .unwrap()
                .unwrap();
            Arc::new(
                builder
                    .with_client_auth_cert(certificates, private_key)
                    .unwrap(),
            )
        } else {
            Arc::new(builder.with_no_client_auth())
        }
    };

    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18470 ssl;
                        ssl_certificate {};
                        ssl_certificate_key {};
                        ssl_protocols TLSv1.2;
                        ssl_verify_client on;
                        ssl_client_certificate {};
                    }}
                }}
            "#,
        certificate.display(),
        key.display(),
        client_ca_certificate.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let server_config = build_tls_configs(&config, std::slice::from_ref(&group))
        .unwrap()
        .remove(&group.address)
        .unwrap();

    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let reject_config = server_config.clone();
    let rejected_server = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(5),
            accept_http_tls(server_io, reject_config),
        )
        .await
        .is_ok_and(|result| result.is_err())
    });
    let connector = TlsConnector::from(client_config(false));
    let domain = TlsServerName::try_from("localhost").unwrap();
    assert!(connector.connect(domain, client_io).await.is_err());
    assert!(rejected_server.await.unwrap());

    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let accept_config = server_config.clone();
    let accepted_server = tokio::spawn(async move {
        let stream = accept_http_tls(server_io, accept_config).await.unwrap();
        stream
            .get_ref()
            .1
            .peer_certificates()
            .is_some_and(|certs| !certs.is_empty())
    });
    let connector = TlsConnector::from(client_config(true));
    let domain = TlsServerName::try_from("localhost").unwrap();
    let stream = connector.connect(domain, client_io).await.unwrap();
    assert!(
        stream
            .get_ref()
            .1
            .peer_certificates()
            .is_some_and(|certs| !certs.is_empty())
    );
    assert!(accepted_server.await.unwrap());
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn accepts_optional_no_ca_client_certificates() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certificate = fixture.join("test.crt");
    let key = fixture.join("test.key");
    let ca_certificate = fixture.join("ca.crt");
    let client_certificate = fixture.join("client.crt");
    let client_key = fixture.join("client.key");
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18471 ssl;
                        ssl_certificate {};
                        ssl_certificate_key {};
                        ssl_protocols TLSv1.2;
                        ssl_verify_client optional_no_ca;
                    }}
                }}
            "#,
        certificate.display(),
        key.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let server_config = build_tls_configs(&config, std::slice::from_ref(&group))
        .unwrap()
        .remove(&group.address)
        .unwrap();

    let mut roots = RootCertStore::empty();
    let mut certificate_reader = StdBufReader::new(File::open(ca_certificate).unwrap());
    for certificate in rustls_pemfile::certs(&mut certificate_reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let client_config =
        ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS12])
            .with_root_certificates(roots.clone())
            .with_no_client_auth();

    let no_client_config = server_config.clone();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server =
        tokio::spawn(async move { accept_http_tls(server_io, no_client_config).await.is_ok() });
    let connector = TlsConnector::from(Arc::new(client_config));
    let domain = TlsServerName::try_from("localhost").unwrap();
    connector.connect(domain, client_io).await.unwrap();
    assert!(server.await.unwrap());

    let mut client_certificate_reader = StdBufReader::new(File::open(client_certificate).unwrap());
    let client_certificates = rustls_pemfile::certs(&mut client_certificate_reader)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut client_key_reader = StdBufReader::new(File::open(client_key).unwrap());
    let client_private_key = rustls_pemfile::private_key(&mut client_key_reader)
        .unwrap()
        .unwrap();
    let client_config =
        ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS12])
            .with_root_certificates(roots)
            .with_client_auth_cert(client_certificates, client_private_key)
            .unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let stream = accept_http_tls(server_io, server_config).await.unwrap();
        stream
            .get_ref()
            .1
            .peer_certificates()
            .is_some_and(|certificates| !certificates.is_empty())
    });
    let connector = TlsConnector::from(Arc::new(client_config));
    let domain = TlsServerName::try_from("localhost").unwrap();
    connector.connect(domain, client_io).await.unwrap();
    assert!(server.await.unwrap());
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn enforces_client_certificate_verify_depth() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let client_ca_certificate = fixture.join("client-ca.crt");
    let client_certificate = fixture.join("client.crt");
    let mut certificate_reader = StdBufReader::new(File::open(client_certificate).unwrap());
    let client_certificate = rustls_pemfile::certs(&mut certificate_reader)
        .next()
        .unwrap()
        .unwrap();
    let provider = Arc::new(tls_crypto_provider(&TlsCipherConfig::default()));
    let now = tokio_rustls::rustls::pki_types::UnixTime::now();

    let depth_zero = build_client_cert_verifier(
        ClientCertificateVerify::On,
        0,
        Some(client_ca_certificate.as_path()),
        None,
        None,
        &provider,
    )
    .unwrap()
    .unwrap();
    assert!(
        depth_zero
            .verify_client_cert(&client_certificate, &[], now)
            .is_err()
    );

    let depth_one = build_client_cert_verifier(
        ClientCertificateVerify::On,
        1,
        Some(client_ca_certificate.as_path()),
        None,
        None,
        &provider,
    )
    .unwrap()
    .unwrap();
    assert!(
        depth_one
            .verify_client_cert(&client_certificate, &[], now)
            .is_ok()
    );
}

#[test]
fn enforces_client_certificate_revocation_lists() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let client_ca_certificate = fixture.join("crl-ca.crt");
    let crl = fixture.join("client.crl");
    let load_certificate = |name: &str| {
        let path = fixture.join(name);
        let mut reader = StdBufReader::new(File::open(path).unwrap());
        rustls_pemfile::certs(&mut reader).next().unwrap().unwrap()
    };
    let revoked = load_certificate("crl-revoked-client.crt");
    let valid = load_certificate("crl-client.crt");
    let provider = Arc::new(tls_crypto_provider(&TlsCipherConfig::default()));
    let now = tokio_rustls::rustls::pki_types::UnixTime::now();
    let verifier = build_client_cert_verifier(
        ClientCertificateVerify::On,
        1,
        Some(client_ca_certificate.as_path()),
        None,
        Some(crl.as_path()),
        &provider,
    )
    .unwrap()
    .unwrap();

    assert!(verifier.verify_client_cert(&revoked, &[], now).is_err());
    assert!(verifier.verify_client_cert(&valid, &[], now).is_ok());
}

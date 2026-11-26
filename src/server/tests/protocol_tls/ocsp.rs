use super::*;

#[tokio::test]
async fn sends_configured_ocsp_response_in_the_tls_handshake() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocsp");
    let certificate = fixture.join("server.crt");
    let key = fixture.join("server.key");
    let response = std::fs::read(fixture.join("response.der")).unwrap();
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18473 ssl;
                    server_name localhost;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_trusted_certificate {};
                    ssl_stapling on;
                    ssl_stapling_file {};
                    location / {{ return 204; }}
                }}
            }}
        "#,
        certificate.display(),
        key.display(),
        fixture.join("ca.crt").display(),
        fixture.join("response.der").display(),
    );
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let server_config = build_tls_configs(&config, std::slice::from_ref(&group))
        .unwrap()
        .remove(&group.address)
        .unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        accept_http_tls(server_io, server_config).await.unwrap();
    });

    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(tls_crypto_provider(&TlsCipherConfig::default()));
    let verifier = Arc::new(CaptureOcspVerifier {
        ocsp: captured.clone(),
        supported_algs: provider.signature_verification_algorithms,
    });
    let client_config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&rustls_protocol_versions(TlsProtocols::default()))
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_config));
    let domain = TlsServerName::try_from("localhost").unwrap();
    let client = connector.connect(domain, client_io).await.unwrap();
    assert_eq!(&*captured.lock().unwrap(), &response);
    server_task.await.unwrap();
    drop(client);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn validates_client_certificate_ocsp_status_during_accept() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let ocsp_fixture = fixture.join("ocsp");
    let server_certificate = fixture.join("test.crt");
    let server_key = fixture.join("test.key");
    let trusted_server_ca = fixture.join("ca.crt");
    let client_certificate = ocsp_fixture.join("server.crt");
    let client_key = ocsp_fixture.join("server.key");
    let client_ca = ocsp_fixture.join("ca.crt");
    let ocsp_response = std::fs::read(ocsp_fixture.join("response.der")).unwrap();
    let responder = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let responder_address = responder.local_addr().unwrap();
    let responder_response = ocsp_response.clone();
    let responder_task = tokio::spawn(async move {
        let (mut stream, _) = responder.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let length = stream.read(&mut buffer).await.unwrap();
            assert_ne!(length, 0);
            request.extend_from_slice(&buffer[..length]);
        }
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/ocsp-response\r\nContent-Length: {}\r\n\r\n",
            responder_response.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&responder_response).await.unwrap();
    });

    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18474 ssl;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_protocols TLSv1.2;
                    ssl_verify_client on;
                    ssl_client_certificate {};
                    ssl_ocsp leaf;
                    ssl_ocsp_responder http://{responder_address}/ocsp;
                }}
            }}
        "#,
        server_certificate.display(),
        server_key.display(),
        client_ca.display(),
    );
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let server_config = build_tls_configs(&config, std::slice::from_ref(&group))
        .unwrap()
        .remove(&group.address)
        .unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server =
        tokio::spawn(async move { accept_http_tls(server_io, server_config).await.is_ok() });

    let mut roots = RootCertStore::empty();
    let mut root_reader = StdBufReader::new(File::open(trusted_server_ca).unwrap());
    for certificate in rustls_pemfile::certs(&mut root_reader) {
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
        ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS12])
            .with_root_certificates(roots)
            .with_client_auth_cert(client_certificates, client_private_key)
            .unwrap();
    let connector = TlsConnector::from(Arc::new(client_config));
    let domain = TlsServerName::try_from("localhost").unwrap();
    let _client = connector.connect(domain, client_io).await.unwrap();
    assert!(server.await.unwrap());
    responder_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[derive(Debug)]
struct CaptureOcspVerifier {
    ocsp: Arc<Mutex<Vec<u8>>>,
    supported_algs: tokio_rustls::rustls::crypto::WebPkiSupportedAlgorithms,
}

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for CaptureOcspVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[tokio_rustls::rustls::pki_types::CertificateDer<'_>],
        _server_name: &TlsServerName<'_>,
        ocsp_response: &[u8],
        _now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::client::danger::ServerCertVerified, tokio_rustls::rustls::Error>
    {
        *self.ocsp.lock().unwrap() = ocsp_response.to_vec();
        Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        tokio_rustls::rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.supported_algs,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        tokio_rustls::rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.supported_algs,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        self.supported_algs.supported_schemes()
    }
}

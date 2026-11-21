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
                    ssl_stapling on;
                    ssl_stapling_file {};
                    location / {{ return 204; }}
                }}
            }}
        "#,
        certificate.display(),
        key.display(),
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

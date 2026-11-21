use super::*;

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &tokio_rustls::rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::client::danger::ServerCertVerified, tokio_rustls::rustls::Error>
    {
        Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        tokio_rustls::rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        tokio_rustls::rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[derive(Debug)]
pub(in crate::server) struct VerifyDepthServerVerifier {
    inner: Arc<dyn tokio_rustls::rustls::client::danger::ServerCertVerifier>,
    max_depth: usize,
}

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for VerifyDepthServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &tokio_rustls::rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::client::danger::ServerCertVerified, tokio_rustls::rustls::Error>
    {
        if intermediates.len() >= self.max_depth {
            let error = io::Error::new(
                io::ErrorKind::InvalidData,
                "server certificate chain exceeds proxy_ssl_verify_depth",
            );
            return Err(tokio_rustls::rustls::Error::InvalidCertificate(
                tokio_rustls::rustls::CertificateError::Other(tokio_rustls::rustls::OtherError(
                    Arc::new(error),
                )),
            ));
        }
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }

    fn requires_raw_public_keys(&self) -> bool {
        self.inner.requires_raw_public_keys()
    }

    fn root_hint_subjects(&self) -> Option<&[tokio_rustls::rustls::DistinguishedName]> {
        self.inner.root_hint_subjects()
    }
}

#[derive(Clone, Copy)]
pub(in crate::server) struct ClientTlsOptions<'a> {
    pub(in crate::server) verify: bool,
    pub(in crate::server) server_name: bool,
    pub(in crate::server) session_reuse: bool,
    pub(in crate::server) trusted_certificate: Option<&'a Path>,
    pub(in crate::server) verify_depth: u32,
    pub(in crate::server) crl: Option<&'a Path>,
    pub(in crate::server) client_certificate: Option<&'a Path>,
    pub(in crate::server) client_certificate_key: Option<&'a Path>,
    pub(in crate::server) client_certificate_passwords: &'a [Vec<u8>],
    pub(in crate::server) protocols: TlsProtocols,
    pub(in crate::server) ciphers: &'a TlsCipherConfig,
    pub(in crate::server) alpn_protocols: &'a [Vec<u8>],
}

pub(in crate::server) fn build_client_tls_config(
    options: ClientTlsOptions<'_>,
) -> io::Result<Arc<tokio_rustls::rustls::ClientConfig>> {
    let ClientTlsOptions {
        verify,
        server_name,
        session_reuse,
        trusted_certificate,
        verify_depth,
        crl,
        client_certificate,
        client_certificate_key,
        client_certificate_passwords,
        protocols,
        ciphers,
        alpn_protocols,
    } = options;
    install_crypto_provider();
    let versions = rustls_protocol_versions(protocols);
    let provider = Arc::new(tls_crypto_provider(ciphers));
    let builder = tokio_rustls::rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&versions)
        .map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid proxy TLS cipher configuration: {err}"),
            )
        })?;
    let builder = if verify {
        let certificate_path = trusted_certificate
            .map(Path::to_path_buf)
            .or_else(|| {
                [
                    "/etc/ssl/certs/ca-certificates.crt",
                    "/etc/pki/tls/certs/ca-bundle.crt",
                    "/etc/ssl/cert.pem",
                ]
                .into_iter()
                .map(PathBuf::from)
                .find(|path| path.exists())
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "proxy_ssl_verify requires a trusted certificate bundle",
                )
            })?;
        let roots = load_root_certificates(&certificate_path)?;
        let mut verifier_builder =
            tokio_rustls::rustls::client::WebPkiServerVerifier::builder_with_provider(
                Arc::new(roots),
                provider.clone(),
            );
        if let Some(crl) = crl {
            verifier_builder = verifier_builder
                .with_crls(load_revocation_lists(crl)?)
                .enforce_revocation_expiration();
        }
        let verifier = verifier_builder.build().map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "cannot configure proxy server certificate verification from {}: {err}",
                    certificate_path.display()
                ),
            )
        })?;
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(VerifyDepthServerVerifier {
                inner: verifier,
                max_depth: verify_depth as usize,
            }))
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertificateVerification(provider)))
    };
    let mut config = match (client_certificate, client_certificate_key) {
        (Some(certificate), Some(key)) => builder
            .with_client_auth_cert(
                load_certificates(certificate)?,
                load_private_key(key, client_certificate_passwords)?,
            )
            .map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("cannot configure proxy client certificate: {err}"),
                )
            })?,
        (None, None) => builder.with_no_client_auth(),
        (Some(_), None) => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no proxy_ssl_certificate_key is defined for proxy_ssl_certificate",
            ));
        }
        (None, Some(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no proxy_ssl_certificate is defined for proxy_ssl_certificate_key",
            ));
        }
    };
    config.alpn_protocols = alpn_protocols.to_vec();
    config.enable_sni = server_name;
    if !session_reuse {
        config.resumption = tokio_rustls::rustls::client::Resumption::disabled();
    }
    Ok(Arc::new(config))
}

pub(in crate::server) async fn accept_http_tls<S>(
    stream: S,
    configs: Arc<HttpTlsConfigs>,
) -> io::Result<tokio_rustls::server::TlsStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let start =
        LazyConfigAcceptor::new(tokio_rustls::rustls::server::Acceptor::default(), stream).await?;
    let config = configs.select(start.client_hello());
    start.into_stream(config).await
}

pub(in crate::server) fn tls_connection_info(
    connection: &tokio_rustls::rustls::ServerConnection,
) -> TlsConnectionInfo {
    let client_certificate = connection
        .peer_certificates()
        .and_then(|certificates| certificates.first())
        .and_then(client_certificate_info);
    TlsConnectionInfo {
        protocol: match connection.protocol_version() {
            Some(tokio_rustls::rustls::ProtocolVersion::TLSv1_2) => "TLSv1.2".into(),
            Some(tokio_rustls::rustls::ProtocolVersion::TLSv1_3) => "TLSv1.3".into(),
            Some(version) => format!("{version:?}"),
            None => String::new(),
        },
        cipher: connection
            .negotiated_cipher_suite()
            .map(|suite| tls_cipher_name(suite.suite()))
            .unwrap_or_default(),
        server_name: connection.server_name().unwrap_or_default().to_string(),
        alpn_protocol: connection
            .alpn_protocol()
            .map(|protocol| String::from_utf8_lossy(protocol).into_owned())
            .unwrap_or_default(),
        client_certificate,
        client_certificate_verified: false,
        session_reused: matches!(
            connection.handshake_kind(),
            Some(tokio_rustls::rustls::HandshakeKind::Resumed)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_the_proxy_server_name_setting() {
        let ciphers = TlsCipherConfig::default();
        for (server_name, session_reuse) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            build_client_tls_config(ClientTlsOptions {
                verify: false,
                server_name,
                session_reuse,
                trusted_certificate: None,
                verify_depth: 1,
                crl: None,
                client_certificate: None,
                client_certificate_key: None,
                client_certificate_passwords: &[],
                protocols: TlsProtocols::default(),
                ciphers: &ciphers,
                alpn_protocols: &[],
            })
            .unwrap();
        }
    }
}

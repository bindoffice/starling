use super::*;

pub(in crate::server) fn build_client_cert_verifier(
    verify: ClientCertificateVerify,
    verify_depth: u32,
    client_certificate: Option<&Path>,
    trusted_certificate: Option<&Path>,
    crl: Option<&Path>,
    provider: &Arc<tokio_rustls::rustls::crypto::CryptoProvider>,
) -> io::Result<Option<Arc<dyn tokio_rustls::rustls::server::danger::ClientCertVerifier>>> {
    use tokio_rustls::rustls::server::WebPkiClientVerifier;

    let verifier: Arc<dyn tokio_rustls::rustls::server::danger::ClientCertVerifier> = match verify {
        ClientCertificateVerify::Off => return Ok(None),
        ClientCertificateVerify::OptionalNoCa => Arc::new(OptionalNoCaClientVerifier {
            root_hint_subjects: Vec::new(),
            supported_algs: provider.signature_verification_algorithms,
        }),
        ClientCertificateVerify::On | ClientCertificateVerify::Optional => {
            let certificate_path = client_certificate.or(trusted_certificate).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "no ssl_client_certificate or ssl_trusted_certificate for ssl_verify_client",
                )
            })?;
            let roots = load_root_certificates(certificate_path)?;
            let mut builder =
                WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone());
            if let Some(crl) = crl {
                builder = builder
                    .with_crls(load_revocation_lists(crl)?)
                    .enforce_revocation_expiration();
            }
            let builder = if matches!(verify, ClientCertificateVerify::Optional) {
                builder.allow_unauthenticated()
            } else {
                builder
            };
            builder.build().map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "cannot configure client certificate verification from {}: {err}",
                        certificate_path.display()
                    ),
                )
            })?
        }
    };

    Ok(Some(Arc::new(VerifyDepthClientVerifier {
        inner: verifier,
        max_depth: verify_depth as usize,
    })))
}

pub(in crate::server) fn load_root_certificates(
    path: &Path,
) -> io::Result<tokio_rustls::rustls::RootCertStore> {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    let mut reader = StdBufReader::new(File::open(path)?);
    let certificates = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    for certificate in certificates {
        roots.add(certificate).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid trusted certificate in {}: {err}", path.display()),
            )
        })?;
    }
    if roots.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("no PEM certificates in {}", path.display()),
        ));
    }
    Ok(roots)
}

pub(in crate::server) fn load_revocation_lists(
    path: &Path,
) -> io::Result<Vec<CertificateRevocationListDer<'static>>> {
    let mut reader = StdBufReader::new(File::open(path)?);
    let crls = rustls_pemfile::crls(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("cannot read CRL from {}: {err}", path.display()),
            )
        })?;
    if crls.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("no PEM CRLs in {}", path.display()),
        ));
    }
    Ok(crls)
}

#[derive(Debug)]
pub(in crate::server) struct VerifyDepthClientVerifier {
    inner: Arc<dyn tokio_rustls::rustls::server::danger::ClientCertVerifier>,
    max_depth: usize,
}

impl tokio_rustls::rustls::server::danger::ClientCertVerifier for VerifyDepthClientVerifier {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }

    fn root_hint_subjects(&self) -> &[tokio_rustls::rustls::DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::server::danger::ClientCertVerified, tokio_rustls::rustls::Error>
    {
        if intermediates.len() >= self.max_depth {
            let error = io::Error::new(
                io::ErrorKind::InvalidData,
                "client certificate chain exceeds ssl_verify_depth",
            );
            return Err(tokio_rustls::rustls::Error::InvalidCertificate(
                tokio_rustls::rustls::CertificateError::Other(tokio_rustls::rustls::OtherError(
                    Arc::new(error),
                )),
            ));
        }
        self.inner
            .verify_client_cert(end_entity, intermediates, now)
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
}

#[derive(Debug)]
pub(in crate::server) struct OptionalNoCaClientVerifier {
    root_hint_subjects: Vec<tokio_rustls::rustls::DistinguishedName>,
    supported_algs: tokio_rustls::rustls::crypto::WebPkiSupportedAlgorithms,
}

impl tokio_rustls::rustls::server::danger::ClientCertVerifier for OptionalNoCaClientVerifier {
    fn client_auth_mandatory(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> &[tokio_rustls::rustls::DistinguishedName] {
        &self.root_hint_subjects
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::server::danger::ClientCertVerified, tokio_rustls::rustls::Error>
    {
        Ok(tokio_rustls::rustls::server::danger::ClientCertVerified::assertion())
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
            &self.supported_algs,
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
            &self.supported_algs,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        self.supported_algs.supported_schemes()
    }
}

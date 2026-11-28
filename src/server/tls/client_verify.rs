use super::*;
use openssl::stack::Stack;
use openssl::x509::store::{File as X509File, X509Lookup, X509Store, X509StoreBuilder};
use openssl::x509::verify::X509VerifyFlags;
use openssl::x509::{X509, X509PurposeId, X509StoreContext};

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
            let verifier = builder.build().map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "cannot configure client certificate verification from {}: {err}",
                        certificate_path.display()
                    ),
                )
            })?;
            let openssl_store = if crl.is_none() {
                Some(build_openssl_client_store(&[certificate_path], None)?)
            } else {
                None
            };
            Arc::new(AcceptCaClientVerifier {
                inner: verifier,
                openssl_store,
            })
        }
    };

    Ok(Some(Arc::new(VerifyDepthClientVerifier {
        inner: verifier,
        max_depth: verify_depth as usize,
    })))
}

pub(in crate::server) fn build_mail_client_cert_verifier(
    verify: ClientCertificateVerify,
    client_certificate: Option<&Path>,
    provider: &Arc<tokio_rustls::rustls::crypto::CryptoProvider>,
) -> io::Result<Option<Arc<dyn tokio_rustls::rustls::server::danger::ClientCertVerifier>>> {
    if verify == ClientCertificateVerify::Off {
        return Ok(None);
    }

    let root_hint_subjects = match client_certificate {
        Some(path) => load_root_certificates(path)?.subjects(),
        None => Vec::new(),
    };
    Ok(Some(Arc::new(MailClientCertVerifier {
        root_hint_subjects,
        supported_algs: provider.signature_verification_algorithms,
    })))
}

pub(in crate::server) fn build_mail_client_certificate_verifier(
    verify: ClientCertificateVerify,
    verify_depth: u32,
    client_certificate: Option<&Path>,
    trusted_certificate: Option<&Path>,
    crl: Option<&Path>,
) -> io::Result<Option<Arc<MailClientCertificateVerifier>>> {
    if verify == ClientCertificateVerify::Off {
        return Ok(None);
    }

    let mut paths = Vec::with_capacity(2);
    for path in [client_certificate, trusted_certificate]
        .into_iter()
        .flatten()
    {
        if !paths.iter().any(|existing| *existing == path) {
            paths.push(path);
        }
    }
    let store = build_openssl_client_store(&paths, crl)?;
    Ok(Some(Arc::new(MailClientCertificateVerifier {
        max_depth: verify_depth as usize,
        store,
    })))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::server) enum MailClientCertificateStatus {
    None,
    Success,
    Failed(String),
}

impl MailClientCertificateStatus {
    pub(in crate::server) fn as_auth_http_value(&self) -> String {
        match self {
            Self::None => "NONE".into(),
            Self::Success => "SUCCESS".into(),
            Self::Failed(error) => format!("FAILED:{error}"),
        }
    }
}

pub(in crate::server) struct MailClientCertificateVerifier {
    max_depth: usize,
    store: X509Store,
}

impl MailClientCertificateVerifier {
    pub(in crate::server) fn verify(
        &self,
        certificates: &[CertificateDer<'_>],
    ) -> MailClientCertificateStatus {
        let Some(end_entity) = certificates.first() else {
            return MailClientCertificateStatus::None;
        };
        if certificates.len().saturating_sub(1) >= self.max_depth {
            return MailClientCertificateStatus::Failed(format!(
                "certificate chain exceeds ssl_verify_depth {}",
                self.max_depth
            ));
        }

        match verify_client_chain(&self.store, end_entity, &certificates[1..]) {
            Ok(()) => MailClientCertificateStatus::Success,
            Err(error) => MailClientCertificateStatus::Failed(error),
        }
    }
}

#[derive(Debug)]
struct MailClientCertVerifier {
    root_hint_subjects: Vec<tokio_rustls::rustls::DistinguishedName>,
    supported_algs: tokio_rustls::rustls::crypto::WebPkiSupportedAlgorithms,
}

impl tokio_rustls::rustls::server::danger::ClientCertVerifier for MailClientCertVerifier {
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

struct AcceptCaClientVerifier {
    inner: Arc<dyn tokio_rustls::rustls::server::danger::ClientCertVerifier>,
    openssl_store: Option<X509Store>,
}

impl std::fmt::Debug for AcceptCaClientVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AcceptCaClientVerifier")
            .field("has_openssl_fallback", &self.openssl_store.is_some())
            .finish_non_exhaustive()
    }
}

impl tokio_rustls::rustls::server::danger::ClientCertVerifier for AcceptCaClientVerifier {
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
        match self
            .inner
            .verify_client_cert(end_entity, intermediates, now)
        {
            Ok(verified) => Ok(verified),
            Err(tokio_rustls::rustls::Error::InvalidCertificate(
                tokio_rustls::rustls::CertificateError::Other(_),
            )) => {
                let Some(store) = &self.openssl_store else {
                    return Err(tokio_rustls::rustls::Error::InvalidCertificate(
                        tokio_rustls::rustls::CertificateError::ApplicationVerificationFailure,
                    ));
                };
                verify_client_chain_with_openssl(store, end_entity, intermediates)
            }
            Err(error) => Err(error),
        }
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

fn build_openssl_client_store(paths: &[&Path], crl: Option<&Path>) -> io::Result<X509Store> {
    use openssl::ssl::SslFiletype;

    let mut store = X509StoreBuilder::new().map_err(openssl_error)?;
    for path in paths {
        let lookup = store
            .add_lookup(X509Lookup::<X509File>::file())
            .map_err(openssl_error)?;
        lookup
            .load_cert_file(path, SslFiletype::PEM)
            .map_err(openssl_error)?;
    }
    if let Some(crl) = crl {
        let lookup = store
            .add_lookup(X509Lookup::<X509File>::file())
            .map_err(openssl_error)?;
        lookup
            .load_crl_file(crl, SslFiletype::PEM)
            .map_err(openssl_error)?;
        store
            .set_flags(X509VerifyFlags::CRL_CHECK)
            .map_err(openssl_error)?;
    }
    store
        .set_purpose(X509PurposeId::SSL_CLIENT)
        .map_err(openssl_error)?;
    Ok(store.build())
}

fn verify_client_chain(
    store: &X509Store,
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
) -> Result<(), String> {
    let verify = || -> io::Result<Result<(), String>> {
        let end_entity = X509::from_der(end_entity.as_ref()).map_err(openssl_error)?;
        let mut chain = Stack::new().map_err(openssl_error)?;
        for certificate in intermediates {
            chain
                .push(X509::from_der(certificate.as_ref()).map_err(openssl_error)?)
                .map_err(openssl_error)?;
        }
        let mut context = X509StoreContext::new().map_err(openssl_error)?;
        context
            .init(store, &end_entity, &chain, |context| {
                if context.verify_cert()? {
                    Ok(Ok(()))
                } else {
                    Ok(Err(context.error().error_string().to_string()))
                }
            })
            .map_err(openssl_error)
    };
    verify().map_err(|error| error.to_string())?
}

fn verify_client_chain_with_openssl(
    store: &X509Store,
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
) -> Result<tokio_rustls::rustls::server::danger::ClientCertVerified, tokio_rustls::rustls::Error> {
    let verify = || -> io::Result<bool> {
        let end_entity = X509::from_der(end_entity.as_ref()).map_err(openssl_error)?;
        let mut chain = Stack::new().map_err(openssl_error)?;
        for certificate in intermediates {
            chain
                .push(X509::from_der(certificate.as_ref()).map_err(openssl_error)?)
                .map_err(openssl_error)?;
        }
        let mut context = X509StoreContext::new().map_err(openssl_error)?;
        context
            .init(store, &end_entity, &chain, |context| context.verify_cert())
            .map_err(openssl_error)
    };
    match verify() {
        Ok(true) => Ok(tokio_rustls::rustls::server::danger::ClientCertVerified::assertion()),
        Ok(false) | Err(_) => Err(tokio_rustls::rustls::Error::InvalidCertificate(
            tokio_rustls::rustls::CertificateError::ApplicationVerificationFailure,
        )),
    }
}

fn openssl_error(error: openssl::error::ErrorStack) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
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

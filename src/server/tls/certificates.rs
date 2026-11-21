use super::*;

pub(in crate::server) fn load_certificate(
    context: &EffectiveConfig,
) -> io::Result<Arc<CertifiedKey>> {
    load_certificate_paths(
        context.ssl_certificate.as_deref(),
        context.ssl_certificate_key.as_deref(),
        &context.ssl_passwords,
    )
}

pub(in crate::server) fn load_certificate_paths(
    certificate_path: Option<&Path>,
    key_path: Option<&Path>,
    passwords: &[Vec<u8>],
) -> io::Result<Arc<CertifiedKey>> {
    let certificate_path = certificate_path.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no ssl_certificate is defined for an SSL listener",
        )
    })?;
    let key_path = key_path.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no ssl_certificate_key is defined for an SSL listener",
        )
    })?;
    let certificates = load_certificates(certificate_path)?;
    let private_key = load_private_key(key_path, passwords)?;
    let signing_key = tokio_rustls::rustls::crypto::ring::sign::any_supported_type(&private_key)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(Arc::new(CertifiedKey::new(certificates, signing_key)))
}

pub(in crate::server) fn load_certificates(
    path: &Path,
) -> io::Result<Vec<CertificateDer<'static>>> {
    let mut reader = StdBufReader::new(File::open(path)?);
    let certificates = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<CertificateDer<'static>>, _>>()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    if certificates.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("no PEM certificates in {}", path.display()),
        ));
    }
    Ok(certificates)
}

pub(in crate::server) fn load_private_key(
    path: &Path,
    passwords: &[Vec<u8>],
) -> io::Result<PrivateKeyDer<'static>> {
    let pem = std::fs::read(path)?;
    let mut reader = std::io::Cursor::new(&pem);
    if let Some(key) = rustls_pemfile::private_key(&mut reader)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?
    {
        return Ok(key);
    }
    for password in passwords {
        if let Ok(key) = openssl::pkey::PKey::private_key_from_pem_passphrase(&pem, password) {
            let der = key.private_key_to_pkcs8().map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("cannot decode private key {}: {err}", path.display()),
                )
            })?;
            return Ok(PrivateKeyDer::Pkcs8(
                tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(der),
            ));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "no usable PEM private key in {}{}",
            path.display(),
            if passwords.is_empty() {
                ""
            } else {
                " with the configured password file"
            }
        ),
    ))
}

pub(in crate::server) fn install_crypto_provider() {
    static INSTALL_PROVIDER: OnceLock<()> = OnceLock::new();
    INSTALL_PROVIDER.get_or_init(|| {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    });
}

#[derive(Debug)]
pub(in crate::server) struct NoCertificateVerification(
    pub(in crate::server) Arc<tokio_rustls::rustls::crypto::CryptoProvider>,
);

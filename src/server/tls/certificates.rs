use super::*;

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

pub(in crate::server) fn load_certificate_entries(
    certificate_paths: &[PathBuf],
    key_paths: &[PathBuf],
    certificate_path: Option<&Path>,
    key_path: Option<&Path>,
    passwords: &[Vec<u8>],
) -> io::Result<Vec<Arc<CertifiedKey>>> {
    if certificate_paths.is_empty() {
        return Ok(vec![load_certificate_paths(
            certificate_path,
            key_path,
            passwords,
        )?]);
    }
    if certificate_paths.len() != key_paths.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the number of ssl_certificate directives does not match ssl_certificate_key",
        ));
    }
    certificate_paths
        .iter()
        .zip(key_paths)
        .map(|(certificate, key)| load_certificate_paths(Some(certificate), Some(key), passwords))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_multiple_certificate_entries_in_order() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let certificate = fixture.join("test.crt");
        let key = fixture.join("test.key");
        let entries = load_certificate_entries(
            &[certificate.clone(), certificate],
            &[key.clone(), key],
            None,
            None,
            &[],
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn rejects_mismatched_certificate_and_key_counts() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let error = load_certificate_entries(
            &[fixture.join("test.crt"), fixture.join("test.crt")],
            &[fixture.join("test.key")],
            None,
            None,
            &[],
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(
            error
                .to_string()
                .contains("does not match ssl_certificate_key"),
            "{error}"
        );
    }
}

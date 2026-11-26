use super::*;
use openssl::asn1::Asn1Time;
use openssl::bn::{BigNum, MsbOption};
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::PKey;
use openssl::x509::extension::{
    BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName,
};
use openssl::x509::{X509, X509NameBuilder};
use std::net::IpAddr;
use std::time::{Duration, SystemTime};
use x509_parser::prelude::FromDer;

mod fs;
pub(in crate::server) use fs::CertificateLock;
use fs::{atomic_write, restrict_directory};

const CERT_LIFETIME_DAYS: u32 = 90;
const CA_LIFETIME_DAYS: u32 = 3650;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::server) enum CertificateSource {
    LocalCa,
    Acme,
}

impl CertificateSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::LocalCa => "local-placeholder",
            Self::Acme => "acme",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "local-placeholder" | "local" => Some(Self::LocalCa),
            "acme" => Some(Self::Acme),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::server) struct CertificatePaths {
    pub(in crate::server) cert: PathBuf,
    pub(in crate::server) key: PathBuf,
}

impl CertificatePaths {
    fn source_path(&self) -> PathBuf {
        self.cert.with_extension("source")
    }

    pub(in crate::server) fn source(&self) -> Option<CertificateSource> {
        std::fs::read_to_string(self.source_path())
            .ok()
            .and_then(|value| CertificateSource::parse(&value))
    }

    pub(in crate::server) fn is_fresh(&self, names: &[String], renew_before: Duration) -> bool {
        certificate_is_fresh(&self.cert, names, renew_before)
    }

    pub(in crate::server) fn is_usable(&self, names: &[String], renew_before: Duration) -> bool {
        certificate_matches_private_key(&self.cert, &self.key) && self.is_fresh(names, renew_before)
    }
}

pub(in crate::server) fn certificate_paths(cert_dir: &Path, names: &[String]) -> CertificatePaths {
    let mut hasher = Sha1::new();
    for name in names {
        hasher.update(name.as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    let stem = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let directory = cert_dir.join("certs");
    CertificatePaths {
        cert: directory.join(format!("{stem}.crt")),
        key: directory.join(format!("{stem}.key")),
    }
}

pub(in crate::server) fn ensure_local_certificate(
    cert_dir: &Path,
    names: &[String],
    renew_before: Duration,
) -> io::Result<CertificatePaths> {
    std::fs::create_dir_all(cert_dir.join("certs"))?;
    restrict_directory(cert_dir)?;
    let paths = certificate_paths(cert_dir, names);
    let local_ca_missing =
        paths.source() != Some(CertificateSource::Acme) && load_local_ca(cert_dir).is_none();
    if paths.is_usable(names, renew_before) && !local_ca_missing {
        return Ok(paths);
    }
    let _lock =
        match CertificateLock::acquire(&cert_dir.join(".auto-https.lock"), Duration::from_secs(10))
        {
            Ok(lock) => lock,
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    && !local_ca_missing
                    && paths.is_usable(names, Duration::ZERO) =>
            {
                // Another process is refreshing the certificate. Keep serving the
                // current still-valid certificate instead of failing startup.
                return Ok(paths);
            }
            Err(error) => return Err(error),
        };
    let local_ca_missing =
        paths.source() != Some(CertificateSource::Acme) && load_local_ca(cert_dir).is_none();
    if paths.is_usable(names, renew_before) && !local_ca_missing {
        return Ok(paths);
    }
    // Keep a still-current ACME certificate in service while the renewal
    // manager obtains a replacement. Replacing it with a local placeholder
    // would introduce a trust failure during every renewal window.
    if paths.source() == Some(CertificateSource::Acme) && paths.is_usable(names, Duration::ZERO) {
        return Ok(paths);
    }
    generate_local_certificate(cert_dir, &paths, names)?;
    Ok(paths)
}

fn generate_local_certificate(
    cert_dir: &Path,
    paths: &CertificatePaths,
    names: &[String],
) -> io::Result<()> {
    std::fs::create_dir_all(cert_dir.join("certs"))?;
    restrict_directory(cert_dir)?;
    let (ca_cert, ca_key) = load_or_create_local_ca(cert_dir)?;
    let leaf_key = generate_ec_key()?;
    let mut builder = X509::builder().map_err(openssl_error)?;
    builder.set_version(2).map_err(openssl_error)?;
    let serial = random_serial()?;
    builder.set_serial_number(&serial).map_err(openssl_error)?;
    let subject = certificate_subject(names)?;
    builder.set_subject_name(&subject).map_err(openssl_error)?;
    builder
        .set_issuer_name(ca_cert.subject_name())
        .map_err(openssl_error)?;
    builder.set_pubkey(&leaf_key).map_err(openssl_error)?;
    let not_before = Asn1Time::days_from_now(0).map_err(openssl_error)?;
    let not_after = Asn1Time::days_from_now(CERT_LIFETIME_DAYS).map_err(openssl_error)?;
    builder.set_not_before(&not_before).map_err(openssl_error)?;
    builder.set_not_after(&not_after).map_err(openssl_error)?;

    let subject_alt_name = {
        let context = builder.x509v3_context(Some(&ca_cert), None);
        subject_alt_name(names)?
            .build(&context)
            .map_err(openssl_error)?
    };
    builder
        .append_extension(
            BasicConstraints::new()
                .critical()
                .build()
                .map_err(openssl_error)?,
        )
        .map_err(openssl_error)?;
    builder
        .append_extension(
            KeyUsage::new()
                .critical()
                .digital_signature()
                .key_encipherment()
                .build()
                .map_err(openssl_error)?,
        )
        .map_err(openssl_error)?;
    builder
        .append_extension(
            ExtendedKeyUsage::new()
                .server_auth()
                .build()
                .map_err(openssl_error)?,
        )
        .map_err(openssl_error)?;
    builder
        .append_extension(subject_alt_name)
        .map_err(openssl_error)?;
    builder
        .sign(&ca_key, MessageDigest::sha256())
        .map_err(openssl_error)?;
    let certificate = builder.build();

    atomic_write(
        &paths.cert,
        &certificate.to_pem().map_err(openssl_error)?,
        0o644,
    )?;
    atomic_write(
        &paths.key,
        &leaf_key.private_key_to_pem_pkcs8().map_err(openssl_error)?,
        0o600,
    )?;
    atomic_write(
        &paths.source_path(),
        CertificateSource::LocalCa.as_str().as_bytes(),
        0o644,
    )?;
    Ok(())
}

fn load_or_create_local_ca(cert_dir: &Path) -> io::Result<(X509, PKey<openssl::pkey::Private>)> {
    let cert_path = cert_dir.join("local-ca.crt");
    let key_path = cert_dir.join("local-ca.key");
    if let Some(pair) = load_local_ca(cert_dir) {
        return Ok(pair);
    }

    let key = generate_ec_key()?;
    let mut builder = X509::builder().map_err(openssl_error)?;
    builder.set_version(2).map_err(openssl_error)?;
    let serial = random_serial()?;
    builder.set_serial_number(&serial).map_err(openssl_error)?;
    let mut subject = X509NameBuilder::new().map_err(openssl_error)?;
    subject
        .append_entry_by_nid(Nid::COMMONNAME, "Starling Local CA")
        .map_err(openssl_error)?;
    subject
        .append_entry_by_nid(Nid::ORGANIZATIONNAME, "Starling Development")
        .map_err(openssl_error)?;
    let subject = subject.build();
    builder.set_subject_name(&subject).map_err(openssl_error)?;
    builder.set_issuer_name(&subject).map_err(openssl_error)?;
    builder.set_pubkey(&key).map_err(openssl_error)?;
    let not_before = Asn1Time::days_from_now(0).map_err(openssl_error)?;
    let not_after = Asn1Time::days_from_now(CA_LIFETIME_DAYS).map_err(openssl_error)?;
    builder.set_not_before(&not_before).map_err(openssl_error)?;
    builder.set_not_after(&not_after).map_err(openssl_error)?;
    builder
        .append_extension(
            BasicConstraints::new()
                .critical()
                .ca()
                .build()
                .map_err(openssl_error)?,
        )
        .map_err(openssl_error)?;
    builder
        .append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .digital_signature()
                .build()
                .map_err(openssl_error)?,
        )
        .map_err(openssl_error)?;
    builder
        .sign(&key, MessageDigest::sha256())
        .map_err(openssl_error)?;
    let certificate = builder.build();
    atomic_write(
        &cert_path,
        &certificate.to_pem().map_err(openssl_error)?,
        0o644,
    )?;
    atomic_write(
        &key_path,
        &key.private_key_to_pem_pkcs8().map_err(openssl_error)?,
        0o600,
    )?;
    Ok((certificate, key))
}

fn load_local_ca(cert_dir: &Path) -> Option<(X509, PKey<openssl::pkey::Private>)> {
    let cert_pem = std::fs::read(cert_dir.join("local-ca.crt")).ok()?;
    let key_pem = std::fs::read(cert_dir.join("local-ca.key")).ok()?;
    let cert = X509::from_pem(&cert_pem).ok()?;
    let key = PKey::private_key_from_pem(&key_pem).ok()?;
    if cert
        .public_key()
        .ok()
        .is_some_and(|public| public.public_eq(&key))
    {
        Some((cert, key))
    } else {
        None
    }
}

fn certificate_subject(names: &[String]) -> io::Result<openssl::x509::X509Name> {
    let mut subject = X509NameBuilder::new().map_err(openssl_error)?;
    let common_name = names
        .first()
        .map(String::as_str)
        .unwrap_or("localhost")
        .trim_start_matches("*.")
        .chars()
        .take(64)
        .collect::<String>();
    subject
        .append_entry_by_nid(Nid::COMMONNAME, &common_name)
        .map_err(openssl_error)?;
    Ok(subject.build())
}

fn subject_alt_name(names: &[String]) -> io::Result<SubjectAlternativeName> {
    let mut san = SubjectAlternativeName::new();
    for name in names {
        if name.parse::<IpAddr>().is_ok() {
            san.ip(name);
        } else {
            san.dns(name);
        }
    }
    Ok(san)
}

fn generate_ec_key() -> io::Result<PKey<openssl::pkey::Private>> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).map_err(openssl_error)?;
    let key = EcKey::generate(&group).map_err(openssl_error)?;
    PKey::from_ec_key(key).map_err(openssl_error)
}

fn random_serial() -> io::Result<openssl::asn1::Asn1Integer> {
    let mut serial = BigNum::new().map_err(openssl_error)?;
    serial
        .rand(128, MsbOption::MAYBE_ZERO, false)
        .map_err(openssl_error)?;
    serial.to_asn1_integer().map_err(openssl_error)
}

pub(in crate::server) fn certificate_is_fresh(
    cert_path: &Path,
    names: &[String],
    renew_before: Duration,
) -> bool {
    let Ok(pem) = std::fs::read(cert_path) else {
        return false;
    };
    let Ok(cert) = X509::from_pem(&pem) else {
        return false;
    };
    let Ok(der) = cert.to_der() else {
        return false;
    };
    let Ok((_, parsed)) = x509_parser::certificate::X509Certificate::from_der(&der) else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let not_before = parsed.validity().not_before.timestamp();
    let not_after = parsed.validity().not_after.timestamp();
    if not_before > now + 300 || not_after <= now + renew_before.as_secs() as i64 {
        return false;
    }
    let Some(san) = parsed.subject_alternative_name().ok().flatten() else {
        return false;
    };
    let mut present_dns = std::collections::HashSet::new();
    let mut present_ips = std::collections::HashSet::new();
    for name in &san.value.general_names {
        match name {
            x509_parser::extensions::GeneralName::DNSName(name) => {
                present_dns.insert(name.to_ascii_lowercase());
            }
            x509_parser::extensions::GeneralName::IPAddress(bytes) => {
                if let Ok(octets) = <[u8; 4]>::try_from(*bytes) {
                    present_ips.insert(IpAddr::from(octets));
                } else if let Ok(octets) = <[u8; 16]>::try_from(*bytes) {
                    present_ips.insert(IpAddr::from(octets));
                }
            }
            _ => {}
        }
    }
    names.iter().all(|name| {
        name.parse::<IpAddr>().map_or_else(
            |_| present_dns.contains(&name.to_ascii_lowercase()),
            |address| present_ips.contains(&address),
        )
    })
}

fn certificate_matches_private_key(cert_path: &Path, key_path: &Path) -> bool {
    let Ok(cert_pem) = std::fs::read(cert_path) else {
        return false;
    };
    let Ok(key_pem) = std::fs::read(key_path) else {
        return false;
    };
    let Ok(cert) = X509::from_pem(&cert_pem) else {
        return false;
    };
    let Ok(key) = PKey::private_key_from_pem(&key_pem) else {
        return false;
    };
    cert.public_key()
        .ok()
        .is_some_and(|public| public.public_eq(&key))
}

pub(in crate::server) fn write_acme_certificate(
    paths: &CertificatePaths,
    certificate_pem: &[u8],
    private_key_pem: &[u8],
) -> io::Result<()> {
    if let Some(parent) = paths.cert.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(&paths.cert, certificate_pem, 0o644)?;
    atomic_write(&paths.key, private_key_pem, 0o600)?;
    atomic_write(
        &paths.source_path(),
        CertificateSource::Acme.as_str().as_bytes(),
        0o644,
    )?;
    Ok(())
}

fn openssl_error(error: openssl::error::ErrorStack) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests;

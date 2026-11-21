use super::*;

pub(in crate::server) fn client_certificate_info(
    certificate: &CertificateDer<'_>,
) -> Option<ClientCertificateInfo> {
    use sha1::{Digest, Sha1};
    use x509_parser::prelude::FromDer;

    let der = certificate.as_ref();
    let (_, parsed) = x509_parser::certificate::X509Certificate::from_der(der).ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).ok()?);
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    let escaped_certificate = escape_uri_component(pem.as_bytes());
    let certificate = pem.trim_end_matches('\n').replace('\n', "\n\t");
    let fingerprint = Sha1::digest(der)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let subject_dn = parsed.subject().to_string();
    let issuer_dn = parsed.issuer().to_string();
    let subject_dn_legacy = legacy_dn(&subject_dn);
    let issuer_dn_legacy = legacy_dn(&issuer_dn);
    let valid_start = format_asn1_time(&parsed.validity().not_before);
    let valid_end = format_asn1_time(&parsed.validity().not_after);
    let end = parsed.validity().not_after.to_datetime();
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    let remaining = end.unix_timestamp().saturating_sub(now);
    let valid_remain = if remaining < 86_400 {
        "0".to_string()
    } else {
        (remaining / 86_400).to_string()
    };
    Some(ClientCertificateInfo {
        certificate,
        escaped_certificate,
        fingerprint,
        serial: parsed.raw_serial_as_string().to_ascii_uppercase(),
        subject_dn,
        issuer_dn,
        subject_dn_legacy,
        issuer_dn_legacy,
        valid_start,
        valid_end,
        valid_remain,
    })
}

pub(in crate::server) fn format_asn1_time(time: &x509_parser::time::ASN1Time) -> String {
    let time = time.to_datetime();
    let month = match time.month() as u8 {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    };
    format!(
        "{month} {:02} {:02}:{:02}:{:02} {} GMT",
        time.day(),
        time.hour(),
        time.minute(),
        time.second(),
        time.year()
    )
}

pub(in crate::server) fn legacy_dn(dn: &str) -> String {
    let mut parts = dn.split(',').collect::<Vec<_>>();
    parts.reverse();
    parts
        .into_iter()
        .map(|part| format!("/{}", part.trim()))
        .collect()
}

pub(in crate::server) fn escape_uri_component(value: &[u8]) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            escaped.push(char::from(*byte));
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

pub(in crate::server) fn tls_cipher_name(suite: tokio_rustls::rustls::CipherSuite) -> String {
    use tokio_rustls::rustls::CipherSuite;

    let name = match suite {
        CipherSuite::TLS13_AES_128_GCM_SHA256 => "TLS_AES_128_GCM_SHA256",
        CipherSuite::TLS13_AES_256_GCM_SHA384 => "TLS_AES_256_GCM_SHA384",
        CipherSuite::TLS13_CHACHA20_POLY1305_SHA256 => "TLS_CHACHA20_POLY1305_SHA256",
        CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256 => "ECDHE-ECDSA-AES128-GCM-SHA256",
        CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384 => "ECDHE-ECDSA-AES256-GCM-SHA384",
        CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256 => {
            "ECDHE-ECDSA-CHACHA20-POLY1305"
        }
        CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 => "ECDHE-RSA-AES128-GCM-SHA256",
        CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384 => "ECDHE-RSA-AES256-GCM-SHA384",
        CipherSuite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256 => "ECDHE-RSA-CHACHA20-POLY1305",
        _ => return format!("{suite:?}"),
    };
    name.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TlsProtocols {
    pub tls12: bool,
    pub tls13: bool,
}

impl Default for TlsProtocols {
    fn default() -> Self {
        Self {
            tls12: true,
            tls13: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClientCertificateVerify {
    #[default]
    Off,
    On,
    Optional,
    OptionalNoCa,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OcspMode {
    #[default]
    Off,
    On,
    Leaf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TlsCipherSuite {
    EcdheEcdsaAes256GcmSha384,
    EcdheEcdsaAes128GcmSha256,
    EcdheEcdsaChaCha20Poly1305Sha256,
    EcdheRsaAes256GcmSha384,
    EcdheRsaAes128GcmSha256,
    EcdheRsaChaCha20Poly1305Sha256,
}

impl TlsCipherSuite {
    pub const TLS12_DEFAULT: [Self; 6] = [
        Self::EcdheEcdsaAes256GcmSha384,
        Self::EcdheEcdsaAes128GcmSha256,
        Self::EcdheEcdsaChaCha20Poly1305Sha256,
        Self::EcdheRsaAes256GcmSha384,
        Self::EcdheRsaAes128GcmSha256,
        Self::EcdheRsaChaCha20Poly1305Sha256,
    ];

    pub(in crate::config::compile) fn openssl_name(self) -> &'static str {
        match self {
            Self::EcdheEcdsaAes256GcmSha384 => "ECDHE-ECDSA-AES256-GCM-SHA384",
            Self::EcdheEcdsaAes128GcmSha256 => "ECDHE-ECDSA-AES128-GCM-SHA256",
            Self::EcdheEcdsaChaCha20Poly1305Sha256 => "ECDHE-ECDSA-CHACHA20-POLY1305",
            Self::EcdheRsaAes256GcmSha384 => "ECDHE-RSA-AES256-GCM-SHA384",
            Self::EcdheRsaAes128GcmSha256 => "ECDHE-RSA-AES128-GCM-SHA256",
            Self::EcdheRsaChaCha20Poly1305Sha256 => "ECDHE-RSA-CHACHA20-POLY1305",
        }
    }

    pub(in crate::config::compile) fn iana_name(self) -> &'static str {
        match self {
            Self::EcdheEcdsaAes256GcmSha384 => "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
            Self::EcdheEcdsaAes128GcmSha256 => "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
            Self::EcdheEcdsaChaCha20Poly1305Sha256 => {
                "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256"
            }
            Self::EcdheRsaAes256GcmSha384 => "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
            Self::EcdheRsaAes128GcmSha256 => "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
            Self::EcdheRsaChaCha20Poly1305Sha256 => "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256",
        }
    }

    pub(in crate::config::compile) fn bits(self) -> u16 {
        match self {
            Self::EcdheEcdsaAes128GcmSha256 | Self::EcdheRsaAes128GcmSha256 => 128,
            _ => 256,
        }
    }

    pub(in crate::config::compile) fn uses_ecdsa(self) -> bool {
        matches!(
            self,
            Self::EcdheEcdsaAes256GcmSha384
                | Self::EcdheEcdsaAes128GcmSha256
                | Self::EcdheEcdsaChaCha20Poly1305Sha256
        )
    }

    pub(in crate::config::compile) fn uses_chacha20(self) -> bool {
        matches!(
            self,
            Self::EcdheEcdsaChaCha20Poly1305Sha256 | Self::EcdheRsaChaCha20Poly1305Sha256
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TlsCipherConfig {
    pub suites: Vec<TlsCipherSuite>,
}

impl Default for TlsCipherConfig {
    fn default() -> Self {
        Self {
            suites: TlsCipherSuite::TLS12_DEFAULT.to_vec(),
        }
    }
}

impl TlsCipherConfig {
    pub(in crate::config::compile) fn parse(value: &str) -> Result<Self, String> {
        let mut selected = Vec::new();
        let mut permanently_removed = std::collections::HashSet::new();
        for clause in value.split(':') {
            if clause.is_empty() {
                return Err("empty cipher-list component".into());
            }
            if let Some(command) = clause.strip_prefix('@') {
                match command {
                    "STRENGTH" => {
                        selected
                            .sort_by_key(|suite: &TlsCipherSuite| std::cmp::Reverse(suite.bits()));
                    }
                    value if value.starts_with("SECLEVEL=") => {
                        let level = value.trim_start_matches("SECLEVEL=");
                        if level.parse::<u8>().is_err() {
                            return Err(format!("invalid cipher-list command \"@{value}\""));
                        }
                    }
                    _ => return Err(format!("unknown cipher-list command \"@{command}\"")),
                }
                continue;
            }

            let (operation, expression) = match clause.as_bytes()[0] {
                b'!' => (CipherListOperation::RemovePermanently, &clause[1..]),
                b'-' => (CipherListOperation::Remove, &clause[1..]),
                b'+' => (CipherListOperation::MoveToEnd, &clause[1..]),
                _ => (CipherListOperation::Add, clause),
            };
            if expression.is_empty() {
                return Err("empty cipher-list component".into());
            }

            let mut candidates: Option<Vec<TlsCipherSuite>> = None;
            for token in expression.split('+') {
                if token.is_empty() {
                    return Err("empty cipher-list component".into());
                }
                let token_candidates = cipher_token_candidates(token)?;
                candidates = Some(match candidates {
                    None => token_candidates,
                    Some(previous) => previous
                        .into_iter()
                        .filter(|suite| token_candidates.contains(suite))
                        .collect(),
                });
            }
            let candidates = candidates
                .unwrap_or_default()
                .into_iter()
                .filter(|suite| !permanently_removed.contains(suite))
                .collect::<Vec<_>>();

            match operation {
                CipherListOperation::RemovePermanently => {
                    permanently_removed.extend(candidates.iter().copied());
                    selected.retain(|suite| !permanently_removed.contains(suite));
                }
                CipherListOperation::Remove => {
                    selected.retain(|suite| !candidates.contains(suite));
                }
                CipherListOperation::MoveToEnd => {
                    let mut moved = Vec::new();
                    selected.retain(|suite| {
                        if candidates.contains(suite) {
                            moved.push(*suite);
                            false
                        } else {
                            true
                        }
                    });
                    selected.extend(moved);
                }
                CipherListOperation::Add => {
                    for suite in candidates {
                        if !selected.contains(&suite) {
                            selected.push(suite);
                        }
                    }
                }
            }
        }

        if selected.is_empty() {
            return Err("no supported TLSv1.2 cipher suites".into());
        }
        Ok(Self { suites: selected })
    }
}

#[derive(Clone, Copy)]
pub(in crate::config::compile) enum CipherListOperation {
    RemovePermanently,
    Remove,
    MoveToEnd,
    Add,
}

pub(in crate::config::compile) fn cipher_token_candidates(
    token: &str,
) -> Result<Vec<TlsCipherSuite>, String> {
    let token = token.to_ascii_uppercase();
    for suite in TlsCipherSuite::TLS12_DEFAULT {
        if token == suite.openssl_name() || token == suite.iana_name() {
            return Ok(vec![suite]);
        }
    }
    if matches!(
        token.as_str(),
        "TLS_AES_128_GCM_SHA256" | "TLS_AES_256_GCM_SHA384" | "TLS_CHACHA20_POLY1305_SHA256"
    ) {
        // OpenSSL cipher lists do not control TLS 1.3 suites; rustls keeps them enabled.
        return Ok(Vec::new());
    }

    let known = matches!(
        token.as_str(),
        "ALL"
            | "DEFAULT"
            | "COMPLEMENTOFDEFAULT"
            | "COMPLEMENTOFALL"
            | "HIGH"
            | "MEDIUM"
            | "LOW"
            | "SECURE"
            | "AEAD"
            | "GCM"
            | "AES"
            | "AESGCM"
            | "AES128"
            | "AES128GCM"
            | "AES256"
            | "AES256GCM"
            | "CHACHA20"
            | "POLY1305"
            | "ECDHE"
            | "EECDH"
            | "KECDHE"
            | "KEECDH"
            | "ECDSA"
            | "ECCDRAFT"
            | "AECDSA"
            | "RSA"
            | "ARSA"
            | "SHA256"
            | "SHA384"
            | "ANULL"
            | "ENULL"
            | "NULL"
            | "MD5"
            | "SHA"
            | "SHA1"
            | "DES"
            | "3DES"
            | "RC4"
            | "EXPORT"
            | "IDEA"
            | "SEED"
            | "CAMELLIA"
            | "ARIA"
            | "CBC"
            | "PSK"
            | "SRP"
            | "DSS"
            | "DHE"
            | "EDH"
            | "KRB5"
            | "KRSA"
            | "KDHE"
            | "KPSK"
            | "KSRP"
            | "TLS13"
            | "TLSV1.3"
    );
    if !known {
        if is_known_unsupported_cipher_name(&token) {
            return Ok(Vec::new());
        }
        return Err(format!("unknown cipher \"{token}\""));
    }

    Ok(TlsCipherSuite::TLS12_DEFAULT
        .into_iter()
        .filter(|suite| cipher_alias_matches(&token, *suite))
        .collect())
}

pub(in crate::config::compile) fn unsupported_cipher_inclusions(
    value: &str,
) -> Result<Vec<String>, String> {
    let mut unsupported = Vec::new();
    for clause in value.split(':') {
        if clause.is_empty() {
            return Err("empty cipher-list component".into());
        }
        if clause.starts_with('@') {
            continue;
        }
        let (operation, expression) = match clause.as_bytes()[0] {
            b'!' => (CipherListOperation::RemovePermanently, &clause[1..]),
            b'-' => (CipherListOperation::Remove, &clause[1..]),
            b'+' => (CipherListOperation::MoveToEnd, &clause[1..]),
            _ => (CipherListOperation::Add, clause),
        };
        if !matches!(operation, CipherListOperation::Add) {
            continue;
        }
        for token in expression.split('+') {
            if token.is_empty() {
                return Err("empty cipher-list component".into());
            }
            if cipher_token_candidates(token)?.is_empty() && !is_tls13_cipher_name(token) {
                unsupported.push(token.to_ascii_uppercase());
            }
        }
    }
    Ok(unsupported)
}

fn is_tls13_cipher_name(token: &str) -> bool {
    matches!(
        token.to_ascii_uppercase().as_str(),
        "TLS_AES_128_GCM_SHA256" | "TLS_AES_256_GCM_SHA384" | "TLS_CHACHA20_POLY1305_SHA256"
    )
}

fn is_known_unsupported_cipher_name(token: &str) -> bool {
    matches!(
        token,
        "DHE-RSA-AES256-GCM-SHA384"
            | "DHE-RSA-AES128-GCM-SHA256"
            | "DHE-RSA-AES256-SHA256"
            | "DHE-RSA-AES128-SHA256"
            | "DHE-RSA-AES256-SHA"
            | "DHE-RSA-AES128-SHA"
            | "ECDHE-RSA-AES256-SHA384"
            | "ECDHE-RSA-AES128-SHA256"
            | "ECDHE-RSA-AES256-SHA"
            | "ECDHE-RSA-AES128-SHA"
            | "ECDHE-ECDSA-AES256-SHA384"
            | "ECDHE-ECDSA-AES128-SHA256"
            | "ECDHE-ECDSA-AES256-SHA"
            | "ECDHE-ECDSA-AES128-SHA"
            | "AES256-GCM-SHA384"
            | "AES128-GCM-SHA256"
            | "AES256-SHA256"
            | "AES128-SHA256"
            | "AES256-SHA"
            | "AES128-SHA"
            | "DES-CBC3-SHA"
    )
}

pub(in crate::config::compile) fn cipher_alias_matches(alias: &str, suite: TlsCipherSuite) -> bool {
    match alias {
        "ALL" | "DEFAULT" | "HIGH" | "SECURE" | "AEAD" | "AES" | "ECDHE" | "EECDH" | "KECDHE"
        | "KEECDH" => true,
        "GCM" | "AESGCM" => !suite.uses_chacha20(),
        "AES128" | "AES128GCM" => suite.bits() == 128 && !suite.uses_chacha20(),
        "AES256" | "AES256GCM" => suite.bits() == 256 && !suite.uses_chacha20(),
        "CHACHA20" | "POLY1305" => suite.uses_chacha20(),
        "ECDSA" | "ECCDRAFT" | "AECDSA" => suite.uses_ecdsa(),
        "RSA" | "ARSA" => !suite.uses_ecdsa(),
        // Every rustls TLS 1.2 suite is AEAD, so legacy digest-only aliases do not match.
        "SHA256" | "SHA384" | "MEDIUM" => false,
        _ => false,
    }
}

#[derive(Debug, Clone)]
pub struct SubFilter {
    pub pattern: String,
    pub replacement: String,
}

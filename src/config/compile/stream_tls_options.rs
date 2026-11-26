use super::stream_server_options::parse_stream_alpn;
use super::*;

use super::compat::{
    reject_proxy_ssl_conf_command, reject_ssl_conf_command, try_apply_compat_directive,
    validate_certificate_cache,
};

#[derive(Clone)]
pub(super) struct StreamTlsOptions {
    pub(super) proxy_ssl: bool,
    pub(super) proxy_ssl_protocols: TlsProtocols,
    pub(super) proxy_ssl_ciphers: TlsCipherConfig,
    pub(super) proxy_ssl_server_name: bool,
    pub(super) proxy_ssl_name: Option<String>,
    pub(super) proxy_ssl_session_reuse: bool,
    pub(super) proxy_ssl_verify: bool,
    pub(super) proxy_ssl_trusted_certificate: Option<PathBuf>,
    pub(super) proxy_ssl_verify_depth: u32,
    pub(super) proxy_ssl_crl: Option<PathBuf>,
    pub(super) proxy_ssl_certificate: Option<PathBuf>,
    pub(super) proxy_ssl_certificate_key: Option<PathBuf>,
    pub(super) proxy_ssl_passwords: Vec<Vec<u8>>,
    pub(super) proxy_ssl_alpn: Vec<String>,
    pub(super) ssl_certificate: Option<PathBuf>,
    pub(super) ssl_certificate_key: Option<PathBuf>,
    pub(super) ssl_certificates: Vec<PathBuf>,
    pub(super) ssl_certificate_keys: Vec<PathBuf>,
    pub(super) ssl_protocols: TlsProtocols,
    pub(super) ssl_prefer_server_ciphers: bool,
    pub(super) ssl_ciphers: TlsCipherConfig,
    pub(super) ssl_verify_client: ClientCertificateVerify,
    pub(super) ssl_verify_depth: u32,
    pub(super) ssl_client_certificate: Option<PathBuf>,
    pub(super) ssl_trusted_certificate: Option<PathBuf>,
    pub(super) ssl_crl: Option<PathBuf>,
    pub(super) ssl_stapling: bool,
    pub(super) ssl_stapling_verify: bool,
    pub(super) ssl_stapling_file: Option<PathBuf>,
    pub(super) ssl_stapling_responder: Option<String>,
    pub(super) ssl_ocsp: OcspMode,
    pub(super) ssl_ocsp_responder: Option<String>,
    pub(super) ssl_ocsp_cache: Option<String>,
    pub(super) resolver: ResolverConfig,
    pub(super) resolver_timeout: Duration,
    pub(super) ssl_passwords: Vec<Vec<u8>>,
    pub(super) ssl_alpn: Vec<String>,
    pub(super) ssl_handshake_timeout: Duration,
    pub(super) ssl_reject_handshake: bool,
}

impl Default for StreamTlsOptions {
    fn default() -> Self {
        Self {
            proxy_ssl: false,
            proxy_ssl_protocols: TlsProtocols::default(),
            proxy_ssl_ciphers: TlsCipherConfig::default(),
            proxy_ssl_server_name: false,
            proxy_ssl_name: None,
            proxy_ssl_session_reuse: true,
            proxy_ssl_verify: false,
            proxy_ssl_trusted_certificate: None,
            proxy_ssl_verify_depth: 1,
            proxy_ssl_crl: None,
            proxy_ssl_certificate: None,
            proxy_ssl_certificate_key: None,
            proxy_ssl_passwords: Vec::new(),
            proxy_ssl_alpn: Vec::new(),
            ssl_certificate: None,
            ssl_certificate_key: None,
            ssl_certificates: Vec::new(),
            ssl_certificate_keys: Vec::new(),
            ssl_protocols: TlsProtocols::default(),
            ssl_prefer_server_ciphers: false,
            ssl_ciphers: TlsCipherConfig::default(),
            ssl_verify_client: ClientCertificateVerify::Off,
            ssl_verify_depth: 1,
            ssl_client_certificate: None,
            ssl_trusted_certificate: None,
            ssl_crl: None,
            ssl_stapling: false,
            ssl_stapling_verify: false,
            ssl_stapling_file: None,
            ssl_stapling_responder: None,
            ssl_ocsp: OcspMode::Off,
            ssl_ocsp_responder: None,
            ssl_ocsp_cache: None,
            resolver: ResolverConfig::default(),
            resolver_timeout: Duration::from_secs(30),
            ssl_passwords: Vec::new(),
            ssl_alpn: Vec::new(),
            ssl_handshake_timeout: Duration::from_secs(60),
            ssl_reject_handshake: false,
        }
    }
}

#[derive(Default)]
pub(super) struct StreamTlsSeen {
    ssl_protocols: bool,
    proxy_ssl_protocols: bool,
    proxy_ssl_alpn: bool,
    ssl_alpn: bool,
    ssl_certificate: bool,
    ssl_certificate_key: bool,
    ssl_stapling: bool,
    ssl_stapling_verify: bool,
    ssl_stapling_file: bool,
    ssl_stapling_responder: bool,
    ssl_ocsp: bool,
    ssl_ocsp_responder: bool,
    ssl_ocsp_cache: bool,
    resolver: bool,
    resolver_timeout: bool,
}

pub(super) fn apply_stream_tls_directive(
    directive: &Directive,
    prefix: &Path,
    options: &mut StreamTlsOptions,
    seen: &mut StreamTlsSeen,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "proxy_ssl" => options.proxy_ssl = parse_flag(directive, "proxy_ssl")?,
        "ssl_protocols" => {
            apply_tls_protocols(
                directive,
                &mut options.ssl_protocols,
                &mut seen.ssl_protocols,
            )?;
        }
        "proxy_ssl_protocols" => {
            apply_tls_protocols(
                directive,
                &mut options.proxy_ssl_protocols,
                &mut seen.proxy_ssl_protocols,
            )?;
        }
        "ssl_prefer_server_ciphers" => {
            options.ssl_prefer_server_ciphers = parse_flag(directive, "ssl_prefer_server_ciphers")?;
        }
        "ssl_ciphers" => options.ssl_ciphers = parse_tls_cipher_config(directive)?,
        "ssl_session_cache" | "ssl_session_timeout" | "ssl_session_tickets" => {
            try_apply_compat_directive(directive)?;
        }
        "proxy_ssl_ciphers" => {
            options.proxy_ssl_ciphers = parse_tls_cipher_config(directive)?;
        }
        "ssl_verify_client" => {
            options.ssl_verify_client = parse_ssl_verify_client(directive)?;
        }
        "ssl_verify_depth" => options.ssl_verify_depth = parse_ssl_verify_depth(directive)?,
        "ssl_certificate" => {
            validate_no_block(directive)?;
            let path = resolve_path(prefix, one_arg(directive)?);
            if !seen.ssl_certificate {
                options.ssl_certificates.clear();
                seen.ssl_certificate = true;
            }
            options.ssl_certificates.push(path.clone());
            options.ssl_certificate = Some(path);
        }
        "ssl_certificate_key" => {
            validate_no_block(directive)?;
            let path = resolve_path(prefix, one_arg(directive)?);
            if !seen.ssl_certificate_key {
                options.ssl_certificate_keys.clear();
                seen.ssl_certificate_key = true;
            }
            options.ssl_certificate_keys.push(path.clone());
            options.ssl_certificate_key = Some(path);
        }
        "ssl_client_certificate" => {
            validate_no_block(directive)?;
            options.ssl_client_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_trusted_certificate" => {
            validate_no_block(directive)?;
            options.ssl_trusted_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_crl" => {
            validate_no_block(directive)?;
            options.ssl_crl = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_stapling" => {
            reject_stream_stapling_duplicate(directive, &mut seen.ssl_stapling)?;
            options.ssl_stapling = parse_flag(directive, "ssl_stapling")?;
        }
        "ssl_stapling_verify" => {
            reject_stream_stapling_duplicate(directive, &mut seen.ssl_stapling_verify)?;
            options.ssl_stapling_verify = parse_flag(directive, "ssl_stapling_verify")?;
        }
        "ssl_stapling_file" => {
            reject_stream_stapling_duplicate(directive, &mut seen.ssl_stapling_file)?;
            options.ssl_stapling_file = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_stapling_responder" => {
            reject_stream_stapling_duplicate(directive, &mut seen.ssl_stapling_responder)?;
            options.ssl_stapling_responder = Some(one_arg(directive)?.to_string());
        }
        "ssl_ocsp" => {
            reject_stream_stapling_duplicate(directive, &mut seen.ssl_ocsp)?;
            validate_no_block(directive)?;
            options.ssl_ocsp = match one_arg(directive)? {
                "off" => OcspMode::Off,
                "on" => OcspMode::On,
                "leaf" => OcspMode::Leaf,
                value => {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!(
                            "invalid value \"{value}\" in \"ssl_ocsp\" directive, \
                             it must be \"off\", \"on\" or \"leaf\""
                        ),
                    ));
                }
            };
        }
        "ssl_ocsp_responder" => {
            reject_stream_stapling_duplicate(directive, &mut seen.ssl_ocsp_responder)?;
            validate_no_block(directive)?;
            let responder = one_arg(directive)?;
            validate_stream_ocsp_responder_url(responder, &directive.location)?;
            options.ssl_ocsp_responder = Some(responder.to_string());
        }
        "ssl_ocsp_cache" => {
            reject_stream_stapling_duplicate(directive, &mut seen.ssl_ocsp_cache)?;
            options.ssl_ocsp_cache = parse_stream_ocsp_cache(directive)?;
        }
        "resolver" => {
            reject_stream_stapling_duplicate(directive, &mut seen.resolver)?;
            options.resolver = parse_resolver(directive)?;
        }
        "resolver_timeout" => {
            reject_stream_stapling_duplicate(directive, &mut seen.resolver_timeout)?;
            options.resolver_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "proxy_ssl_server_name" => {
            options.proxy_ssl_server_name = parse_flag(directive, "proxy_ssl_server_name")?;
        }
        "proxy_ssl_name" => {
            validate_no_block(directive)?;
            options.proxy_ssl_name = Some(one_arg(directive)?.to_string());
        }
        "proxy_ssl_session_reuse" => {
            options.proxy_ssl_session_reuse = parse_flag(directive, "proxy_ssl_session_reuse")?;
        }
        "ssl_certificate_cache" | "proxy_ssl_certificate_cache" => {
            validate_certificate_cache(directive)?;
        }
        "proxy_ssl_conf_command" => {
            reject_proxy_ssl_conf_command(directive)?;
        }
        "ssl_conf_command" => {
            reject_ssl_conf_command(directive, "ssl_conf_command")?;
        }
        "proxy_ssl_verify" => {
            options.proxy_ssl_verify = parse_flag(directive, "proxy_ssl_verify")?;
        }
        "proxy_ssl_trusted_certificate" => {
            validate_no_block(directive)?;
            options.proxy_ssl_trusted_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "proxy_ssl_verify_depth" => {
            validate_no_block(directive)?;
            options.proxy_ssl_verify_depth = parse_usize(
                one_arg(directive)?,
                &directive.location,
                "proxy_ssl_verify_depth",
            )? as u32;
        }
        "proxy_ssl_crl" => {
            validate_no_block(directive)?;
            options.proxy_ssl_crl = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "proxy_ssl_certificate" => {
            validate_no_block(directive)?;
            options.proxy_ssl_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "proxy_ssl_certificate_key" => {
            validate_no_block(directive)?;
            options.proxy_ssl_certificate_key = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "proxy_ssl_password_file" => {
            options.proxy_ssl_passwords = parse_password_file(prefix, directive)?;
        }
        "proxy_ssl_alpn" => {
            if seen.proxy_ssl_alpn {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"proxy_ssl_alpn\" directive is duplicate",
                ));
            }
            options.proxy_ssl_alpn = parse_stream_alpn(directive, "proxy_ssl_alpn")?;
            seen.proxy_ssl_alpn = true;
        }
        "ssl_password_file" => {
            options.ssl_passwords = parse_password_file(prefix, directive)?;
        }
        "ssl_alpn" => {
            if seen.ssl_alpn {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"ssl_alpn\" directive is duplicate",
                ));
            }
            options.ssl_alpn = parse_stream_alpn(directive, "ssl_alpn")?;
            seen.ssl_alpn = true;
        }
        "ssl_handshake_timeout" => {
            validate_no_block(directive)?;
            options.ssl_handshake_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "ssl_reject_handshake" => {
            options.ssl_reject_handshake = parse_flag(directive, "ssl_reject_handshake")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn reject_stream_stapling_duplicate(directive: &Directive, seen: &mut bool) -> ConfigResult<()> {
    if *seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    *seen = true;
    Ok(())
}

fn validate_stream_ocsp_responder_url(value: &str, location: &SourceLocation) -> ConfigResult<()> {
    let lower = value.to_ascii_lowercase();
    if (lower.starts_with("http://") || lower.starts_with("https://"))
        && !value.contains(char::is_whitespace)
        && value.len() > if lower.starts_with("https://") { 8 } else { 7 }
    {
        return Ok(());
    }
    Err(ConfigError::at(
        location.clone(),
        format!("invalid OCSP responder URL \"{value}\""),
    ))
}

fn parse_stream_ocsp_cache(directive: &Directive) -> ConfigResult<Option<String>> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    if value == "off" {
        return Ok(None);
    }
    let mut parts = value.split(':');
    let valid = parts.next() == Some("shared")
        && parts.next().is_some_and(|name| !name.is_empty())
        && parts.next().is_some_and(|size| {
            let digits = size.strip_suffix(['k', 'K', 'm', 'M']).unwrap_or(size);
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
        && parts.next().is_none();
    if !valid {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid value \"{value}\" in \"ssl_ocsp_cache\" directive, \
                 expected \"off\" or \"shared:name:size\""
            ),
        ));
    }
    Ok(Some(value.to_string()))
}

use super::stream_server_options::parse_stream_alpn;
use super::*;

use super::compat::{reject_proxy_ssl_conf_command, validate_certificate_cache};

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
    ssl_stapling: bool,
    ssl_stapling_verify: bool,
    ssl_stapling_file: bool,
    ssl_stapling_responder: bool,
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
        "proxy_ssl_ciphers" => {
            options.proxy_ssl_ciphers = parse_tls_cipher_config(directive)?;
        }
        "ssl_verify_client" => {
            options.ssl_verify_client = parse_ssl_verify_client(directive)?;
        }
        "ssl_verify_depth" => options.ssl_verify_depth = parse_ssl_verify_depth(directive)?,
        "ssl_certificate" => {
            validate_no_block(directive)?;
            options.ssl_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_certificate_key" => {
            validate_no_block(directive)?;
            options.ssl_certificate_key = Some(resolve_path(prefix, one_arg(directive)?));
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

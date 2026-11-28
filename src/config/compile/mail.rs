use super::*;

use super::compat::{reject_ssl_conf_command, try_apply_compat_directive};

#[derive(Clone)]
struct MailAuthHttpBase {
    address: MailAuthHttpAddress,
    host_header: String,
    uri: String,
}

#[derive(Default)]
struct MailTlsSeen {
    certificate: bool,
    certificate_key: bool,
}

struct MailDefaults {
    error_logs: Vec<LogTarget>,
    real_ip_from: Vec<RealIpSource>,
    timeout: Duration,
    max_errors: usize,
    server_name: String,
    resolver: ResolverConfig,
    resolver_timeout: Duration,
    auth_http: Option<MailAuthHttpBase>,
    auth_http_timeout: Duration,
    auth_http_headers: Vec<(String, String)>,
    auth_http_pass_client_cert: bool,
    proxy: MailProxyConfig,
    tls: MailTlsConfig,
    tls_seen: MailTlsSeen,
    pop3_auth: Option<Vec<MailAuthMethod>>,
    pop3_capabilities: Option<Vec<String>>,
    imap_auth: Option<Vec<MailAuthMethod>>,
    imap_capabilities: Option<Vec<String>>,
    imap_client_buffer_size: Option<usize>,
    smtp_auth: Option<Vec<MailAuthMethod>>,
    smtp_capabilities: Option<Vec<String>>,
    smtp_client_buffer_size: Option<usize>,
    smtp_greeting_delay: Option<Duration>,
}

impl Default for MailDefaults {
    fn default() -> Self {
        Self {
            error_logs: Vec::new(),
            real_ip_from: Vec::new(),
            timeout: Duration::from_secs(60),
            max_errors: 5,
            server_name: default_mail_server_name(),
            resolver: ResolverConfig::default(),
            resolver_timeout: Duration::from_secs(30),
            auth_http: None,
            auth_http_timeout: Duration::from_secs(60),
            auth_http_headers: Vec::new(),
            auth_http_pass_client_cert: false,
            proxy: MailProxyConfig::default(),
            tls: MailTlsConfig::default(),
            tls_seen: MailTlsSeen::default(),
            pop3_auth: None,
            pop3_capabilities: None,
            imap_auth: None,
            imap_capabilities: None,
            imap_client_buffer_size: None,
            smtp_auth: None,
            smtp_capabilities: None,
            smtp_client_buffer_size: None,
            smtp_greeting_delay: None,
        }
    }
}

pub(super) fn compile_mail(directive: &Directive, prefix: &Path) -> ConfigResult<MailConfig> {
    let mut defaults = MailDefaults::default();
    let mut server_directives = Vec::new();

    for child in &directive.children {
        if apply_mail_tls_directive(child, prefix, &mut defaults.tls, &mut defaults.tls_seen)? {
            continue;
        }
        match child.name.as_str() {
            "server" => {
                require_block(child)?;
                server_directives.push(child);
            }
            "error_log" => defaults.error_logs.push(parse_log_target(prefix, child)?),
            "set_real_ip_from" => {
                validate_no_block(child)?;
                defaults
                    .real_ip_from
                    .push(parse_real_ip_source(one_arg(child)?, &child.location)?);
            }
            "timeout" => {
                validate_no_block(child)?;
                defaults.timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "max_errors" => {
                validate_no_block(child)?;
                defaults.max_errors = parse_usize(one_arg(child)?, &child.location, "max_errors")?;
            }
            "server_name" => {
                validate_no_block(child)?;
                defaults.server_name = one_arg(child)?.to_string();
            }
            "resolver" => defaults.resolver = parse_resolver(child)?,
            "resolver_timeout" => {
                validate_no_block(child)?;
                defaults.resolver_timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "auth_http" => defaults.auth_http = Some(parse_auth_http(child)?),
            "auth_http_timeout" => {
                validate_no_block(child)?;
                defaults.auth_http_timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "auth_http_header" => {
                defaults
                    .auth_http_headers
                    .push(parse_auth_http_header(child)?);
            }
            "auth_http_pass_client_cert" => {
                defaults.auth_http_pass_client_cert =
                    parse_flag(child, "auth_http_pass_client_cert")?;
            }
            "proxy" => defaults.proxy.enabled = parse_flag(child, "proxy")?,
            "proxy_timeout" => {
                validate_no_block(child)?;
                defaults.proxy.timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "proxy_buffer" => defaults.proxy.buffer_size = parse_proxy_buffer(child)?,
            "proxy_pass_error_message" => {
                defaults.proxy.pass_error_message = parse_flag(child, "proxy_pass_error_message")?;
            }
            "proxy_protocol" => {
                defaults.proxy.proxy_protocol = parse_mail_proxy_protocol(child)?;
            }
            "proxy_smtp_auth" => {
                defaults.proxy.smtp_auth = parse_flag(child, "proxy_smtp_auth")?;
            }
            "xclient" => defaults.proxy.xclient = parse_flag(child, "xclient")?,
            "pop3_auth" => {
                defaults.pop3_auth = Some(parse_mail_auth_methods(
                    child,
                    &[
                        MailAuthMethod::Plain,
                        MailAuthMethod::Apop,
                        MailAuthMethod::CramMd5,
                        MailAuthMethod::External,
                    ],
                )?);
            }
            "pop3_capabilities" => {
                defaults
                    .pop3_capabilities
                    .get_or_insert_with(Vec::new)
                    .extend(parse_mail_capabilities(child)?);
            }
            "imap_auth" => {
                defaults.imap_auth = Some(parse_mail_auth_methods(
                    child,
                    &[
                        MailAuthMethod::Plain,
                        MailAuthMethod::Login,
                        MailAuthMethod::CramMd5,
                        MailAuthMethod::External,
                    ],
                )?);
            }
            "imap_capabilities" => {
                defaults
                    .imap_capabilities
                    .get_or_insert_with(Vec::new)
                    .extend(parse_mail_capabilities(child)?);
            }
            "imap_client_buffer" => {
                defaults.imap_client_buffer_size =
                    Some(parse_buffer_size(child, "imap_client_buffer")?);
            }
            "smtp_auth" => {
                defaults.smtp_auth = Some(parse_mail_auth_methods(
                    child,
                    &[
                        MailAuthMethod::Plain,
                        MailAuthMethod::Login,
                        MailAuthMethod::CramMd5,
                        MailAuthMethod::External,
                        MailAuthMethod::None,
                    ],
                )?);
            }
            "smtp_capabilities" => {
                defaults
                    .smtp_capabilities
                    .get_or_insert_with(Vec::new)
                    .extend(parse_mail_capabilities(child)?);
            }
            "smtp_client_buffer" => {
                defaults.smtp_client_buffer_size =
                    Some(parse_buffer_size(child, "smtp_client_buffer")?);
            }
            "smtp_greeting_delay" => {
                validate_no_block(child)?;
                defaults.smtp_greeting_delay =
                    Some(parse_duration(one_arg(child)?, &child.location)?);
            }
            other => {
                return Err(ConfigError::at(
                    child.location.clone(),
                    format!("unknown directive \"{other}\" in mail context"),
                ));
            }
        }
    }

    let mut servers = Vec::with_capacity(server_directives.len());
    for (id, server) in server_directives.into_iter().enumerate() {
        servers.push(compile_mail_server(server, id, prefix, &defaults)?);
    }
    validate_all_listen_conflicts(
        servers.iter().flat_map(|server| &server.listens),
        &directive.location,
    )?;

    Ok(MailConfig {
        servers,
        error_logs: defaults.error_logs,
    })
}

fn compile_mail_server(
    directive: &Directive,
    id: usize,
    prefix: &Path,
    defaults: &MailDefaults,
) -> ConfigResult<MailServer> {
    let mut listens = Vec::new();
    let mut protocol = None;
    let mut real_ip_from = defaults.real_ip_from.clone();
    let mut server_real_ip_from = false;
    let mut server_name = defaults.server_name.clone();
    let mut timeout = defaults.timeout;
    let mut max_errors = defaults.max_errors;
    let mut error_logs = defaults.error_logs.clone();
    let mut error_log_seen = false;
    let mut resolver = defaults.resolver.clone();
    let mut resolver_timeout = defaults.resolver_timeout;
    let mut auth_http = defaults.auth_http.clone();
    let mut auth_http_timeout = defaults.auth_http_timeout;
    let mut auth_http_headers = defaults.auth_http_headers.clone();
    let mut auth_http_headers_seen = false;
    let mut auth_http_pass_client_cert = defaults.auth_http_pass_client_cert;
    let mut proxy = defaults.proxy.clone();
    let mut tls = defaults.tls.clone();
    let mut tls_seen = MailTlsSeen::default();
    let mut pop3_auth = None;
    let mut pop3_capabilities = None;
    let mut imap_auth = None;
    let mut imap_capabilities = None;
    let mut imap_client_buffer_size = None;
    let mut smtp_auth = None;
    let mut smtp_capabilities = None;
    let mut smtp_client_buffer_size = None;
    let mut smtp_greeting_delay = None;

    for child in &directive.children {
        if apply_mail_tls_directive(child, prefix, &mut tls, &mut tls_seen)? {
            continue;
        }
        match child.name.as_str() {
            "listen" => {
                validate_no_block(child)?;
                for listen in parse_listen(child, prefix)? {
                    validate_mail_listen_options(&listen, &child.location)?;
                    validate_listen_conflicts(&listen, &listens, &child.location)?;
                    listens.push(listen);
                }
            }
            "protocol" => {
                validate_no_block(child)?;
                if protocol.is_some() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"protocol\" directive is duplicate",
                    ));
                }
                protocol = Some(parse_mail_protocol(child)?);
            }
            "set_real_ip_from" => {
                validate_no_block(child)?;
                if !server_real_ip_from {
                    real_ip_from.clear();
                    server_real_ip_from = true;
                }
                real_ip_from.push(parse_real_ip_source(one_arg(child)?, &child.location)?);
            }
            "server_name" => {
                validate_no_block(child)?;
                server_name = one_arg(child)?.to_string();
            }
            "timeout" => {
                validate_no_block(child)?;
                timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "max_errors" => {
                validate_no_block(child)?;
                max_errors = parse_usize(one_arg(child)?, &child.location, "max_errors")?;
            }
            "error_log" => {
                if !error_log_seen {
                    error_logs.clear();
                    error_log_seen = true;
                }
                error_logs.push(parse_log_target(prefix, child)?);
            }
            "resolver" => resolver = parse_resolver(child)?,
            "resolver_timeout" => {
                validate_no_block(child)?;
                resolver_timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "auth_http" => auth_http = Some(parse_auth_http(child)?),
            "auth_http_timeout" => {
                validate_no_block(child)?;
                auth_http_timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "auth_http_header" => {
                if !auth_http_headers_seen {
                    auth_http_headers.clear();
                    auth_http_headers_seen = true;
                }
                auth_http_headers.push(parse_auth_http_header(child)?);
            }
            "auth_http_pass_client_cert" => {
                auth_http_pass_client_cert = parse_flag(child, "auth_http_pass_client_cert")?;
            }
            "proxy" => proxy.enabled = parse_flag(child, "proxy")?,
            "proxy_timeout" => {
                validate_no_block(child)?;
                proxy.timeout = parse_duration(one_arg(child)?, &child.location)?;
            }
            "proxy_buffer" => proxy.buffer_size = parse_proxy_buffer(child)?,
            "proxy_pass_error_message" => {
                proxy.pass_error_message = parse_flag(child, "proxy_pass_error_message")?;
            }
            "proxy_protocol" => proxy.proxy_protocol = parse_mail_proxy_protocol(child)?,
            "proxy_smtp_auth" => proxy.smtp_auth = parse_flag(child, "proxy_smtp_auth")?,
            "xclient" => proxy.xclient = parse_flag(child, "xclient")?,
            "pop3_auth" => {
                pop3_auth = Some(parse_mail_auth_methods(
                    child,
                    &[
                        MailAuthMethod::Plain,
                        MailAuthMethod::Apop,
                        MailAuthMethod::CramMd5,
                        MailAuthMethod::External,
                    ],
                )?);
            }
            "pop3_capabilities" => {
                pop3_capabilities = Some(parse_mail_capabilities(child)?);
            }
            "imap_auth" => {
                imap_auth = Some(parse_mail_auth_methods(
                    child,
                    &[
                        MailAuthMethod::Plain,
                        MailAuthMethod::Login,
                        MailAuthMethod::CramMd5,
                        MailAuthMethod::External,
                    ],
                )?);
            }
            "imap_capabilities" => {
                imap_capabilities = Some(parse_mail_capabilities(child)?);
            }
            "imap_client_buffer" => {
                imap_client_buffer_size = Some(parse_buffer_size(child, "imap_client_buffer")?);
            }
            "smtp_auth" => {
                smtp_auth = Some(parse_mail_auth_methods(
                    child,
                    &[
                        MailAuthMethod::Plain,
                        MailAuthMethod::Login,
                        MailAuthMethod::CramMd5,
                        MailAuthMethod::External,
                        MailAuthMethod::None,
                    ],
                )?);
            }
            "smtp_capabilities" => {
                smtp_capabilities = Some(parse_mail_capabilities(child)?);
            }
            "smtp_client_buffer" => {
                smtp_client_buffer_size = Some(parse_buffer_size(child, "smtp_client_buffer")?);
            }
            "smtp_greeting_delay" => {
                validate_no_block(child)?;
                smtp_greeting_delay = Some(parse_duration(one_arg(child)?, &child.location)?);
            }
            other => {
                return Err(ConfigError::at(
                    child.location.clone(),
                    format!("unknown directive \"{other}\" in mail server context"),
                ));
            }
        }
    }

    if listens.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "no \"listen\" is defined for mail server",
        ));
    }
    let protocol = protocol.ok_or_else(|| {
        ConfigError::at(
            directive.location.clone(),
            "no \"protocol\" is defined for mail server",
        )
    })?;
    let auth_http = auth_http.ok_or_else(|| {
        ConfigError::at(
            directive.location.clone(),
            "no \"auth_http\" is defined for mail server",
        )
    })?;

    let (auth_methods, capabilities, client_buffer_size, greeting_delay) =
        resolve_protocol_defaults(
            protocol,
            defaults,
            pop3_auth,
            pop3_capabilities,
            imap_auth,
            imap_capabilities,
            imap_client_buffer_size,
            smtp_auth,
            smtp_capabilities,
            smtp_client_buffer_size,
            smtp_greeting_delay,
        );
    if protocol == MailProtocol::Pop3 && auth_methods.contains(&MailAuthMethod::Plain) {
        if !auth_methods.contains(&MailAuthMethod::Login) {
            // Nginx makes LOGIN available whenever POP3 PLAIN is enabled.
        }
    }
    validate_ssl_client_verification(
        &directive.location,
        tls.verify_client,
        tls.client_certificate.as_ref(),
        tls.trusted_certificate.as_ref(),
    )?;
    if listens.iter().any(|listen| listen.ssl) || tls.starttls != MailStartTls::Off {
        if tls.certificates.is_empty() {
            return Err(ConfigError::at(
                directive.location.clone(),
                "no \"ssl_certificate\" is defined for mail TLS listener",
            ));
        }
        if tls.certificate_keys.is_empty() {
            return Err(ConfigError::at(
                directive.location.clone(),
                "no \"ssl_certificate_key\" is defined for mail TLS listener",
            ));
        }
    }
    if protocol != MailProtocol::Smtp && auth_methods.contains(&MailAuthMethod::None) {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"none\" authentication is supported only for SMTP",
        ));
    }
    if protocol != MailProtocol::Pop3 && auth_methods.contains(&MailAuthMethod::Apop) {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"apop\" authentication is supported only for POP3",
        ));
    }

    Ok(MailServer {
        id,
        listens,
        protocol,
        real_ip_from,
        server_name,
        timeout,
        max_errors,
        error_logs,
        resolver,
        resolver_timeout,
        auth_http: MailAuthHttpConfig {
            address: auth_http.address,
            host_header: auth_http.host_header,
            uri: auth_http.uri,
            timeout: auth_http_timeout,
            headers: auth_http_headers,
            pass_client_cert: auth_http_pass_client_cert,
        },
        auth_methods,
        capabilities,
        client_buffer_size,
        greeting_delay,
        proxy,
        tls,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_protocol_defaults(
    protocol: MailProtocol,
    defaults: &MailDefaults,
    pop3_auth: Option<Vec<MailAuthMethod>>,
    pop3_capabilities: Option<Vec<String>>,
    imap_auth: Option<Vec<MailAuthMethod>>,
    imap_capabilities: Option<Vec<String>>,
    imap_client_buffer_size: Option<usize>,
    smtp_auth: Option<Vec<MailAuthMethod>>,
    smtp_capabilities: Option<Vec<String>>,
    smtp_client_buffer_size: Option<usize>,
    smtp_greeting_delay: Option<Duration>,
) -> (Vec<MailAuthMethod>, Vec<String>, usize, Duration) {
    match protocol {
        MailProtocol::Pop3 => {
            let mut auth = pop3_auth
                .or_else(|| defaults.pop3_auth.clone())
                .unwrap_or_else(|| vec![MailAuthMethod::Plain, MailAuthMethod::Login]);
            if auth.contains(&MailAuthMethod::Plain) && !auth.contains(&MailAuthMethod::Login) {
                auth.push(MailAuthMethod::Login);
            }
            auth.sort_by_key(|method| method.canonical_order());
            (
                auth,
                pop3_capabilities
                    .or_else(|| defaults.pop3_capabilities.clone())
                    .unwrap_or_else(|| vec!["TOP".into(), "USER".into(), "UIDL".into()]),
                4096,
                Duration::ZERO,
            )
        }
        MailProtocol::Imap => {
            let mut auth = imap_auth
                .or_else(|| defaults.imap_auth.clone())
                .unwrap_or_else(|| vec![MailAuthMethod::Plain]);
            auth.sort_by_key(|method| method.canonical_order());
            (
                auth,
                imap_capabilities
                    .or_else(|| defaults.imap_capabilities.clone())
                    .unwrap_or_else(|| vec!["IMAP4".into(), "IMAP4rev1".into(), "UIDPLUS".into()]),
                imap_client_buffer_size
                    .or(defaults.imap_client_buffer_size)
                    .unwrap_or(4096),
                Duration::ZERO,
            )
        }
        MailProtocol::Smtp => {
            let mut auth = smtp_auth
                .or_else(|| defaults.smtp_auth.clone())
                .unwrap_or_else(|| vec![MailAuthMethod::Plain, MailAuthMethod::Login]);
            auth.sort_by_key(|method| method.canonical_order());
            (
                auth,
                smtp_capabilities
                    .or_else(|| defaults.smtp_capabilities.clone())
                    .unwrap_or_default(),
                smtp_client_buffer_size
                    .or(defaults.smtp_client_buffer_size)
                    .unwrap_or(4096),
                smtp_greeting_delay
                    .or(defaults.smtp_greeting_delay)
                    .unwrap_or(Duration::ZERO),
            )
        }
    }
}

fn apply_mail_tls_directive(
    directive: &Directive,
    prefix: &Path,
    tls: &mut MailTlsConfig,
    seen: &mut MailTlsSeen,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "ssl_certificate" => {
            validate_no_block(directive)?;
            let path = resolve_path(prefix, one_arg(directive)?);
            if !seen.certificate {
                tls.certificates.clear();
                seen.certificate = true;
            }
            tls.certificates.push(path.clone());
            tls.certificate = Some(path);
        }
        "ssl_certificate_key" => {
            validate_no_block(directive)?;
            let path = resolve_path(prefix, one_arg(directive)?);
            if !seen.certificate_key {
                tls.certificate_keys.clear();
                seen.certificate_key = true;
            }
            tls.certificate_keys.push(path.clone());
            tls.certificate_key = Some(path);
        }
        "ssl_protocols" => {
            let mut protocols_seen = false;
            apply_tls_protocols(directive, &mut tls.protocols, &mut protocols_seen)?;
        }
        "ssl_prefer_server_ciphers" => {
            tls.prefer_server_ciphers = parse_flag(directive, "ssl_prefer_server_ciphers")?;
        }
        "ssl_ciphers" => tls.ciphers = parse_tls_cipher_config(directive)?,
        "ssl_verify_client" => tls.verify_client = parse_ssl_verify_client(directive)?,
        "ssl_verify_depth" => tls.verify_depth = parse_ssl_verify_depth(directive)?,
        "ssl_client_certificate" => {
            validate_no_block(directive)?;
            tls.client_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_trusted_certificate" => {
            validate_no_block(directive)?;
            tls.trusted_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_crl" => {
            validate_no_block(directive)?;
            tls.crl = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_password_file" => tls.passwords = parse_password_file(prefix, directive)?,
        "ssl_session_cache" => tls.session_cache = parse_mail_session_cache(directive)?,
        "ssl_session_timeout" => {
            validate_no_block(directive)?;
            tls.session_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "ssl_session_tickets" => {
            tls.session_tickets = parse_flag(directive, "ssl_session_tickets")?;
        }
        "starttls" => {
            validate_no_block(directive)?;
            tls.starttls = match one_arg(directive)? {
                "off" => MailStartTls::Off,
                "on" => MailStartTls::On,
                "only" => MailStartTls::Only,
                value => {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!(
                            "invalid value \"{value}\" in \"starttls\" directive, \
                             it must be \"off\", \"on\" or \"only\""
                        ),
                    ));
                }
            };
        }
        "ssl_certificate_compression" => {
            parse_flag(directive, "ssl_certificate_compression")?;
        }
        "ssl_session_ticket_key" => {
            validate_no_block(directive)?;
            let path = resolve_path(prefix, one_arg(directive)?);
            if !path.exists() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("open() \"{}\" failed", path.display()),
                ));
            }
        }
        "ssl_conf_command" => reject_ssl_conf_command(directive, "ssl_conf_command")?,
        _ if try_apply_compat_directive(directive)? => {}
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_mail_session_cache(directive: &Directive) -> ConfigResult<MailSessionCache> {
    validate_no_block(directive)?;
    if directive.args.is_empty() || directive.args.len() > 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"ssl_session_cache\" directive",
        ));
    }

    let mut builtin = None;
    let mut shared = None;
    for value in &directive.args {
        match value.as_str() {
            "off" => return Ok(MailSessionCache::Off),
            "none" => builtin = Some(MailSessionCache::None),
            "builtin" => builtin = Some(MailSessionCache::Builtin(None)),
            value if value.starts_with("builtin:") => {
                let count = value["builtin:".len()..].parse::<usize>().map_err(|_| {
                    ConfigError::at(
                        directive.location.clone(),
                        format!("invalid session cache \"{value}\""),
                    )
                })?;
                if count == 0 {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!("invalid session cache \"{value}\""),
                    ));
                }
                builtin = Some(MailSessionCache::Builtin(Some(count)));
            }
            value if value.starts_with("shared:") => {
                let rest = &value["shared:".len()..];
                let (name, size) = rest.split_once(':').ok_or_else(|| {
                    ConfigError::at(
                        directive.location.clone(),
                        format!("invalid session cache \"{value}\""),
                    )
                })?;
                if name.is_empty() {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!("invalid session cache \"{value}\""),
                    ));
                }
                let size = parse_size(size, &directive.location).map_err(|_| {
                    ConfigError::at(
                        directive.location.clone(),
                        format!("invalid session cache \"{value}\""),
                    )
                })?;
                if size == 0 {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!("invalid session cache \"{value}\""),
                    ));
                }
                shared = Some(MailSessionCache::Shared {
                    name: name.to_string(),
                    size: size as usize,
                });
            }
            value => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid session cache \"{value}\""),
                ));
            }
        }
    }

    if let Some(shared) = shared {
        return Ok(shared);
    }
    Ok(builtin.unwrap_or(MailSessionCache::None))
}

fn parse_mail_protocol(directive: &Directive) -> ConfigResult<MailProtocol> {
    match one_arg(directive)? {
        "pop3" => Ok(MailProtocol::Pop3),
        "imap" => Ok(MailProtocol::Imap),
        "smtp" => Ok(MailProtocol::Smtp),
        value => Err(ConfigError::at(
            directive.location.clone(),
            format!("unknown protocol \"{value}\""),
        )),
    }
}

fn parse_mail_auth_methods(
    directive: &Directive,
    allowed: &[MailAuthMethod],
) -> ConfigResult<Vec<MailAuthMethod>> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid number of arguments in \"{}\" directive",
                directive.name
            ),
        ));
    }
    let mut methods = Vec::new();
    for value in &directive.args {
        let method = match value.as_str() {
            "plain" => MailAuthMethod::Plain,
            "login" => MailAuthMethod::Login,
            "apop" => MailAuthMethod::Apop,
            "cram-md5" => MailAuthMethod::CramMd5,
            "external" => MailAuthMethod::External,
            "none" => MailAuthMethod::None,
            _ => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!(
                        "invalid value \"{value}\" in \"{}\" directive",
                        directive.name
                    ),
                ));
            }
        };
        if !allowed.contains(&method) {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!(
                    "\"{value}\" authentication is not supported by \"{}\"",
                    directive.name
                ),
            ));
        }
        if !methods.contains(&method) {
            methods.push(method);
        }
    }
    Ok(methods)
}

fn parse_mail_capabilities(directive: &Directive) -> ConfigResult<Vec<String>> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid number of arguments in \"{}\" directive",
                directive.name
            ),
        ));
    }
    Ok(directive.args.clone())
}

fn parse_proxy_buffer(directive: &Directive) -> ConfigResult<usize> {
    let size = parse_buffer_size(directive, "proxy_buffer")?;
    if size == 0 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "value must be nonzero in \"proxy_buffer\" directive",
        ));
    }
    Ok(size)
}

fn parse_buffer_size(directive: &Directive, name: &str) -> ConfigResult<usize> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    let size = parse_size(value, &directive.location).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        )
    })?;
    if size == 0 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("value must be nonzero in \"{name}\" directive"),
        ));
    }
    usize::try_from(size).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        )
    })
}

fn parse_mail_proxy_protocol(directive: &Directive) -> ConfigResult<MailProxyProtocol> {
    validate_no_block(directive)?;
    match one_arg(directive)? {
        "off" => Ok(MailProxyProtocol::Off),
        "on" => Ok(MailProxyProtocol::On),
        "v2" => Ok(MailProxyProtocol::V2),
        value => Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid value \"{value}\" in \"proxy_protocol\" directive, \
                 it must be \"off\", \"on\" or \"v2\""
            ),
        )),
    }
}

fn parse_auth_http_header(directive: &Directive) -> ConfigResult<(String, String)> {
    validate_no_block(directive)?;
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"auth_http_header\" directive",
        ));
    }
    let name = &directive.args[0];
    let value = &directive.args[1];
    if name.is_empty()
        || name
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b' ' | b':' | b'\r' | b'\n'))
        || value.bytes().any(|byte| matches!(byte, b'\r' | b'\n'))
    {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid header in \"auth_http_header\" directive",
        ));
    }
    Ok((name.clone(), value.clone()))
}

fn parse_auth_http(directive: &Directive) -> ConfigResult<MailAuthHttpBase> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    if value.contains(char::is_whitespace) {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid URL in auth_http \"{value}\""),
        ));
    }
    let value = value.strip_prefix("http://").unwrap_or(value);
    if value.starts_with("https://") {
        return Err(ConfigError::at(
            directive.location.clone(),
            "auth_http supports only plain HTTP",
        ));
    }

    if let Some(value) = value.strip_prefix("unix:") {
        let (path, uri) = value
            .split_once(":/")
            .map(|(path, uri)| (path, format!("/{uri}")))
            .unwrap_or((value, "/".into()));
        if path.is_empty() {
            return Err(ConfigError::at(
                directive.location.clone(),
                "invalid UNIX socket path in auth_http",
            ));
        }
        return Ok(MailAuthHttpBase {
            address: MailAuthHttpAddress::Unix(PathBuf::from(path)),
            host_header: "localhost".into(),
            uri,
        });
    }

    let (authority, uri) = value
        .split_once('/')
        .map(|(authority, uri)| (authority, format!("/{uri}")))
        .unwrap_or((value, "/".into()));
    if authority.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid URL in auth_http \"{value}\""),
        ));
    }
    let (host, port, host_header) = parse_http_authority(authority, &directive.location)?;
    let mut addresses: Vec<SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| {
            ConfigError::at(
                directive.location.clone(),
                format!("host not found in auth_http \"{value}\": {error}"),
            )
        })?
        .collect();
    addresses.sort();
    addresses.dedup();
    if addresses.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("host not found in auth_http \"{value}\""),
        ));
    }
    Ok(MailAuthHttpBase {
        address: MailAuthHttpAddress::Inet(addresses),
        host_header,
        uri,
    })
}

fn parse_http_authority(
    authority: &str,
    location: &SourceLocation,
) -> ConfigResult<(String, u16, String)> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, rest)) = rest.split_once(']') else {
            return Err(ConfigError::at(
                location.clone(),
                format!("invalid host in auth_http \"{authority}\""),
            ));
        };
        let port = if let Some(port) = rest.strip_prefix(':') {
            port.parse::<u16>().map_err(|_| {
                ConfigError::at(
                    location.clone(),
                    format!("invalid port in auth_http \"{authority}\""),
                )
            })?
        } else if rest.is_empty() {
            80
        } else {
            return Err(ConfigError::at(
                location.clone(),
                format!("invalid host in auth_http \"{authority}\""),
            ));
        };
        (host.to_string(), port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if host.contains(':') {
            return Err(ConfigError::at(
                location.clone(),
                format!("invalid IPv6 address in auth_http \"{authority}\""),
            ));
        }
        let port = port.parse::<u16>().map_err(|_| {
            ConfigError::at(
                location.clone(),
                format!("invalid port in auth_http \"{authority}\""),
            )
        })?;
        (host.to_string(), port)
    } else {
        (authority.to_string(), 80)
    };
    if host.is_empty() {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid host in auth_http \"{authority}\""),
        ));
    }
    Ok((host, port, authority.to_string()))
}

fn validate_mail_listen_options(
    listen: &ListenConfig,
    location: &SourceLocation,
) -> ConfigResult<()> {
    if listen.http2 {
        return Err(ConfigError::at(
            location.clone(),
            "invalid parameter \"http2\" in listen directive",
        ));
    }
    if listen.quic {
        return Err(ConfigError::at(
            location.clone(),
            "invalid parameter \"quic\" in listen directive",
        ));
    }
    if listen.udp {
        return Err(ConfigError::at(
            location.clone(),
            "invalid parameter \"udp\" in listen directive",
        ));
    }
    if listen.address.unix_path().is_some() && listen.ssl {
        return Err(ConfigError::at(
            location.clone(),
            "\"ssl\" is incompatible with UNIX domain mail listen sockets",
        ));
    }
    Ok(())
}

fn default_mail_server_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ipv4_and_ipv6_auth_http_authorities() {
        let location = SourceLocation::new("test", 1, 1);
        assert_eq!(
            parse_http_authority("127.0.0.1:8080", &location).unwrap(),
            ("127.0.0.1".into(), 8080, "127.0.0.1:8080".into())
        );
        assert_eq!(
            parse_http_authority("[::1]:8081", &location).unwrap(),
            ("::1".into(), 8081, "[::1]:8081".into())
        );
        assert_eq!(
            parse_http_authority("localhost", &location).unwrap(),
            ("localhost".into(), 80, "localhost".into())
        );
    }
}

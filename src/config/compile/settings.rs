use super::*;

pub(super) fn parse_flag(directive: &Directive, name: &str) -> ConfigResult<bool> {
    validate_no_block(directive)?;
    match one_arg(directive)? {
        "on" => Ok(true),
        "off" => Ok(false),
        value => Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid value \"{value}\" in \"{name}\" directive, it must be \"on\" or \"off\""
            ),
        )),
    }
}

pub(super) fn parse_if_modified_since(directive: &Directive) -> ConfigResult<IfModifiedSince> {
    validate_no_block(directive)?;
    match one_arg(directive)? {
        "off" => Ok(IfModifiedSince::Off),
        "exact" => Ok(IfModifiedSince::Exact),
        "before" => Ok(IfModifiedSince::Before),
        value => Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid value \"{value}\" in \"if_modified_since\" directive, \
                 it must be \"off\", \"exact\" or \"before\""
            ),
        )),
    }
}

pub(super) fn apply_tls_protocols(
    directive: &Directive,
    current: &mut TlsProtocols,
    seen: &mut bool,
) -> ConfigResult<()> {
    let name = &directive.name;
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid number of arguments in \"{name}\" directive"),
        ));
    }

    let mut parsed = TlsProtocols {
        tls12: false,
        tls13: false,
    };
    for value in &directive.args {
        match value.to_ascii_lowercase().as_str() {
            "tlsv1.2" => parsed.tls12 = true,
            "tlsv1.3" => parsed.tls13 = true,
            "sslv2" | "sslv3" | "tlsv1" | "tlsv1.1" => {}
            _ => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid value \"{value}\" in \"{name}\" directive"),
                ));
            }
        }
    }

    let keeps_supported_protocol =
        parsed.tls12 || parsed.tls13 || (*seen && (current.tls12 || current.tls13));
    if !keeps_supported_protocol {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "\"{name}\" selects no protocols supported by rustls; \
                 add TLSv1.2 or TLSv1.3"
            ),
        ));
    }

    if *seen {
        current.tls12 |= parsed.tls12;
        current.tls13 |= parsed.tls13;
    } else {
        *current = parsed;
        *seen = true;
    }
    Ok(())
}

pub(super) fn parse_tls_cipher_config(directive: &Directive) -> ConfigResult<TlsCipherConfig> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    TlsCipherConfig::parse(value).map_err(|message| {
        ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid cipher list \"{value}\" in \"{}\" directive: {message}",
                directive.name
            ),
        )
    })
}

pub(super) fn parse_ssl_verify_client(
    directive: &Directive,
) -> ConfigResult<ClientCertificateVerify> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    match value {
        "off" => Ok(ClientCertificateVerify::Off),
        "on" => Ok(ClientCertificateVerify::On),
        "optional" => Ok(ClientCertificateVerify::Optional),
        "optional_no_ca" => Ok(ClientCertificateVerify::OptionalNoCa),
        _ => Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"ssl_verify_client\" directive"),
        )),
    }
}

pub(super) fn parse_ssl_verify_depth(directive: &Directive) -> ConfigResult<u32> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    value.parse::<u32>().map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"ssl_verify_depth\" directive"),
        )
    })
}

pub(super) fn validate_ssl_client_verification(
    location: &SourceLocation,
    verify: ClientCertificateVerify,
    client_certificate: Option<&PathBuf>,
    trusted_certificate: Option<&PathBuf>,
) -> ConfigResult<()> {
    if !matches!(
        verify,
        ClientCertificateVerify::Off | ClientCertificateVerify::OptionalNoCa
    ) && client_certificate.is_none()
        && trusted_certificate.is_none()
    {
        return Err(ConfigError::at(
            location.clone(),
            "no ssl_client_certificate or ssl_trusted_certificate for ssl_verify_client",
        ));
    }
    Ok(())
}

pub(super) fn parse_gzip_proxied(directive: &Directive) -> ConfigResult<GzipProxiedConfig> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"gzip_proxied\" directive",
        ));
    }
    if directive.args.len() == 1 && directive.args[0] == "off" {
        return Ok(GzipProxiedConfig::default());
    }
    let mut config = GzipProxiedConfig::default();
    for parameter in &directive.args {
        match parameter.as_str() {
            "any" => config.any = true,
            "auth" => config.auth = true,
            "expired" => config.expired = true,
            "no-cache" => config.no_cache = true,
            "no-store" => config.no_store = true,
            "private" => config.private = true,
            "no_last_modified" => config.no_last_modified = true,
            "no_etag" => config.no_etag = true,
            "off" => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"off\" cannot be combined with other \"gzip_proxied\" parameters",
                ));
            }
            value => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid value \"{value}\" in \"gzip_proxied\" directive"),
                ));
            }
        }
    }
    Ok(config)
}

pub(super) fn parse_gzip_static(directive: &Directive) -> ConfigResult<GzipStatic> {
    validate_no_block(directive)?;
    match one_arg(directive)? {
        "off" => Ok(GzipStatic::Off),
        "on" => Ok(GzipStatic::On),
        "always" => Ok(GzipStatic::Always),
        value => Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"gzip_static\" directive"),
        )),
    }
}

pub(super) fn parse_sub_filter(directive: &Directive) -> ConfigResult<SubFilter> {
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"sub_filter\" directive",
        ));
    }
    if directive.args[0].is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "empty \"sub_filter\" pattern",
        ));
    }
    Ok(SubFilter {
        pattern: directive.args[0].clone(),
        replacement: directive.args[1].clone(),
    })
}

pub(super) fn parse_sub_filter_types(directive: &Directive) -> ConfigResult<Vec<String>> {
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"sub_filter_types\" directive",
        ));
    }
    let mut types = directive
        .args
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if !types.iter().any(|value| value == "text/html") {
        types.push("text/html".into());
    }
    Ok(types)
}

pub(super) fn parse_add_header(directive: &Directive) -> ConfigResult<AddHeader> {
    if directive.args.len() < 2 || directive.args.len() > 3 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid number of arguments in \"{}\" directive",
                directive.name
            ),
        ));
    }
    let always = directive.args.get(2).is_some_and(|value| value == "always");
    if directive.args.len() == 3 && !always {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid parameter in \"{}\" directive", directive.name),
        ));
    }
    Ok(AddHeader {
        name: directive.args[0].clone(),
        value: directive.args[1].clone(),
        always,
    })
}

pub(super) fn parse_header_inheritance(directive: &Directive) -> ConfigResult<HeaderInheritance> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    match value {
        "off" => Ok(HeaderInheritance::Off),
        "on" => Ok(HeaderInheritance::On),
        "merge" => Ok(HeaderInheritance::Merge),
        _ => Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid value \"{value}\" in \"{}\" directive",
                directive.name
            ),
        )),
    }
}

pub(super) fn parse_file_access(directive: &Directive, name: &str) -> ConfigResult<u32> {
    validate_no_block(directive)?;
    if directive.args.is_empty() || directive.args.len() > 3 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid number of arguments in \"{name}\" directive"),
        ));
    }
    let mut access = 0;
    let mut user = 0o600;
    for value in &directive.args {
        let Some((class, right)) = value.split_once(':') else {
            return Err(invalid_file_access(directive, name, value));
        };
        let shift = match class {
            "user" => {
                user = 0;
                6
            }
            "group" => 3,
            "all" => 0,
            _ => return Err(invalid_file_access(directive, name, value)),
        };
        let permission = match right {
            "rw" => 0o6,
            "r" => 0o4,
            _ => return Err(invalid_file_access(directive, name, value)),
        };
        access |= permission << shift;
    }
    Ok(access | user)
}

fn invalid_file_access(directive: &Directive, name: &str, value: &str) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        format!("invalid value \"{value}\" in \"{name}\" directive"),
    )
}

pub(super) fn parse_access_rule(directive: &Directive) -> ConfigResult<AccessRule> {
    let value = one_arg(directive)?;
    let allow = directive.name == "allow";
    if value == "all" {
        return Ok(if allow {
            AccessRule::AllowAll
        } else {
            AccessRule::DenyAll
        });
    }
    let network = parse_ip_network(value, &directive.location)?;
    Ok(if allow {
        AccessRule::Allow(network)
    } else {
        AccessRule::Deny(network)
    })
}

pub(super) fn parse_ip_network(value: &str, location: &SourceLocation) -> ConfigResult<IpNetwork> {
    let (address, prefix) = match value.split_once('/') {
        Some((address, prefix)) => {
            let address: IpAddr = address.parse().map_err(|_| {
                ConfigError::at(location.clone(), format!("invalid IP address: {address}"))
            })?;
            let prefix = prefix.parse::<u8>().map_err(|_| {
                ConfigError::at(location.clone(), format!("invalid CIDR prefix: {prefix}"))
            })?;
            (address, prefix)
        }
        None => {
            let address: IpAddr = value.parse().map_err(|_| {
                ConfigError::at(location.clone(), format!("invalid IP address: {value}"))
            })?;
            let prefix = if address.is_ipv4() { 32 } else { 128 };
            (address, prefix)
        }
    };
    let max = if address.is_ipv4() { 32 } else { 128 };
    if prefix > max {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid CIDR prefix: {prefix}"),
        ));
    }
    Ok(IpNetwork { address, prefix })
}

pub(super) fn parse_ip_range(value: &str, location: &SourceLocation) -> ConfigResult<IpRange> {
    let Some((start, end)) = value.split_once('-') else {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid IP range: {value}"),
        ));
    };
    let start: Ipv4Addr = start.parse().map_err(|_| {
        ConfigError::at(
            location.clone(),
            format!("invalid IPv4 address in range: {start}"),
        )
    })?;
    let end: Ipv4Addr = end.parse().map_err(|_| {
        ConfigError::at(
            location.clone(),
            format!("invalid IPv4 address in range: {end}"),
        )
    })?;
    let start = u32::from(start);
    let end = u32::from(end);
    if start > end {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid IP range: {value}"),
        ));
    }
    Ok(IpRange { start, end })
}

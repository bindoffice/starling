use std::net::{IpAddr, SocketAddr};

use super::*;

#[derive(Default)]
pub(in crate::config::compile) struct StaplingSeen {
    pub(super) ssl_stapling: bool,
    pub(super) ssl_stapling_verify: bool,
    pub(super) ssl_stapling_file: bool,
    pub(super) ssl_stapling_responder: bool,
    pub(super) resolver: bool,
    pub(super) resolver_timeout: bool,
}

pub(in crate::config::compile) fn try_apply_stapling_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    scope: Scope,
    seen: &mut StaplingSeen,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "ssl_stapling" => {
            require_tls_stapling_scope(directive, scope)?;
            reject_duplicate(directive, &mut seen.ssl_stapling)?;
            context.ssl_stapling = parse_flag(directive, "ssl_stapling")?;
        }
        "ssl_stapling_verify" => {
            require_tls_stapling_scope(directive, scope)?;
            reject_duplicate(directive, &mut seen.ssl_stapling_verify)?;
            context.ssl_stapling_verify = parse_flag(directive, "ssl_stapling_verify")?;
        }
        "ssl_stapling_file" => {
            require_tls_stapling_scope(directive, scope)?;
            reject_duplicate(directive, &mut seen.ssl_stapling_file)?;
            context.ssl_stapling_file = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_stapling_responder" => {
            require_tls_stapling_scope(directive, scope)?;
            reject_duplicate(directive, &mut seen.ssl_stapling_responder)?;
            context.ssl_stapling_responder = Some(one_arg(directive)?.to_string());
        }
        "resolver" => {
            reject_duplicate(directive, &mut seen.resolver)?;
            context.resolver = parse_resolver(directive)?;
        }
        "resolver_timeout" => {
            reject_duplicate(directive, &mut seen.resolver_timeout)?;
            context.resolver_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

pub(in crate::config::compile) fn validate_ssl_stapling(
    location: &SourceLocation,
    context: &EffectiveConfig,
) -> ConfigResult<()> {
    if context.ssl_stapling
        && context.ssl_stapling_verify
        && context.ssl_trusted_certificate.is_none()
    {
        return Err(ConfigError::at(
            location.clone(),
            "no ssl_trusted_certificate for ssl_stapling_verify",
        ));
    }
    Ok(())
}

fn require_tls_stapling_scope(directive: &Directive, scope: Scope) -> ConfigResult<()> {
    if matches!(scope, Scope::Server) {
        return Ok(());
    }
    Err(ConfigError::at(
        directive.location.clone(),
        format!("\"{}\" directive is not allowed here", directive.name),
    ))
}

fn reject_duplicate(directive: &Directive, seen: &mut bool) -> ConfigResult<()> {
    if *seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    *seen = true;
    Ok(())
}

pub(in crate::config::compile) fn parse_resolver(
    directive: &Directive,
) -> ConfigResult<ResolverConfig> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"resolver\" directive",
        ));
    }
    if directive.args.len() == 1 && directive.args[0] == "off" {
        return Ok(ResolverConfig::default());
    }

    let mut config = ResolverConfig::default();
    for value in &directive.args {
        if let Some(value) = value.strip_prefix("valid=") {
            config.valid = Some(parse_duration(value, &directive.location)?);
        } else if let Some(value) = value.strip_prefix("ipv6=") {
            config.ipv6 = match value {
                "on" => true,
                "off" => false,
                _ => {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!("invalid parameter: {value}"),
                    ));
                }
            };
        } else if value.starts_with("status_zone=") {
            continue;
        } else if value == "off" {
            return Err(ConfigError::at(
                directive.location.clone(),
                "\"off\" cannot be combined with other \"resolver\" parameters",
            ));
        } else {
            config
                .servers
                .push(parse_resolver_server(value, &directive.location)?);
        }
    }
    if config.servers.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "no name servers defined",
        ));
    }
    Ok(config)
}

fn parse_resolver_server(value: &str, location: &SourceLocation) -> ConfigResult<SocketAddr> {
    if let Ok(address) = value.parse::<SocketAddr>() {
        return Ok(address);
    }
    if let Ok(address) = value.parse::<IpAddr>() {
        return Ok(SocketAddr::new(address, 53));
    }
    if let Some(address) = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .and_then(|value| value.parse::<IpAddr>().ok())
    {
        return Ok(SocketAddr::new(address, 53));
    }
    Err(ConfigError::at(
        location.clone(),
        format!("invalid resolver address \"{value}\""),
    ))
}

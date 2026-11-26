use super::*;

pub(super) fn apply_auto_https_directive(
    directive: &Directive,
    config: &mut AutoHttpsConfig,
    prefix: &Path,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "auto_https" => apply_auto_https(directive, config, prefix)?,
        "auto_https_cert_dir" => {
            validate_no_block(directive)?;
            config.cert_dir = resolve_path(prefix, one_arg(directive)?);
        }
        "auto_https_email" => {
            validate_no_block(directive)?;
            config.email = Some(one_arg(directive)?.to_string());
        }
        "auto_https_staging" => {
            config.staging = parse_flag(directive, "auto_https_staging")?;
        }
        "auto_https_acme_directory" => {
            validate_no_block(directive)?;
            config.acme_directory = Some(parse_url(directive, "auto_https_acme_directory")?);
        }
        "auto_https_acme_ca_file" => {
            validate_no_block(directive)?;
            config.acme_ca_file = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "auto_https_challenge" => {
            validate_no_block(directive)?;
            config.challenge = parse_challenge(one_arg(directive)?, &directive.location)?;
        }
        "auto_https_dns_hook" => {
            validate_no_block(directive)?;
            config.dns_hook = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "auto_https_dns_propagation_delay" => {
            validate_no_block(directive)?;
            config.dns_propagation_delay =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "auto_https_redirect" => {
            config.redirect_http = parse_flag(directive, "auto_https_redirect")?;
        }
        "auto_https_http_port" => {
            validate_no_block(directive)?;
            config.http_port = parse_port(one_arg(directive)?, &directive.location)?;
        }
        "auto_https_https_port" => {
            validate_no_block(directive)?;
            config.https_port = parse_port(one_arg(directive)?, &directive.location)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn apply_auto_https(
    directive: &Directive,
    config: &mut AutoHttpsConfig,
    prefix: &Path,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"auto_https\" directive",
        ));
    }
    config.enabled = match directive.args[0].as_str() {
        "on" => true,
        "off" => false,
        value => {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("invalid value \"{value}\" in \"auto_https\" directive"),
            ));
        }
    };
    for option in &directive.args[1..] {
        if let Some(value) = option.strip_prefix("cert_dir=") {
            config.cert_dir = resolve_path(prefix, value);
        } else if let Some(value) = option.strip_prefix("email=") {
            config.email = Some(value.to_string());
        } else if let Some(value) = option.strip_prefix("staging=") {
            config.staging = parse_inline_flag(value, &directive.location, "staging")?;
        } else if let Some(value) = option.strip_prefix("acme_directory=") {
            config.acme_directory = Some(validate_url(value, &directive.location)?);
        } else if let Some(value) = option.strip_prefix("acme_ca_file=") {
            config.acme_ca_file = Some(resolve_path(prefix, value));
        } else if let Some(value) = option.strip_prefix("challenge=") {
            config.challenge = parse_challenge(value, &directive.location)?;
        } else if let Some(value) = option.strip_prefix("dns_hook=") {
            config.dns_hook = Some(resolve_path(prefix, value));
        } else if let Some(value) = option.strip_prefix("dns_propagation_delay=") {
            config.dns_propagation_delay = parse_duration(value, &directive.location)?;
        } else if let Some(value) = option.strip_prefix("redirect=") {
            config.redirect_http = parse_inline_flag(value, &directive.location, "redirect")?;
        } else if let Some(value) = option.strip_prefix("http_port=") {
            config.http_port = parse_port(value, &directive.location)?;
        } else if let Some(value) = option.strip_prefix("https_port=") {
            config.https_port = parse_port(value, &directive.location)?;
        } else {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("invalid parameter \"{option}\" in \"auto_https\" directive"),
            ));
        }
    }
    Ok(())
}

fn parse_challenge(value: &str, location: &SourceLocation) -> ConfigResult<AutoHttpsChallenge> {
    match value {
        "http-01" => Ok(AutoHttpsChallenge::Http01),
        "dns-01" => Ok(AutoHttpsChallenge::Dns01),
        _ => Err(ConfigError::at(
            location.clone(),
            format!("invalid ACME challenge \"{value}\", expected \"http-01\" or \"dns-01\""),
        )),
    }
}

fn parse_inline_flag(value: &str, location: &SourceLocation, name: &str) -> ConfigResult<bool> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(ConfigError::at(
            location.clone(),
            format!("invalid value \"{value}\" in auto_https {name}= parameter"),
        )),
    }
}

fn parse_url(directive: &Directive, name: &str) -> ConfigResult<String> {
    validate_url(one_arg(directive)?, &directive.location).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid URL in \"{name}\" directive"),
        )
    })
}

fn validate_url(value: &str, location: &SourceLocation) -> ConfigResult<String> {
    if value.starts_with("http://") || value.starts_with("https://") {
        Ok(value.to_string())
    } else {
        Err(ConfigError::at(
            location.clone(),
            format!("invalid URL \"{value}\""),
        ))
    }
}

fn parse_port(value: &str, location: &SourceLocation) -> ConfigResult<u16> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| {
            ConfigError::at(
                location.clone(),
                format!("invalid auto_https port \"{value}\""),
            )
        })
}

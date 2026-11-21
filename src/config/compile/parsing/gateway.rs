use super::*;

pub(in crate::config::compile) fn parse_fastcgi_pass(
    directive: &Directive,
) -> ConfigResult<FastCgiPassConfig> {
    Ok(FastCgiPassConfig {
        raw: parse_gateway_pass(directive, "FastCGI")?,
    })
}

pub(in crate::config::compile) fn parse_uwsgi_pass(
    directive: &Directive,
) -> ConfigResult<UwsgiPassConfig> {
    let raw = one_arg(directive)?.to_string();
    let tls = if raw
        .get(..9)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("suwsgi://"))
        && raw.len() > 9
    {
        true
    } else if raw
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("uwsgi://"))
        && raw.len() > 8
    {
        false
    } else {
        parse_gateway_pass(directive, "uWSGI")?;
        false
    };
    Ok(UwsgiPassConfig { raw, tls })
}

pub(in crate::config::compile) fn parse_scgi_pass(
    directive: &Directive,
) -> ConfigResult<ScgiPassConfig> {
    Ok(ScgiPassConfig {
        raw: parse_gateway_pass(directive, "SCGI")?,
    })
}

pub(in crate::config::compile) fn parse_gateway_pass(
    directive: &Directive,
    label: &str,
) -> ConfigResult<String> {
    let raw = one_arg(directive)?.to_string();
    if raw.contains("://") {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid URL prefix in \"{}_pass\"",
                label.to_ascii_lowercase()
            ),
        ));
    }
    if raw == "unix:" {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid UNIX domain socket path in \"{}_pass\"",
                label.to_ascii_lowercase()
            ),
        ));
    }
    Ok(raw)
}

pub(in crate::config::compile) fn parse_modifier(
    value: &str,
    name: &str,
    location: &SourceLocation,
) -> ConfigResult<u8> {
    value.parse::<u8>().map_err(|_| {
        ConfigError::at(
            location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        )
    })
}

pub(in crate::config::compile) fn parse_cgi_param(
    directive: &Directive,
) -> ConfigResult<FastCgiParam> {
    if directive.args.len() < 2 || directive.args.len() > 3 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid number of arguments in \"{}\" directive",
                directive.name
            ),
        ));
    }
    let if_not_empty = directive
        .args
        .get(2)
        .is_some_and(|value| value == "if_not_empty");
    if directive.args.len() == 3 && !if_not_empty {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid parameter in \"{}\" directive", directive.name),
        ));
    }
    Ok(FastCgiParam {
        name: directive.args[0].clone(),
        value: directive.args[1].clone(),
        if_not_empty,
    })
}

pub(in crate::config::compile) fn parse_fastcgi_split_path_info(
    directive: &Directive,
) -> ConfigResult<Regex> {
    let pattern = one_arg(directive)?;
    let regex = Regex::new(pattern).map_err(|err| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid regular expression: {err}"),
        )
    })?;
    if regex.captures_len() != 3 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("pattern \"{pattern}\" must have 2 captures"),
        ));
    }
    Ok(regex)
}

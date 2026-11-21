use super::*;

pub(super) fn validate_valid_referers(
    referers: &ValidReferers,
    location: &SourceLocation,
) -> ConfigResult<()> {
    if (referers.no_referer || referers.blocked_referer)
        && !referers.server_names
        && referers.hosts.is_empty()
        && referers.regexes.is_empty()
    {
        return Err(ConfigError::at(
            location.clone(),
            "the \"none\" or \"blocked\" referers are specified in the \"valid_referers\" directive without any valid referer",
        ));
    }
    Ok(())
}

pub(super) fn apply_valid_referers(
    directive: &Directive,
    referers: &mut ValidReferers,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"valid_referers\" directive",
        ));
    }
    for value in &directive.args {
        match value.as_str() {
            "none" => {
                referers.no_referer = true;
                continue;
            }
            "blocked" => {
                referers.blocked_referer = true;
                continue;
            }
            "server_names" => {
                referers.server_names = true;
                continue;
            }
            _ => {}
        }
        if let Some(pattern) = value.strip_prefix('~') {
            let regex = RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .map_err(|err| {
                    ConfigError::at(
                        directive.location.clone(),
                        format!("invalid referer regular expression: {err}"),
                    )
                })?;
            referers.regexes.push(regex);
            continue;
        }
        referers
            .hosts
            .push(parse_referer_host(value, &directive.location)?);
    }
    Ok(())
}

fn parse_referer_host(value: &str, location: &SourceLocation) -> ConfigResult<RefererHostRule> {
    if value.is_empty() {
        return Err(ConfigError::at(
            location.clone(),
            "invalid empty referer host",
        ));
    }
    let (host, uri) = match value.find('/') {
        Some(position) => (&value[..position], Some(value[position..].to_string())),
        None => (value, None),
    };
    if host.is_empty() {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid referer \"{value}\""),
        ));
    }
    let host = host.to_ascii_lowercase();
    let host = if let Some(suffix) = host.strip_prefix("*.") {
        RefererHost::LeadingWildcard(format!(".{suffix}"))
    } else if let Some(suffix) = host.strip_prefix('.') {
        RefererHost::LeadingWildcard(format!(".{suffix}"))
    } else if let Some(prefix) = host.strip_suffix(".*") {
        RefererHost::TrailingWildcard(format!("{prefix}."))
    } else if let Some(prefix) = host.strip_suffix('.') {
        RefererHost::TrailingWildcard(format!("{prefix}."))
    } else if host.contains('*') {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid referer wildcard \"{value}\""),
        ));
    } else {
        RefererHost::Exact(host)
    };
    Ok(RefererHostRule { host, uri })
}

use super::*;

pub(super) fn parse_proxy_cache_valid(directive: &Directive) -> ConfigResult<ProxyCacheValid> {
    if directive.args.is_empty() {
        return Err(invalid_arguments(directive));
    }
    let (statuses, time) = if directive.args.len() == 1 {
        (vec![200, 301, 302], directive.args[0].as_str())
    } else {
        let (time, statuses) = directive.args.split_last().expect("non-empty arguments");
        let statuses = statuses
            .iter()
            .map(|value| {
                if value == "any" {
                    return Ok(0);
                }
                value
                    .parse::<u16>()
                    .ok()
                    .filter(|status| (100..=599).contains(status))
                    .ok_or_else(|| invalid_value(directive, value))
            })
            .collect::<ConfigResult<Vec<_>>>()?;
        (statuses, time.as_str())
    };
    Ok(ProxyCacheValid {
        statuses,
        valid: parse_duration(time, &directive.location)?,
    })
}

pub(super) fn parse_proxy_cache_use_stale(
    directive: &Directive,
) -> ConfigResult<ProxyCacheUseStaleConfig> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(invalid_arguments(directive));
    }
    let mut flags = 0;
    for parameter in &directive.args {
        match parameter.as_str() {
            "error" => flags |= CACHE_STALE_ERROR,
            "timeout" => flags |= CACHE_STALE_TIMEOUT,
            "invalid_header" => flags |= CACHE_STALE_INVALID_HEADER,
            "updating" => flags |= CACHE_STALE_UPDATING,
            "http_403" => flags |= CACHE_STALE_HTTP_403,
            "http_404" => flags |= CACHE_STALE_HTTP_404,
            "http_429" => flags |= CACHE_STALE_HTTP_429,
            "http_500" => flags |= CACHE_STALE_HTTP_500,
            "http_502" => flags |= CACHE_STALE_HTTP_502,
            "http_503" => flags |= CACHE_STALE_HTTP_503,
            "http_504" => flags |= CACHE_STALE_HTTP_504,
            "off" => flags |= CACHE_STALE_OFF,
            _ => return Err(invalid_value(directive, parameter)),
        }
    }
    Ok(ProxyCacheUseStaleConfig { flags })
}

pub(super) fn parse_positive_usize(value: &str, directive: &Directive) -> ConfigResult<usize> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_value(directive, value))
}

pub(super) fn parse_on_off(value: &str, directive: &Directive) -> ConfigResult<bool> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(invalid_value(directive, value)),
    }
}

pub(super) fn mark_duplicate(directive: &Directive, seen: &mut bool) -> ConfigResult<()> {
    if *seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    *seen = true;
    Ok(())
}

pub(super) fn invalid_arguments(directive: &Directive) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        format!(
            "invalid number of arguments in \"{}\" directive",
            directive.name
        ),
    )
}

pub(super) fn invalid_value(directive: &Directive, value: &str) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        format!(
            "invalid value \"{value}\" in \"{}\" directive",
            directive.name
        ),
    )
}

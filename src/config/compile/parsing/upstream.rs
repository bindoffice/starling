use super::*;

pub(in crate::config::compile) fn apply_next_upstream(
    directive: &Directive,
    config: &mut NextUpstreamConfig,
) -> ConfigResult<()> {
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
    let mut flags = 0;
    let mut off = false;
    for parameter in &directive.args {
        match parameter.as_str() {
            "error" => flags |= NEXT_UPSTREAM_ERROR,
            "timeout" => flags |= NEXT_UPSTREAM_TIMEOUT,
            "invalid_header" => flags |= NEXT_UPSTREAM_INVALID_HEADER,
            "non_idempotent" => flags |= NEXT_UPSTREAM_NON_IDEMPOTENT,
            "http_500" => flags |= NEXT_UPSTREAM_HTTP_500,
            "http_502" => flags |= NEXT_UPSTREAM_HTTP_502,
            "http_503" => flags |= NEXT_UPSTREAM_HTTP_503,
            "http_504" => flags |= NEXT_UPSTREAM_HTTP_504,
            "http_403" => flags |= NEXT_UPSTREAM_HTTP_403,
            "http_404" => flags |= NEXT_UPSTREAM_HTTP_404,
            "http_429" => flags |= NEXT_UPSTREAM_HTTP_429,
            "updating" => {}
            "off" => off = true,
            _ => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid parameter \"{parameter}\""),
                ));
            }
        }
    }
    config.flags = if off { NEXT_UPSTREAM_OFF } else { flags };
    Ok(())
}

pub(in crate::config::compile) fn parse_next_upstream_tries(
    directive: &Directive,
    name: &str,
) -> ConfigResult<usize> {
    validate_no_block(directive)?;
    parse_usize(one_arg(directive)?, &directive.location, name)
}

pub(in crate::config::compile) fn parse_next_upstream_timeout(
    directive: &Directive,
    name: &str,
) -> ConfigResult<Duration> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    parse_duration(value, &directive.location).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        )
    })
}

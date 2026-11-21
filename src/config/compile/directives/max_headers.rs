use super::*;

pub(super) fn try_apply_max_headers_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
    allowed: bool,
) -> ConfigResult<bool> {
    if directive.name != "max_headers" {
        return Ok(false);
    }
    if !allowed {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"max_headers\" directive is not allowed here",
        ));
    }
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    if seen.max_headers {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"max_headers\" directive is duplicate",
        ));
    }
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || value
            .parse::<u64>()
            .map_or(true, |headers| headers > i64::MAX as u64)
    {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid number \"{value}\" in \"max_headers\" directive"),
        ));
    }
    context.max_headers = value.parse().map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid number \"{value}\" in \"max_headers\" directive"),
        )
    })?;
    seen.max_headers = true;
    Ok(true)
}

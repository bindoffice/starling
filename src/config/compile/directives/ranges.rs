use super::*;

pub(super) fn try_apply_range_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    if directive.name != "max_ranges" {
        return Ok(false);
    }
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    if seen.max_ranges {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"max_ranges\" directive is duplicate",
        ));
    }
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || value
            .parse::<u64>()
            .map_or(true, |ranges| ranges > i64::MAX as u64)
    {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid number \"{value}\" in \"max_ranges\" directive"),
        ));
    }
    context.max_ranges = value.parse().map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid number \"{value}\" in \"max_ranges\" directive"),
        )
    })?;
    seen.max_ranges = true;
    Ok(true)
}

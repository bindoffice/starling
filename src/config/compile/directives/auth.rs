use super::*;

pub(super) fn try_apply_auth_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    if directive.name != "auth_delay" {
        return Ok(false);
    }
    validate_no_block(directive)?;
    if seen.auth_delay {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"auth_delay\" directive is duplicate",
        ));
    }
    seen.auth_delay = true;
    context.auth_delay = parse_duration(one_arg(directive)?, &directive.location)?;
    Ok(true)
}

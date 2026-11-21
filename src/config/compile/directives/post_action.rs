use super::*;

pub(super) fn try_apply_post_action_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    if directive.name != "post_action" {
        return Ok(false);
    }
    validate_no_block(directive)?;
    if seen.post_action {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"post_action\" directive is duplicate",
        ));
    }
    context.post_action = Some(one_arg(directive)?.to_string());
    seen.post_action = true;
    Ok(true)
}

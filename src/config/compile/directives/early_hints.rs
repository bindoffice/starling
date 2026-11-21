use super::*;

pub(super) fn try_apply_early_hints_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    if directive.name != "early_hints" {
        return Ok(false);
    }
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"early_hints\" directive",
        ));
    }
    if !seen.early_hints {
        context.early_hints.predicates.clear();
        seen.early_hints = true;
    }
    context
        .early_hints
        .predicates
        .extend(directive.args.iter().cloned());
    Ok(true)
}

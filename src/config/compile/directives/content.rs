use super::*;

pub(super) fn try_apply_content_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    scope: Scope,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "random_index" => {
            require_location_scope(directive, scope)?;
            context.random_index = parse_flag(directive, "random_index")?;
        }
        "empty_gif" => {
            require_empty_location_directive(directive, scope)?;
            context.empty_gif = true;
        }
        "flv" => {
            require_empty_location_directive(directive, scope)?;
            context.flv = true;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn require_location_scope(directive: &Directive, scope: Scope) -> ConfigResult<()> {
    if matches!(scope, Scope::Location) {
        return Ok(());
    }
    Err(ConfigError::at(
        directive.location.clone(),
        format!("\"{}\" directive is not allowed here", directive.name),
    ))
}

fn require_empty_location_directive(directive: &Directive, scope: Scope) -> ConfigResult<()> {
    validate_no_block(directive)?;
    require_location_scope(directive, scope)?;
    if directive.args.is_empty() {
        return Ok(());
    }
    Err(ConfigError::at(
        directive.location.clone(),
        format!(
            "invalid number of arguments in \"{}\" directive",
            directive.name
        ),
    ))
}

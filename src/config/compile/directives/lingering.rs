use super::*;

pub(super) fn try_apply_lingering_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "lingering_close" => {
            validate_no_block(directive)?;
            reject_duplicate(directive, seen.lingering_close)?;
            context.lingering_close = match one_arg(directive)? {
                "off" => LingeringClose::Off,
                "on" => LingeringClose::On,
                "always" => LingeringClose::Always,
                value => {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!(
                            "invalid value \"{value}\" in \"lingering_close\" directive, \
                             it must be \"off\", \"on\" or \"always\""
                        ),
                    ));
                }
            };
            seen.lingering_close = true;
        }
        "lingering_time" => {
            validate_no_block(directive)?;
            reject_duplicate(directive, seen.lingering_time)?;
            context.lingering_time = parse_duration(one_arg(directive)?, &directive.location)?;
            seen.lingering_time = true;
        }
        "lingering_timeout" => {
            validate_no_block(directive)?;
            reject_duplicate(directive, seen.lingering_timeout)?;
            context.lingering_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
            seen.lingering_timeout = true;
        }
        "reset_timedout_connection" => {
            reject_duplicate(directive, seen.reset_timedout_connection)?;
            context.reset_timedout_connection = parse_flag(directive, "reset_timedout_connection")?;
            seen.reset_timedout_connection = true;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn reject_duplicate(directive: &Directive, seen: bool) -> ConfigResult<()> {
    if seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    Ok(())
}

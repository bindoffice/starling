use super::*;

pub(super) fn try_apply_msie_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "msie_padding" => {
            if seen.msie_padding {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"msie_padding\" directive is duplicate",
                ));
            }
            seen.msie_padding = true;
            context.msie_padding = parse_flag(directive, "msie_padding")?;
        }
        "msie_refresh" => {
            if seen.msie_refresh {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"msie_refresh\" directive is duplicate",
                ));
            }
            seen.msie_refresh = true;
            context.msie_refresh = parse_flag(directive, "msie_refresh")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

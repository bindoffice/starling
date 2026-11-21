use super::*;

pub(super) fn try_apply_keepalive_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "keepalive_min_timeout" => {
            validate_no_block(directive)?;
            if seen.keepalive_min_timeout {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"keepalive_min_timeout\" directive is duplicate",
                ));
            }
            seen.keepalive_min_timeout = true;
            context.keepalive_min_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
            return Ok(true);
        }
        "keepalive_disable" => {}
        _ => return Ok(false),
    }

    validate_no_block(directive)?;
    if !(1..=2).contains(&directive.args.len()) {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"keepalive_disable\" directive",
        ));
    }

    if !seen.keepalive_disable {
        context.keepalive_disable = KeepaliveDisableConfig {
            msie6: false,
            safari: false,
        };
        seen.keepalive_disable = true;
    }
    for value in &directive.args {
        if value.eq_ignore_ascii_case("none") {
            continue;
        }
        if value.eq_ignore_ascii_case("msie6") {
            context.keepalive_disable.msie6 = true;
            continue;
        }
        if value.eq_ignore_ascii_case("safari") {
            context.keepalive_disable.safari = true;
            continue;
        }
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"keepalive_disable\" directive"),
        ));
    }
    Ok(true)
}

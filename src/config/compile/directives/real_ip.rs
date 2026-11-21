use super::*;

pub(in crate::config::compile) fn apply_set_real_ip_from(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut bool,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    if !*seen {
        context.real_ip_from.clear();
        *seen = true;
    }
    context
        .real_ip_from
        .push(parse_real_ip_source(value, &directive.location)?);
    Ok(())
}

pub(in crate::config::compile) fn parse_real_ip_source(
    value: &str,
    location: &SourceLocation,
) -> ConfigResult<RealIpSource> {
    if value == "unix:" {
        return Ok(RealIpSource::Unix);
    }
    Ok(RealIpSource::Network(parse_ip_network(value, location)?))
}

pub(super) fn try_apply_real_ip_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "set_real_ip_from" => {
            apply_set_real_ip_from(directive, context, &mut seen.real_ip_from)?;
        }
        "real_ip_header" => {
            validate_no_block(directive)?;
            context.real_ip_header = one_arg(directive)?.to_string();
        }
        "real_ip_recursive" => {
            context.real_ip_recursive = parse_flag(directive, "real_ip_recursive")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

use super::*;

pub(super) fn try_apply_request_headers_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    scope: Option<Scope>,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    let name = directive.name.as_str();
    if !matches!(
        name,
        "client_header_buffer_size"
            | "large_client_header_buffers"
            | "ignore_invalid_headers"
            | "underscores_in_headers"
    ) {
        return Ok(false);
    }
    if matches!(scope, Some(Scope::Location)) {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{name}\" directive is not allowed here"),
        ));
    }
    validate_no_block(directive)?;

    match name {
        "client_header_buffer_size" => {
            mark_duplicate(seen.client_header_buffer_size, directive)?;
            seen.client_header_buffer_size = true;
            let size = parse_size(one_arg(directive)?, &directive.location)?;
            let size = usize::try_from(size)
                .ok()
                .filter(|size| *size > 0)
                .ok_or_else(|| {
                    ConfigError::at(
                        directive.location.clone(),
                        format!(
                            "invalid value \"{}\" in \"{name}\" directive",
                            directive.args[0]
                        ),
                    )
                })?;
            context.client_header_buffer_size = size;
        }
        "large_client_header_buffers" => {
            mark_duplicate(seen.large_client_header_buffers, directive)?;
            seen.large_client_header_buffers = true;
            if directive.args.len() != 2 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid number of arguments in \"{name}\" directive"),
                ));
            }
            let number = directive.args[0].parse::<usize>().ok().filter(|n| *n > 0);
            let size = parse_size(&directive.args[1], &directive.location)
                .ok()
                .and_then(|size| usize::try_from(size).ok())
                .filter(|size| *size > 0);
            let (Some(number), Some(size)) = (number, size) else {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!(
                        "invalid value \"{} {}\" in \"{name}\" directive",
                        directive.args[0], directive.args[1]
                    ),
                ));
            };
            if number.checked_mul(size).is_none() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!(
                        "invalid value \"{} {}\" in \"{name}\" directive",
                        directive.args[0], directive.args[1]
                    ),
                ));
            }
            context.large_client_header_buffers = LargeClientHeaderBuffers { number, size };
        }
        "ignore_invalid_headers" => {
            mark_duplicate(seen.ignore_invalid_headers, directive)?;
            seen.ignore_invalid_headers = true;
            context.ignore_invalid_headers = parse_flag(directive, name)?;
        }
        "underscores_in_headers" => {
            mark_duplicate(seen.underscores_in_headers, directive)?;
            seen.underscores_in_headers = true;
            context.underscores_in_headers = parse_flag(directive, name)?;
        }
        _ => unreachable!("recognized request header directive"),
    }
    Ok(true)
}

fn mark_duplicate(seen: bool, directive: &Directive) -> ConfigResult<()> {
    if seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    Ok(())
}

use super::*;

pub(super) fn try_apply_header_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "add_header" => {
            validate_no_block(directive)?;
            seen.add_headers.push(parse_add_header(directive)?);
        }
        "add_trailer" => {
            validate_no_block(directive)?;
            seen.add_trailers.push(parse_add_header(directive)?);
        }
        "add_header_inherit" => {
            if seen.add_header_inherit {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"add_header_inherit\" directive is duplicate",
                ));
            }
            seen.add_header_inherit = true;
            context.add_header_inherit = parse_header_inheritance(directive)?;
        }
        "add_trailer_inherit" => {
            if seen.add_trailer_inherit {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"add_trailer_inherit\" directive is duplicate",
                ));
            }
            seen.add_trailer_inherit = true;
            context.add_trailer_inherit = parse_header_inheritance(directive)?;
        }
        "expires" => {
            apply_expires_directive(directive, context, &mut seen.expires)?;
        }
        "if_modified_since" => {
            validate_no_block(directive)?;
            if seen.if_modified_since {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"if_modified_since\" directive is duplicate",
                ));
            }
            seen.if_modified_since = true;
            context.if_modified_since = parse_if_modified_since(directive)?;
        }
        "sub_filter" => {
            validate_no_block(directive)?;
            if !seen.sub_filter {
                context.sub_filters.clear();
                seen.sub_filter = true;
            }
            context.sub_filters.push(parse_sub_filter(directive)?);
        }
        "sub_filter_types" => {
            validate_no_block(directive)?;
            context.sub_filter_types = parse_sub_filter_types(directive)?;
        }
        "sub_filter_once" => {
            context.sub_filter_once = parse_flag(directive, "sub_filter_once")?;
        }
        "sub_filter_last_modified" => {
            context.sub_filter_last_modified = parse_flag(directive, "sub_filter_last_modified")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

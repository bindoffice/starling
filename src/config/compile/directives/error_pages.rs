use super::*;

pub(super) fn try_apply_error_page_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "error_page" => {
            validate_no_block(directive)?;
            if !seen.error_page {
                context.error_pages.clear();
                seen.error_page = true;
            }
            context.error_pages.push(parse_error_page(directive)?);
        }
        "recursive_error_pages" => {
            if seen.recursive_error_pages {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"recursive_error_pages\" directive is duplicate",
                ));
            }
            seen.recursive_error_pages = true;
            context.recursive_error_pages = parse_flag(directive, "recursive_error_pages")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

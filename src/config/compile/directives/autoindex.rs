use super::*;

pub(super) fn try_apply_autoindex_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "autoindex" => {
            reject_duplicate(directive, seen.autoindex)?;
            context.autoindex = parse_flag(directive, "autoindex")?;
            seen.autoindex = true;
        }
        "autoindex_exact_size" => {
            reject_duplicate(directive, seen.autoindex_exact_size)?;
            context.autoindex_exact_size = parse_flag(directive, "autoindex_exact_size")?;
            seen.autoindex_exact_size = true;
        }
        "autoindex_localtime" => {
            reject_duplicate(directive, seen.autoindex_localtime)?;
            context.autoindex_localtime = parse_flag(directive, "autoindex_localtime")?;
            seen.autoindex_localtime = true;
        }
        "autoindex_format" => {
            reject_duplicate(directive, seen.autoindex_format)?;
            validate_no_block(directive)?;
            context.autoindex_format = match one_arg(directive)? {
                "html" => AutoindexFormat::Html,
                "json" => AutoindexFormat::Json,
                "jsonp" => AutoindexFormat::Jsonp,
                "xml" => AutoindexFormat::Xml,
                value => {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!(
                            "invalid value \"{value}\" in \"autoindex_format\" directive, \
                             it must be \"html\", \"json\", \"jsonp\" or \"xml\""
                        ),
                    ));
                }
            };
            seen.autoindex_format = true;
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

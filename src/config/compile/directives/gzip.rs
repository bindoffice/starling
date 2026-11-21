use super::*;

pub(super) fn try_apply_gzip_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "gzip_http_version" => {
            validate_no_block(directive)?;
            let value = one_arg(directive)?;
            context.gzip_http_version = GzipHttpVersion::parse(value).ok_or_else(|| {
                ConfigError::at(
                    directive.location.clone(),
                    format!(
                        "invalid value \"{value}\" in \"gzip_http_version\" directive, \
                         it must be \"1.0\" or \"1.1\""
                    ),
                )
            })?;
        }
        "gzip_disable" => apply_gzip_disable(directive, context, seen)?,
        _ => return Ok(false),
    }
    Ok(true)
}

fn apply_gzip_disable(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"gzip_disable\" directive",
        ));
    }

    if !seen.gzip_disable {
        context.gzip_disable.regexes.clear();
        seen.gzip_disable = true;
    }

    for value in &directive.args {
        if value == "msie6" {
            context.gzip_disable.msie6 = true;
            continue;
        }
        if value == "degradation" {
            return Err(ConfigError::at(
                directive.location.clone(),
                "the \"degradation\" parameter in \"gzip_disable\" is not supported",
            ));
        }

        let regex = RegexBuilder::new(value)
            .case_insensitive(true)
            .build()
            .map_err(|err| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid gzip_disable regular expression \"{value}\": {err}"),
                )
            })?;
        context.gzip_disable.regexes.push(regex);
    }
    Ok(())
}

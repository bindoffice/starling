use super::*;

pub(in crate::config::compile) fn apply_proxy_cookie_flags(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut bool,
) -> ConfigResult<()> {
    validate_no_block(directive)?;

    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"proxy_cookie_flags\" directive",
        ));
    }

    let first = !*seen;
    if first {
        context.proxy_cookie_flags.clear();
        *seen = true;
    }

    if directive.args.len() == 1 {
        if directive.args[0] != "off" {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("invalid parameter \"{}\"", directive.args[0]),
            ));
        }
        if !first {
            return Err(duplicate_error(directive));
        }
        return Ok(());
    }

    if !first && context.proxy_cookie_flags.is_empty() {
        return Err(duplicate_error(directive));
    }

    let cookie = &directive.args[0];
    let (cookie, regex) = if let Some(pattern) = cookie.strip_prefix('~') {
        (
            pattern.to_string(),
            Some(
                RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .map_err(|err| {
                        ConfigError::at(
                            directive.location.clone(),
                            format!("invalid regular expression: {err}"),
                        )
                    })?,
            ),
        )
    } else {
        (cookie.clone(), None)
    };

    context.proxy_cookie_flags.push(ProxyCookieFlagsRule {
        cookie,
        regex,
        flags: directive.args[1..].to_vec(),
    });
    Ok(())
}

fn duplicate_error(directive: &Directive) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        "\"proxy_cookie_flags\" directive is duplicate",
    )
}

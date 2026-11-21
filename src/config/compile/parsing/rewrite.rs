use super::*;

pub(in crate::config::compile) fn parse_return(
    directive: &Directive,
) -> ConfigResult<ReturnConfig> {
    if directive.args.is_empty() || directive.args.len() > 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"return\" directive",
        ));
    }
    let first = &directive.args[0];
    if first.bytes().all(|byte| byte.is_ascii_digit()) {
        let status = first.parse::<u16>().map_err(|_| {
            ConfigError::at(
                directive.location.clone(),
                format!("invalid return code \"{first}\""),
            )
        })?;
        if status > 999 {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("invalid return code \"{first}\""),
            ));
        }
        return Ok(ReturnConfig {
            status,
            value: directive.args.get(1).cloned(),
        });
    }
    if directive.args.len() == 1 && is_return_redirect(first) {
        return Ok(ReturnConfig {
            status: 302,
            value: Some(first.clone()),
        });
    }
    Err(ConfigError::at(
        directive.location.clone(),
        format!("invalid return code \"{first}\""),
    ))
}

fn is_return_redirect(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://") || value.starts_with("$scheme")
}

pub(in crate::config::compile) fn parse_rewrite(
    directive: &Directive,
) -> ConfigResult<RewriteRule> {
    if directive.args.len() < 2 || directive.args.len() > 3 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"rewrite\" directive",
        ));
    }
    let regex = Regex::new(&directive.args[0]).map_err(|err| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid regular expression: {err}"),
        )
    })?;
    if directive.args[1].is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "empty replacement",
        ));
    }
    let flag = match directive.args.get(2).map(String::as_str) {
        None => RewriteFlag::None,
        Some("last") => RewriteFlag::Last,
        Some("break") => RewriteFlag::Break,
        Some("redirect") => RewriteFlag::Redirect,
        Some("permanent") => RewriteFlag::Permanent,
        Some(value) => {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("invalid parameter \"{value}\""),
            ));
        }
    };
    Ok(RewriteRule {
        regex,
        replacement: directive.args[1].clone(),
        flag,
    })
}

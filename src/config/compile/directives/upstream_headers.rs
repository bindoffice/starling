use super::*;

pub(in crate::config::compile) fn apply_header_list(
    directive: &Directive,
    headers: &mut Vec<String>,
    seen: &mut bool,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if !*seen {
        headers.clear();
        *seen = true;
    }
    headers.push(one_arg(directive)?.to_string());
    Ok(())
}

pub(in crate::config::compile) fn apply_ignore_headers(
    directive: &Directive,
    headers: &mut Vec<String>,
    seen: &mut bool,
) -> ConfigResult<()> {
    const SUPPORTED: &[&str] = &[
        "X-Accel-Redirect",
        "X-Accel-Expires",
        "X-Accel-Limit-Rate",
        "X-Accel-Buffering",
        "X-Accel-Charset",
        "Expires",
        "Cache-Control",
        "Set-Cookie",
        "Vary",
    ];

    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid number of arguments in \"{}\" directive",
                directive.name
            ),
        ));
    }
    if !*seen {
        headers.clear();
        *seen = true;
    }
    for value in &directive.args {
        let Some(canonical) = SUPPORTED
            .iter()
            .find(|supported| supported.eq_ignore_ascii_case(value))
        else {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!(
                    "invalid value \"{value}\" in \"{}\" directive",
                    directive.name
                ),
            ));
        };
        if !headers
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(canonical))
        {
            headers.push((*canonical).to_string());
        }
    }
    Ok(())
}

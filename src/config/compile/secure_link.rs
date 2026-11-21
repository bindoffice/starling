use super::*;

#[derive(Default)]
pub(super) struct SecureLinkSeen {
    variable: bool,
    md5: bool,
    secret: bool,
}

pub(super) fn try_apply_secure_link_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SecureLinkSeen,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "secure_link" => {
            if seen.variable {
                return Err(duplicate(directive, "secure_link"));
            }
            validate_no_block(directive)?;
            context.secure_link.variable = Some(one_arg(directive)?.to_string());
            seen.variable = true;
        }
        "secure_link_md5" => {
            if seen.md5 {
                return Err(duplicate(directive, "secure_link_md5"));
            }
            validate_no_block(directive)?;
            context.secure_link.md5 = Some(one_arg(directive)?.to_string());
            seen.md5 = true;
        }
        "secure_link_secret" => {
            if seen.secret {
                return Err(duplicate(directive, "secure_link_secret"));
            }
            validate_no_block(directive)?;
            context.secure_link.secret = Some(one_arg(directive)?.to_string());
            seen.secret = true;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

pub(super) fn finalize_secure_link(
    config: &mut SecureLinkConfig,
    seen: &SecureLinkSeen,
    location: &SourceLocation,
) -> ConfigResult<()> {
    if seen.secret && (seen.variable || seen.md5) {
        return Err(ConfigError::at(
            location.clone(),
            "\"secure_link_secret\" cannot be mixed with \"secure_link\" and \"secure_link_md5\"",
        ));
    }

    // Nginx uses the local mode to replace inherited configuration entirely.
    if seen.secret {
        config.variable = None;
        config.md5 = None;
    } else if seen.variable || seen.md5 {
        config.secret = None;
    }
    Ok(())
}

fn duplicate(directive: &Directive, name: &str) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        format!("\"{name}\" directive is duplicate"),
    )
}

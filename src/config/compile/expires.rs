use super::*;

pub(super) fn apply_expires_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut bool,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if *seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"expires\" directive is duplicate",
        ));
    }
    *seen = true;
    context.expires = Some(parse_expires(directive)?);
    Ok(())
}

pub(super) fn parse_expires(directive: &Directive) -> ConfigResult<ExpiresRule> {
    let (modified, value) = match directive.args.as_slice() {
        [value] => (false, value.as_str()),
        [kind, value] if kind == "modified" => (true, value.as_str()),
        [_, value] => {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("invalid value \"{value}\" in \"expires\" directive"),
            ));
        }
        _ => {
            return Err(ConfigError::at(
                directive.location.clone(),
                "invalid number of arguments in \"expires\" directive",
            ));
        }
    };

    let value = if value.contains('$') {
        let offset = ExpiresOffset::Variable(value.to_string());
        if modified {
            ExpiresValue::Modified(offset)
        } else {
            ExpiresValue::Access(offset)
        }
    } else {
        ExpiresValue::parse_literal(value, modified).ok_or_else(|| {
            ConfigError::at(
                directive.location.clone(),
                format!("invalid value \"{value}\" in \"expires\" directive"),
            )
        })?
    };
    Ok(ExpiresRule { value })
}

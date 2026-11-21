use super::*;

pub(super) fn try_apply_disable_symlinks_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut bool,
) -> ConfigResult<bool> {
    if directive.name != "disable_symlinks" {
        return Ok(false);
    }
    validate_no_block(directive)?;
    if *seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"disable_symlinks\" directive is duplicate",
        ));
    }
    if directive.args.is_empty() || directive.args.len() > 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"disable_symlinks\" directive",
        ));
    }

    let mut mode = None;
    let mut from = None;
    for parameter in &directive.args {
        match parameter.as_str() {
            "off" => mode = Some(DisableSymlinksMode::Off),
            "on" => mode = Some(DisableSymlinksMode::On),
            "if_not_owner" => mode = Some(DisableSymlinksMode::IfNotOwner),
            value if value.starts_with("from=") => {
                if from.is_some() {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!(
                            "duplicate parameters \"{} {}\"",
                            directive.args[0], directive.args[1]
                        ),
                    ));
                }
                from = Some(value["from=".len()..].to_string());
            }
            value => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid parameter \"{value}\""),
                ));
            }
        }
    }
    let Some(mode) = mode else {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"disable_symlinks\" must have \"off\", \"on\" or \"if_not_owner\" parameter",
        ));
    };
    if directive.args.len() == 2 && from.is_none() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "duplicate parameters \"{} {}\"",
                directive.args[0], directive.args[1]
            ),
        ));
    }
    if mode == DisableSymlinksMode::Off && from.is_some() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"from=\" cannot be used with \"off\" parameter",
        ));
    }
    context.disable_symlinks = DisableSymlinksConfig { mode, from };
    *seen = true;
    Ok(true)
}

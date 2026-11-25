use super::*;

pub(in crate::config::compile) fn try_apply_open_file_cache_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "open_file_cache" => apply_open_file_cache(directive, context)?,
        "open_file_cache_valid" => {
            let value = parse_duration(one_arg(directive)?, &directive.location)?;
            Arc::make_mut(&mut context.open_file_cache).valid = value;
        }
        "open_file_cache_min_uses" => {
            let value = one_arg(directive)?.parse::<usize>().map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    "invalid value in \"open_file_cache_min_uses\" directive",
                )
            })?;
            if value == 0 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid value in \"open_file_cache_min_uses\" directive",
                ));
            }
            Arc::make_mut(&mut context.open_file_cache).min_uses = value;
        }
        "open_file_cache_errors" => {
            Arc::make_mut(&mut context.open_file_cache).errors =
                parse_flag(directive, "open_file_cache_errors")?;
        }
        "open_file_cache_events" => {
            Arc::make_mut(&mut context.open_file_cache).events =
                parse_flag(directive, "open_file_cache_events")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn apply_open_file_cache(directive: &Directive, context: &mut EffectiveConfig) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args == ["off"] {
        Arc::make_mut(&mut context.open_file_cache).enabled = false;
        return Ok(());
    }
    if !(1..=2).contains(&directive.args.len()) {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"open_file_cache\" directive",
        ));
    }
    let Some(max) = directive.args[0].strip_prefix("max=") else {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid \"open_file_cache\" parameter",
        ));
    };
    let max = max.parse::<usize>().map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            "invalid \"open_file_cache\" parameter",
        )
    })?;
    if max == 0 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"open_file_cache\" must have a positive \"max\" parameter",
        ));
    }
    let inactive = directive
        .args
        .get(1)
        .map(|value| {
            value
                .strip_prefix("inactive=")
                .ok_or_else(|| {
                    ConfigError::at(
                        directive.location.clone(),
                        "invalid \"open_file_cache\" parameter",
                    )
                })
                .and_then(|value| parse_duration(value, &directive.location))
        })
        .transpose()?
        .unwrap_or_else(|| Duration::from_secs(60));

    let config = Arc::make_mut(&mut context.open_file_cache);
    config.enabled = true;
    config.max = max;
    config.inactive = inactive;
    Ok(())
}
